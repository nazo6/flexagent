//! Session management (agent launch, event delivery, prompt queue).
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1-§4,
//! `docs/01-architecture-and-sync.md` §2.2-§2.4.
//!
//! - **Driver integration**: Launches an [`AgentDriver`] and streams [`DriverEvent`]
//!   into the [`SessionEventBus`] (persistent events via `record`, deltas via `publish_delta`).
//! - **Turn-start snapshot**: Right before `SendPrompt`, captures a Tree Hash with
//!   the Shadow Git Tree and records it as `UserMessage.snapshot_tree_hash`
//!   (the revert baseline).
//! - **Command idempotency**: Detects duplicate `command_id` submissions and
//!   returns [`NodeError::CommandDuplicate`] (guards against double taps / WS resends).
//! - **Pending queue**: `SendPrompt` while busy is queued, and the next prompt is
//!   sent automatically when the turn ends (`status = idle`).
//! - **Idempotent permission resolution**: The second and later responses return
//!   [`NodeError::AlreadyResolved`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fxg_acp::registry::{AcpRegistry, RegistryIndex};
use fxg_acp::{
    AcpDriver, ActiveSessionHandle, AgentDriver, AgentLaunchSpec, DriverEvent, DriverKind,
    NativeResumeUnavailable, ResumeRequest,
};
use fxg_protocol::common::{
    ClientSource, ElicitationAction, ElicitationContent, ElicitationMode, ElicitationSchema,
    ForkHistoryItem, HistoryRole, PermissionOption, ResolvedBy, SessionControlAction,
    SessionStatus,
};
use fxg_protocol::events::{SessionEventEnvelope, UnifiedEventPayload};
use fxg_protocol::util::uuid_v7;
use tokio::sync::mpsc;

use crate::error::NodeError;
use crate::paths::NodePaths;
use crate::project;
use crate::session::SessionEventBus;
use crate::snapshot::ShadowGitTree;

/// Upper bound on retained processed `command_id`s (idempotency history).
const PROCESSED_COMMAND_HISTORY: usize = 256;

/// Batch size when paging through `session_events`.
const EVENT_LOAD_BATCH: u32 = 200;

/// Upper bound on waiting for auto-resume (native restore) on send.
///
/// Tolerates agent cold starts (e.g. PyInstaller extraction taking ~25 seconds).
const AUTO_RESUME_TIMEOUT: Duration = Duration::from_secs(120);

/// Polling interval for waiting on other resume operations (explicit resume / auto-resume) to finish.
const RESUME_WAIT_INTERVAL: Duration = Duration::from_millis(100);

/// Upper bound on waiting for session stop (agent process exit) before deletion.
const STOP_BEFORE_DELETE_TIMEOUT: Duration = Duration::from_secs(30);

/// Factory that selects a driver from an agent launch spec.
///
/// Driver instances are shared across the daemon (not created per session).
/// `AcpDriver` keeps exited processes in a reuse pool, so separate instances
/// would break the pool.
pub type DriverFactory =
    Arc<dyn Fn(&AgentLaunchSpec) -> anyhow::Result<Arc<dyn AgentDriver>> + Send + Sync>;

/// The default driver factory.
///
/// Uses [`AcpDriver`] for all agents.
/// The driver is shared across all sessions.
pub fn default_driver_factory() -> DriverFactory {
    let acp: Arc<dyn AgentDriver> = Arc::new(AcpDriver::new());
    Arc::new(move |spec: &AgentLaunchSpec| match &spec.driver_kind {
        DriverKind::Acp => Ok(Arc::clone(&acp)),
        DriverKind::Other(other) => Err(anyhow::anyhow!("unknown driver kind: {other}")),
    })
}

/// Result of `ensure_session`.
#[derive(Debug, Clone, PartialEq)]
pub struct EnsureSessionOutcome {
    /// The started session ID
    pub session_id: String,
}

/// Start request with an explicit session ID ([`SessionManager::start_session`]).
///
/// Used by the central server (`ServerToNodeMsg::StartSession`) and the local
/// REST API (`POST /api/v1/sessions`). The session ID is assigned by the caller
/// (central server) so that all clients can correlate it.
#[derive(Debug, Clone)]
pub struct StartSessionParams<'a> {
    /// Correlation ID (idempotent deduplication of duplicate submissions)
    pub command_id: &'a str,
    /// Session ID (a UUID v7 assigned by the caller)
    pub session_id: &'a str,
    /// Working directory (including worktree paths)
    pub local_path: &'a Path,
    /// Agent ID
    pub agent_id: &'a str,
    /// Initial prompt
    pub initial_prompt: Option<&'a str>,
    /// Initial mode (`code` / `plan` etc.)
    pub mode: Option<&'a str>,
    /// Additional pass-through arguments for the agent
    pub extra_args: Option<&'a [String]>,
    /// History replay injection on fork (handover from another node / ephemeral VM)
    pub fork_context: Option<&'a [ForkHistoryItem]>,
    /// Restore of a Git bundle saved from another node / ephemeral VM (local file path)
    pub restore_git_bundle: Option<&'a Path>,
}

/// Resume request for a stopped session ([`SessionManager::resume`]).
///
/// Starts the agent under the same `session_id`, preferring native restore
/// (`session/resume` → `session/load`). If restore is impossible, continues by
/// injecting a history replay
/// (design: `docs/04-agent-drivers-and-windows.md` §4.3).
#[derive(Debug, Clone, Copy)]
pub struct ResumeParams<'a> {
    /// Correlation ID (idempotent deduplication of duplicate submissions)
    pub command_id: &'a str,
    /// Target session ID
    pub session_id: &'a str,
    /// Continue via history replay without attempting native restore
    pub force_replay: bool,
}

/// Result of `revert`.
#[derive(Debug, Clone, PartialEq)]
pub struct RevertOutcome {
    /// `node_seq` of the `UserMessage` used as the revert baseline
    pub target_node_seq: u64,
    /// Tree Hash of the restore target
    pub restored_tree_hash: String,
    /// Backup Tree Hash saved right before restoring
    pub backup_tree_hash: Option<String>,
    /// Number of restored files
    pub restored_files: usize,
    /// Number of removed files
    pub removed_files: usize,
}

/// Result of `resume`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResumeOutcome {
    /// The resumed session ID (identical to the request target)
    pub session_id: String,
    /// Whether native restore succeeded (`false` = continued via history replay)
    pub context_restored: bool,
}

/// Restore strategy for [`SessionManager::resume_stopped_session`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeMode {
    /// Prefer native restore, and create a new session if unavailable
    /// (the caller injects the history replay).
    AllowReplay {
        /// Create a new session without using the recorded agent session ID (`--replay`)
        force_replay: bool,
    },
    /// Native restore only. Returns [`NodeError::ResumeRequired`] if unavailable
    /// (auto-resume on prompt send).
    NativeOnly,
}

/// One row of the session list (for IPC).
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSessionSummary {
    /// Session ID
    pub session_id: String,
    /// Agent ID
    pub agent_id: String,
    /// Working directory
    pub cwd: PathBuf,
    /// Current status
    pub status: SessionStatus,
    /// Whether a turn is running (busy)
    pub busy: bool,
    /// Number of queued prompts
    pub queued_prompts: usize,
}

/// Pending permission entry for `fxg inbox`.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingPermissionSummary {
    /// Target session ID
    pub session_id: String,
    /// ACP request id
    pub request_id: String,
    /// Tool name
    pub tool_name: String,
    /// Summary
    pub summary: String,
    /// Options
    pub options: Vec<PermissionOption>,
}

/// Pending elicitation entry for `fxg inbox`.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingElicitationSummary {
    /// Target session ID
    pub session_id: String,
    /// ACP elicitation id (the JSON-RPC request id in form mode)
    pub elicitation_id: String,
    /// Message presented to the user
    pub message: String,
    /// Request mode (only `form` is supported)
    pub mode: ElicitationMode,
    /// Request schema for form mode
    pub requested_schema: ElicitationSchema,
}

/// Session management (one instance held by the daemon).
#[derive(Clone)]
pub struct SessionManager {
    inner: Arc<Inner>,
}

struct Inner {
    bus: SessionEventBus,
    paths: NodePaths,
    node_id: String,
    registry: AcpRegistry,
    factory: DriverFactory,
    sessions: Mutex<Sessions>,
    /// Drivers returned by the factory (shared instances; retained for `shutdown_idle`)
    drivers: Mutex<Vec<Arc<dyn AgentDriver>>>,
}

#[derive(Default)]
struct Sessions {
    active: HashMap<String, ActiveSession>,
    /// Most recently processed `command_id`s (oldest discarded first)
    processed_commands: VecDeque<String>,
    /// Session IDs currently being resumed (excludes double resumes)
    resuming: HashSet<String>,
}

struct ActiveSession {
    /// Operation handle (shared via Arc so it is not held across await points with the guard)
    handle: Arc<dyn ActiveSessionHandle>,
    cwd: PathBuf,
    agent_id: String,
    status: SessionStatus,
    busy: bool,
    pending_prompts: VecDeque<PendingPrompt>,
    /// Pending permissions (`request_id` → display information)
    pending_permissions: HashMap<String, PendingPermissionSummary>,
    /// Pending elicitations (`elicitation_id` → display information)
    pending_elicitations: HashMap<String, PendingElicitationSummary>,
    /// Whether a meaningful title has been set (derived from a prompt or notified by the driver)
    has_custom_title: bool,
}

#[derive(Debug, Clone)]
struct PendingPrompt {
    text: String,
    client_source: ClientSource,
}

struct StartDriverParams<'a> {
    session_id: &'a str,
    title: &'a str,
    cwd: &'a Path,
    spec: &'a AgentLaunchSpec,
    extra_args: Vec<String>,
    initial_mode: Option<String>,
    has_custom_title: bool,
    /// Resume request from an existing agent session (`None` means a new session)
    resume: Option<ResumeRequest>,
}

/// Result of [`SessionManager::start_driver_session`] (restore success).
struct StartedDriverSession {
    /// `true` = the agent-side context was natively restored
    /// (`false` means the caller injects a history replay)
    context_restored: bool,
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sessions = self.inner.sessions.lock().expect("sessions poisoned");
        f.debug_struct("SessionManager")
            .field("active_sessions", &sessions.active.len())
            .finish()
    }
}

impl SessionManager {
    /// Creates a manager.
    pub fn new(
        bus: SessionEventBus,
        paths: NodePaths,
        node_id: impl Into<String>,
        registry: AcpRegistry,
    ) -> Self {
        Self::with_factory(bus, paths, node_id, registry, default_driver_factory())
    }

    /// Creates a manager with a replaced driver factory (for tests).
    pub fn with_factory(
        bus: SessionEventBus,
        paths: NodePaths,
        node_id: impl Into<String>,
        registry: AcpRegistry,
        factory: DriverFactory,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                bus,
                paths,
                node_id: node_id.into(),
                registry,
                factory,
                sessions: Mutex::new(Sessions::default()),
                drivers: Mutex::new(Vec::new()),
            }),
        }
    }

    /// The registry (used by the CLI's `fxg agents` etc.).
    pub fn registry(&self) -> &AcpRegistry {
        &self.inner.registry
    }

    /// Session event bus.
    pub fn bus(&self) -> &SessionEventBus {
        &self.inner.bus
    }

    /// Records a shared driver returned by the factory (deduplicated by pointer).
    ///
    /// `shutdown_all` uses this to discard idle processes (warm pool).
    fn remember_driver(&self, driver: &Arc<dyn AgentDriver>) {
        let mut drivers = self.inner.drivers.lock().expect("drivers poisoned");
        if !drivers.iter().any(|existing| Arc::ptr_eq(existing, driver)) {
            drivers.push(Arc::clone(driver));
        }
    }

    /// Records `command_id` as processed.
    ///
    /// Returns `false` if it was already processed (the caller should return
    /// [`NodeError::CommandDuplicate`]).
    fn begin_command(&self, command_id: &str) -> bool {
        let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
        if sessions
            .processed_commands
            .iter()
            .any(|id| id == command_id)
        {
            return false;
        }
        sessions.processed_commands.push_back(command_id.to_owned());
        while sessions.processed_commands.len() > PROCESSED_COMMAND_HISTORY {
            sessions.processed_commands.pop_front();
        }
        true
    }

    /// Starts a new session (`fxg run`).
    pub async fn ensure_session(
        &self,
        command_id: &str,
        cwd: &Path,
        agent_id: &str,
        extra_args: &[String],
        initial_mode: Option<&str>,
    ) -> Result<EnsureSessionOutcome, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let resolved = project::resolve_project(cwd, &self.inner.node_id).await?;
        let session_id = uuid_v7();

        let spec = self.resolve_launch_spec(agent_id, extra_args).await?;

        // SessionCreated (node_seq = 1)
        let (git_branch, is_worktree) =
            git_metadata_or_warn(&resolved.local_path, &session_id).await;
        let title = format!("{} @ {}", spec.display_name, resolved.name);
        self.inner
            .bus
            .create_session(
                &session_id,
                UnifiedEventPayload::SessionCreated {
                    node_id: self.inner.node_id.clone(),
                    project_id: resolved.project_id.clone(),
                    project_name: resolved.name.clone(),
                    local_path: resolved.local_path.to_string_lossy().into_owned(),
                    git_branch,
                    is_worktree,
                    agent_id: spec.agent_id.clone(),
                    parent_session_id: None,
                    fork_from_node_seq: None,
                    title: title.clone(),
                },
            )
            .await?;

        let _started = self
            .start_driver_session(StartDriverParams {
                session_id: &session_id,
                title: &title,
                cwd: &resolved.local_path,
                spec: &spec,
                extra_args: extra_args.to_vec(),
                initial_mode: initial_mode.map(str::to_owned),
                has_custom_title: false,
                resume: None,
            })
            .await?;

        Ok(EnsureSessionOutcome { session_id })
    }

    /// Starts a new session with an explicit session ID (`StartSession` / local REST).
    ///
    /// Differs from [`Self::ensure_session`] in that (1) the caller assigns
    /// `session_id` and (2) it takes a resolved `local_path` instead of `cwd`.
    /// Project resolution (`project_id`) uses the node-side authoritative data.
    pub async fn start_session(
        &self,
        params: StartSessionParams<'_>,
    ) -> Result<EnsureSessionOutcome, NodeError> {
        if !self.begin_command(params.command_id) {
            return Err(NodeError::CommandDuplicate(params.command_id.to_owned()));
        }

        // Restore a saved Git bundle (handover to another node).
        // Materialize the workspace before resolving the project.
        if let Some(bundle_file) = params.restore_git_bundle {
            crate::bundle::restore_workspace_bundle(params.local_path, bundle_file).await?;
        }

        let resolved = project::resolve_project(params.local_path, &self.inner.node_id).await?;
        let extra_args_vec: Vec<String> = params.extra_args.map(|s| s.to_vec()).unwrap_or_default();
        let spec = self
            .resolve_launch_spec(params.agent_id, &extra_args_vec)
            .await?;

        // SessionCreated (node_seq = 1)
        let (git_branch, is_worktree) =
            git_metadata_or_warn(&resolved.local_path, params.session_id).await;
        let default_title = format!("{} @ {}", spec.display_name, resolved.name);
        let (title, has_custom_title) = if let Some(prompt) = params.initial_prompt
            && let Some(derived) = derive_title_from_prompt(prompt)
        {
            (derived, true)
        } else {
            (default_title, false)
        };
        self.inner
            .bus
            .create_session(
                params.session_id,
                UnifiedEventPayload::SessionCreated {
                    node_id: self.inner.node_id.clone(),
                    project_id: resolved.project_id.clone(),
                    project_name: resolved.name.clone(),
                    local_path: resolved.local_path.to_string_lossy().into_owned(),
                    git_branch,
                    is_worktree,
                    agent_id: spec.agent_id.clone(),
                    parent_session_id: None,
                    fork_from_node_seq: None,
                    title: title.clone(),
                },
            )
            .await?;

        let _started = self
            .start_driver_session(StartDriverParams {
                session_id: params.session_id,
                title: &title,
                cwd: &resolved.local_path,
                spec: &spec,
                extra_args: extra_args_vec,
                initial_mode: params.mode.map(str::to_owned),
                has_custom_title,
                resume: None,
            })
            .await?;

        // Initial prompt (executed as-is when a session is started remotely)
        if let Some(prompt) = params.initial_prompt.filter(|text| !text.trim().is_empty())
            && let Err(err) = self
                .start_turn(params.session_id, prompt, ClientSource::Web)
                .await
        {
            return Err(self
                .fail_context_injection(params.session_id, "initial prompt injection", err)
                .await);
        }

        // Fork from another node / ephemeral VM: inject the history replay as initial context
        if let Some(context) = params
            .fork_context
            .and_then(|items| build_history_replay_context_from_items(items, ReplayPurpose::Fork))
            && let Err(err) = self
                .start_turn(params.session_id, &context, ClientSource::Fork)
                .await
        {
            return Err(self
                .fail_context_injection(params.session_id, "fork context injection", err)
                .await);
        }

        Ok(EnsureSessionOutcome {
            session_id: params.session_id.to_owned(),
        })
    }

    /// Resolves the launch spec (custom/built-in agents need no index).
    async fn resolve_launch_spec(
        &self,
        agent_id: &str,
        extra_args: &[String],
    ) -> Result<AgentLaunchSpec, NodeError> {
        match self
            .inner
            .registry
            .launch_spec(agent_id, &RegistryIndex::default(), extra_args)
            .await
        {
            Ok(spec) => Ok(spec),
            Err(_) => {
                let index = self
                    .inner
                    .registry
                    .index(false)
                    .await
                    .map_err(|err| NodeError::Agent(err.to_string()))?;
                self.inner
                    .registry
                    .launch_spec(agent_id, &index, extra_args)
                    .await
                    .map_err(|err| NodeError::Agent(err.to_string()))
            }
        }
    }

    /// Launches the driver, registers it in the active list, and starts the event pump.
    ///
    /// Records `StatusChanged(Error)` on launch failure. On success, returns the
    /// handle used by the CLI to decide attach mode and the native restore result.
    async fn start_driver_session(
        &self,
        params: StartDriverParams<'_>,
    ) -> Result<StartedDriverSession, NodeError> {
        // Inject the Git credential proxy (GIT_ASKPASS for ephemeral VMs) into the agent process.
        // The script is generated at daemon startup and authenticates via
        // `fxg git-askpass` → local IPC → the central server's in-memory relay
        // (design: docs/01 §6.4).
        let mut spec = params.spec.clone();
        let askpass = crate::credentials::askpass_script_path(self.inner.paths.fxg_home());
        if askpass.exists() {
            for (key, value) in crate::credentials::git_env(self.inner.paths.fxg_home()) {
                spec.env.retain(|(k, _)| k != &key);
                spec.env.push((key, value));
            }
        }
        let params = StartDriverParams {
            spec: &spec,
            ..params
        };

        let driver =
            (self.inner.factory)(params.spec).map_err(|err| NodeError::Agent(err.to_string()))?;
        self.remember_driver(&driver);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let started = match driver
            .start_session(
                fxg_acp::StartSessionRequest {
                    session_id: params.session_id.to_owned(),
                    title: Some(params.title.to_owned()),
                    cwd: params.cwd.to_path_buf(),
                    launch: params.spec.clone(),
                    extra_args: params.extra_args,
                    initial_mode: params.initial_mode,
                    resume: params.resume,
                },
                event_tx,
            )
            .await
        {
            Ok(started) => started,
            Err(err) => {
                // In native-only auto-resume (`allow_fresh = false`), return
                // "native restore unavailable" to the caller while leaving the session stopped
                // (do not implicitly run a history replay; prompt for an explicit Resume)
                if let Some(unavailable) = err.downcast_ref::<NativeResumeUnavailable>() {
                    tracing::info!(
                        session_id = params.session_id,
                        "native resume is not available: {unavailable}"
                    );
                    return Err(NodeError::ResumeRequired {
                        session_id: params.session_id.to_owned(),
                        reason: unavailable.0.clone(),
                    });
                }
                let message = format!("failed to start agent: {err:#}");
                tracing::warn!(session_id = params.session_id, "{message}");
                self.inner
                    .bus
                    .record(
                        params.session_id,
                        UnifiedEventPayload::StatusChanged {
                            status: SessionStatus::Error,
                            error_message: Some(message.clone()),
                        },
                    )
                    .await?;
                return Err(NodeError::Agent(message));
            }
        };

        let context_restored = started.context_restored;
        let handle: Arc<dyn ActiveSessionHandle> = Arc::from(started.handle);
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            sessions.active.insert(
                params.session_id.to_owned(),
                ActiveSession {
                    handle: Arc::clone(&handle),
                    cwd: params.cwd.to_path_buf(),
                    agent_id: params.spec.agent_id.clone(),
                    status: SessionStatus::Idle,
                    busy: false,
                    pending_prompts: VecDeque::new(),
                    pending_permissions: HashMap::new(),
                    pending_elicitations: HashMap::new(),
                    has_custom_title: params.has_custom_title,
                },
            );
        }

        // Event pump
        let inner = Arc::clone(&self.inner);
        let pump_session_id = params.session_id.to_owned();
        fxg_protocol::spawn_guarded!(
            "session event pump",
            pump_events(inner, pump_session_id, event_rx)
        );
        Ok(StartedDriverSession { context_restored })
    }

    /// Loads the persistent events of a session (limited to `up_to_node_seq` and below; `None` means all).
    async fn load_session_events(
        &self,
        session_id: &str,
        up_to_node_seq: Option<u64>,
    ) -> Result<Vec<SessionEventEnvelope>, NodeError> {
        let mut cursor = 0u64;
        let mut events = Vec::new();
        loop {
            let batch = self
                .inner
                .bus
                .db()
                .session_events_after(session_id, cursor, EVENT_LOAD_BATCH)
                .await?;
            let full_batch = batch.events.len() as u32 >= EVENT_LOAD_BATCH;
            cursor = batch.cursor;
            for event in batch.events {
                if let Some(limit) = up_to_node_seq
                    && event.node_seq > limit
                {
                    return Ok(events);
                }
                events.push(event);
            }
            if !full_batch {
                return Ok(events);
            }
        }
    }

    /// Restores the workspace to the point of the specified turn (`fxg session revert`).
    ///
    /// - `target_node_seq` is the `node_seq` of a `UserMessage`
    ///   (the latest turn when omitted). Restores files to the `snapshot_tree_hash`
    ///   at that point and appends `SessionReverted`.
    /// - The state right before restoring is saved as a backup Tree
    ///   (reverting again returns to it).
    /// - Running (busy) sessions are rejected with [`NodeError::Busy`].
    pub async fn revert(
        &self,
        command_id: &str,
        session_id: &str,
        target_node_seq: Option<u64>,
    ) -> Result<RevertOutcome, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let cwd = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions
                .active
                .get(session_id)
                .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;
            if session.busy {
                return Err(NodeError::Busy(session_id.to_owned()));
            }
            session.cwd.clone()
        };

        let events = self
            .load_session_events(session_id, target_node_seq)
            .await?;
        let (target_seq, tree_hash) = find_revert_snapshot(&events).ok_or_else(|| {
            NodeError::InvalidSession(format!(
                "no snapshot found at or before the specified point: {session_id}"
            ))
        })?;

        let tree = ShadowGitTree::new(
            &cwd,
            self.inner.paths.snapshot_index_path(session_id),
            session_id,
        );
        let outcome = tree.restore(&tree_hash).await?;

        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::SessionReverted {
                    target_node_seq: target_seq,
                    restored_tree_hash: tree_hash.clone(),
                    backup_tree_hash: outcome.backup_tree_hash.clone(),
                    restored_files: outcome.restored_files as u64,
                    removed_files: outcome.removed_files as u64,
                },
            )
            .await?;

        Ok(RevertOutcome {
            restored_tree_hash: tree_hash,
            backup_tree_hash: outcome.backup_tree_hash,
            restored_files: outcome.restored_files,
            removed_files: outcome.removed_files,
            target_node_seq: target_seq,
        })
    }

    /// Branches from the specified turn into a new session (`fxg session fork`).
    ///
    /// - Inherits history from the source `SessionCreated` up to `from_node_seq`,
    ///   and records `parent_session_id` / `fork_from_node_seq` on the new
    ///   `SessionCreated`.
    /// - Since standard ACP has no native fork API, the conversation history up to
    ///   `node_seq` is injected as initial context (one prompt)
    ///   (recorded as a `UserMessage` with `client_source = "fork"`).
    pub async fn fork(
        &self,
        command_id: &str,
        session_id: &str,
        from_node_seq: Option<u64>,
        agent_id: Option<&str>,
        new_cwd: Option<&Path>,
    ) -> Result<EnsureSessionOutcome, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let source = self
            .inner
            .bus
            .db()
            .get_session(session_id)
            .await?
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;

        let events = self.load_session_events(session_id, from_node_seq).await?;
        let fork_seq = events
            .last()
            .map(|event| event.node_seq)
            .or(source.fork_from_node_seq)
            .unwrap_or(1);

        let spec = self
            .resolve_launch_spec(agent_id.unwrap_or(&source.agent_id), &[])
            .await?;
        let project_name = self.project_name(&source.project_id).await;

        let new_session_id = uuid_v7();
        let cwd = new_cwd
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(&source.local_path));
        let (git_branch, is_worktree) = git_metadata_or_warn(&cwd, &new_session_id).await;
        let title = format!("Fork of {}", source.title);
        self.inner
            .bus
            .create_session(
                &new_session_id,
                UnifiedEventPayload::SessionCreated {
                    node_id: self.inner.node_id.clone(),
                    project_id: source.project_id.clone(),
                    project_name,
                    local_path: cwd.to_string_lossy().into_owned(),
                    git_branch,
                    is_worktree,
                    agent_id: spec.agent_id.clone(),
                    parent_session_id: Some(source.session_id.clone()),
                    fork_from_node_seq: Some(fork_seq),
                    title: title.clone(),
                },
            )
            .await?;

        let _started = self
            .start_driver_session(StartDriverParams {
                session_id: &new_session_id,
                title: &title,
                cwd: &cwd,
                spec: &spec,
                extra_args: Vec::new(),
                initial_mode: None,
                has_custom_title: true,
                resume: None,
            })
            .await?;

        // History replay injection (initial context for the new agent session)
        if let Some(context) = build_history_replay_context(&events, fork_seq, ReplayPurpose::Fork)
            && let Err(err) = self
                .start_turn(&new_session_id, &context, ClientSource::Fork)
                .await
        {
            return Err(self
                .fail_context_injection(&new_session_id, "fork context injection", err)
                .await);
        }

        Ok(EnsureSessionOutcome {
            session_id: new_session_id,
        })
    }

    /// Resumes a stopped session (`fxg session resume`).
    ///
    /// - Starts the agent under the same `session_id`, preferring native restore
    ///   (`session/resume` → `session/load` → bind an existing OpenCode2 session).
    ///   If restore fails, continues by injecting a history replay.
    /// - Resumability is based on `sessions.active` (the actual running state).
    ///   On forced termination no transition to `stopped` is written, so resuming
    ///   is allowed even when the DB `status` is still `idle` / `running`.
    /// - Ephemeral VM (`provisioning` / `bootstrapping`) sessions cannot be
    ///   resumed in v1 (design: `docs/04-agent-drivers-and-windows.md` §4.3).
    pub async fn resume(&self, params: ResumeParams<'_>) -> Result<ResumeOutcome, NodeError> {
        if !self.begin_command(params.command_id) {
            return Err(NodeError::CommandDuplicate(params.command_id.to_owned()));
        }

        // Exclude double resumes (reject both active and currently starting sessions)
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            if sessions.active.contains_key(params.session_id)
                || !sessions.resuming.insert(params.session_id.to_owned())
            {
                return Err(NodeError::InvalidState(format!(
                    "session is already active or resuming: {}",
                    params.session_id
                )));
            }
        }
        let result = self.resume_session_state(params).await;
        self.inner
            .sessions
            .lock()
            .expect("sessions poisoned")
            .resuming
            .remove(params.session_id);
        result
    }

    /// The body of [`Self::resume`] (the caller releases the exclusive guard).
    async fn resume_session_state(
        &self,
        params: ResumeParams<'_>,
    ) -> Result<ResumeOutcome, NodeError> {
        let (started, events) = self
            .resume_stopped_session(
                params.session_id,
                ResumeMode::AllowReplay {
                    force_replay: params.force_replay,
                },
            )
            .await?;

        // If native restore failed, continue the context via history replay
        if !started.context_restored {
            let up_to_seq = events.last().map(|event| event.node_seq).unwrap_or(0);
            if let Some(context) =
                build_history_replay_context(&events, up_to_seq, ReplayPurpose::Resume)
                && let Err(err) = self
                    .start_turn(params.session_id, &context, ClientSource::Resume)
                    .await
            {
                return Err(self
                    .fail_context_injection(params.session_id, "resume context injection", err)
                    .await);
            }
        }

        Ok(ResumeOutcome {
            session_id: params.session_id.to_owned(),
            context_restored: started.context_restored,
        })
    }

    /// Validates a stopped session, launches the driver, and records up to
    /// `StatusChanged(Idle)` (shared by explicit resume / auto-resume on send).
    ///
    /// - Ephemeral VM (`provisioning` / `bootstrapping`) sessions and missing
    ///   working directories are rejected with [`NodeError::InvalidState`].
    /// - When native restore is impossible in [`ResumeMode::NativeOnly`], the driver
    ///   returns [`NativeResumeUnavailable`], which reaches the caller as
    ///   [`NodeError::ResumeRequired`] (the session remains stopped).
    ///
    /// Returns the started session and the loaded events (for replay decisions).
    async fn resume_stopped_session(
        &self,
        session_id: &str,
        mode: ResumeMode,
    ) -> Result<(StartedDriverSession, Vec<SessionEventEnvelope>), NodeError> {
        let session = self
            .inner
            .bus
            .db()
            .get_session(session_id)
            .await?
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;

        // Resuming an ephemeral VM (provisioner) session is v2 (not yet supported)
        if matches!(
            session.status,
            SessionStatus::Provisioning | SessionStatus::Bootstrapping
        ) {
            return Err(NodeError::InvalidState(format!(
                "resuming ephemeral sessions is not supported yet: {session_id}"
            )));
        }

        // Cannot resume if the working directory is gone (e.g. worktree removed)
        let cwd = PathBuf::from(&session.local_path);
        if !cwd.is_dir() {
            return Err(NodeError::InvalidState(format!(
                "session workspace does not exist: {}",
                session.local_path
            )));
        }

        let events = self.load_session_events(session_id, None).await?;
        let spec = self.resolve_launch_spec(&session.agent_id, &[]).await?;

        // Attempt native restore (the caller injects a replay if it fails)
        let (agent_session_id, allow_fresh) = match mode {
            // With `--replay`, do not pass the agent-side ID so a fresh session is created
            ResumeMode::AllowReplay { force_replay: true } => (None, true),
            ResumeMode::AllowReplay {
                force_replay: false,
            } => (session.agent_session_id.clone(), true),
            ResumeMode::NativeOnly => (session.agent_session_id.clone(), false),
        };
        let started = self
            .start_driver_session(StartDriverParams {
                session_id,
                title: &session.title,
                cwd: &cwd,
                spec: &spec,
                extra_args: Vec::new(),
                initial_mode: None,
                has_custom_title: true,
                resume: Some(ResumeRequest {
                    agent_session_id,
                    allow_fresh,
                }),
            })
            .await?;

        // Reflect the resume in the state projection (stopped / error → idle)
        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Idle,
                    error_message: None,
                },
            )
            .await?;

        Ok((started, events))
    }

    /// Automatically resumes a stopped session via native restore (before sending a prompt).
    ///
    /// Serialized by the same `resuming` guard as explicit resume; if another
    /// resume is in progress, waits for it to finish before returning control.
    /// Returns [`NodeError::ResumeRequired`] when native restore is impossible and
    /// leaves the session stopped (does not implicitly run a history replay; prompts
    /// the client for an explicit Resume).
    async fn auto_resume_for_prompt(&self, session_id: &str) -> Result<(), NodeError> {
        let deadline = Instant::now() + AUTO_RESUME_TIMEOUT;
        loop {
            // Confine guard acquisition and checks to a scope so the lock is not held across await
            let acquired = {
                let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
                if sessions.active.contains_key(session_id) {
                    return Ok(());
                }
                sessions.resuming.insert(session_id.to_owned())
            };
            if acquired {
                let result = self
                    .resume_stopped_session(session_id, ResumeMode::NativeOnly)
                    .await
                    .map(|(started, _events)| {
                        // Driver contract: in native-only mode (`allow_fresh = false`)
                        // launch fails (Err) when restore is impossible, so only
                        // `context_restored = true` is normal here
                        if !started.context_restored {
                            tracing::warn!(
                                session_id,
                                "native-only resume returned a fresh session"
                            );
                        }
                    });
                self.inner
                    .sessions
                    .lock()
                    .expect("sessions poisoned")
                    .resuming
                    .remove(session_id);
                return result;
            }
            if Instant::now() >= deadline {
                return Err(NodeError::InvalidState(format!(
                    "session is resuming: {session_id}"
                )));
            }
            tokio::time::sleep(RESUME_WAIT_INTERVAL).await;
        }
    }

    /// Resolves the display name of a logical project (returns `project_id` if unknown).
    async fn project_name(&self, project_id: &str) -> String {
        match self.inner.bus.db().list_projects().await {
            Ok(projects) => projects
                .into_iter()
                .find(|project| project.project_id == project_id)
                .map(|project| project.name)
                .unwrap_or_else(|| project_id.to_owned()),
            Err(_) => project_id.to_owned(),
        }
    }

    /// Starts a turn (busy reservation → snapshot → `UserMessage` → send).
    ///
    /// The caller has already decided busy handling (append to the pending queue or reject).
    async fn start_turn(
        &self,
        session_id: &str,
        text: &str,
        client_source: ClientSource,
    ) -> Result<(), NodeError> {
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions
                .active
                .get_mut(session_id)
                .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;
            session.busy = true;
        }
        match dispatch_prompt(&self.inner, session_id, text, client_source).await {
            Ok(()) => Ok(()),
            Err(err) => {
                // Clear busy on send failure to keep the state consistent
                if let Some(session) = self
                    .inner
                    .sessions
                    .lock()
                    .expect("sessions poisoned")
                    .active
                    .get_mut(session_id)
                {
                    session.busy = false;
                }
                Err(err)
            }
        }
    }

    /// Sends a prompt (enqueues into the pending queue when busy).
    ///
    /// If the session is stopped (not active), first attempts auto-resume via
    /// native restore and then sends. Agents that do not support native restore
    /// cause [`NodeError::ResumeRequired`], so the client should guide the user to
    /// an explicit Resume (history replay).
    pub async fn send_prompt(
        &self,
        command_id: &str,
        session_id: &str,
        text: &str,
        client_source: ClientSource,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let active = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            sessions.active.contains_key(session_id)
        };
        if !active {
            // Auto-resume stopped sessions (native restore only) before sending
            self.auto_resume_for_prompt(session_id).await?;
        }

        // Busy check and reservation (check-and-set)
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions.active.get_mut(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
            if session.busy {
                session.pending_prompts.push_back(PendingPrompt {
                    text: text.to_owned(),
                    client_source,
                });
                return Ok(());
            }
        }

        self.start_turn(session_id, text, client_source).await
    }

    /// Responds to a permission request (`AlreadyResolved` on the second and later attempts).
    pub async fn respond_permission(
        &self,
        command_id: &str,
        session_id: &str,
        request_id: &str,
        selected_option_id: &str,
        resolved_by: ResolvedBy,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let handle = {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions.active.get_mut(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
            if session.pending_permissions.remove(request_id).is_none() {
                return Err(NodeError::AlreadyResolved(request_id.to_owned()));
            }
            Arc::clone(&session.handle)
        };
        handle
            .respond_permission(request_id.to_owned(), selected_option_id.to_owned())
            .await
            .map_err(|err| NodeError::Server(format!("failed to respond: {err:#}")))?;

        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::PermissionResolved {
                    request_id: request_id.to_owned(),
                    selected_option_id: selected_option_id.to_owned(),
                    resolved_by,
                },
            )
            .await?;
        Ok(())
    }

    /// Responds to an elicitation (structured input) (`AlreadyResolved` on the second and later attempts).
    pub async fn respond_elicitation(
        &self,
        command_id: &str,
        session_id: &str,
        elicitation_id: &str,
        action: ElicitationAction,
        content: Option<ElicitationContent>,
        resolved_by: ResolvedBy,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let (handle, summary) = {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions.active.get_mut(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
            let Some(summary) = session.pending_elicitations.remove(elicitation_id) else {
                return Err(NodeError::AlreadyResolved(elicitation_id.to_owned()));
            };
            (Arc::clone(&session.handle), summary)
        };
        if let Err(err) = handle
            .respond_elicitation(elicitation_id.to_owned(), action, content.clone())
            .await
        {
            // If the driver rejects it (e.g. schema violation in content), restore the
            // pending entry so the user can fix and resend it
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            if let Some(session) = sessions.active.get_mut(session_id) {
                session
                    .pending_elicitations
                    .insert(elicitation_id.to_owned(), summary);
            }
            return Err(NodeError::Server(format!("failed to respond: {err:#}")));
        }

        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::ElicitationResolved {
                    elicitation_id: elicitation_id.to_owned(),
                    action,
                    content,
                    resolved_by,
                },
            )
            .await?;
        Ok(())
    }

    /// Mode switching / config change / cancel / kill.
    pub async fn control(
        &self,
        command_id: &str,
        session_id: &str,
        action: &SessionControlAction,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        // Do not hold the lock across await (clone the handle, then operate)
        let (handle, is_busy) = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions.active.get(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
            (Arc::clone(&session.handle), session.busy)
        };
        match action {
            SessionControlAction::SetMode { mode_id } => handle
                .set_mode(mode_id.clone())
                .await
                .map_err(|err| NodeError::Server(format!("failed to set mode: {err:#}"))),
            SessionControlAction::SetConfig { key, value } => handle
                .set_config(key.clone(), value.clone())
                .await
                .map_err(|err| NodeError::Server(format!("failed to set config: {err:#}"))),
            // Compaction runs as a single turn, so allow it while busy
            SessionControlAction::Cancel => {
                // If already idle (no turn running) there is nothing to cancel, so treat as success
                if !is_busy {
                    return Ok(());
                }
                handle
                    .cancel_turn()
                    .await
                    .map_err(|err| NodeError::Server(format!("failed to cancel: {err:#}")))
            }
            SessionControlAction::Kill => handle
                .shutdown()
                .await
                .map_err(|err| NodeError::Server(format!("failed to shutdown: {err:#}"))),
        }
    }

    /// Archives/restores a session (`fxg session archive` / Web UI).
    ///
    /// Archiving is a reversible visibility flag; it does not change the running state.
    /// Returns the archive timestamp (Unix epoch ms; `None` when restoring).
    pub async fn set_archived(
        &self,
        command_id: &str,
        session_id: &str,
        archived: bool,
    ) -> Result<Option<i64>, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }
        self.ensure_session_exists(session_id).await?;
        let envelope = self
            .inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::SessionArchived { archived },
            )
            .await?;
        Ok(archived.then_some(envelope.created_at))
    }

    /// Deletes a session (`fxg session delete` / Web UI).
    ///
    /// If active (agent process alive), waits for it to stop before appending
    /// `SessionDeleted` (tombstone). Event bodies before this event are purged in
    /// the same transaction (irreversible).
    pub async fn delete(&self, command_id: &str, session_id: &str) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }
        self.ensure_session_exists(session_id).await?;
        {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            if sessions.resuming.contains(session_id) {
                return Err(NodeError::InvalidState(format!(
                    "session is resuming: {session_id}"
                )));
            }
        }
        self.stop_if_active(session_id).await?;
        self.inner
            .bus
            .record(session_id, UnifiedEventPayload::SessionDeleted {})
            .await?;
        Ok(())
    }

    /// Whether the session is active (agent process alive).
    pub fn is_active(&self, session_id: &str) -> bool {
        self.inner
            .sessions
            .lock()
            .expect("sessions poisoned")
            .active
            .contains_key(session_id)
    }

    /// Verifies that the session exists (has not been deleted).
    async fn ensure_session_exists(&self, session_id: &str) -> Result<(), NodeError> {
        if self.inner.bus.db().get_session(session_id).await?.is_none() {
            return Err(NodeError::InvalidSession(session_id.to_owned()));
        }
        Ok(())
    }

    /// Stops an active session and waits for the agent process to exit.
    ///
    /// Stops a session whose context injection failed and records the reason as
    /// its error status.
    ///
    /// Returning a plain error while leaving the session active would make a
    /// retry fail with "already active" and leave the user with a session that
    /// looks usable but has no context. Stopping it first keeps the failure
    /// recoverable.
    async fn fail_context_injection(
        &self,
        session_id: &str,
        purpose: &str,
        err: NodeError,
    ) -> NodeError {
        let message = format!("{purpose} failed: {err}");
        tracing::warn!(session_id, "{message}");
        if let Err(stop_err) = self.stop_if_active(session_id).await {
            tracing::warn!(
                session_id,
                "failed to stop session after context injection failure: {stop_err:#}"
            );
        }
        if let Err(record_err) = self
            .inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Error,
                    error_message: Some(message.clone()),
                },
            )
            .await
        {
            tracing::warn!(
                session_id,
                "failed to record context injection failure: {record_err:#}"
            );
        }
        NodeError::Agent(message)
    }

    /// On timeout it only warns and continues (does not block deletion; later events
    /// are discarded as appends to a deleted session).
    async fn stop_if_active(&self, session_id: &str) -> Result<(), NodeError> {
        if !self.is_active(session_id) {
            return Ok(());
        }
        let command_id = uuid_v7();
        match self
            .control(&command_id, session_id, &SessionControlAction::Kill)
            .await
        {
            // It is fine if it had already terminated naturally after the check
            Ok(()) | Err(NodeError::InvalidSession(_)) => {}
            Err(err) => return Err(err),
        }
        let deadline = Instant::now() + STOP_BEFORE_DELETE_TIMEOUT;
        while self.is_active(session_id) {
            if Instant::now() >= deadline {
                tracing::warn!(
                    session_id,
                    "timed out waiting for session to stop before delete"
                );
                break;
            }
            tokio::time::sleep(RESUME_WAIT_INTERVAL).await;
        }
        Ok(())
    }

    /// List of active sessions.
    pub fn list_active(&self) -> Vec<ActiveSessionSummary> {
        let sessions = self.inner.sessions.lock().expect("sessions poisoned");
        let mut list: Vec<ActiveSessionSummary> = sessions
            .active
            .iter()
            .map(|(session_id, session)| ActiveSessionSummary {
                session_id: session_id.clone(),
                agent_id: session.agent_id.clone(),
                cwd: session.cwd.clone(),
                status: session.status,
                busy: session.busy,
                queued_prompts: session.pending_prompts.len(),
            })
            .collect();
        list.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        list
    }

    /// List of pending permission requests (`fxg inbox list`).
    pub fn pending_permissions(&self) -> Vec<PendingPermissionSummary> {
        let sessions = self.inner.sessions.lock().expect("sessions poisoned");
        let mut list: Vec<PendingPermissionSummary> = sessions
            .active
            .values()
            .flat_map(|session| session.pending_permissions.values().cloned())
            .collect();
        list.sort_by(|a, b| a.request_id.cmp(&b.request_id));
        list
    }

    /// List of pending elicitations (`fxg inbox list`).
    pub fn pending_elicitations(&self) -> Vec<PendingElicitationSummary> {
        let sessions = self.inner.sessions.lock().expect("sessions poisoned");
        let mut list: Vec<PendingElicitationSummary> = sessions
            .active
            .values()
            .flat_map(|session| session.pending_elicitations.values().cloned())
            .collect();
        list.sort_by(|a, b| a.elicitation_id.cmp(&b.elicitation_id));
        list
    }

    /// Terminates the agent processes of all sessions (`fxg kill-all`).
    pub async fn shutdown_all(&self) -> Vec<String> {
        let handles: Vec<(String, Arc<dyn ActiveSessionHandle>)> = {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            sessions
                .active
                .drain()
                .map(|(session_id, session)| (session_id, session.handle))
                .collect()
        };
        let mut stopped = Vec::with_capacity(handles.len());
        for (session_id, handle) in handles {
            // Discard completely instead of keeping it warm
            // (`shutdown` returns it to the pool, so do not use it)
            if let Err(err) = handle.dispose().await {
                tracing::warn!(session_id = %session_id, "failed to shutdown: {err:#}");
            }
            remove_snapshot_index(&self.inner.paths, &session_id);
            stopped.push(session_id);
        }
        // Also discard idle (warm) processes to empty the pool
        let drivers: Vec<Arc<dyn AgentDriver>> =
            { self.inner.drivers.lock().expect("drivers poisoned").clone() };
        for driver in drivers {
            let disposed = driver.shutdown_idle().await;
            if disposed > 0 {
                tracing::info!(
                    driver = %driver.driver_kind(),
                    count = disposed,
                    "disposed idle agent processes"
                );
            }
        }
        stopped
    }
}

/// Persists and delivers driver events, managing status transitions and queues.
async fn pump_events(
    inner: Arc<Inner>,
    session_id: String,
    mut event_rx: mpsc::UnboundedReceiver<DriverEvent>,
) {
    while let Some(event) = event_rx.recv().await {
        match event {
            DriverEvent::Delta(delta) => inner.bus.publish_delta(&session_id, delta),
            DriverEvent::Failed { message } => {
                mark_status(
                    &inner,
                    &session_id,
                    SessionStatus::Error,
                    Some(message.clone()),
                );
                if let Err(err) = inner
                    .bus
                    .record(
                        &session_id,
                        UnifiedEventPayload::StatusChanged {
                            status: SessionStatus::Error,
                            error_message: Some(message),
                        },
                    )
                    .await
                {
                    tracing::warn!(
                        session_id = %session_id,
                        "failed to record the driver failure: {err}"
                    );
                }
            }
            DriverEvent::Event(payload) => {
                match &payload {
                    UnifiedEventPayload::StatusChanged {
                        status,
                        error_message,
                    } => {
                        mark_status(&inner, &session_id, *status, error_message.clone());
                    }
                    UnifiedEventPayload::PermissionRequest {
                        request_id,
                        tool_name,
                        summary,
                        options,
                        ..
                    } => {
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        if let Some(session) = sessions.active.get_mut(&session_id) {
                            session.pending_permissions.insert(
                                request_id.clone(),
                                PendingPermissionSummary {
                                    session_id: session_id.clone(),
                                    request_id: request_id.clone(),
                                    tool_name: tool_name.clone(),
                                    summary: summary.clone(),
                                    options: options.clone(),
                                },
                            );
                        }
                    }
                    UnifiedEventPayload::PermissionResolved { request_id, .. } => {
                        // Permissions resolved on the driver side (e.g. OpenCode2's
                        // native TUI) also arrive here. Responses via fxg are already
                        // recorded by `SessionManager::respond_permission`,
                        // so only record it if it is still in the pending list.
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        let already_recorded =
                            sessions.active.get_mut(&session_id).is_some_and(|session| {
                                session.pending_permissions.remove(request_id).is_none()
                            });
                        if already_recorded {
                            continue;
                        }
                    }
                    UnifiedEventPayload::ElicitationRequest {
                        elicitation_id,
                        message,
                        mode,
                        requested_schema,
                        ..
                    } => {
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        if let Some(session) = sessions.active.get_mut(&session_id) {
                            session.pending_elicitations.insert(
                                elicitation_id.clone(),
                                PendingElicitationSummary {
                                    session_id: session_id.clone(),
                                    elicitation_id: elicitation_id.clone(),
                                    message: message.clone(),
                                    mode: mode.clone(),
                                    requested_schema: requested_schema.clone(),
                                },
                            );
                        }
                    }
                    UnifiedEventPayload::ElicitationResolved { elicitation_id, .. } => {
                        // Elicitations resolved on the driver side (e.g. automatic
                        // resolution on turn cancel) also arrive here. Responses via fxg
                        // are already recorded by `SessionManager::respond_elicitation`,
                        // so only record it if it is still in the pending list.
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        let already_recorded =
                            sessions.active.get_mut(&session_id).is_some_and(|session| {
                                session
                                    .pending_elicitations
                                    .remove(elicitation_id)
                                    .is_none()
                            });
                        if already_recorded {
                            continue;
                        }
                    }
                    UnifiedEventPayload::SessionTitleChanged { .. } => {
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        if let Some(session) = sessions.active.get_mut(&session_id) {
                            session.has_custom_title = true;
                        }
                    }
                    _ => {}
                }
                match inner.bus.record(&session_id, payload).await {
                    Ok(_) => maybe_dispatch_queued(&inner, &session_id).await,
                    Err(err) => {
                        tracing::warn!(session_id = %session_id, "failed to persist driver event: {err:#}");
                    }
                }
            }
        }
    }
    // Channel closed = the driver (agent process) exited.
    // The transition to `Stopped` must also be reflected in the DB `sessions`
    // projection, so append it as an event (leave it as-is if already Stopped / Error).
    let previous = {
        let sessions = inner.sessions.lock().expect("sessions poisoned");
        sessions
            .active
            .get(&session_id)
            .map(|session| session.status)
    };
    mark_status(&inner, &session_id, SessionStatus::Stopped, None);
    if matches!(
        previous,
        Some(
            SessionStatus::Provisioning
                | SessionStatus::Bootstrapping
                | SessionStatus::Idle
                | SessionStatus::Running
                | SessionStatus::WaitingPermission
                | SessionStatus::WaitingInput
        )
    ) && let Err(err) = inner
        .bus
        .record(
            &session_id,
            UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Stopped,
                error_message: None,
            },
        )
        .await
    {
        tracing::warn!(
            session_id = %session_id,
            "failed to record the stopped status: {err}"
        );
    }
    {
        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
        sessions.active.remove(&session_id);
    }
    remove_snapshot_index(&inner.paths, &session_id);
}

/// Removes the session's shadow Git index file (releases resources).
fn remove_snapshot_index(paths: &NodePaths, session_id: &str) {
    let path = paths.snapshot_index_path(session_id);
    if let Err(err) = std::fs::remove_file(&path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(session_id, path = %path.display(), "failed to remove snapshot index: {err:#}");
    }
}

/// Updates the session status (busy is synchronized too).
fn mark_status(
    inner: &Inner,
    session_id: &str,
    status: SessionStatus,
    _error_message: Option<String>,
) {
    let mut sessions = inner.sessions.lock().expect("sessions poisoned");
    if let Some(session) = sessions.active.get_mut(session_id) {
        session.busy = matches!(
            status,
            SessionStatus::Running | SessionStatus::WaitingPermission | SessionStatus::WaitingInput
        );
        session.status = status;
    }
}

/// Sends the prompt at the head of the queue after a turn ends.
async fn maybe_dispatch_queued(inner: &Arc<Inner>, session_id: &str) {
    let next = {
        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
        let Some(session) = sessions.active.get_mut(session_id) else {
            return;
        };
        if session.busy {
            return;
        }
        match session.pending_prompts.pop_front() {
            Some(prompt) => {
                session.busy = true;
                Some(prompt)
            }
            None => None,
        }
    };
    let Some(prompt) = next else {
        return;
    };

    // busy has already been reserved by the caller. Clear it on failure to keep the state consistent.
    if let Err(err) = dispatch_prompt(
        inner,
        session_id,
        &prompt.text,
        prompt.client_source.clone(),
    )
    .await
    {
        tracing::warn!(session_id = %session_id, "failed to dispatch queued prompt: {err:#}");
        if let Err(record_err) = inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Error,
                    error_message: Some(format!("failed to dispatch queued prompt: {err:#}")),
                },
            )
            .await
        {
            tracing::warn!(
                session_id = %session_id,
                "failed to record the queued prompt failure: {record_err}"
            );
        }
        if let Some(session) = inner
            .sessions
            .lock()
            .expect("sessions poisoned")
            .active
            .get_mut(session_id)
        {
            session.busy = false;
        }
    }
}

/// Derives a human-readable session title from the beginning of the prompt.
///
/// - Trims leading whitespace, blank lines and Markdown markers (`#`, `-`, `*`, `>`, backticks, etc.)
/// - Takes the first line (up to 40 characters; appends `...` when exceeded)
/// - Returns `None` if no usable string can be extracted
pub fn derive_title_from_prompt(prompt: &str) -> Option<String> {
    for line in prompt.lines() {
        let trimmed = line.trim();
        let cleaned = trimmed.trim_start_matches(['#', '>', '-', '*', '`']).trim();
        if !cleaned.is_empty() {
            let char_count = cleaned.chars().count();
            if char_count <= 40 {
                return Some(cleaned.to_owned());
            } else {
                let truncated: String = cleaned.chars().take(40).collect();
                return Some(format!("{truncated}..."));
            }
        }
    }
    None
}

/// Starts a single turn (snapshot → record `UserMessage` → send to the driver).
///
/// Busy reservation and release are handled by the caller (`SessionManager::start_turn` /
/// [`maybe_dispatch_queued`]).
async fn dispatch_prompt(
    inner: &Arc<Inner>,
    session_id: &str,
    text: &str,
    client_source: ClientSource,
) -> Result<(), NodeError> {
    let (cwd, should_update_title) = {
        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
        let session = sessions
            .active
            .get_mut(session_id)
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;
        let should_update = !session.has_custom_title;
        if should_update {
            session.has_custom_title = true;
        }
        (session.cwd.clone(), should_update)
    };

    if should_update_title && let Some(title) = derive_title_from_prompt(text) {
        inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::SessionTitleChanged { title },
            )
            .await?;
    }

    let snapshot_tree_hash = snapshot_tree_hash(
        &cwd,
        &inner.paths.snapshot_index_path(session_id),
        session_id,
    )
    .await;

    inner
        .bus
        .record(
            session_id,
            UnifiedEventPayload::UserMessage {
                text: text.to_owned(),
                attachments: Vec::new(),
                client_source,
                snapshot_tree_hash,
            },
        )
        .await?;

    let handle = {
        let sessions = inner.sessions.lock().expect("sessions poisoned");
        sessions
            .active
            .get(session_id)
            .map(|session| Arc::clone(&session.handle))
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?
    };
    handle
        .send_prompt(text.to_owned())
        .await
        .map_err(|err| NodeError::Server(format!("failed to send prompt: {err:#}")))
}

/// Selects the revert baseline snapshot from the loaded events.
///
/// Returns the last `UserMessage.snapshot_tree_hash` (`node_seq`, Tree Hash).
fn find_revert_snapshot(events: &[SessionEventEnvelope]) -> Option<(u64, String)> {
    events.iter().rev().find_map(|event| match &event.payload {
        UnifiedEventPayload::UserMessage {
            snapshot_tree_hash: Some(tree_hash),
            ..
        } => Some((event.node_seq, tree_hash.clone())),
        _ => None,
    })
}

/// Purpose of injecting a history replay (used for the introductory text at the start of the prompt).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplayPurpose {
    /// Conversation branching (`fxg session fork`)
    Fork,
    /// Resuming a stopped session (`fxg session resume`)
    Resume,
}

impl ReplayPurpose {
    /// Builds the introductory text at the start of the prompt.
    fn header(self, range: &str) -> String {
        match self {
            Self::Fork => format!(
                "The history below is from a previous session{range} forked at this point. \
                 Continue working with this context in mind.\n\n"
            ),
            Self::Resume => format!(
                "The history below is from a stopped session{range} that has been resumed. \
                 Continue working with this context in mind.\n\n"
            ),
        }
    }
}

/// Builds the conversation context for a history replay (fork / resume).
///
/// Concatenates the conversation (user / agent messages) plus tool call summaries
/// and changed files in `node_seq` order (truncated at `up_to_seq`).
/// Returns `None` when there is no conversation (no history).
fn build_history_replay_context(
    events: &[SessionEventEnvelope],
    up_to_seq: u64,
    purpose: ReplayPurpose,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for event in events {
        if event.node_seq > up_to_seq {
            break;
        }
        match &event.payload {
            UnifiedEventPayload::UserMessage { text, .. } => {
                parts.push(format!("### User\n{text}"));
            }
            UnifiedEventPayload::AgentMessage {
                text,
                is_complete: true,
                ..
            } => {
                parts.push(format!("### Assistant\n{text}"));
            }
            UnifiedEventPayload::ToolCall {
                title,
                status,
                locations,
                diff,
                ..
            } => {
                let mut line = format!("- [{status}] {title}");
                if !locations.is_empty() {
                    line.push_str(&format!(" ({})", locations.join(", ")));
                }
                if let Some(diff) = diff {
                    line.push_str(&format!(
                        " — {} (+{}/-{})",
                        diff.path, diff.additions, diff.deletions
                    ));
                }
                parts.push(line);
            }
            _ => {}
        }
    }
    assemble_replay_context(parts, up_to_seq, purpose)
}

/// Builds the context for replay injection from the structured history
/// ([`ForkHistoryItem`]) received from the central server (handover to another node / ephemeral VM).
fn build_history_replay_context_from_items(
    items: &[ForkHistoryItem],
    purpose: ReplayPurpose,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for item in items {
        let role = if item.role == HistoryRole::User {
            "User"
        } else {
            "Assistant"
        };
        if !item.text.trim().is_empty() {
            parts.push(format!("### {role}\n{}", item.text));
        }
        if let Some(summary) = item.tool_summary.as_deref()
            && !summary.trim().is_empty()
        {
            parts.push(format!("- {summary}"));
        }
    }
    assemble_replay_context(parts, 0, purpose)
}

/// Shared assembly of the replay context (truncated at the character limit).
fn assemble_replay_context(
    parts: Vec<String>,
    up_to_seq: u64,
    purpose: ReplayPurpose,
) -> Option<String> {
    /// Maximum number of characters in the injected context (prevents oversized prompts).
    const MAX_CONTEXT_CHARS: usize = 16_000;

    if parts.is_empty() {
        return None;
    }
    let range = if up_to_seq > 0 {
        format!(" (node_seq <= {up_to_seq})")
    } else {
        String::new()
    };
    let mut context = purpose.header(&range);
    for part in parts {
        if context.chars().count() + part.chars().count() > MAX_CONTEXT_CHARS {
            context.push_str("\n(history omitted because it exceeded the limit)");
            break;
        }
        context.push_str(&part);
        context.push_str("\n\n");
    }
    Some(context)
}

/// Captures a Shadow Git Tree snapshot of the working directory.
///
/// `None` for non-Git directories or on failure (recorded as a turn that cannot
/// be reverted). Failures are logged at `warn` with the working directory so
/// that a later "no snapshot" revert error can be traced back to this turn.
async fn snapshot_tree_hash(cwd: &Path, index_path: &Path, session_id: &str) -> Option<String> {
    let tree = ShadowGitTree::new(cwd, index_path, session_id);
    match tree.snapshot().await {
        Ok(outcome) => outcome.tree_hash().map(str::to_owned),
        Err(err) => {
            tracing::warn!(
                session_id,
                cwd = %cwd.display(),
                "snapshot skipped (revert will be unavailable for this turn): {err:#}"
            );
            None
        }
    }
}

/// Returns the current branch and the worktree determination.
async fn branch_and_worktree(cwd: &Path) -> Result<(Option<String>, bool), NodeError> {
    let branch = crate::git::try_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    let git_dir = crate::git::try_git(cwd, &["rev-parse", "--git-dir"]).await?;
    let is_worktree = git_dir
        .as_deref()
        .map(|dir| dir.contains("worktrees"))
        .unwrap_or(false);
    Ok((branch, is_worktree))
}

/// Resolves git metadata for `SessionCreated`, degrading to "unknown" with a
/// warning instead of silently losing the branch / worktree flags.
async fn git_metadata_or_warn(cwd: &Path, session_id: &str) -> (Option<String>, bool) {
    match branch_and_worktree(cwd).await {
        Ok(metadata) => metadata,
        Err(err) => {
            tracing::warn!(
                session_id,
                cwd = %cwd.display(),
                "failed to resolve git metadata: {err:#}"
            );
            (None, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockAgent;
    use fxg_db::{Db, DbRole};
    use fxg_protocol::common::{EventType, PermissionOptionKind};
    use fxg_protocol::config::{AgentsConfig, CustomAgentConfig};

    async fn setup() -> (SessionManager, MockAgent, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open_in_memory(DbRole::Node).await.expect("db");
        db.upsert_node(&fxg_db::NodeRecord::new(
            "test-node",
            "Test Node",
            "linux",
            "aarch64",
            "0.1.0",
        ))
        .await
        .expect("node");
        let bus = SessionEventBus::new(db);
        let paths = NodePaths::new(dir.path().to_path_buf());
        paths.ensure_dirs().expect("dirs");

        // Register a custom agent "mock" (resolved without an index)
        let mut custom = std::collections::BTreeMap::new();
        custom.insert(
            "mock".to_owned(),
            CustomAgentConfig {
                name: Some("Mock Agent".to_owned()),
                command: "mock".to_owned(),
                args: Vec::new(),
                env: Default::default(),
            },
        );
        let config = AgentsConfig {
            custom,
            ..AgentsConfig::default()
        };
        let registry = AcpRegistry::new(dir.path(), &config);

        let mock = MockAgent::default();
        let manager =
            SessionManager::with_factory(bus, paths, "test-node", registry, mock.factory());
        (manager, mock, dir)
    }

    /// Waits until the session becomes Idle (waits for the driver's initial event processing).
    async fn wait_idle(manager: &SessionManager, session_id: &str) {
        for _ in 0..100 {
            if manager
                .list_active()
                .iter()
                .any(|session| session.session_id == session_id && !session.busy)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("session did not become idle");
    }

    /// Waits until `busy = false` (waits for the turn to finish).
    async fn wait_not_busy(manager: &SessionManager, session_id: &str) {
        for _ in 0..200 {
            let busy = manager
                .list_active()
                .iter()
                .find(|session| session.session_id == session_id)
                .map(|session| session.busy);
            match busy {
                Some(false) | None => return,
                _ => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
        panic!("session did not finish its turn");
    }

    /// Waits until a `UserMessage` with the given text is persisted and returns its `node_seq`.
    async fn wait_user_message(manager: &SessionManager, session_id: &str, text: &str) -> u64 {
        for _ in 0..200 {
            let batch = manager
                .bus()
                .db()
                .session_events_after(session_id, 0, 100)
                .await
                .expect("events");
            if let Some(event) = batch.events.iter().find(|event| {
                matches!(&event.payload, UnifiedEventPayload::UserMessage { text: body, .. } if body == text)
            }) {
                return event.node_seq;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("UserMessage was not recorded: {text}");
    }

    /// Waits until an event with the given `event_type` is persisted.
    async fn wait_event_type(manager: &SessionManager, session_id: &str, event_type: EventType) {
        for _ in 0..200 {
            let batch = manager
                .bus()
                .db()
                .session_events_after(session_id, 0, 100)
                .await
                .expect("events");
            if batch
                .events
                .iter()
                .any(|event| event.payload.event_type() == event_type)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("event was not recorded: {event_type}");
    }

    /// Prepares a Git repository (`repo`) for tests and starts a session.
    async fn setup_repo_session(
        manager: &SessionManager,
        dir: &tempfile::TempDir,
    ) -> (PathBuf, String) {
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        crate::testutil::init_test_repo(&repo).await;
        let outcome = manager
            .ensure_session("c1", &repo, "mock", &[], None)
            .await
            .expect("ensure");
        wait_idle(manager, &outcome.session_id).await;
        (repo, outcome.session_id)
    }

    #[tokio::test]
    async fn ensure_session_records_created_event_and_start() {
        let (manager, _mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");
        assert!(!outcome.session_id.is_empty());

        // SessionCreated is persisted and the sessions projection exists
        let session = manager
            .bus()
            .db()
            .get_session(&outcome.session_id)
            .await
            .expect("query")
            .expect("session row");
        assert_eq!(session.agent_id, "mock");
    }

    #[tokio::test]
    async fn send_prompt_records_user_message_and_dispatches() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        manager
            .send_prompt("c2", &outcome.session_id, "hello", ClientSource::Cli)
            .await
            .expect("prompt");

        // Sent to the driver and becomes busy
        for _ in 0..100 {
            if !mock.prompts().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(mock.prompts(), vec!["hello"]);
        let summary = manager
            .list_active()
            .into_iter()
            .find(|session| session.session_id == outcome.session_id)
            .expect("active");
        assert!(summary.busy);
    }

    #[tokio::test]
    async fn busy_prompt_is_queued_and_drained_on_idle() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        manager
            .send_prompt("c2", &outcome.session_id, "first", ClientSource::Cli)
            .await
            .expect("first");
        // Enqueued while busy
        manager
            .send_prompt("c3", &outcome.session_id, "second", ClientSource::Web)
            .await
            .expect("second queued");
        let summary = manager
            .list_active()
            .into_iter()
            .find(|session| session.session_id == outcome.session_id)
            .expect("active");
        assert_eq!(summary.queued_prompts, 1);
        assert_eq!(mock.prompts(), vec!["first"]);

        // Turn complete (idle) → the queue is dispatched automatically
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        for _ in 0..100 {
            if mock.prompts().len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(mock.prompts(), vec!["first", "second"]);
    }

    #[tokio::test]
    async fn duplicate_command_id_is_rejected() {
        let (manager, _mock, dir) = setup().await;
        manager
            .ensure_session("c1", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");
        let err = manager
            .ensure_session("c1", dir.path(), "mock", &[], None)
            .await
            .expect_err("duplicate");
        assert!(matches!(err, NodeError::CommandDuplicate(_)));
    }

    #[tokio::test]
    async fn cancel_on_idle_session_is_safe_noop() {
        let (manager, _mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        let res = manager
            .control("c2", &outcome.session_id, &SessionControlAction::Cancel)
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn permission_resolution_is_idempotent() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        // Inject a permission request from the driver
        mock.emit(DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
            request_id: "req-1".to_owned(),
            tool_name: "terminal".to_owned(),
            summary: "Run tests".to_owned(),
            options: vec![PermissionOption {
                option_id: "allow_once".to_owned(),
                name: "Allow once".to_owned(),
                kind: PermissionOptionKind::AllowOnce,
            }],
            details: serde_json::json!({}).into(),
        }));
        for _ in 0..100 {
            if !manager.pending_permissions().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(manager.pending_permissions().len(), 1);

        manager
            .respond_permission(
                "c2",
                &outcome.session_id,
                "req-1",
                "allow_once",
                ResolvedBy::Cli,
            )
            .await
            .expect("resolve");
        assert!(manager.pending_permissions().is_empty());

        // The second attempt is ALREADY_RESOLVED
        let err = manager
            .respond_permission(
                "c3",
                &outcome.session_id,
                "req-1",
                "allow_once",
                ResolvedBy::Cli,
            )
            .await
            .expect_err("already resolved");
        assert!(matches!(err, NodeError::AlreadyResolved(_)));
    }

    #[tokio::test]
    async fn elicitation_resolution_is_idempotent() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        // Inject an elicitation (question) from the driver
        mock.emit(DriverEvent::Event(
            UnifiedEventPayload::ElicitationRequest {
                elicitation_id: "elic-1".to_owned(),
                message: "Which strategy should we go with?".to_owned(),
                mode: ElicitationMode::Form,
                requested_schema: serde_json::from_value(serde_json::json!({
                    "type": "object",
                    "properties": { "strategy": { "type": "string", "enum": ["a", "b"] } },
                    "required": ["strategy"]
                }))
                .expect("schema"),
                tool_call_id: None,
            },
        ));
        for _ in 0..100 {
            if !manager.pending_elicitations().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(manager.pending_elicitations().len(), 1);
        assert_eq!(
            manager.pending_elicitations()[0].message,
            "Which strategy should we go with?"
        );

        manager
            .respond_elicitation(
                "c2",
                &outcome.session_id,
                "elic-1",
                ElicitationAction::Accept,
                Some(
                    serde_json::from_value(serde_json::json!({ "strategy": "a" }))
                        .expect("content"),
                ),
                ResolvedBy::Cli,
            )
            .await
            .expect("resolve");
        assert!(manager.pending_elicitations().is_empty());
        let recorded = mock.elicitations();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "elic-1");
        assert_eq!(recorded[0].1, ElicitationAction::Accept);
        assert_eq!(
            recorded[0].2.as_ref().expect("content").get("strategy"),
            Some(&fxg_protocol::common::ElicitationValue::String(
                "a".to_owned()
            ))
        );

        // The second attempt is ALREADY_RESOLVED
        let err = manager
            .respond_elicitation(
                "c3",
                &outcome.session_id,
                "elic-1",
                ElicitationAction::Decline,
                None,
                ResolvedBy::Cli,
            )
            .await
            .expect_err("already resolved");
        assert!(matches!(err, NodeError::AlreadyResolved(_)));
    }

    #[tokio::test]
    async fn driver_resolved_elicitation_clears_pending() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        // An elicitation issued by the driver appears in pending
        mock.emit(DriverEvent::Event(
            UnifiedEventPayload::ElicitationRequest {
                elicitation_id: "elic-2".to_owned(),
                message: "Do you want to continue?".to_owned(),
                mode: ElicitationMode::Form,
                requested_schema: serde_json::from_value(
                    serde_json::json!({ "type": "object", "properties": {} }),
                )
                .expect("schema"),
                tool_call_id: None,
            },
        ));
        for _ in 0..100 {
            if !manager.pending_elicitations().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(manager.pending_elicitations().len(), 1);

        // Automatic resolution on the driver side (e.g. turn cancel) removes it from pending
        mock.emit(DriverEvent::Event(
            UnifiedEventPayload::ElicitationResolved {
                elicitation_id: "elic-2".to_owned(),
                action: ElicitationAction::Cancel,
                content: None,
                resolved_by: ResolvedBy::Unknown("system".to_owned()),
            },
        ));
        for _ in 0..100 {
            if manager.pending_elicitations().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(manager.pending_elicitations().is_empty());
    }

    #[tokio::test]
    async fn revert_restores_workspace_files() {
        let (manager, mock, dir) = setup().await;
        let (repo, session_id) = setup_repo_session(&manager, &dir).await;

        // Record the snapshot at turn start
        manager
            .send_prompt("c2", &session_id, "first turn", ClientSource::Cli)
            .await
            .expect("prompt");
        let user_seq = wait_user_message(&manager, &session_id, "first turn").await;

        // Turn complete → assume the agent changed files
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        wait_not_busy(&manager, &session_id).await;
        std::fs::write(repo.join("README.md"), "changed by agent\n").expect("write");

        // Revert: restore the file state at the start of the latest turn
        let outcome = manager
            .revert("c3", &session_id, None)
            .await
            .expect("revert");
        assert_eq!(outcome.target_node_seq, user_seq);
        assert!(outcome.restored_files >= 1);
        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).expect("read"),
            "# test\n"
        );

        // SessionReverted is appended to the event log
        let batch = manager
            .bus()
            .db()
            .session_events_after(&session_id, 0, 100)
            .await
            .expect("events");
        let reverted = batch
            .events
            .iter()
            .find(|event| {
                matches!(&event.payload, UnifiedEventPayload::SessionReverted { target_node_seq, .. } if *target_node_seq == user_seq)
            })
            .expect("session_reverted event");
        match &reverted.payload {
            UnifiedEventPayload::SessionReverted {
                restored_tree_hash,
                backup_tree_hash,
                ..
            } => {
                assert!(!restored_tree_hash.is_empty());
                assert!(backup_tree_hash.is_some(), "a backup Tree is saved");
            }
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn revert_rejects_busy_session() {
        let (manager, _mock, dir) = setup().await;
        let (_repo, session_id) = setup_repo_session(&manager, &dir).await;

        // A running turn (no Idle event received yet) is rejected as Busy
        manager
            .send_prompt("c2", &session_id, "running", ClientSource::Cli)
            .await
            .expect("prompt");
        let err = manager
            .revert("c3", &session_id, None)
            .await
            .expect_err("busy");
        assert!(matches!(err, NodeError::Busy(_)));
    }

    #[tokio::test]
    async fn fork_creates_child_session_and_injects_history() {
        let (manager, mock, dir) = setup().await;
        let (repo, source_id) = setup_repo_session(&manager, &dir).await;

        manager
            .send_prompt("c2", &source_id, "hello world", ClientSource::Cli)
            .await
            .expect("prompt");
        let user_seq = wait_user_message(&manager, &source_id, "hello world").await;
        mock.emit(DriverEvent::Event(UnifiedEventPayload::AgentMessage {
            message_id: "m1".to_owned(),
            text: "Understood".to_owned(),
            is_complete: true,
        }));
        wait_event_type(&manager, &source_id, EventType::AgentMessage).await;
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        wait_not_busy(&manager, &source_id).await;

        // Fork at the `user_seq` point
        let fork = manager
            .fork("c3", &source_id, Some(user_seq), None, None)
            .await
            .expect("fork");
        assert_ne!(fork.session_id, source_id);

        // The parent-child relationship is recorded
        let child = manager
            .bus()
            .db()
            .get_session(&fork.session_id)
            .await
            .expect("query")
            .expect("child row");
        assert_eq!(child.parent_session_id.as_deref(), Some(source_id.as_str()));
        assert_eq!(child.fork_from_node_seq, Some(user_seq));
        assert_eq!(child.agent_id, "mock");

        // History replay injection: recorded as a UserMessage of the fork session and
        // includes the conversation (user utterances) up to the `user_seq` point
        let batch = manager
            .bus()
            .db()
            .session_events_after(&fork.session_id, 0, 100)
            .await
            .expect("events");
        let injected = batch
            .events
            .iter()
            .find_map(|event| match &event.payload {
                UnifiedEventPayload::UserMessage {
                    text,
                    client_source,
                    ..
                } if client_source == &ClientSource::Fork => Some(text.clone()),
                _ => None,
            })
            .expect("injected fork context");
        assert!(
            injected.contains("hello world"),
            "history is included: {injected}"
        );
        assert!(
            !injected.contains("Understood"),
            "conversation after the fork point is excluded: {injected}"
        );
        assert!(
            mock.prompts().iter().any(|prompt| prompt == &injected),
            "the context is sent to the new agent"
        );

        // The fork's working directory shares the same repository
        // (canonicalize before comparing because macOS resolves `/var` → `/private/var`)
        assert_eq!(
            PathBuf::from(&child.local_path)
                .canonicalize()
                .expect("canonicalize child"),
            repo.canonicalize().expect("canonicalize repo")
        );
    }

    #[tokio::test]
    async fn driver_exit_records_stopped_status() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        // Driver (agent process) exit: when the event channel closes, the
        // pump records `Stopped` and removes it from the active list
        mock.close_events();
        for _ in 0..200 {
            let row = manager
                .bus()
                .db()
                .get_session(&outcome.session_id)
                .await
                .expect("query");
            if row.is_some_and(|session| session.status == SessionStatus::Stopped) {
                assert!(
                    manager.list_active().is_empty(),
                    "a stopped session is removed from the active list"
                );
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("driver exit did not record Stopped");
    }

    /// Waits until the session becomes `stopped` and is removed from the active list.
    async fn wait_stopped(manager: &SessionManager, session_id: &str) {
        for _ in 0..200 {
            let row = manager
                .bus()
                .db()
                .get_session(session_id)
                .await
                .expect("query");
            if row.is_some_and(|session| session.status == SessionStatus::Stopped) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("session did not stop");
    }

    /// Prepares a stopped session with one conversation turn plus an agent-side ID.
    ///
    /// Returns (repository path, session ID).
    async fn setup_stopped_session(
        manager: &SessionManager,
        mock: &MockAgent,
        dir: &tempfile::TempDir,
        prompt: &str,
    ) -> (PathBuf, String) {
        let (repo, session_id) = setup_repo_session(manager, dir).await;
        manager
            .send_prompt("c2", &session_id, prompt, ClientSource::Cli)
            .await
            .expect("prompt");
        wait_user_message(manager, &session_id, prompt).await;
        mock.emit(DriverEvent::Event(UnifiedEventPayload::SessionAgentBound {
            agent_session_id: "agent-sess-1".to_owned(),
        }));
        wait_event_type(manager, &session_id, EventType::SessionAgentBound).await;
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        wait_not_busy(manager, &session_id).await;
        // Terminate the driver (agent process)
        mock.close_events();
        wait_stopped(manager, &session_id).await;
        (repo, session_id)
    }

    #[tokio::test]
    async fn resume_restores_natively_without_replay() {
        let (manager, mock, dir) = setup().await;
        let (_repo, session_id) =
            setup_stopped_session(&manager, &mock, &dir, "native resume").await;

        mock.set_resume_supported(true);
        let outcome = manager
            .resume(ResumeParams {
                command_id: "c-resume",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect("resume");
        assert_eq!(outcome.session_id, session_id);
        assert!(outcome.context_restored);

        // After resume, status returns to idle and it rejoins the active list
        wait_idle(&manager, &session_id).await;
        let row = manager
            .bus()
            .db()
            .get_session(&session_id)
            .await
            .expect("query")
            .expect("session row");
        assert_eq!(row.status, SessionStatus::Idle);

        // The driver is asked to resume with the recorded agent-side ID
        let start = mock.starts().last().cloned().expect("start_session");
        let resume = start.resume.expect("resume request");
        assert_eq!(resume.agent_session_id.as_deref(), Some("agent-sess-1"));
        assert!(resume.allow_fresh);

        // No history replay is injected because of native restore
        let batch = manager
            .bus()
            .db()
            .session_events_after(&session_id, 0, 200)
            .await
            .expect("events");
        assert!(
            !batch.events.iter().any(|event| matches!(
                &event.payload,
                UnifiedEventPayload::UserMessage { client_source, .. } if client_source == &ClientSource::Resume
            )),
            "native resume must not inject replay context"
        );
    }

    #[tokio::test]
    async fn resume_falls_back_to_replay_context() {
        let (manager, mock, dir) = setup().await;
        let (_repo, session_id) =
            setup_stopped_session(&manager, &mock, &dir, "fallback resume").await;

        // Driver without native restore support → continue the context via replay
        let outcome = manager
            .resume(ResumeParams {
                command_id: "c-resume",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect("resume");
        assert!(!outcome.context_restored);

        // The history replay is injected as a UserMessage with `client_source = "resume"`
        // and sent to the agent
        let batch = manager
            .bus()
            .db()
            .session_events_after(&session_id, 0, 200)
            .await
            .expect("events");
        let injected = batch
            .events
            .iter()
            .find_map(|event| match &event.payload {
                UnifiedEventPayload::UserMessage {
                    text,
                    client_source,
                    ..
                } if client_source == &ClientSource::Resume => Some(text.clone()),
                _ => None,
            })
            .expect("injected resume context");
        assert!(injected.contains("fallback resume"), "history is included");
        assert!(
            mock.prompts().iter().any(|prompt| prompt == &injected),
            "the context is sent to the agent"
        );
        // Busy is cleared when the replay turn completes (Idle)
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        wait_not_busy(&manager, &session_id).await;
    }

    #[tokio::test]
    async fn resume_force_replay_skips_agent_session_id() {
        let (manager, mock, dir) = setup().await;
        let (_repo, session_id) =
            setup_stopped_session(&manager, &mock, &dir, "force replay").await;

        // Even with native restore support, `--replay` does not pass the agent-side ID
        mock.set_resume_supported(true);
        let outcome = manager
            .resume(ResumeParams {
                command_id: "c-resume",
                session_id: &session_id,
                force_replay: true,
            })
            .await
            .expect("resume");
        assert!(!outcome.context_restored);
        assert_eq!(
            mock.starts()
                .last()
                .and_then(|start| start.resume.as_ref())
                .and_then(|resume| resume.agent_session_id.clone()),
            None
        );
    }

    #[tokio::test]
    async fn resume_rejects_active_unknown_and_missing_workspace() {
        let (manager, mock, dir) = setup().await;

        // An unknown session yields InvalidSession
        let err = manager
            .resume(ResumeParams {
                command_id: "u1",
                session_id: "missing-session",
                force_replay: false,
            })
            .await
            .expect_err("unknown session");
        assert!(matches!(err, NodeError::InvalidSession(_)));

        // An active session yields InvalidState
        let (repo, session_id) = setup_repo_session(&manager, &dir).await;
        let err = manager
            .resume(ResumeParams {
                command_id: "c2",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect_err("active session");
        assert!(matches!(err, NodeError::InvalidState(_)));

        // A missing working directory (e.g. worktree removed) yields InvalidState
        mock.close_events();
        wait_stopped(&manager, &session_id).await;
        std::fs::remove_dir_all(&repo).expect("remove repo");
        let err = manager
            .resume(ResumeParams {
                command_id: "c3",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect_err("missing workspace");
        assert!(matches!(err, NodeError::InvalidState(_)));

        // Even on failure the exclusive guard (`resuming`) is released, so resume works after recovery
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        crate::testutil::init_test_repo(&repo).await;
        let outcome = manager
            .resume(ResumeParams {
                command_id: "c4",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect("resume after recovery");
        assert_eq!(outcome.session_id, session_id);
    }

    #[tokio::test]
    async fn send_prompt_auto_resumes_stopped_session_natively() {
        let (manager, mock, dir) = setup().await;
        let (_repo, session_id) = setup_stopped_session(&manager, &mock, &dir, "first turn").await;

        // Agent with native restore support: sending without an explicit resume auto-resumes
        mock.set_resume_supported(true);
        manager
            .send_prompt("c-next", &session_id, "auto resumed", ClientSource::Web)
            .await
            .expect("send prompt");

        // Native-only restore is requested with the recorded agent session ID
        let start = mock.starts().last().cloned().expect("start_session");
        let resume = start.resume.expect("resume request");
        assert_eq!(resume.agent_session_id.as_deref(), Some("agent-sess-1"));
        assert!(!resume.allow_fresh, "auto-resume is native-only");

        // When auto-resume completes, the state projection returns to idle and the prompt is sent
        wait_user_message(&manager, &session_id, "auto resumed").await;
        let row = manager
            .bus()
            .db()
            .get_session(&session_id)
            .await
            .expect("query")
            .expect("session row");
        assert_eq!(row.status, SessionStatus::Idle);
        assert!(mock.prompts().iter().any(|prompt| prompt == "auto resumed"));

        // No history replay is injected because of native restore
        let batch = manager
            .bus()
            .db()
            .session_events_after(&session_id, 0, 200)
            .await
            .expect("events");
        assert!(
            !batch.events.iter().any(|event| matches!(
                &event.payload,
                UnifiedEventPayload::UserMessage { client_source, .. } if client_source == &ClientSource::Resume
            )),
            "native auto resume must not inject replay context"
        );

        // Busy is cleared when the turn completes
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        wait_not_busy(&manager, &session_id).await;
    }

    #[tokio::test]
    async fn send_prompt_requires_explicit_resume_when_native_unsupported() {
        let (manager, mock, dir) = setup().await;
        let (_repo, session_id) = setup_stopped_session(&manager, &mock, &dir, "first turn").await;

        // Native restore unsupported: the send fails with ResumeRequired and the
        // history replay is not implicitly run (the session stays stopped)
        let err = manager
            .send_prompt("c-next", &session_id, "needs replay", ClientSource::Web)
            .await
            .expect_err("native resume is unavailable");
        assert!(matches!(err, NodeError::ResumeRequired { .. }));
        assert_eq!(
            err.error_code(),
            fxg_protocol::common::ErrorCode::ResumeRequired
        );

        let row = manager
            .bus()
            .db()
            .get_session(&session_id)
            .await
            .expect("query")
            .expect("session row");
        assert_eq!(row.status, SessionStatus::Stopped, "it remains stopped");
        assert!(manager.list_active().is_empty());
        assert!(
            !mock.prompts().iter().any(|prompt| prompt == "needs replay"),
            "the prompt is not sent"
        );

        // An explicit resume (history replay) can resume it
        let outcome = manager
            .resume(ResumeParams {
                command_id: "c-resume",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect("explicit resume");
        assert!(!outcome.context_restored);
    }

    #[tokio::test]
    async fn session_status_transitions_on_driver_events() {
        let (manager, mock, dir) = setup().await;
        manager
            .ensure_session("c1", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");
        assert_eq!(manager.list_active().len(), 1);

        // StatusChanged from the driver is reflected in the session state
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Stopped,
            error_message: None,
        }));
        for _ in 0..100 {
            let active = manager.list_active();
            if active
                .iter()
                .any(|session| session.status == SessionStatus::Stopped)
            {
                // The status is also reflected in the node.db sessions projection
                let session_id = active[0].session_id.clone();
                let row = manager
                    .bus()
                    .db()
                    .get_session(&session_id)
                    .await
                    .expect("query")
                    .expect("session row");
                assert_eq!(row.status, SessionStatus::Stopped);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("status did not become stopped");
    }

    #[test]
    fn derives_title_from_prompts() {
        assert_eq!(
            derive_title_from_prompt("Hello world"),
            Some("Hello world".to_owned())
        );
        assert_eq!(
            derive_title_from_prompt("# Task title\nSome description here"),
            Some("Task title".to_owned())
        );
        assert_eq!(
            derive_title_from_prompt("   \n\n> Blockquote prompt\nNext line"),
            Some("Blockquote prompt".to_owned())
        );
        let long = "あ".repeat(50); // allow-japanese: exercises multibyte character truncation
        let expected = format!("{}...", "あ".repeat(40)); // allow-japanese: same multibyte truncation
        assert_eq!(derive_title_from_prompt(&long), Some(expected));
        assert_eq!(derive_title_from_prompt("   \n\n  "), None);
    }

    #[tokio::test]
    async fn start_session_uses_initial_prompt_for_title() {
        let (manager, _mock, dir) = setup().await;
        let session_id = fxg_protocol::util::uuid_v7();
        manager
            .start_session(StartSessionParams {
                command_id: "c_init",
                session_id: &session_id,
                local_path: dir.path(),
                agent_id: "mock",
                initial_prompt: Some("# First Turn\nDo something"),
                mode: None,
                extra_args: None,
                fork_context: None,
                restore_git_bundle: None,
            })
            .await
            .expect("start");

        let row = manager
            .bus()
            .db()
            .get_session(&session_id)
            .await
            .expect("query")
            .expect("session");
        assert_eq!(row.title, "First Turn");
    }

    #[tokio::test]
    async fn first_prompt_updates_default_title() {
        let (manager, _mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c_ensure", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");

        // The initial title is the default
        let initial_row = manager
            .bus()
            .db()
            .get_session(&outcome.session_id)
            .await
            .expect("query")
            .expect("session");
        assert!(initial_row.title.contains('@'));

        // Send the first prompt
        manager
            .send_prompt(
                "c_prompt1",
                &outcome.session_id,
                "Implement login page",
                ClientSource::Web,
            )
            .await
            .expect("prompt");

        // The title is updated
        let updated_row = manager
            .bus()
            .db()
            .get_session(&outcome.session_id)
            .await
            .expect("query")
            .expect("session");
        assert_eq!(updated_row.title, "Implement login page");

        // The title is not updated again on the second prompt (first time only)
        manager
            .send_prompt(
                "c_prompt2",
                &outcome.session_id,
                "Now add logout button",
                ClientSource::Web,
            )
            .await
            .expect("prompt");

        let second_row = manager
            .bus()
            .db()
            .get_session(&outcome.session_id)
            .await
            .expect("query")
            .expect("session");
        assert_eq!(second_row.title, "Implement login page");
    }

    #[tokio::test]
    async fn shutdown_all_cleans_up_snapshot_index() {
        let (manager, _mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c_ensure", dir.path(), "mock", &[], None)
            .await
            .expect("ensure");

        // Create the index file
        let index_path = manager.inner.paths.snapshot_index_path(&outcome.session_id);
        std::fs::create_dir_all(index_path.parent().unwrap()).expect("create parent dir");
        std::fs::write(&index_path, b"test index data").expect("write index");
        assert!(index_path.exists());

        // Run shutdown_all
        let stopped = manager.shutdown_all().await;
        assert_eq!(stopped, vec![outcome.session_id]);

        // Confirm the index file has been deleted
        assert!(
            !index_path.exists(),
            "snapshot index must be cleaned up on shutdown"
        );
    }
}
