//! Session management (agent launch, event delivery, prompt queue).
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1-§4,
//! `docs/01-architecture-and-sync.md` §2.2-§2.4.
//!
//! - **Driver integration**: Launches an [`AgentDriver`] and streams [`DriverEvent`]
//!   into the [`SessionEventBus`] (persistent events via `record`, deltas via `publish_delta`).
//! - **Command idempotency**: Detects duplicate `command_id` submissions and
//!   returns [`NodeError::CommandDuplicate`] (guards against double taps / WS resends).
//! - **Pending queue**: `SendPrompt` while busy is queued, and the next prompt is
//!   sent automatically when the turn ends (`status = idle`).
//! - **Idempotent permission resolution**: The second and later responses return
//!   [`NodeError::AlreadyResolved`].

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fxg_acp::registry::{AcpRegistry, RegistryIndex};
use fxg_acp::{
    AcpDriver, ActiveSessionHandle, AgentAuthRequired, AgentDriver, AgentLaunchSpec, DriverEvent,
    DriverKind, NativeForkUnavailable, NativeResumeUnavailable, ResumeRequest,
};
use fxg_protocol::CommandId;
use fxg_protocol::common::{
    ClientSource, ElicitationAction, ElicitationContent, ElicitationMode, ElicitationSchema,
    PermissionOption, PermissionPolicy, PromptInput, ResolvedBy, SessionControlAction,
    SessionStatus,
};
use fxg_protocol::events::{SessionEventEnvelope, UnifiedEventPayload};
use fxg_protocol::util::uuid_v7;
use tokio::sync::mpsc;

use crate::error::NodeError;
use crate::paths::NodePaths;
use crate::project;
use crate::session::SessionEventBus;

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
    pub command_id: &'a CommandId,
    /// Session ID (a UUID v7 assigned by the caller)
    pub session_id: &'a str,
    /// Working directory (including worktree paths)
    pub local_path: &'a Path,
    /// Agent ID
    pub agent_id: &'a str,
    /// Initial prompt (text and optional attachments)
    pub initial_prompt: Option<&'a PromptInput>,
    /// Initial session configuration (model / thinking level, etc.)
    pub initial_config: &'a [fxg_protocol::common::InitialConfig],
    /// Initial mode (`code` / `plan` etc.)
    pub mode: Option<&'a str>,
    /// Additional pass-through arguments for the agent
    pub extra_args: Option<&'a [String]>,
    /// Source session to fork natively (ACP `session/fork` + `session/load`).
    pub fork_from_session_id: Option<&'a str>,
}

/// Resume request for a stopped session ([`SessionManager::resume`]).
///
/// Starts the agent under the same `session_id`, preferring native restore
/// (`session/resume` → `session/load`). If restore is impossible, continues by
/// injecting a history replay
/// (design: `docs/04-agent-drivers-and-windows.md` §4.1).
#[derive(Debug, Clone, Copy)]
pub struct ResumeParams<'a> {
    /// Correlation ID (idempotent deduplication of duplicate submissions)
    pub command_id: &'a CommandId,
    /// Target session ID
    pub session_id: &'a str,
    /// Continue via history replay without attempting native restore
    pub force_replay: bool,
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
    processed_commands: VecDeque<CommandId>,
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
    /// Approval policy for this session
    permission_policy: PermissionPolicy,
}

#[derive(Debug, Clone)]
struct PendingPrompt {
    input: PromptInput,
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
    /// Source ACP session ID for a native `session/fork`
    fork_from: Option<&'a str>,
    /// Initial configuration applied after `session/new`
    initial_config: Vec<fxg_protocol::common::InitialConfig>,
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

    /// Recent agent stderr lines from every registered driver (diagnostics).
    pub fn agent_diagnostics(&self) -> BTreeMap<String, Vec<String>> {
        let drivers: Vec<Arc<dyn AgentDriver>> =
            { self.inner.drivers.lock().expect("drivers poisoned").clone() };
        let mut result = BTreeMap::new();
        for driver in drivers {
            for (agent_id, lines) in driver.stderr_snapshot() {
                result
                    .entry(agent_id)
                    .or_insert_with(Vec::new)
                    .extend(lines);
            }
        }
        result
    }

    /// Records `command_id` as processed.
    ///
    /// Returns `false` if it was already processed (the caller should return
    /// [`NodeError::CommandDuplicate`]).
    fn begin_command(&self, command_id: &CommandId) -> bool {
        let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
        if sessions
            .processed_commands
            .iter()
            .any(|id| id == command_id)
        {
            return false;
        }
        sessions.processed_commands.push_back(command_id.clone());
        while sessions.processed_commands.len() > PROCESSED_COMMAND_HISTORY {
            sessions.processed_commands.pop_front();
        }
        true
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
        let command_id = CommandId::from(uuid_v7());
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

mod approval;
mod lifecycle;
mod prompt;
mod replay;
mod stream;

#[cfg(test)]
mod tests;
