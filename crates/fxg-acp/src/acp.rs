//! Driver for standard ACP agents (`AcpDriver`).
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §2.2.
//!
//! Acts as a client for `agent-client-protocol` (the ACP SDK), connects to external
//! agent processes, and converts `session/update` notifications,
//! `session/request_permission`, `fs/*` and `terminal/*` into FXG normalized
//! events ([`DriverEvent`]).
//!
//! - **Session ownership**: a dedicated task owns the ACP connection and serializes
//!   reading updates from [`ActiveSession`] with prompt sends.
//!   External operations go through [`ConnectionTo`] (clonable) and a command channel.
//! - **Approval**: `request_permission` registers a `oneshot` and is resolved from
//!   [`ActiveSessionHandle::respond_permission`]
//!   (returns `cancelled` if the turn is cancelled while unresolved).
//! - **fs/terminal**: reads and writes relative to the session's `local_path`;
//!   writes compute a unified diff and attach it to the event.
//!   `terminal/*` is executed via `fxg-pty`.

#![allow(
    clippy::disallowed_types,
    reason = "ACP SDK <-> FXG conversions use serde_json::Value as an intermediate; see docs/06-type-safety.md"
)]

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::util::MatchDispatch;
use agent_client_protocol::v1::schema::{
    AgentCapabilities, AuthCapabilities, AuthMethod, CancelNotification, ClientCapabilities,
    CloseSessionRequest, CompleteElicitationNotification, ContentBlock, ContentChunk,
    CreateElicitationRequest, CreateElicitationResponse, CreateTerminalRequest,
    CreateTerminalResponse, ElicitationAcceptAction, ElicitationAction as AcpElicitationAction,
    ElicitationCapabilities, ElicitationContentValue, ElicitationFormCapabilities, ElicitationMode,
    ElicitationScope, FileSystemCapabilities, ForkSessionRequest, ForkSessionResponse,
    ImageContent, Implementation, InitializeRequest, KillTerminalRequest, KillTerminalResponse,
    NewSessionRequest, PermissionOptionKind as AcpPermissionOptionKind,
    PlanEntryStatus as AcpPlanEntryStatus, PromptCapabilities, ReadTextFileRequest,
    ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, ResourceLink,
    SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption, SessionConfigOptionValue,
    SessionId, SessionModeState, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionModeRequest, StopReason, TerminalExitStatus, TerminalId, TerminalOutputRequest,
    TerminalOutputResponse, TextContent, ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate,
    ToolKind, WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use agent_client_protocol::v1::{ActiveSession, ConnectionTo, SessionMessage, SessionSetup};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, Client};
use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use fxg_protocol::common::{
    AgentAuthMethodInfo, AgentAuthMethodKind, CommandInfo, ConfigOptionInfo, ConfigOptionValue,
    ElicitationContent, ElicitationMode as FxgElicitationMode, ElicitationPropertyKind,
    ElicitationSchema, ElicitationValue, FileDiff, ModeInfo, PermissionOption,
    PermissionOptionKind as FxgPermissionOptionKind, PlanEntry,
    PlanEntryStatus as FxgPlanEntryStatus, PromptInput, StreamDeltaPayload,
    ToolCallKind as FxgToolCallKind, ToolCallStatus as FxgToolCallStatus, TurnStopReason,
    UsageCost,
};
use fxg_protocol::events::UnifiedEventPayload;
use fxg_protocol::util::uuid_v7;
use tokio::sync::{mpsc, oneshot, watch};

use crate::driver::{
    ActiveSessionHandle, AgentAuthRequired, AgentDriver, DriverEvent, NativeForkUnavailable,
    NativeResumeUnavailable, ResumeRequest, StartSessionRequest, StartedSession,
};
use crate::warm::{DEFAULT_WARM_IDLE, LaunchKey, WarmPool, WarmProcess};

/// Driver that controls standard ACP agents via `agent-client-protocol`.
///
/// Keeps the process idle with a TTL after session end and reuses it in the next
/// session (for costly-to-start agents, the second and later sessions start with
/// just `session/new`). Disable the pool with [`AcpDriver::without_pool`].
pub struct AcpDriver {
    /// Reuse pool for idle processes (`None` = pool disabled)
    pool: Option<Arc<WarmPool<ProcessHandle>>>,
    /// Recent agent stderr lines for diagnostics
    stderr: StderrRegistry,
}

/// Bounded per-agent stderr buffer used by diagnostics.
#[derive(Debug, Clone, Default)]
pub struct StderrRegistry {
    lines: Arc<Mutex<BTreeMap<String, VecDeque<String>>>>,
}

impl StderrRegistry {
    /// Maximum retained lines per agent.
    const MAX_LINES: usize = 200;

    /// Appends one stderr line, dropping the oldest when over the limit.
    pub fn push(&self, agent_id: &str, line: &str) {
        let mut lines = self.lines.lock().expect("stderr registry poisoned");
        let buffer = lines.entry(agent_id.to_owned()).or_default();
        if buffer.len() >= Self::MAX_LINES {
            buffer.pop_front();
        }
        buffer.push_back(line.to_owned());
    }

    /// Returns the retained stderr lines for one agent.
    pub fn tail(&self, agent_id: &str) -> Vec<String> {
        self.lines
            .lock()
            .expect("stderr registry poisoned")
            .get(agent_id)
            .map(|lines| lines.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Returns a snapshot of all retained stderr lines.
    pub fn snapshot(&self) -> BTreeMap<String, Vec<String>> {
        self.lines
            .lock()
            .expect("stderr registry poisoned")
            .iter()
            .map(|(agent, lines)| (agent.clone(), lines.iter().cloned().collect()))
            .collect()
    }
}

impl std::fmt::Debug for AcpDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpDriver")
            .field("warm_pool", &self.pool.is_some())
            .finish()
    }
}

impl Default for AcpDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpDriver {
    /// Creates a driver with the idle pool enabled.
    ///
    /// The idle TTL defaults to 10 minutes; `FXG_AGENT_WARM_IDLE_SECS` overrides
    /// it (`0` keeps processes until daemon shutdown).
    pub fn new() -> Self {
        let idle = match fxg_protocol::config::process_env(
            fxg_protocol::config::env_keys::AGENT_WARM_IDLE_SECS,
        ) {
            Some(value) => match value.trim().parse::<u64>() {
                Ok(secs) => Duration::from_secs(secs),
                Err(err) => {
                    tracing::warn!(
                        "invalid FXG_AGENT_WARM_IDLE_SECS value {value:?}: {err}; using the default"
                    );
                    DEFAULT_WARM_IDLE
                }
            },
            None => DEFAULT_WARM_IDLE,
        };
        Self::with_warm_idle(idle)
    }

    /// Creates a driver with the given warm-process idle TTL.
    pub fn with_warm_idle(idle: Duration) -> Self {
        Self {
            pool: Some(Arc::new(WarmPool::new(idle))),
            stderr: StderrRegistry::default(),
        }
    }

    /// Creates a driver with the idle pool disabled (process disposed at session end).
    ///
    /// Use when agent state should not be shared across sessions (tests / diagnostics).
    pub fn without_pool() -> Self {
        Self {
            pool: None,
            stderr: StderrRegistry::default(),
        }
    }
}

/// Resolution channel for permission requests (`request_id` → `oneshot`).
type PermissionRegistry = Arc<Mutex<HashMap<String, oneshot::Sender<RequestPermissionOutcome>>>>;

/// Resolution channel for elicitations (`elicitation_id` → pending info).
type ElicitationRegistry = Arc<Mutex<HashMap<String, PendingElicitation>>>;

/// State of a pending elicitation (response channel and validation schema).
struct PendingElicitation {
    /// Sender for responses from the UI / CLI
    sender: oneshot::Sender<ElicitationOutcome>,
    /// request schema for form mode (for validating `accept` content)
    schema: ElicitationSchema,
}

/// Elicitation response (normalized representation before conversion to an ACP response).
enum ElicitationOutcome {
    /// accept (with form content)
    Accept(BTreeMap<String, ElicitationContentValue>),
    /// decline
    Decline,
    /// cancel
    Cancel,
}

/// Commands for a session (turn loop).
#[derive(Debug)]
enum AcpCommand {
    /// Send a prompt (runs until the turn completes)
    Prompt(PromptInput),
    /// End the session (keep the process for reuse in the next session)
    Shutdown,
}

/// Commands for the resident process.
enum ProcessCommand {
    /// Create a new session and start its turn loop
    NewSession {
        /// Session creation parameters
        params: SessionParams,
        /// Event sender for the session
        events: mpsc::UnboundedSender<DriverEvent>,
        /// Process lease held by the session
        lease: Arc<ProcessLease>,
        /// Command channel to the session (the handle holds the sender)
        commands: (
            mpsc::UnboundedSender<AcpCommand>,
            mpsc::UnboundedReceiver<AcpCommand>,
        ),
        /// Session ready notification (handle + native restore status / error)
        ready: oneshot::Sender<anyhow::Result<(AcpSessionHandle, bool)>>,
    },
    /// Terminate the process (run teardown)
    Dispose,
}

/// Parameters required to create a new session within a process.
#[derive(Debug, Clone)]
struct SessionParams {
    /// fxg-side session ID (for logging)
    session_id: String,
    /// Working directory (the session's `local_path`)
    cwd: PathBuf,
    /// Initial mode
    initial_mode: Option<String>,
    /// Resume selection from an existing agent session (`None` means a fresh session)
    resume: Option<ResumeRequest>,
    /// Source ACP session ID for a native `session/fork`
    fork_from: Option<String>,
    /// Initial configuration applied after `session/new`
    initial_config: Vec<fxg_protocol::common::InitialConfig>,
}

/// Per-session shared information (handlers look it up by the ACP session_id).
///
/// A single process is reused across sessions, so the fs / terminal handlers do
/// not capture connection-build-time information and instead resolve it here each time.
#[derive(Clone)]
struct SessionContext {
    /// Working directory (used to resolve relative paths for fs / terminal)
    cwd: PathBuf,
    /// Event sender (status updates, approvals, fs change notifications)
    events: mpsc::UnboundedSender<DriverEvent>,
}

/// ACP session ID → session information.
type SessionContexts = Arc<Mutex<HashMap<String, SessionContext>>>;

/// Looks up session information by ACP session ID.
fn session_context(contexts: &SessionContexts, session_id: &SessionId) -> Option<SessionContext> {
    let contexts = contexts.lock().expect("session contexts poisoned");
    if let Some(context) = contexts.get(&session_id.to_string()) {
        return Some(context.clone());
    }
    // Some agents (e.g. opencode subagents) send requests with a child session
    // ID. A process hosts at most one parent session, so fall back to it when
    // exactly one context is registered.
    if contexts.len() == 1 {
        let context = contexts.values().next().cloned();
        tracing::debug!(
            session_id = %session_id,
            "falling back to the single active session context (child session)"
        );
        return context;
    }
    None
}

/// Handle to the resident process (shared by the pool / leases).
struct ProcessHandle {
    /// Command sender to the process
    control_tx: mpsc::UnboundedSender<ProcessCommand>,
    /// JoinHandle of the resident task (`dispose_process` waits for teardown to finish)
    join: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ProcessHandle {
    /// Creates a new session and returns the handle plus native restore status.
    async fn new_session(
        &self,
        params: SessionParams,
        events: mpsc::UnboundedSender<DriverEvent>,
        lease: Arc<ProcessLease>,
    ) -> anyhow::Result<(AcpSessionHandle, bool)> {
        let commands = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        match self.control_tx.send(ProcessCommand::NewSession {
            params,
            events,
            lease,
            commands,
            ready: ready_tx,
        }) {
            Ok(()) => {}
            Err(_) => return Err(anyhow!("acp agent process is no longer running")),
        }
        ready_rx
            .await
            .context("acp agent process ended before the session became ready")?
    }

    /// Terminates the process and waits for teardown to finish (immediate if already done).
    async fn dispose_process(&self) {
        fxg_protocol::discard_with!(
            self.control_tx.send(ProcessCommand::Dispose),
            "request process disposal"
        );
        let join = self.join.lock().expect("process join poisoned").take();
        if let Some(join) = join
            && let Err(err) = join.await
        {
            tracing::warn!("acp process task terminated unexpectedly: {err}");
        }
    }
}

impl WarmProcess for ProcessHandle {
    fn dispose(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(self.dispose_process())
    }
}

/// Process lease held by a session. Returned to the pool on end, or disposed.
struct ProcessLease {
    /// Reuse key
    key: LaunchKey,
    /// Resident process
    process: Arc<ProcessHandle>,
    /// Destination pool (`None` = pool disabled → dispose)
    pool: Option<Arc<WarmPool<ProcessHandle>>>,
    /// [`AcpSessionHandle::dispose`] requested (dispose the process instead of
    /// returning it to the pool)
    dispose_requested: AtomicBool,
    /// Prevents double return / dispose
    finished: AtomicBool,
    /// Sender for turn-loop end notification (taken by the process side when serve starts)
    session_end_tx: Mutex<Option<oneshot::Sender<()>>>,
    /// Receiver for turn-loop end notification (`shutdown` waits on it)
    session_end_rx: Mutex<Option<oneshot::Receiver<()>>>,
}

impl std::fmt::Debug for ProcessLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessLease")
            .field("key", &self.key)
            .field("dispose_requested", &self.dispose_requested)
            .field("finished", &self.finished)
            .finish()
    }
}

impl ProcessLease {
    /// Creates a new lease (including the turn-loop end notification channel).
    fn new(
        key: LaunchKey,
        process: Arc<ProcessHandle>,
        pool: Option<Arc<WarmPool<ProcessHandle>>>,
    ) -> Self {
        let (session_end_tx, session_end_rx) = oneshot::channel();
        Self {
            key,
            process,
            pool,
            dispose_requested: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            session_end_tx: Mutex::new(Some(session_end_tx)),
            session_end_rx: Mutex::new(Some(session_end_rx)),
        }
    }

    /// Takes the turn-loop end notifier (process side, once).
    fn take_session_end_sender(&self) -> Option<oneshot::Sender<()>> {
        self.session_end_tx
            .lock()
            .expect("process lease poisoned")
            .take()
    }

    /// Takes the receiver for waiting on turn-loop end (`shutdown`, once).
    fn take_session_end_receiver(&self) -> Option<oneshot::Receiver<()>> {
        self.session_end_rx
            .lock()
            .expect("process lease poisoned")
            .take()
    }

    /// Marks the process to be disposed instead of returned to the pool (e.g.
    /// abnormal session end).
    fn mark_dispose(&self) {
        self.dispose_requested.store(true, Ordering::SeqCst);
    }

    /// Post-session cleanup (return to the pool or dispose). Duplicate calls are ignored.
    async fn finish(&self) {
        if self.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.dispose_requested.load(Ordering::SeqCst) || self.pool.is_none() {
            self.process.dispose_process().await;
        } else if let Some(pool) = &self.pool {
            pool.release(self.key.clone(), Arc::clone(&self.process));
        }
    }
}

#[async_trait]
impl AgentDriver for AcpDriver {
    fn driver_kind(&self) -> crate::driver::DriverKind {
        crate::driver::DriverKind::Acp
    }

    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession> {
        match self.start_session_inner(req, &event_tx).await {
            Ok(started) => Ok(started),
            Err(err) => {
                // Record the start failure as an event (the session moves to the error state)
                fxg_protocol::discard_with!(
                    event_tx.send(DriverEvent::Failed {
                        message: err.to_string(),
                    }),
                    "notify event subscribers"
                );
                Err(err)
            }
        }
    }

    async fn shutdown_idle(&self) -> usize {
        match &self.pool {
            Some(pool) => pool.shutdown_idle().await,
            None => 0,
        }
    }

    fn stderr_tail(&self, agent_id: &str) -> Vec<String> {
        self.stderr.tail(agent_id)
    }

    fn stderr_snapshot(&self) -> BTreeMap<String, Vec<String>> {
        self.stderr.snapshot()
    }
}

impl AcpDriver {
    /// Core session start (reuse idle process, then cold start).
    async fn start_session_inner(
        &self,
        req: StartSessionRequest,
        event_tx: &mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession> {
        // Extension resolution for `npx` / `uvx` (Windows PATHEXT support)
        let program = fxg_pty::resolve_command(&req.launch.program.to_string_lossy(), &req.cwd)
            .map_err(|err| {
                anyhow!(
                    "failed to resolve agent program {}: {err}",
                    req.launch.program.display()
                )
            })?;
        let key = LaunchKey::new(&req.launch, program.clone());
        let session_params = SessionParams {
            session_id: req.session_id.clone(),
            cwd: req.cwd.clone(),
            initial_mode: req.initial_mode.clone(),
            resume: req.resume.clone(),
            fork_from: req.fork_from.clone(),
            initial_config: req.initial_config.clone(),
        };

        // 1) Reuse an idle process (just session/new on an initialized connection)
        if let Some(pool) = &self.pool
            && let Some(process) = pool.checkout(&key)
        {
            let lease = Arc::new(ProcessLease::new(
                key.clone(),
                Arc::clone(&process),
                Some(Arc::clone(pool)),
            ));
            match process
                .new_session(session_params.clone(), event_tx.clone(), lease)
                .await
            {
                Ok((handle, context_restored)) => {
                    tracing::debug!(
                        session_id = %req.session_id,
                        agent = %key.agent_id(),
                        "reusing idle acp agent process"
                    );
                    return Ok(StartedSession {
                        handle: Box::new(handle),
                        context_restored,
                    });
                }
                Err(err) => {
                    tracing::warn!(
                        session_id = %req.session_id,
                        agent = %key.agent_id(),
                        "failed to reuse idle acp agent process: {err:#}"
                    );
                    process.dispose_process().await;
                }
            }
        }

        // 2) Cold start (spawn process → initialize → create session)
        let config = AcpAgentConfig::new(program)
            .args(req.launch.args.iter().cloned())
            .envs(
                req.launch
                    .env
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        let (process, ready_rx) = spawn_agent_process(
            AcpAgent::new(config),
            req.launch.agent_id.clone(),
            self.stderr.clone(),
        );
        ready_rx
            .await
            .context("acp agent process ended during startup")??;
        let lease = Arc::new(ProcessLease::new(
            key,
            Arc::clone(&process),
            self.pool.clone(),
        ));
        let (handle, context_restored) = process
            .new_session(session_params, event_tx.clone(), lease)
            .await?;
        Ok(StartedSession {
            handle: Box::new(handle),
            context_restored,
        })
    }
}

/// Spawns the agent process and returns the resident task and the process ready notification.
///
/// ready is notified when `initialize` completes (sessions are created by
/// [`ProcessHandle::new_session`]).
fn spawn_agent_process(
    agent: AcpAgent,
    agent_id: String,
    stderr: StderrRegistry,
) -> (Arc<ProcessHandle>, oneshot::Receiver<anyhow::Result<()>>) {
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        if let Err(err) = run_acp_process(AcpProcessTaskParams {
            agent,
            agent_id,
            stderr,
            ready_tx,
            control_rx,
        })
        .await
        {
            tracing::warn!("acp agent process ended: {err:#}");
        }
    });
    (
        Arc::new(ProcessHandle {
            control_tx,
            join: Mutex::new(Some(join)),
        }),
        ready_rx,
    )
}

/// Handle for a started ACP session.
struct AcpSessionHandle {
    /// ACP (agent-side) session ID
    acp_session_id: SessionId,
    /// Connection (clonable; used to issue requests/notifications)
    conn: ConnectionTo<Agent>,
    /// Commands to the session (turn loop)
    commands: mpsc::UnboundedSender<AcpCommand>,
    /// Permission resolution registry (shared per process)
    permissions: PermissionRegistry,
    /// Elicitation resolution registry (shared per process)
    elicitations: ElicitationRegistry,
    /// Event sender (for status updates).
    ///
    /// Released on session end ([`ActiveSessionHandle::shutdown`]). If it were kept,
    /// the session manager's event pump could not detect that the driver ended and
    /// the session would remain in the active list.
    events: Mutex<Option<mpsc::UnboundedSender<DriverEvent>>>,
    /// Latest modes / commands / config options (shared with the turn loop).
    capabilities: Arc<Mutex<SessionCapabilitiesState>>,
    /// PTY manager (for ACP terminal/*; shared per process)
    pty: Arc<fxg_pty::PtySessionManager>,
    /// Process lease (returned to the pool or disposed on end)
    lease: Arc<ProcessLease>,
}

#[async_trait]
impl ActiveSessionHandle for AcpSessionHandle {
    async fn send_prompt(&self, input: PromptInput) -> anyhow::Result<()> {
        match self.commands.send(AcpCommand::Prompt(input)) {
            Ok(()) => Ok(()),
            Err(_) => Err(anyhow!("acp session is no longer running")),
        }
    }

    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()> {
        let sender = self
            .permissions
            .lock()
            .expect("permission registry poisoned")
            .remove(&request_id);
        let Some(sender) = sender else {
            return Err(anyhow!(
                "unknown or already resolved permission: {request_id}"
            ));
        };
        let outcome = if selected_option_id.is_empty() {
            RequestPermissionOutcome::Cancelled
        } else {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(selected_option_id))
        };
        if sender.send(outcome).is_err() {
            return Err(anyhow!("permission resolver dropped: {request_id}"));
        }
        Ok(())
    }

    async fn respond_elicitation(
        &self,
        elicitation_id: String,
        action: fxg_protocol::common::ElicitationAction,
        content: Option<ElicitationContent>,
    ) -> anyhow::Result<()> {
        use fxg_protocol::common::ElicitationAction as FxgElicitationAction;

        // Read the schema without consuming the pending entry (invalid content keeps
        // the entry pending and returns an error so the client can fix and resend)
        let schema = self
            .elicitations
            .lock()
            .expect("elicitation registry poisoned")
            .get(&elicitation_id)
            .map(|pending| pending.schema.clone())
            .ok_or_else(|| anyhow!("unknown or already resolved elicitation: {elicitation_id}"))?;
        let outcome = match action {
            FxgElicitationAction::Accept => {
                let content = content.unwrap_or_default();
                validate_elicitation_content(&schema, &content)
                    .map_err(|err| anyhow!("invalid elicitation content: {err}"))?;
                let content = content
                    .into_iter()
                    .map(|(key, value)| Ok((key, elicitation_content_value(value)?)))
                    .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
                ElicitationOutcome::Accept(content)
            }
            FxgElicitationAction::Decline => ElicitationOutcome::Decline,
            FxgElicitationAction::Cancel => ElicitationOutcome::Cancel,
        };
        let sender = self
            .elicitations
            .lock()
            .expect("elicitation registry poisoned")
            .remove(&elicitation_id)
            .map(|pending| pending.sender)
            .ok_or_else(|| anyhow!("unknown or already resolved elicitation: {elicitation_id}"))?;
        if sender.send(outcome).is_err() {
            return Err(anyhow!("elicitation resolver dropped: {elicitation_id}"));
        }
        Ok(())
    }

    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()> {
        self.conn
            .send_request(SetSessionModeRequest::new(
                self.acp_session_id.clone(),
                mode_id,
            ))
            .block_task()
            .await
            .map_err(|err| anyhow!("failed to set mode: {err}"))?;
        Ok(())
    }

    async fn set_config(&self, key: String, value: ConfigOptionValue) -> anyhow::Result<()> {
        let value = parse_config_value(value)
            .map_err(|err| anyhow!("unsupported config value for {key}: {err}"))?;
        let response = self
            .conn
            .send_request(SetSessionConfigOptionRequest::new(
                self.acp_session_id.clone(),
                key,
                value,
            ))
            .block_task()
            .await
            .map_err(|err| anyhow!("failed to set config option: {err}"))?;
        // ACP agents are not required to push a `config_option_update` notification
        // after a config change, and opencode only returns the new options in the
        // response. Publish them so the UI does not keep showing the old value.
        if !response.config_options.is_empty() {
            let payload = {
                let mut capabilities = self
                    .capabilities
                    .lock()
                    .expect("session capabilities poisoned");
                capabilities.config_options = response
                    .config_options
                    .iter()
                    .map(map_config_option)
                    .collect();
                capabilities.to_payload()
            };
            let events = self.events.lock().expect("session events poisoned").clone();
            if let Some(events) = events.as_ref() {
                fxg_protocol::discard_with!(
                    events.send(DriverEvent::Event(payload)),
                    "notify event subscribers"
                );
            }
        }
        Ok(())
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        self.conn
            .send_notification(CancelNotification::new(self.acp_session_id.clone()))
            .map_err(|err| anyhow!("failed to cancel turn: {err}"))?;
        // The spec requires unresolved approvals to be returned as cancelled (MUST)
        cancel_pending_permissions(&self.permissions);
        // The spec also requires unresolved elicitations to be resolved as cancel,
        // clearing the waiting-for-input state
        let events = self.events.lock().expect("session events poisoned").clone();
        cancel_pending_elicitations(&self.elicitations, events.as_ref());
        if let Some(events) = events.as_ref() {
            fxg_protocol::discard_with!(
                events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: fxg_protocol::common::SessionStatus::Running,
                    error_message: None,
                })),
                "notify event subscribers"
            );
        }
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        // Close the session and return the process to the reuse pool (disposal is
        // [`Self::dispose`])
        let session_end = self.lease.take_session_end_receiver();
        fxg_protocol::discard_with!(
            self.commands.send(AcpCommand::Shutdown),
            "send a command to the ACP session"
        );
        if let Some(session_end) = session_end {
            // Wait for the turn loop to end (returns Err immediately if the loop never
            // started or failed)
            fxg_protocol::discard_with!(session_end.await, "wait for the ACP turn loop to end");
        }
        // Best-effort logical close (the process is kept for the warm pool).
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.conn
                .send_request(CloseSessionRequest::new(self.acp_session_id.clone()))
                .block_task(),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                tracing::debug!(session_id = %self.acp_session_id, "session/close failed: {err}");
            }
            Err(_) => {
                tracing::debug!(session_id = %self.acp_session_id, "session/close timed out");
            }
        }
        for terminal_id in self.pty.list() {
            fxg_protocol::discard_with!(
                self.pty.kill(&terminal_id.pty_id),
                "kill the pty terminal on session shutdown"
            );
        }
        // Release the event sender (so the session manager's pump detects that the
        // driver ended and removes the session from the active list)
        self.events.lock().expect("session events poisoned").take();
        self.lease.finish().await;
        Ok(())
    }

    async fn dispose(&self) -> anyhow::Result<()> {
        // Dispose the process instead of keeping it (`fxg kill-all` / daemon shutdown)
        self.lease.dispose_requested.store(true, Ordering::SeqCst);
        self.shutdown().await
    }
}

/// Closes all unresolved permission requests as `cancelled`.
fn cancel_pending_permissions(permissions: &PermissionRegistry) {
    let mut registry = permissions.lock().expect("permission registry poisoned");
    for (_, sender) in registry.drain() {
        fxg_protocol::discard_with!(
            sender.send(RequestPermissionOutcome::Cancelled),
            "discard a non-critical result"
        );
    }
}

/// Closes all unresolved elicitations as `cancel` (the spec requires resolving
/// unresolved elicitations with cancel when a turn is interrupted).
///
/// When `events` is provided, also emits `ElicitationResolved` to clear the
/// waiting-for-input state in the UI / Inbox (responses via fxg are already
/// recorded by the manager, so this only records driver-side auto-resolution).
fn cancel_pending_elicitations(
    elicitations: &ElicitationRegistry,
    events: Option<&mpsc::UnboundedSender<DriverEvent>>,
) {
    let mut registry = elicitations.lock().expect("elicitation registry poisoned");
    for (elicitation_id, pending) in registry.drain() {
        if pending.sender.send(ElicitationOutcome::Cancel).is_err() {
            tracing::debug!(
                elicitation_id = %elicitation_id,
                "elicitation waiter dropped before the cancel was delivered"
            );
        }
        if let Some(events) = events {
            fxg_protocol::discard_with!(
                events.send(DriverEvent::Event(
                    UnifiedEventPayload::ElicitationResolved {
                        elicitation_id,
                        action: fxg_protocol::common::ElicitationAction::Cancel,
                        content: None,
                        resolved_by: fxg_protocol::common::ResolvedBy::Unknown(
                            "system".to_owned(),
                        ),
                    },
                )),
                "notify event subscribers"
            );
        }
    }
}

/// Launch parameters for `run_acp_process`.
struct AcpProcessTaskParams {
    /// Agent to launch
    agent: AcpAgent,
    /// Agent ID (diagnostics key)
    agent_id: String,
    /// Recent stderr buffer shared with the driver
    stderr: StderrRegistry,
    /// Process ready (initialize complete) notification
    ready_tx: oneshot::Sender<anyhow::Result<()>>,
    /// Command receiver for the process
    control_rx: mpsc::UnboundedReceiver<ProcessCommand>,
}

/// Resident task that establishes the ACP connection and alternates between idle
/// waiting and session handling.
///
/// Handles one session at a time per process, and after the session ends waits
/// for the next `NewSession` (process disposal happens via the pool TTL / `Dispose`).
#[allow(clippy::too_many_lines)]
async fn run_acp_process(params: AcpProcessTaskParams) -> Result<(), agent_client_protocol::Error> {
    let AcpProcessTaskParams {
        agent,
        agent_id,
        stderr,
        ready_tx,
        mut control_rx,
    } = params;
    let mut ready_tx = Some(ready_tx);
    let pty = Arc::new(fxg_pty::PtySessionManager::new());
    let terminals = Arc::new(TerminalRegistry::default());
    let permissions: PermissionRegistry = Arc::new(Mutex::new(HashMap::new()));
    let elicitations: ElicitationRegistry = Arc::new(Mutex::new(HashMap::new()));
    let contexts: SessionContexts = Arc::new(Mutex::new(HashMap::new()));

    // Register handlers (fs / approval / terminal).
    // NOTE: a helper returning `impl AsyncFnMut` cannot prove `Send` for the associated
    // future, so closures are passed directly to the builder chain.
    let builder = Client
        .v1()
        .name("fxg")
        .on_receive_request(
            {
                let permissions = Arc::clone(&permissions);
                let contexts = Arc::clone(&contexts);
                async move |request: RequestPermissionRequest, responder, cx| {
                    // If the session is already closed (race), fail safe and
                    // respond with cancel
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ));
                    };
                    let events = context.events.clone();
                    let request_id = responder.id().to_string();
                    let options = request
                        .options
                        .iter()
                        .map(|option| PermissionOption {
                            option_id: option.option_id.to_string(),
                            name: option.name.clone(),
                            kind: permission_kind(option.kind),
                        })
                        .collect::<Vec<_>>();
                    let tool = &request.tool_call.fields;
                    let summary = tool
                        .title
                        .clone()
                        .unwrap_or_else(|| request.tool_call.tool_call_id.to_string());

                    fxg_protocol::discard_with!(
                        events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                            status: fxg_protocol::common::SessionStatus::WaitingPermission,
                            error_message: None,
                        })),
                        "notify event subscribers"
                    );
                    fxg_protocol::discard_with!(
                        events.send(DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
                            request_id: request_id.clone(),
                            tool_name: tool.name.clone().unwrap_or_else(|| "tool".to_owned()),
                            summary,
                            options,
                            details: serde_json::to_value(&request.tool_call)
                                .unwrap_or(serde_json::Value::Null)
                                .into(),
                        })),
                        "notify event subscribers"
                    );

                    let (decision_tx, decision_rx) = oneshot::channel();
                    permissions
                        .lock()
                        .expect("permission registry poisoned")
                        .insert(request_id, decision_tx);
                    let events_for_task = context.events.clone();
                    cx.spawn(async move {
                        let outcome = decision_rx
                            .await
                            .unwrap_or(RequestPermissionOutcome::Cancelled);
                        fxg_protocol::discard_with!(
                            events_for_task.send(DriverEvent::Event(
                                UnifiedEventPayload::StatusChanged {
                                    status: fxg_protocol::common::SessionStatus::Running,
                                    error_message: None,
                                },
                            )),
                            "notify event subscribers"
                        );
                        responder.respond(RequestPermissionResponse::new(outcome))
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let contexts = Arc::clone(&contexts);
                async move |request: ReadTextFileRequest, responder, _cx| {
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond_with_internal_error("unknown session");
                    };
                    let path = resolve_path(&context.cwd, &request.path);
                    let result = (|| -> anyhow::Result<String> {
                        let content = std::fs::read_to_string(&path)
                            .with_context(|| format!("failed to read {}", path.display()))?;
                        if request.line.is_some() || request.limit.is_some() {
                            let start = request.line.unwrap_or(1).saturating_sub(1) as usize;
                            let take = request.limit.unwrap_or(u32::MAX) as usize;
                            Ok(content
                                .lines()
                                .skip(start)
                                .take(take)
                                .collect::<Vec<_>>()
                                .join("\n"))
                        } else {
                            Ok(content)
                        }
                    })();
                    match result {
                        Ok(content) => responder.respond(ReadTextFileResponse::new(content)),
                        Err(err) => responder.respond_with_internal_error(err.to_string()),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let contexts = Arc::clone(&contexts);
                async move |request: WriteTextFileRequest, responder, _cx| {
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond_with_internal_error("unknown session");
                    };
                    let path = resolve_path(&context.cwd, &request.path);
                    let old_text = match std::fs::read_to_string(&path) {
                        Ok(text) => text,
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
                        Err(err) => {
                            return responder.respond_with_internal_error(format!(
                                "failed to read {} before writing: {err}",
                                path.display()
                            ));
                        }
                    };
                    match std::fs::write(&path, &request.content) {
                        Ok(()) => {
                            let path_string = path.to_string_lossy().into_owned();
                            let diff = build_file_diff(&path_string, &old_text, &request.content);
                            fxg_protocol::discard_with!(
                                context.events.send(DriverEvent::Event(
                                    UnifiedEventPayload::ToolCall {
                                        tool_call_id: format!("fs-{}", uuid_v7()),
                                        title: format!("write_text_file: {path_string}"),
                                        kind: FxgToolCallKind::Edit,
                                        status: FxgToolCallStatus::Completed,
                                        locations: vec![path_string],
                                        diff: Some(diff),
                                        raw_output: None,
                                    },
                                )),
                                "notify event subscribers"
                            );
                            responder.respond(WriteTextFileResponse::new())
                        }
                        Err(err) => responder.respond_with_internal_error(format!(
                            "failed to write {}: {err}",
                            path.display()
                        )),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let pty = Arc::clone(&pty);
                let terminals = Arc::clone(&terminals);
                let contexts = Arc::clone(&contexts);
                async move |request: CreateTerminalRequest, responder, _cx| {
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond_with_internal_error("unknown session");
                    };
                    // `AsyncFnMut` is called multiple times, so pass cloned captures
                    create_terminal(
                        pty.clone(),
                        terminals.clone(),
                        context.events.clone(),
                        request,
                        responder,
                    )
                    .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let terminals = Arc::clone(&terminals);
                async move |request: TerminalOutputRequest, responder, _cx| match terminals
                    .get(&request.terminal_id.to_string())
                {
                    Ok((_, _, output, exit_rx)) => {
                        let data = output.lock().expect("terminal output poisoned").clone();
                        let text = String::from_utf8_lossy(&data).into_owned();
                        let exit_status = exit_rx
                            .borrow()
                            .as_ref()
                            .copied()
                            .map(|code| exit_status_of(Some(code)));
                        responder.respond(
                            TerminalOutputResponse::new(text, false).exit_status(exit_status),
                        )
                    }
                    Err(err) => responder.respond_with_internal_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let terminals = Arc::clone(&terminals);
                async move |request: WaitForTerminalExitRequest, responder, cx| {
                    let Ok((_, _, _, mut exit_rx)) =
                        terminals.get(&request.terminal_id.to_string())
                    else {
                        return responder.respond_with_internal_error("unknown terminal");
                    };
                    cx.spawn(async move {
                        // Return the current value if already exited; otherwise wait for a change
                        if exit_rx.borrow().is_none() {
                            fxg_protocol::discard_with!(
                                exit_rx.changed().await,
                                "wait for a terminal exit change"
                            );
                        }
                        let status = exit_status_of(*exit_rx.borrow());
                        responder.respond(WaitForTerminalExitResponse::new(status))
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let pty = Arc::clone(&pty);
                let terminals = Arc::clone(&terminals);
                async move |request: KillTerminalRequest, responder, _cx| match terminals
                    .get(&request.terminal_id.to_string())
                {
                    Ok((pty_id, ..)) => {
                        fxg_protocol::discard_with!(
                            pty.kill(&pty_id),
                            "kill the pty terminal on a kill-terminal request"
                        );
                        responder.respond(KillTerminalResponse::new())
                    }
                    Err(err) => responder.respond_with_internal_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let pty = Arc::clone(&pty);
                let terminals = Arc::clone(&terminals);
                async move |request: ReleaseTerminalRequest, responder, _cx| {
                    if let Some(state) = terminals.remove(&request.terminal_id.to_string()) {
                        fxg_protocol::discard_with!(
                            pty.kill(&state.pty_id),
                            "kill the pty terminal on a release-terminal request"
                        );
                    }
                    responder.respond(ReleaseTerminalResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let elicitations = Arc::clone(&elicitations);
                let contexts = Arc::clone(&contexts);
                async move |request: CreateElicitationRequest, responder, cx| {
                    // Phase 1 supports form mode only (the range advertised in capabilities).
                    // Requests for unadvertised modes return Invalid params (spec).
                    let (scope, schema) = match &request.mode {
                        ElicitationMode::Form(form) => {
                            let value = serde_json::to_value(&form.requested_schema)
                                .unwrap_or(serde_json::Value::Null);
                            match serde_json::from_value::<ElicitationSchema>(value) {
                                Ok(schema) => (form.scope.clone(), schema),
                                Err(err) => {
                                    return responder.respond_with_error(
                                        agent_client_protocol::Error::invalid_params()
                                            .data(format!("unsupported elicitation schema: {err}")),
                                    );
                                }
                            }
                        }
                        other => {
                            return responder.respond_with_error(
                                agent_client_protocol::Error::invalid_params()
                                    .data(format!("unsupported elicitation mode: {other:?}")),
                            );
                        }
                    };
                    // Only session scope is supported (request scope is for auth flows
                    // and unsupported)
                    let ElicitationScope::Session(scope) = scope else {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("request-scoped elicitation is not supported"),
                        );
                    };
                    let Some(context) = session_context(&contexts, &scope.session_id) else {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params().data("unknown session"),
                        );
                    };
                    // Form mode has no protocol-level elicitation id, so use the
                    // JSON-RPC request id as the elicitation id (same as approvals).
                    let elicitation_id = responder.id().to_string();
                    let events = context.events.clone();
                    fxg_protocol::discard_with!(
                        events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                            status: fxg_protocol::common::SessionStatus::WaitingInput,
                            error_message: None,
                        })),
                        "notify event subscribers"
                    );
                    fxg_protocol::discard_with!(
                        events.send(DriverEvent::Event(
                            UnifiedEventPayload::ElicitationRequest {
                                elicitation_id: elicitation_id.clone(),
                                message: request.message.clone(),
                                mode: FxgElicitationMode::Form,
                                requested_schema: schema.clone(),
                                tool_call_id: scope.tool_call_id.as_ref().map(|id| id.to_string()),
                            },
                        )),
                        "notify event subscribers"
                    );

                    let (decision_tx, decision_rx) = oneshot::channel();
                    elicitations
                        .lock()
                        .expect("elicitation registry poisoned")
                        .insert(
                            elicitation_id,
                            PendingElicitation {
                                sender: decision_tx,
                                schema,
                            },
                        );
                    let events_for_task = context.events.clone();
                    cx.spawn(async move {
                        let outcome = decision_rx.await.unwrap_or(ElicitationOutcome::Cancel);
                        fxg_protocol::discard_with!(
                            events_for_task.send(DriverEvent::Event(
                                UnifiedEventPayload::StatusChanged {
                                    status: fxg_protocol::common::SessionStatus::Running,
                                    error_message: None,
                                },
                            )),
                            "notify event subscribers"
                        );
                        let response = match outcome {
                            ElicitationOutcome::Accept(content) => {
                                CreateElicitationResponse::new(AcpElicitationAction::Accept(
                                    ElicitationAcceptAction::new().content(content),
                                ))
                            }
                            ElicitationOutcome::Decline => {
                                CreateElicitationResponse::new(AcpElicitationAction::Decline)
                            }
                            ElicitationOutcome::Cancel => {
                                CreateElicitationResponse::new(AcpElicitationAction::Cancel)
                            }
                        };
                        responder.respond(response)
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CompleteElicitationNotification, _cx| {
                // Phase 1 supports form mode only, so URL-mode completion notifications are
                // ignored as unknown IDs (spec: unknown IDs must be ignored).
                tracing::debug!(
                    elicitation_id = %notification.elicitation_id,
                    "elicitation/complete notification ignored (url mode not supported)"
                );
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        );

    // The closures (async move) move `permissions` / `elicitations`, so keep
    // clones for teardown
    let permissions_for_teardown = Arc::clone(&permissions);
    let elicitations_for_teardown = Arc::clone(&elicitations);

    // Forward agent stderr to the daemon log
    let stderr_for_debug = stderr.clone();
    let agent = agent.with_debug(move |line, direction| {
        if matches!(direction, agent_client_protocol::LineDirection::Stderr) {
            stderr_for_debug.push(&agent_id, line);
            tracing::debug!(target: "fxg_acp::acp", "acp agent: {line}");
        }
    });

    let run_result = builder
        .connect_with(agent, async move |cx: ConnectionTo<Agent>| {
            // 1) initialize (once per process; declare support for fs read/write +
            //    terminal + elicitation (form))
            let initialize = match cx
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1)
                        .client_capabilities(client_capabilities())
                        .client_info(Implementation::new("fxg", env!("CARGO_PKG_VERSION"))),
                )
                .block_task()
                .await
            {
                Ok(initialize) => initialize,
                Err(err) => {
                    if let Some(tx) = ready_tx.take() {
                        fxg_protocol::discard_with!(
                            tx.send(Err(anyhow!("acp initialize failed: {err}"))),
                            "discard a non-critical result"
                        );
                    }
                    return Err(err);
                }
            };
            let agent_capabilities = initialize.agent_capabilities.clone();
            let prompt_capabilities = agent_capabilities.prompt_capabilities.clone();
            let auth_methods = initialize
                .auth_methods
                .iter()
                .map(map_auth_method)
                .collect::<Vec<_>>();
            if let Some(tx) = ready_tx.take() {
                fxg_protocol::discard_with!(tx.send(Ok(())), "discard a non-critical result");
            }

            // 2) Session processing loop. Handles one session at a time per process,
            //    waiting idle for the next `NewSession` after a session ends.
            while let Some(command) = control_rx.recv().await {
                let ProcessCommand::NewSession {
                    params,
                    events,
                    lease,
                    commands: (cmd_tx, cmd_rx),
                    ready,
                } = command
                else {
                    // Dispose = process termination
                    break;
                };
                // Build the session. On failure the connection health is unknown, so
                // return the error and shut the process down (the caller falls back
                // to a cold start).
                let (session, acp_session_id, context_restored, capabilities_state) =
                    match build_session(
                        &cx,
                        &agent_capabilities,
                        &params,
                        &events,
                        &contexts,
                        &auth_methods,
                    )
                    .await
                    {
                        Ok(built) => built,
                        Err(err) => {
                            if ready.send(Err(err)).is_err() {
                                tracing::debug!(
                                    "session waiter dropped before the start failure was reported"
                                );
                            }
                            lease.mark_dispose();
                            break;
                        }
                    };
                let session_end_tx = lease.take_session_end_sender();
                let handle = AcpSessionHandle {
                    acp_session_id: acp_session_id.clone(),
                    conn: cx.clone(),
                    commands: cmd_tx,
                    permissions: Arc::clone(&permissions),
                    elicitations: Arc::clone(&elicitations),
                    events: Mutex::new(Some(events.clone())),
                    capabilities: Arc::clone(&capabilities_state),
                    pty: Arc::clone(&pty),
                    lease: Arc::clone(&lease),
                };
                if ready.send(Ok((handle, context_restored))).is_err() {
                    tracing::debug!("session waiter dropped before the session became ready");
                }

                // 3) Turn loop (serializes prompt sends with update reads)
                let outcome = serve_session(
                    session,
                    cmd_rx,
                    &events,
                    capabilities_state,
                    &prompt_capabilities,
                )
                .await;
                contexts
                    .lock()
                    .expect("session contexts poisoned")
                    .remove(&acp_session_id.to_string());
                if let Some(tx) = session_end_tx {
                    fxg_protocol::discard_with!(tx.send(()), "discard a non-critical result");
                }
                // Close unresolved approvals and questions tied to the session with cancel
                cancel_pending_permissions(&permissions);
                cancel_pending_elicitations(&elicitations, Some(&events));
                match outcome {
                    ServeOutcome::Closed => {
                        fxg_protocol::discard_with!(
                            events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                                status: fxg_protocol::common::SessionStatus::Stopped,
                                error_message: None,
                            })),
                            "notify event subscribers"
                        );
                    }
                    ServeOutcome::Fatal(err) => {
                        tracing::warn!(
                            session_id = %params.session_id,
                            "acp session ended: {err:#}"
                        );
                        fxg_protocol::discard_with!(
                            events.send(DriverEvent::Failed {
                                message: err.to_string(),
                            }),
                            "notify event subscribers"
                        );
                        // Shut the process down on connection failure (do not return it
                        // to the pool)
                        lease.mark_dispose();
                        break;
                    }
                }
            }

            // Exiting the loop = process shutdown
            Ok(())
        })
        .await;

    // Close any remaining unresolved approvals and questions with cancel (process exit)
    cancel_pending_permissions(&permissions_for_teardown);
    cancel_pending_elicitations(&elicitations_for_teardown, None);

    run_result
}

/// Reason the turn loop ended.
enum ServeOutcome {
    /// Normal end (`AcpCommand::Shutdown`)
    Closed,
    /// Connection / turn failure (shut the process down)
    Fatal(anyhow::Error),
}

/// Creates and initializes a session (`session/new` | `session/resume` | `session/load`).
///
/// Registers session information in [`SessionContexts`] (referenced by the fs /
/// terminal handlers), applies the initial mode, and publishes modes / commands /
/// config options.
#[allow(clippy::type_complexity)]
async fn build_session(
    cx: &ConnectionTo<Agent>,
    agent_capabilities: &AgentCapabilities,
    params: &SessionParams,
    events: &mpsc::UnboundedSender<DriverEvent>,
    contexts: &SessionContexts,
    auth_methods: &[AgentAuthMethodInfo],
) -> anyhow::Result<(
    ActiveSession<'static, Agent>,
    SessionId,
    bool,
    Arc<Mutex<SessionCapabilitiesState>>,
)> {
    // Acquire the session (native fork / new / session/resume / session/load)
    let acquired = if let Some(source_id) = params.fork_from.as_deref() {
        if agent_capabilities.session_capabilities.fork.is_none()
            || !agent_capabilities.load_session
        {
            return Err(NativeForkUnavailable(
                "agent does not support session/fork + loadSession".to_owned(),
            )
            .into());
        }
        match fork_and_load_session(cx, source_id, &params.cwd).await {
            Ok(acquired) => acquired,
            Err(err) if err.code == agent_client_protocol::ErrorCode::AUTH_REQUIRED => {
                return Err(AgentAuthRequired(auth_methods.to_vec()).into());
            }
            Err(err) => {
                return Err(NativeForkUnavailable(format!("session/fork failed: {err}")).into());
            }
        }
    } else {
        let plan = plan_resume(agent_capabilities, params.resume.as_ref())?;
        match acquire_session(cx, plan, &params.cwd).await {
            Ok(acquired) => acquired,
            Err(err) if err.code == agent_client_protocol::ErrorCode::AUTH_REQUIRED => {
                return Err(AgentAuthRequired(auth_methods.to_vec()).into());
            }
            // For native-only resume (`allow_fresh = false`), report a restore failure to
            // the caller (the node) as "native resume unavailable"
            Err(err)
                if params
                    .resume
                    .as_ref()
                    .is_some_and(|resume| !resume.allow_fresh) =>
            {
                return Err(NativeResumeUnavailable(format!(
                    "failed to resume acp session: {err}"
                ))
                .into());
            }
            Err(err) => return Err(anyhow!("failed to create acp session: {err}")),
        }
    };
    let session = acquired.session;
    let context_restored = acquired.is_resumed;
    let acp_session_id = session.session_id().clone();
    // Register session information (referenced by the fs / terminal handlers)
    contexts.lock().expect("session contexts poisoned").insert(
        acp_session_id.to_string(),
        SessionContext {
            cwd: params.cwd.clone(),
            events: events.clone(),
        },
    );
    fxg_protocol::discard_with!(
        events.send(DriverEvent::Event(UnifiedEventPayload::SessionAgentBound {
            agent_session_id: acp_session_id.to_string(),
        })),
        "notify event subscribers"
    );

    // Apply initial configuration (model / thinking level, etc.)
    for setting in &params.initial_config {
        let value = match parse_config_value(setting.value.clone()) {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(key = %setting.key, "invalid initial config value: {err}");
                continue;
            }
        };
        if let Err(err) = cx
            .send_request(SetSessionConfigOptionRequest::new(
                acp_session_id.clone(),
                setting.key.clone(),
                value,
            ))
            .block_task()
            .await
        {
            tracing::warn!(key = %setting.key, "failed to set initial config: {err}");
        }
    }

    // Apply the initial mode
    if let Some(mode_id) = params.initial_mode.as_ref()
        && let Err(err) = cx
            .send_request(SetSessionModeRequest::new(
                acp_session_id.clone(),
                mode_id.clone(),
            ))
            .block_task()
            .await
    {
        tracing::warn!("failed to set initial mode {mode_id}: {err}");
    }

    // Publish modes, slash commands and config options
    let (modes, commands, config_options) =
        session_capabilities(acquired.modes.as_ref(), acquired.config_options.as_deref());
    let current_mode = acquired
        .modes
        .as_ref()
        .map(|state| state.current_mode_id.to_string());
    let capabilities_state = SessionCapabilitiesState {
        current_mode: current_mode.clone(),
        available_modes: modes.clone(),
        available_commands: commands.clone(),
        config_options: config_options.clone(),
    };
    if !modes.is_empty()
        || !commands.is_empty()
        || !config_options.is_empty()
        || current_mode.is_some()
    {
        fxg_protocol::discard_with!(
            events.send(DriverEvent::Event(capabilities_state.to_payload())),
            "notify event subscribers"
        );
    }
    fxg_protocol::discard_with!(
        events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: fxg_protocol::common::SessionStatus::Idle,
            error_message: None,
        })),
        "notify event subscribers"
    );

    Ok((
        session,
        acp_session_id,
        context_restored,
        Arc::new(Mutex::new(capabilities_state)),
    ))
}

/// Turn loop for one session (serializes prompt sends with update reads).
async fn serve_session(
    mut session: ActiveSession<'static, Agent>,
    mut cmd_rx: mpsc::UnboundedReceiver<AcpCommand>,
    events: &mpsc::UnboundedSender<DriverEvent>,
    capabilities_state: Arc<Mutex<SessionCapabilitiesState>>,
    prompt_capabilities: &PromptCapabilities,
) -> ServeOutcome {
    let mut accumulator = StreamingAccumulator::default();
    while let Some(command) = cmd_rx.recv().await {
        match command {
            AcpCommand::Prompt(input) => {
                accumulator.reset();
                fxg_protocol::discard_with!(
                    events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                        status: fxg_protocol::common::SessionStatus::Running,
                        error_message: None,
                    })),
                    "notify event subscribers"
                );
                let blocks = match prompt_blocks(input, prompt_capabilities) {
                    Ok(blocks) => blocks,
                    Err(err) => {
                        fxg_protocol::discard_with!(
                            events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                                status: fxg_protocol::common::SessionStatus::Error,
                                error_message: Some(format!("invalid prompt: {err}")),
                            })),
                            "notify event subscribers"
                        );
                        return ServeOutcome::Fatal(err);
                    }
                };
                if let Err(err) = session.send_prompt_blocks(blocks) {
                    return ServeOutcome::Fatal(err.into());
                }
                loop {
                    match session.read_update().await {
                        Ok(SessionMessage::PromptResponse(response)) => {
                            for payload in accumulator.finish_all() {
                                fxg_protocol::discard_with!(
                                    events.send(DriverEvent::Event(payload)),
                                    "notify event subscribers"
                                );
                            }
                            match response {
                                Ok(resp) => {
                                    let reason = resp.stop_reason;
                                    tracing::debug!(?reason, "acp turn finished");
                                    if let Some(payload) = turn_ended_event(reason) {
                                        fxg_protocol::discard_with!(
                                            events.send(DriverEvent::Event(payload)),
                                            "notify event subscribers"
                                        );
                                    }
                                }
                                Err(err) => {
                                    let message = format!("agent turn failed: {err}");
                                    tracing::warn!("{message}");
                                    fxg_protocol::discard_with!(
                                        events.send(DriverEvent::Event(
                                            UnifiedEventPayload::StatusChanged {
                                                status: fxg_protocol::common::SessionStatus::Error,
                                                error_message: Some(message),
                                            },
                                        )),
                                        "notify event subscribers"
                                    );
                                    break;
                                }
                            }
                            fxg_protocol::discard_with!(
                                events.send(DriverEvent::Event(
                                    UnifiedEventPayload::StatusChanged {
                                        status: fxg_protocol::common::SessionStatus::Idle,
                                        error_message: None,
                                    },
                                )),
                                "notify event subscribers"
                            );
                            break;
                        }
                        Ok(SessionMessage::Message(dispatch)) => {
                            let acc = &mut accumulator;
                            let ev = events;
                            let caps = Arc::clone(&capabilities_state);
                            let result = MatchDispatch::new(dispatch.into_raw())
                                .if_notification(async move |notification: SessionNotification| {
                                    let mapped = {
                                        let mut caps =
                                            caps.lock().expect("session capabilities poisoned");
                                        map_session_update(notification.update, acc, &mut caps)
                                    };
                                    for event in mapped {
                                        fxg_protocol::discard_with!(
                                            ev.send(event),
                                            "notify event subscribers"
                                        );
                                    }
                                    Ok(())
                                })
                                .await
                                .otherwise_ignore();
                            if let Err(err) = result {
                                return ServeOutcome::Fatal(err.into());
                            }
                        }
                        // `SessionMessage` is #[non_exhaustive]
                        Ok(_) => {}
                        Err(err) => return ServeOutcome::Fatal(err.into()),
                    }
                }
            }
            AcpCommand::Shutdown => break,
        }
    }
    ServeOutcome::Closed
}

/// Turns ACP `StopReason` values that should be surfaced to the user into events.
///
/// `EndTurn` (normal completion) / `Cancelled` (user cancellation) count as
/// normal completion and return `None` (`StopReason` is `#[non_exhaustive]`).
fn turn_ended_event(reason: StopReason) -> Option<UnifiedEventPayload> {
    let reason = match reason {
        StopReason::MaxTokens => TurnStopReason::MaxTokens,
        StopReason::MaxTurnRequests => TurnStopReason::MaxTurnRequests,
        StopReason::Refusal => TurnStopReason::Refusal,
        _ => return None,
    };
    Some(UnifiedEventPayload::TurnEnded { reason })
}

/// Extracts the session's modes, commands and config options.
fn session_capabilities(
    modes: Option<&SessionModeState>,
    config_options: Option<&[SessionConfigOption]>,
) -> (Vec<ModeInfo>, Vec<CommandInfo>, Vec<ConfigOptionInfo>) {
    let modes = modes
        .map(|state| {
            state
                .available_modes
                .iter()
                .map(|mode| ModeInfo {
                    mode_id: mode.id.to_string(),
                    name: mode.name.clone(),
                    description: mode.description.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let config_options = config_options
        .map(|options| options.iter().map(map_config_option).collect())
        .unwrap_or_default();
    // Slash commands arrive via session/update (AvailableCommandsUpdate), so this
    // starts empty (later notifications update it)
    (modes, Vec::new(), config_options)
}

/// Decides how to acquire the session from the `resume` selection and agent capabilities.
#[derive(Debug)]
enum ResumePlan {
    /// `session/resume` (no history replay)
    Resume(String),
    /// `session/load` (replay notifications are discarded)
    Load(String),
    /// Create a fresh session (the caller injects history replay)
    Fresh,
}

/// Decides how to acquire the session.
///
/// fxg keeps the canonical conversation history in its own event log, so it prefers
/// `session/resume`, which does not send history back. Only when resume is
/// unsupported does it use `session/load` (discarding replay notifications), and
/// if neither is available it creates a fresh session
/// (design: `docs/04-agent-drivers-and-windows.md` §4.3).
///
/// When `resume.allow_fresh` is `false` (automatic resume on send), it returns
/// [`NativeResumeUnavailable`] instead of creating a fresh session, prompting the
/// caller to perform an explicit resume via history replay.
fn plan_resume(
    capabilities: &AgentCapabilities,
    resume: Option<&ResumeRequest>,
) -> anyhow::Result<ResumePlan> {
    // No resume selection → create fresh
    let Some(resume) = resume else {
        return Ok(ResumePlan::Fresh);
    };
    let Some(agent_session_id) = resume.agent_session_id.clone() else {
        // No agent-side ID recorded (caller is expected to inject history replay)
        return if resume.allow_fresh {
            Ok(ResumePlan::Fresh)
        } else {
            Err(NativeResumeUnavailable("no agent session id recorded".to_owned()).into())
        };
    };

    if capabilities.session_capabilities.resume.is_some() {
        return Ok(ResumePlan::Resume(agent_session_id));
    }
    if capabilities.load_session {
        return Ok(ResumePlan::Load(agent_session_id));
    }
    if resume.allow_fresh {
        return Ok(ResumePlan::Fresh);
    }
    Err(NativeResumeUnavailable(format!(
        "agent does not support session/resume or session/load (agent_session_id={agent_session_id})"
    ))
    .into())
}

struct AcquiredSession {
    session: ActiveSession<'static, Agent>,
    is_resumed: bool,
    modes: Option<SessionModeState>,
    config_options: Option<Vec<SessionConfigOption>>,
}

/// Acquires a session (following [`ResumePlan`]).
///
/// The returned `is_resumed` indicates native restore success (`false` = fresh
/// session; the caller injects history replay).
async fn acquire_session(
    cx: &ConnectionTo<Agent>,
    plan: ResumePlan,
    cwd: &Path,
) -> Result<AcquiredSession, agent_client_protocol::Error> {
    match plan {
        ResumePlan::Fresh => {
            let SessionSetup {
                session, response, ..
            } = cx
                .build_session_from(NewSessionRequest::new(cwd))
                .block_task()
                .start_session()
                .await?;
            Ok(AcquiredSession {
                session,
                is_resumed: false,
                modes: response.modes,
                config_options: response.config_options,
            })
        }
        ResumePlan::Resume(agent_session_id) => {
            let SessionSetup {
                session, response, ..
            } = cx
                .resume_session(agent_session_id, cwd)
                .block_task()
                .start_session()
                .await?;
            Ok(AcquiredSession {
                session,
                is_resumed: true,
                modes: response.modes,
                config_options: response.config_options,
            })
        }
        ResumePlan::Load(agent_session_id) => {
            let SessionSetup {
                mut session,
                response,
                ..
            } = cx
                .load_session(agent_session_id, cwd)
                .block_task()
                .start_session()
                .await?;
            discard_replay_updates(&mut session);
            Ok(AcquiredSession {
                session,
                is_resumed: true,
                modes: response.modes,
                config_options: response.config_options,
            })
        }
    }
}

/// Creates a native fork and attaches to it via `session/load`.
///
/// The ACP SDK does not expose a v1 fork builder, so the unstable
/// `session/fork` request is sent directly. `session/load` then attaches the
/// returned session to an `ActiveSession`; its replay notifications are
/// discarded because fxg keeps the canonical history itself.
async fn fork_and_load_session(
    cx: &ConnectionTo<Agent>,
    source_session_id: &str,
    cwd: &Path,
) -> Result<AcquiredSession, agent_client_protocol::Error> {
    let fork: ForkSessionResponse = cx
        .send_request(ForkSessionRequest::new(
            SessionId::new(source_session_id),
            cwd,
        ))
        .block_task()
        .await?;
    let SessionSetup {
        mut session,
        response,
        ..
    } = cx
        .load_session(fork.session_id, cwd)
        .block_task()
        .start_session()
        .await?;
    discard_replay_updates(&mut session);
    Ok(AcquiredSession {
        session,
        is_resumed: true,
        modes: response.modes.or(fork.modes),
        config_options: response.config_options.or(fork.config_options),
    })
}

/// Discards replay notifications that arrived before the `session/load` response.
///
/// Per the ACP spec, conversation history replay completes before the load
/// response, and the SDK buffers notifications from that period into the restored
/// session. fxg keeps the canonical history in its own event log, so it drains and
/// drops the queue right after the response to avoid double recording.
fn discard_replay_updates(session: &mut ActiveSession<'static, Agent>) {
    use futures_util::FutureExt;

    let mut discarded = 0usize;
    while let Some(result) = session.read_update().now_or_never() {
        if result.is_err() {
            break;
        }
        discarded += 1;
    }
    if discarded > 0 {
        tracing::debug!(discarded, "discarded session/load replay updates");
    }
}

/// Client capabilities advertised to every ACP agent.
///
/// `auth.terminal` is enabled because fxg can reproduce the configured agent
/// invocation in an interactive terminal (`fxg agents login`).
pub(crate) fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
        .fs(FileSystemCapabilities::new()
            .read_text_file(true)
            .write_text_file(true))
        .terminal(true)
        .auth(AuthCapabilities::new().terminal(true))
        .elicitation(ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()))
}

/// Builds ACP content blocks from a prompt and its attachments.
///
/// Images are sent as `ImageContent` when the agent advertises `image`
/// support. Non-image attachments use `ResourceLink`, which every agent must
/// support. When image support is unavailable, the attachment is described in
/// text so the user's input is not lost.
fn prompt_blocks(
    input: PromptInput,
    capabilities: &PromptCapabilities,
) -> anyhow::Result<Vec<ContentBlock>> {
    let mut blocks = Vec::new();
    if !input.text.trim().is_empty() {
        blocks.push(ContentBlock::Text(TextContent::new(input.text)));
    }
    for attachment in input.attachments {
        if attachment.mime_type.starts_with("image/") {
            let data = match attachment.data_b64 {
                Some(data) => data,
                None => {
                    let path = attachment.local_path.as_deref().ok_or_else(|| {
                        anyhow!(
                            "image attachment {} has no data or local path",
                            attachment.file_name
                        )
                    })?;
                    base64_encode(
                        &std::fs::read(path)
                            .with_context(|| format!("failed to read image attachment {path}"))?,
                    )
                }
            };
            if capabilities.image {
                blocks.push(ContentBlock::Image(ImageContent::new(
                    data,
                    attachment.mime_type,
                )));
            } else {
                blocks.push(ContentBlock::Text(TextContent::new(format!(
                    "[image attachment: {} ({})]",
                    attachment.file_name,
                    attachment.local_path.as_deref().unwrap_or("inline")
                ))));
            }
        } else {
            let uri = attachment
                .local_path
                .as_deref()
                .map(file_uri)
                .unwrap_or_else(|| attachment.file_name.clone());
            blocks.push(ContentBlock::ResourceLink(
                ResourceLink::new(attachment.file_name, uri).mime_type(attachment.mime_type),
            ));
        }
    }
    if blocks.is_empty() {
        bail!("prompt has no content");
    }
    Ok(blocks)
}

/// Builds a `file://` URI for a node-local path.
fn file_uri(path: &str) -> String {
    #[cfg(windows)]
    {
        format!("file:///{}", path.replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        format!("file://{path}")
    }
}

/// Converts an ACP authentication method into fxg's display / login type.
pub(crate) fn map_auth_method(method: &AuthMethod) -> AgentAuthMethodInfo {
    let id = method.id().to_string();
    let name = method.name().to_owned();
    let description = method.description().map(ToOwned::to_owned);
    match method {
        AuthMethod::Agent(_) => AgentAuthMethodInfo {
            id,
            name,
            description,
            kind: AgentAuthMethodKind::Agent,
            args: Vec::new(),
            env: BTreeMap::new(),
        },
        AuthMethod::Terminal(terminal) => AgentAuthMethodInfo {
            id,
            name,
            description,
            kind: AgentAuthMethodKind::Terminal,
            args: terminal.args.clone(),
            env: terminal
                .env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        },
        _ => AgentAuthMethodInfo {
            id,
            name,
            description,
            kind: AgentAuthMethodKind::Unknown("unknown".to_owned()),
            args: Vec::new(),
            env: BTreeMap::new(),
        },
    }
}

/// Converts an ACP config option into FXG's display type.
fn map_config_option(option: &SessionConfigOption) -> ConfigOptionInfo {
    let (current_value, options) = match &option.kind {
        SessionConfigKind::Select(select) => (
            ConfigOptionValue::Select(select.current_value.to_string()),
            match &select.options {
                agent_client_protocol::v1::schema::SessionConfigSelectOptions::Ungrouped(
                    values,
                ) => values
                    .iter()
                    .map(|value| ConfigOptionValue::Select(value.value.to_string()))
                    .collect(),
                agent_client_protocol::v1::schema::SessionConfigSelectOptions::Grouped(groups) => {
                    groups
                        .iter()
                        .flat_map(|group| &group.options)
                        .map(|value| ConfigOptionValue::Select(value.value.to_string()))
                        .collect()
                }
                _ => Vec::new(),
            },
        ),
        SessionConfigKind::Boolean(boolean) => (
            ConfigOptionValue::Boolean(boolean.current_value),
            vec![
                ConfigOptionValue::Boolean(true),
                ConfigOptionValue::Boolean(false),
            ],
        ),
        _ => (
            ConfigOptionValue::Unknown(serde_json::Value::Null.into()),
            Vec::new(),
        ),
    };
    ConfigOptionInfo {
        key: option.id.to_string(),
        name: option.name.clone(),
        current_value,
        options,
    }
}

/// Converts an FXG config value into the ACP wire value.
///
/// FXG exposes select values as bare strings (for example
/// `"opencode-go/deepseek-v4.1-flash"`), while ACP serializes the same value as
/// `{ "value": "..." }`. Deserializing the bare string directly into
/// [`SessionConfigOptionValue`] fails, so map the scalar shapes explicitly and
/// only fall back to serde for the tagged (object) forms.
fn parse_config_value(value: ConfigOptionValue) -> anyhow::Result<SessionConfigOptionValue> {
    match value {
        ConfigOptionValue::Select(id) => Ok(SessionConfigOptionValue::value_id(id)),
        ConfigOptionValue::Boolean(enabled) => Ok(SessionConfigOptionValue::boolean(enabled)),
        ConfigOptionValue::Unknown(raw) => {
            serde_json::from_value(raw.into_inner()).map_err(Into::into)
        }
    }
}

/// Tracking state for session capabilities (modes, commands, config options).
#[derive(Debug, Default, Clone)]
struct SessionCapabilitiesState {
    current_mode: Option<String>,
    available_modes: Vec<ModeInfo>,
    available_commands: Vec<CommandInfo>,
    config_options: Vec<ConfigOptionInfo>,
}

impl SessionCapabilitiesState {
    fn to_payload(&self) -> UnifiedEventPayload {
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode: self.current_mode.clone(),
            available_modes: self.available_modes.clone(),
            available_commands: self.available_commands.clone(),
            config_options: self.config_options.clone(),
        }
    }
}

/// Accumulates streaming chunks and emits completion events when the stream changes.
///
/// Only completion events are persisted (docs/01 §2.3), and they consume a
/// `node_seq` at the moment they are emitted. A stream must therefore be
/// finalized as soon as the agent moves on to a different stream, otherwise the
/// reasoning would be recorded after the answer it preceded.
#[derive(Debug, Default)]
struct StreamingAccumulator {
    message: Option<(String, String)>,
    thought: Option<(String, String)>,
}

impl StreamingAccumulator {
    fn reset(&mut self) {
        self.message = None;
        self.thought = None;
    }

    fn finish_message(&mut self) -> Option<UnifiedEventPayload> {
        self.message
            .take()
            .map(|(message_id, text)| UnifiedEventPayload::AgentMessage {
                message_id,
                text,
                is_complete: true,
            })
    }

    fn finish_thought(&mut self) -> Option<UnifiedEventPayload> {
        self.thought
            .take()
            .map(|(thought_id, text)| UnifiedEventPayload::AgentThought {
                thought_id,
                text,
                is_complete: true,
            })
    }

    /// Appends a message chunk. Returns the completion events for any stream that
    /// ended before this message started (a thought, or a message with another ID).
    fn push_message(&mut self, message_id: String, delta: &str) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        // Reasoning always precedes the answer it belongs to.
        out.extend(self.finish_thought());
        if self
            .message
            .as_ref()
            .is_some_and(|(id, _)| *id != message_id)
        {
            out.extend(self.finish_message());
        }
        match &mut self.message {
            Some((_, text)) => text.push_str(delta),
            None => self.message = Some((message_id, delta.to_owned())),
        }
        out
    }

    /// Appends a thought chunk. Returns the completion events for any stream that
    /// ended before this thought started (a message, or a thought with another ID).
    fn push_thought(&mut self, thought_id: String, delta: &str) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        out.extend(self.finish_message());
        if self
            .thought
            .as_ref()
            .is_some_and(|(id, _)| *id != thought_id)
        {
            out.extend(self.finish_thought());
        }
        match &mut self.thought {
            Some((_, text)) => text.push_str(delta),
            None => self.thought = Some((thought_id, delta.to_owned())),
        }
        out
    }

    /// Takes the accumulated message / thought as completion events.
    fn finish_all(&mut self) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        // Defensive ordering: reasoning precedes the answer within a turn.
        out.extend(self.finish_thought());
        out.extend(self.finish_message());
        out
    }
}

/// Converts a `session/update` notification into a list of [`DriverEvent`]s.
fn map_session_update(
    update: SessionUpdate,
    accumulator: &mut StreamingAccumulator,
    capabilities: &mut SessionCapabilitiesState,
) -> Vec<DriverEvent> {
    let mut events = Vec::new();
    match update {
        SessionUpdate::UserMessageChunk(_) => {
            // User messages are already recorded by the manager as `UserMessage`
        }
        SessionUpdate::AgentMessageChunk(ContentChunk {
            content,
            message_id,
            ..
        }) => {
            if let ContentBlock::Text(TextContent { text, .. }) = content {
                let message_id = message_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "acp-message".to_owned());
                for finished in accumulator.push_message(message_id.clone(), &text) {
                    events.push(DriverEvent::Event(finished));
                }
                events.push(DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta {
                    message_id,
                    text_delta: text,
                }));
            }
        }
        SessionUpdate::AgentThoughtChunk(ContentChunk {
            content,
            message_id,
            ..
        }) => {
            if let ContentBlock::Text(TextContent { text, .. }) = content {
                let thought_id = message_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "acp-thought".to_owned());
                for finished in accumulator.push_thought(thought_id.clone(), &text) {
                    events.push(DriverEvent::Event(finished));
                }
                events.push(DriverEvent::Delta(StreamDeltaPayload::AgentThoughtDelta {
                    thought_id,
                    text_delta: text,
                }));
            }
        }
        SessionUpdate::ToolCall(tool_call) => {
            events.push(DriverEvent::Event(tool_call_event(&tool_call)));
        }
        SessionUpdate::ToolCallUpdate(update) => {
            events.push(DriverEvent::Event(tool_call_update_event(&update)));
        }
        SessionUpdate::Plan(plan) => {
            let entries = plan
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| PlanEntry {
                    id: format!("plan-{index}"),
                    title: entry.content.clone(),
                    status: plan_status(&entry.status),
                })
                .collect();
            events.push(DriverEvent::Event(UnifiedEventPayload::PlanUpdate {
                entries,
            }));
        }
        SessionUpdate::AvailableCommandsUpdate(update) => {
            let commands = update
                .available_commands
                .iter()
                .map(|command| CommandInfo {
                    name: command.name.clone(),
                    description: command.description.clone(),
                    input_hint: command.input.as_ref().map(|input| match input {
                        agent_client_protocol::v1::schema::AvailableCommandInput::Unstructured(
                            unstructured,
                        ) => unstructured.hint.clone(),
                        _ => String::new(),
                    }),
                })
                .collect();
            capabilities.available_commands = commands;
            events.push(DriverEvent::Event(capabilities.to_payload()));
        }
        SessionUpdate::CurrentModeUpdate(update) => {
            capabilities.current_mode = Some(update.current_mode_id.to_string());
            events.push(DriverEvent::Event(capabilities.to_payload()));
        }
        SessionUpdate::ConfigOptionUpdate(update) => {
            capabilities.config_options = update
                .config_options
                .iter()
                .map(map_config_option)
                .collect();
            events.push(DriverEvent::Event(capabilities.to_payload()));
        }
        SessionUpdate::SessionInfoUpdate(update) => {
            if let Some(title) = update.title.value() {
                let trimmed = title.trim();
                if !trimmed.is_empty() {
                    events.push(DriverEvent::Event(
                        UnifiedEventPayload::SessionTitleChanged {
                            title: trimmed.to_owned(),
                        },
                    ));
                }
            }
        }
        SessionUpdate::UsageUpdate(update) => {
            events.push(DriverEvent::Event(UnifiedEventPayload::UsageUpdated {
                used_tokens: update.used,
                context_size: update.size,
                cost: update.cost.map(|cost| UsageCost {
                    amount: cost.amount,
                    currency: cost.currency,
                }),
            }));
        }
        // `SessionUpdate` is #[non_exhaustive]
        other => {
            tracing::trace!("unhandled session update: {other:?}");
        }
    }
    events
}

/// Converts an ACP `ToolCall` into a normalized event.
fn tool_call_event(tool_call: &ToolCall) -> UnifiedEventPayload {
    let (diff, _, text_content) = tool_call_contents(&tool_call.content);
    UnifiedEventPayload::ToolCall {
        tool_call_id: tool_call.tool_call_id.to_string(),
        title: tool_call.title.clone(),
        kind: tool_kind(tool_call.kind),
        status: tool_status(tool_call.status),
        locations: tool_call
            .locations
            .iter()
            .map(|location| location.path.to_string_lossy().into_owned())
            .collect(),
        diff,
        raw_output: tool_call
            .raw_output
            .as_ref()
            .map(|value| value.to_string())
            .or_else(|| (!text_content.is_empty()).then(|| text_content.join("\n"))),
    }
}

/// Converts an ACP `ToolCallUpdate` into a normalized event (appended to the same `tool_call_id`).
fn tool_call_update_event(update: &ToolCallUpdate) -> UnifiedEventPayload {
    let fields = &update.fields;
    let (diff, locations, text_content) = fields
        .content
        .as_ref()
        .map(|content| tool_call_contents(content))
        .unwrap_or((None, Vec::new(), Vec::new()));
    let locations = fields
        .locations
        .as_ref()
        .map(|values| {
            values
                .iter()
                .map(|location| location.path.to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or(locations);
    UnifiedEventPayload::ToolCall {
        tool_call_id: update.tool_call_id.to_string(),
        title: fields.title.clone().unwrap_or_default(),
        kind: fields
            .kind
            .map(tool_kind)
            .unwrap_or_else(|| FxgToolCallKind::Unknown(String::new())),
        status: fields
            .status
            .map(tool_status)
            .unwrap_or_else(|| FxgToolCallStatus::Unknown(String::new())),
        locations,
        diff,
        raw_output: fields
            .raw_output
            .as_ref()
            .map(|value| value.to_string())
            .or_else(|| (!text_content.is_empty()).then(|| text_content.join("\n"))),
    }
}

/// Extracts the diff, file paths and text output from a `ToolCallContent` list.
fn tool_call_contents(
    contents: &[ToolCallContent],
) -> (Option<FileDiff>, Vec<String>, Vec<String>) {
    let mut diff = None;
    let mut locations = Vec::new();
    let mut texts = Vec::new();
    for content in contents {
        match content {
            ToolCallContent::Diff(acp_diff) => {
                let path = acp_diff.path.to_string_lossy().into_owned();
                locations.push(path.clone());
                let old_text = acp_diff.old_text.clone().unwrap_or_default();
                diff = Some(build_file_diff(&path, &old_text, &acp_diff.new_text));
            }
            ToolCallContent::Content(content) => {
                if let ContentBlock::Text(text) = &content.content {
                    texts.push(text.text.clone());
                }
            }
            _ => {}
        }
    }
    (diff, locations, texts)
}

/// Computes a unified diff from the before/after text and builds a [`FileDiff`].
pub(crate) fn build_file_diff(path: &str, old_text: &str, new_text: &str) -> FileDiff {
    let patch = diffy::create_patch(old_text, new_text);
    let unified_diff = patch.to_string();
    let mut additions = 0u32;
    let mut deletions = 0u32;
    for line in unified_diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            additions += 1;
        } else if line.starts_with('-') {
            deletions += 1;
        }
    }
    FileDiff {
        path: path.to_owned(),
        old_text: Some(old_text.to_owned()),
        new_text: Some(new_text.to_owned()),
        unified_diff,
        additions,
        deletions,
    }
}

/// Serializes an ACP enum value to the snake_case string used on the wire.
fn acp_wire_value<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        _ => "unknown".to_owned(),
    }
}

fn tool_kind(kind: ToolKind) -> FxgToolCallKind {
    match kind {
        ToolKind::Read => FxgToolCallKind::Read,
        ToolKind::Edit | ToolKind::Delete | ToolKind::Move => FxgToolCallKind::Edit,
        ToolKind::Search => FxgToolCallKind::Search,
        ToolKind::Execute => FxgToolCallKind::Execute,
        ToolKind::Think | ToolKind::Fetch | ToolKind::SwitchMode | ToolKind::Other => {
            FxgToolCallKind::Other
        }
        _ => FxgToolCallKind::Unknown(acp_wire_value(&kind)),
    }
}

fn tool_status(status: ToolCallStatus) -> FxgToolCallStatus {
    match status {
        ToolCallStatus::Pending => FxgToolCallStatus::Pending,
        ToolCallStatus::InProgress => FxgToolCallStatus::InProgress,
        ToolCallStatus::Completed => FxgToolCallStatus::Completed,
        ToolCallStatus::Failed => FxgToolCallStatus::Failed,
        _ => FxgToolCallStatus::Unknown(acp_wire_value(&status)),
    }
}

fn plan_status(status: &AcpPlanEntryStatus) -> FxgPlanEntryStatus {
    match status {
        AcpPlanEntryStatus::Pending => FxgPlanEntryStatus::Pending,
        AcpPlanEntryStatus::InProgress => FxgPlanEntryStatus::InProgress,
        AcpPlanEntryStatus::Completed => FxgPlanEntryStatus::Completed,
        _ => FxgPlanEntryStatus::Unknown(acp_wire_value(status)),
    }
}

fn permission_kind(kind: AcpPermissionOptionKind) -> FxgPermissionOptionKind {
    match kind {
        AcpPermissionOptionKind::AllowOnce => FxgPermissionOptionKind::AllowOnce,
        AcpPermissionOptionKind::AllowAlways => FxgPermissionOptionKind::AllowAlways,
        AcpPermissionOptionKind::RejectOnce => FxgPermissionOptionKind::RejectOnce,
        AcpPermissionOptionKind::RejectAlways => FxgPermissionOptionKind::RejectAlways,
        _ => FxgPermissionOptionKind::Unknown(acp_wire_value(&kind)),
    }
}

// ----------------------------------------------------------------------
// Handler helpers (fs / terminal)
// ----------------------------------------------------------------------

/// Resolves a path relative to the session working directory.
fn resolve_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Lightly validates the `accept` content against `requested_schema`.
///
/// Only the restricted schema types allowed by ACP elicitation (object / string
/// (enum, oneOf) / integer / number / boolean / string array) are validated (the
/// client SHOULD do this per the spec; the agent re-validates too). Unknown types
/// (future extensions) pass through.
fn validate_elicitation_content(
    schema: &ElicitationSchema,
    content: &ElicitationContent,
) -> Result<(), String> {
    // Do not validate shape for schemas without properties.
    if schema.properties.is_empty() {
        return Ok(());
    }
    if let Some(required) = &schema.required {
        for key in required {
            if !content.contains_key(key) {
                return Err(format!("missing required property: {key}"));
            }
        }
    }
    for (key, value) in content {
        let Some(property) = schema.properties.get(key) else {
            return Err(format!("unexpected property: {key}"));
        };
        validate_elicitation_property(key, property, value)?;
    }
    Ok(())
}

/// Type validation for a single property in the elicitation restricted schema.
fn validate_elicitation_property(
    key: &str,
    property: &fxg_protocol::common::ElicitationProperty,
    value: &ElicitationValue,
) -> Result<(), String> {
    match (&property.kind, value) {
        (ElicitationPropertyKind::String, ElicitationValue::String(text)) => {
            if !property.r#enum.is_empty() && !property.r#enum.iter().any(|value| value == text) {
                return Err(format!("property {key} is not one of the allowed values"));
            }
            if !property.one_of.is_empty()
                && !property
                    .one_of
                    .iter()
                    .any(|option| option.const_value == *text)
            {
                return Err(format!("property {key} is not one of the allowed options"));
            }
        }
        (ElicitationPropertyKind::Integer, ElicitationValue::Integer(_)) => {}
        (
            ElicitationPropertyKind::Number,
            ElicitationValue::Integer(_) | ElicitationValue::Number(_),
        ) => {}
        (ElicitationPropertyKind::Boolean, ElicitationValue::Boolean(_)) => {}
        (ElicitationPropertyKind::Array, ElicitationValue::StringArray(items)) => {
            if let Some(items_schema) = &property.items
                && !items_schema.r#enum.is_empty()
                && !items
                    .iter()
                    .all(|item| items_schema.r#enum.iter().any(|value| value == item))
            {
                return Err(format!("property {key} has a disallowed array value"));
            }
        }
        // Unknown restricted schema types (future extensions) are not validated
        (ElicitationPropertyKind::Unknown(_), _) => {}
        (kind, _) => {
            let expected = match kind {
                ElicitationPropertyKind::Integer => "an integer".to_owned(),
                other => format!("a {}", other.as_str()),
            };
            return Err(format!("property {key} must be {expected}"));
        }
    }
    Ok(())
}

/// Converts a typed answer value into the ACP wire value.
fn elicitation_content_value(value: ElicitationValue) -> anyhow::Result<ElicitationContentValue> {
    Ok(match value {
        ElicitationValue::String(value) => ElicitationContentValue::String(value),
        ElicitationValue::Integer(value) => ElicitationContentValue::Integer(value),
        ElicitationValue::Number(value) => ElicitationContentValue::Number(value),
        ElicitationValue::Boolean(value) => ElicitationContentValue::Boolean(value),
        ElicitationValue::StringArray(value) => ElicitationContentValue::StringArray(value),
        ElicitationValue::Unknown(_) => {
            anyhow::bail!("unsupported elicitation content value")
        }
    })
}

// ----------------------------------------------------------------------
// terminal/* (fxg-pty)
// ----------------------------------------------------------------------

/// Execution state of an ACP terminal.
struct TerminalState {
    pty_id: String,
    command: String,
    output: Arc<Mutex<Vec<u8>>>,
    exit_rx: watch::Receiver<Option<i32>>,
}

/// ACP terminal registry.
#[derive(Default)]
struct TerminalRegistry {
    terminals: Mutex<HashMap<String, TerminalState>>,
}

/// Return value of `TerminalRegistry::get` (pty_id, command, output, exit_rx).
type TerminalSnapshot = (
    String,
    String,
    Arc<Mutex<Vec<u8>>>,
    watch::Receiver<Option<i32>>,
);

impl TerminalRegistry {
    fn get(&self, terminal_id: &str) -> Result<TerminalSnapshot, String> {
        let terminals = self.terminals.lock().expect("terminal registry poisoned");
        let state = terminals
            .get(terminal_id)
            .ok_or_else(|| format!("unknown terminal: {terminal_id}"))?;
        Ok((
            state.pty_id.clone(),
            state.command.clone(),
            Arc::clone(&state.output),
            state.exit_rx.clone(),
        ))
    }

    fn remove(&self, terminal_id: &str) -> Option<TerminalState> {
        self.terminals
            .lock()
            .expect("terminal registry poisoned")
            .remove(terminal_id)
    }
}

/// `terminal/create`: spawns an ACP terminal via fxg-pty.
async fn create_terminal(
    pty: Arc<fxg_pty::PtySessionManager>,
    terminals: Arc<TerminalRegistry>,
    events: mpsc::UnboundedSender<DriverEvent>,
    request: CreateTerminalRequest,
    responder: agent_client_protocol::v1::Responder<CreateTerminalResponse>,
) -> Result<(), agent_client_protocol::Error> {
    let terminal_id = uuid_v7();
    let mut spawn = fxg_pty::PtySpawnRequest::new(terminal_id.clone());
    spawn.shell_cmd = Some(request.command.clone());
    spawn.args = request.args.clone();
    spawn.cwd = request.cwd.clone();
    spawn.env = request
        .env
        .iter()
        .map(|variable| (variable.name.clone(), variable.value.clone()))
        .collect();
    let command_line = format!("{} {}", request.command, request.args.join(" "));

    match pty.spawn(spawn) {
        Ok(_) => {
            let Ok(mut receiver) = pty.subscribe(&terminal_id) else {
                return responder.respond_with_internal_error("failed to subscribe terminal");
            };
            let output: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
            let (exit_tx, exit_rx) = watch::channel(None);
            let limit = request.output_byte_limit.map(|limit| limit as usize);
            // PTY output collection task (delta delivery + completion event on exit)
            let output_for_task = Arc::clone(&output);
            let events_for_task = events.clone();
            let command_for_task = command_line.clone();
            let terminal_id_for_task = terminal_id.clone();
            fxg_protocol::spawn_guarded!("terminal output collector", async move {
                loop {
                    match receiver.recv().await {
                        Ok(fxg_pty::PtyEvent::Output { data, .. }) => {
                            {
                                let mut buffer =
                                    output_for_task.lock().expect("terminal output poisoned");
                                buffer.extend_from_slice(&data);
                                if let Some(limit) = limit
                                    && buffer.len() > limit
                                {
                                    let excess = buffer.len() - limit;
                                    buffer.drain(..excess);
                                }
                            }
                            fxg_protocol::discard_with!(
                                events_for_task.send(DriverEvent::Delta(
                                    StreamDeltaPayload::TerminalOutputDelta {
                                        terminal_id: terminal_id_for_task.clone(),
                                        data_b64: base64_encode(&data),
                                    },
                                )),
                                "notify event subscribers"
                            );
                        }
                        Ok(fxg_pty::PtyEvent::Exit { exit_code, .. }) => {
                            let data = std::mem::take(
                                &mut *output_for_task.lock().expect("terminal output poisoned"),
                            );
                            fxg_protocol::discard_with!(
                                events_for_task.send(DriverEvent::Event(
                                    UnifiedEventPayload::TerminalOutput {
                                        terminal_id: terminal_id_for_task.clone(),
                                        command: command_for_task.clone(),
                                        data_b64: base64_encode(&data),
                                        exit_code,
                                    },
                                )),
                                "notify event subscribers"
                            );
                            fxg_protocol::discard_with!(
                                exit_tx.send(exit_code),
                                "discard a non-critical result"
                            );
                            break;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            terminals
                .terminals
                .lock()
                .expect("terminal registry poisoned")
                .insert(
                    terminal_id.clone(),
                    TerminalState {
                        pty_id: terminal_id.clone(),
                        command: command_line,
                        output,
                        exit_rx,
                    },
                );
            responder.respond(CreateTerminalResponse::new(TerminalId::new(terminal_id)))
        }
        Err(err) => {
            responder.respond_with_internal_error(format!("failed to spawn terminal: {err}"))
        }
    }
}

/// Builds an ACP `TerminalExitStatus` from an exit code.
fn exit_status_of(code: Option<i32>) -> TerminalExitStatus {
    let mut status = TerminalExitStatus::new();
    if let Some(code) = code
        && let Ok(code) = u32::try_from(code)
    {
        status = status.exit_code(code);
    }
    status
}

/// Base64 encoding (no line breaks).
fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::v1::schema::{
        AuthMethodAgent, AuthMethodTerminal, Plan, PlanEntry as AcpPlanEntry, PlanEntryPriority,
    };

    fn text_chunk(text: &str, message_id: Option<&str>) -> SessionUpdate {
        // `ContentChunk` is non_exhaustive, so construct it via JSON
        let mut value = serde_json::json!({
            "content": { "type": "text", "text": text },
        });
        if let Some(id) = message_id {
            value["messageId"] = serde_json::Value::String(id.to_owned());
        }
        SessionUpdate::AgentMessageChunk(
            serde_json::from_value(value).expect("content chunk fixture"),
        )
    }

    fn thought_chunk(text: &str, message_id: Option<&str>) -> SessionUpdate {
        let mut value = serde_json::json!({
            "content": { "type": "text", "text": text },
        });
        if let Some(id) = message_id {
            value["messageId"] = serde_json::Value::String(id.to_owned());
        }
        SessionUpdate::AgentThoughtChunk(
            serde_json::from_value(value).expect("content chunk fixture"),
        )
    }

    /// Builds `AgentCapabilities` from a combination of capabilities.
    fn agent_capabilities(load_session: bool, resume: bool) -> AgentCapabilities {
        // `AgentCapabilities` is non_exhaustive, so build it from default
        let mut capabilities = AgentCapabilities::default();
        capabilities.load_session = load_session;
        if resume {
            capabilities.session_capabilities.resume =
                Some(agent_client_protocol::v1::schema::SessionResumeCapabilities::default());
        }
        capabilities
    }

    fn resume_request(agent_session_id: Option<&str>, allow_fresh: bool) -> ResumeRequest {
        ResumeRequest {
            agent_session_id: agent_session_id.map(str::to_owned),
            allow_fresh,
        }
    }

    fn elicitation_schema() -> ElicitationSchema {
        serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "strategy": { "type": "string", "enum": ["conservative", "balanced"] },
                "count": { "type": "integer" },
                "confirm": { "type": "boolean" },
                "tags": { "type": "array" }
            },
            "required": ["strategy"]
        }))
        .expect("schema")
    }

    fn elicitation_content(value: serde_json::Value) -> ElicitationContent {
        serde_json::from_value(value).expect("content")
    }

    #[test]
    fn elicitation_content_validation_accepts_valid_values() {
        let schema = elicitation_schema();
        assert!(
            validate_elicitation_content(
                &schema,
                &elicitation_content(serde_json::json!({
                    "strategy": "balanced",
                    "count": 3,
                    "confirm": true,
                    "tags": ["a", "b"]
                }))
            )
            .is_ok()
        );
        // Do not validate schemas without properties
        assert!(
            validate_elicitation_content(
                &serde_json::from_value(serde_json::json!({ "type": "object" }))
                    .expect("empty schema"),
                &elicitation_content(serde_json::json!({ "anything": 1 }))
            )
            .is_ok()
        );
    }

    #[test]
    fn elicitation_content_validation_rejects_invalid_values() {
        let schema = elicitation_schema();
        // Missing required property
        assert!(
            validate_elicitation_content(&schema, &elicitation_content(serde_json::json!({})))
                .expect_err("required")
                .contains("missing required property")
        );
        // Not in enum
        assert!(
            validate_elicitation_content(
                &schema,
                &elicitation_content(serde_json::json!({ "strategy": "fast" }))
            )
            .expect_err("enum")
            .contains("allowed values")
        );
        // Type mismatch
        assert!(
            validate_elicitation_content(
                &schema,
                &elicitation_content(serde_json::json!({
                    "strategy": "balanced",
                    "count": "3"
                }))
            )
            .expect_err("integer")
            .contains("must be an integer")
        );
        // Unknown property
        assert!(
            validate_elicitation_content(
                &schema,
                &elicitation_content(serde_json::json!({
                    "strategy": "balanced",
                    "extra": 1
                }))
            )
            .expect_err("unknown property")
            .contains("unexpected property")
        );
    }

    #[test]
    fn elicitation_content_validation_checks_titled_enum() {
        let schema = serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "country": {
                    "type": "string",
                    "oneOf": [{ "const": "jp", "title": "Japan" }]
                }
            }
        }))
        .expect("schema");
        assert!(
            validate_elicitation_content(
                &schema,
                &elicitation_content(serde_json::json!({ "country": "jp" }))
            )
            .is_ok()
        );
        assert!(
            validate_elicitation_content(
                &schema,
                &elicitation_content(serde_json::json!({ "country": "us" }))
            )
            .expect_err("oneOf")
            .contains("allowed options")
        );
    }

    #[test]
    fn plan_resume_prefers_session_resume_over_load() {
        // When both are supported, prefer `session/resume` (no history replay)
        let capabilities = agent_capabilities(true, true);
        let resume = resume_request(Some("ses_1"), true);
        match plan_resume(&capabilities, Some(&resume)).expect("plan") {
            ResumePlan::Resume(id) => assert_eq!(id, "ses_1"),
            ResumePlan::Load(_) | ResumePlan::Fresh => {
                panic!("session/resume must be preferred")
            }
        }
    }

    #[test]
    fn plan_resume_falls_back_to_load_then_fresh() {
        // Only load supported → session/load (replay discarded by the caller)
        let capabilities = agent_capabilities(true, false);
        let resume = resume_request(Some("ses_1"), true);
        match plan_resume(&capabilities, Some(&resume)).expect("plan") {
            ResumePlan::Load(id) => assert_eq!(id, "ses_1"),
            ResumePlan::Resume(_) | ResumePlan::Fresh => panic!("session/load expected"),
        }

        // Neither supported + allow_fresh → fresh session (replay fallback)
        let capabilities = agent_capabilities(false, false);
        assert!(matches!(
            plan_resume(&capabilities, Some(&resume)).expect("plan"),
            ResumePlan::Fresh
        ));

        // Neither supported + no allow_fresh → native restore unavailable
        let resume = resume_request(Some("ses_1"), false);
        let err = plan_resume(&capabilities, Some(&resume)).expect_err("native-only must fail");
        assert!(err.downcast_ref::<NativeResumeUnavailable>().is_some());
    }

    #[test]
    fn plan_resume_without_ids_is_fresh() {
        let capabilities = agent_capabilities(true, true);
        // No resume selection
        assert!(matches!(
            plan_resume(&capabilities, None).expect("plan"),
            ResumePlan::Fresh
        ));
        // No agent-side ID + allow_fresh (fresh session assuming history replay)
        let resume = resume_request(None, true);
        assert!(matches!(
            plan_resume(&capabilities, Some(&resume)).expect("plan"),
            ResumePlan::Fresh
        ));
        // No agent-side ID + no allow_fresh (automatic resume) means native restore is
        // unavailable (requires explicit resume via history replay)
        let resume = resume_request(None, false);
        let err = plan_resume(&capabilities, Some(&resume)).expect_err("native-only must fail");
        assert!(err.downcast_ref::<NativeResumeUnavailable>().is_some());
    }

    #[test]
    fn accumulator_emits_completed_message_on_id_switch() {
        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState::default();
        let events = map_session_update(text_chunk("こん", Some("m1")), &mut acc, &mut caps); // allow-japanese: streaming chunk split across multibyte text
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], DriverEvent::Delta(_)));

        // Same ID: delta only
        let events = map_session_update(text_chunk("にちは", Some("m1")), &mut acc, &mut caps); // allow-japanese: streaming chunk split across multibyte text
        assert_eq!(events.len(), 1);

        // Different ID: completion event for the previous message + new delta
        let events = map_session_update(text_chunk("次", Some("m2")), &mut acc, &mut caps); // allow-japanese: multibyte streaming chunk fixture
        assert_eq!(events.len(), 2);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                message_id,
                text,
                is_complete,
            }) => {
                assert_eq!(message_id, "m1");
                assert_eq!(text, "こんにちは"); // allow-japanese: expected accumulation of multibyte chunks
                assert!(is_complete);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        // Flush the rest at turn end
        let finished = acc.finish_all();
        assert_eq!(finished.len(), 1);
        match &finished[0] {
            UnifiedEventPayload::AgentMessage {
                message_id, text, ..
            } => {
                assert_eq!(message_id, "m2");
                assert_eq!(text, "次"); // allow-japanese: expected multibyte chunk text
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn message_chunks_without_id_accumulate_into_single_message() {
        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState::default();
        map_session_update(text_chunk("a", None), &mut acc, &mut caps);
        map_session_update(text_chunk("b", None), &mut acc, &mut caps);
        let finished = acc.finish_all();
        assert_eq!(finished.len(), 1);
        match &finished[0] {
            UnifiedEventPayload::AgentMessage {
                message_id, text, ..
            } => {
                assert_eq!(message_id, "acp-message");
                assert_eq!(text, "ab");
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn thought_is_completed_before_the_answer_that_follows_it() {
        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState::default();

        let events = map_session_update(thought_chunk("thinking", Some("m1")), &mut acc, &mut caps);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], DriverEvent::Delta(_)));

        // The first message chunk must finalize the reasoning stream so its
        // completion event is recorded before the answer.
        let events = map_session_update(text_chunk("answer", Some("m1")), &mut acc, &mut caps);
        assert_eq!(events.len(), 2);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::AgentThought {
                thought_id,
                text,
                is_complete,
            }) => {
                assert_eq!(thought_id, "m1");
                assert_eq!(text, "thinking");
                assert!(is_complete);
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(matches!(
            events[1],
            DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta { .. })
        ));

        let finished = acc.finish_all();
        assert_eq!(finished.len(), 1);
        assert!(matches!(
            finished[0],
            UnifiedEventPayload::AgentMessage { .. }
        ));
    }

    #[test]
    fn finish_all_emits_thought_before_message() {
        // Defensive ordering for a state that the chunk APIs no longer produce
        // (both streams still open at turn end).
        let mut acc = StreamingAccumulator {
            message: Some(("m1".to_owned(), "answer".to_owned())),
            thought: Some(("t1".to_owned(), "thinking".to_owned())),
        };
        let finished = acc.finish_all();
        assert_eq!(finished.len(), 2);
        assert!(matches!(
            finished[0],
            UnifiedEventPayload::AgentThought { .. }
        ));
        assert!(matches!(
            finished[1],
            UnifiedEventPayload::AgentMessage { .. }
        ));
    }

    #[test]
    fn thought_after_message_finalizes_the_message_first() {
        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState::default();

        map_session_update(text_chunk("answer", Some("m1")), &mut acc, &mut caps);
        let events = map_session_update(thought_chunk("thinking", Some("m2")), &mut acc, &mut caps);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            DriverEvent::Event(UnifiedEventPayload::AgentMessage { .. })
        ));
        assert!(matches!(
            events[1],
            DriverEvent::Delta(StreamDeltaPayload::AgentThoughtDelta { .. })
        ));
    }

    #[test]
    fn maps_tool_call_with_diff_content() {
        let tool_call = ToolCall::new("t-1", "Edit file")
            .kind(ToolKind::Edit)
            .status(ToolCallStatus::Completed)
            .content(vec![ToolCallContent::Diff(
                agent_client_protocol::v1::schema::Diff::new("src/lib.rs", "old\nnew\n"),
            )])
            .locations(vec![
                agent_client_protocol::v1::schema::ToolCallLocation::new("src/lib.rs"),
            ]);
        let payload = tool_call_event(&tool_call);
        match payload {
            UnifiedEventPayload::ToolCall {
                tool_call_id,
                kind,
                status,
                diff,
                locations,
                ..
            } => {
                assert_eq!(tool_call_id, "t-1");
                assert_eq!(kind, FxgToolCallKind::Edit);
                assert_eq!(status, FxgToolCallStatus::Completed);
                assert_eq!(locations, vec!["src/lib.rs"]);
                let diff = diff.expect("diff");
                assert_eq!(diff.path, "src/lib.rs");
                assert!(diff.additions >= 1);
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn maps_plan_update() {
        let plan = Plan::new(vec![AcpPlanEntry::new(
            "Run tests",
            PlanEntryPriority::High,
            agent_client_protocol::v1::schema::PlanEntryStatus::InProgress,
        )]);
        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState::default();
        let events = map_session_update(SessionUpdate::Plan(plan), &mut acc, &mut caps);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::PlanUpdate { entries }) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].title, "Run tests");
                assert_eq!(entries[0].status, FxgPlanEntryStatus::InProgress);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn builds_unified_diff_with_counts() {
        let diff = build_file_diff("a.txt", "one\ntwo\n", "one\ntwo\nthree\n");
        assert_eq!(diff.additions, 1);
        assert_eq!(diff.deletions, 0);
        assert!(diff.unified_diff.contains("+three"));
        assert_eq!(diff.old_text.as_deref(), Some("one\ntwo\n"));
        assert_eq!(diff.new_text.as_deref(), Some("one\ntwo\nthree\n"));
    }

    #[test]
    fn resolves_relative_paths_against_cwd() {
        let cwd = PathBuf::from("/tmp/session");
        assert_eq!(
            resolve_path(&cwd, Path::new("src/lib.rs")),
            PathBuf::from("/tmp/session/src/lib.rs")
        );
        assert_eq!(
            resolve_path(&cwd, Path::new("/etc/hosts")),
            PathBuf::from("/etc/hosts")
        );
    }

    #[test]
    fn base64_encodes_terminal_output() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn maps_session_info_update() {
        use agent_client_protocol::v1::schema::SessionInfoUpdate;

        let update = SessionInfoUpdate::new().title("Refactor UI layout".to_owned());
        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState::default();
        let events = map_session_update(
            SessionUpdate::SessionInfoUpdate(update),
            &mut acc,
            &mut caps,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::SessionTitleChanged { title }) => {
                assert_eq!(title, "Refactor UI layout");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn capabilities_partial_updates_preserve_existing_state() {
        use agent_client_protocol::v1::schema::{
            AvailableCommand, AvailableCommandsUpdate, CurrentModeUpdate,
        };

        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState {
            current_mode: Some("ask".to_owned()),
            available_modes: vec![ModeInfo {
                mode_id: "ask".to_owned(),
                name: "Ask".to_owned(),
                description: None,
            }],
            available_commands: Vec::new(),
            config_options: Vec::new(),
        };

        // Existing current_mode / available_modes are preserved when a command list update arrives
        let cmd_update =
            AvailableCommandsUpdate::new(vec![AvailableCommand::new("review", "Review code")]);
        let events = map_session_update(
            SessionUpdate::AvailableCommandsUpdate(cmd_update),
            &mut acc,
            &mut caps,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
                current_mode,
                available_modes,
                available_commands,
                ..
            }) => {
                assert_eq!(current_mode.as_deref(), Some("ask"));
                assert_eq!(available_modes.len(), 1);
                assert_eq!(available_commands.len(), 1);
                assert_eq!(available_commands[0].name, "review");
            }
            other => panic!("unexpected event: {other:?}"),
        }

        // Previously added available_commands are preserved when a mode change arrives
        let mode_update = CurrentModeUpdate::new("code");
        let events = map_session_update(
            SessionUpdate::CurrentModeUpdate(mode_update),
            &mut acc,
            &mut caps,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
                current_mode,
                available_modes,
                available_commands,
                ..
            }) => {
                assert_eq!(current_mode.as_deref(), Some("code"));
                assert_eq!(available_modes.len(), 1);
                assert_eq!(available_commands.len(), 1);
                assert_eq!(available_commands[0].name, "review");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn maps_grouped_select_options() {
        use agent_client_protocol::v1::schema::{
            SessionConfigOption, SessionConfigSelect, SessionConfigSelectGroup,
            SessionConfigSelectOption,
        };

        let option = SessionConfigOption::new(
            "model",
            "Model",
            SessionConfigKind::Select(SessionConfigSelect::new(
                "gpt-4",
                vec![
                    SessionConfigSelectGroup::new(
                        "openai",
                        "OpenAI",
                        vec![
                            SessionConfigSelectOption::new("gpt-4", "GPT-4"),
                            SessionConfigSelectOption::new("gpt-3.5", "GPT-3.5"),
                        ],
                    ),
                    SessionConfigSelectGroup::new(
                        "anthropic",
                        "Anthropic",
                        vec![SessionConfigSelectOption::new("claude-3", "Claude 3")],
                    ),
                ],
            )),
        );

        let mapped = map_config_option(&option);
        assert_eq!(mapped.key, "model");
        assert_eq!(mapped.name, "Model");
        assert_eq!(
            mapped.current_value,
            ConfigOptionValue::Select("gpt-4".to_owned())
        );
        assert_eq!(
            mapped.options,
            vec![
                ConfigOptionValue::Select("gpt-4".to_owned()),
                ConfigOptionValue::Select("gpt-3.5".to_owned()),
                ConfigOptionValue::Select("claude-3".to_owned()),
            ]
        );
    }

    #[test]
    fn parses_bare_config_value_ids() {
        let value = parse_config_value(ConfigOptionValue::Select(
            "opencode-go/deepseek-v4.1-flash".to_owned(),
        ))
        .expect("bare string config value");
        assert_eq!(
            value.as_value_id().map(ToString::to_string).as_deref(),
            Some("opencode-go/deepseek-v4.1-flash")
        );
    }

    #[test]
    fn parses_boolean_and_tagged_config_values() {
        let bool_value =
            parse_config_value(ConfigOptionValue::Boolean(true)).expect("bare boolean");
        assert_eq!(bool_value.as_bool(), Some(true));

        let tagged = parse_config_value(ConfigOptionValue::Unknown(
            serde_json::json!({
                "type": "boolean",
                "value": false
            })
            .into(),
        ))
        .expect("tagged boolean");
        assert_eq!(tagged.as_bool(), Some(false));

        let wire_id = parse_config_value(ConfigOptionValue::Unknown(
            serde_json::json!({ "value": "plan" }).into(),
        ))
        .expect("wire id");
        assert_eq!(
            wire_id.as_value_id().map(ToString::to_string).as_deref(),
            Some("plan")
        );
    }

    #[test]
    fn rejects_unknown_config_values() {
        assert!(
            parse_config_value(ConfigOptionValue::Unknown(
                serde_json::json!({ "unexpected": true }).into()
            ))
            .is_err()
        );
    }

    #[test]
    fn maps_usage_update_to_event() {
        use agent_client_protocol::v1::schema::{Cost, UsageUpdate};

        let update = SessionUpdate::UsageUpdate(
            UsageUpdate::new(53_000, 200_000).cost(Cost::new(0.045, "USD")),
        );
        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState::default();
        let events = map_session_update(update, &mut acc, &mut caps);
        assert_eq!(events.len(), 1);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::UsageUpdated {
                used_tokens,
                context_size,
                cost,
            }) => {
                assert_eq!(*used_tokens, 53_000);
                assert_eq!(*context_size, 200_000);
                let cost = cost.as_ref().expect("cost");
                assert_eq!(cost.currency, "USD");
                assert!((cost.amount - 0.045).abs() < f64::EPSILON);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn stop_reason_event_only_for_abnormal_reasons() {
        // Normal completion and user cancellation are not turned into events
        assert!(turn_ended_event(StopReason::EndTurn).is_none());
        assert!(turn_ended_event(StopReason::Cancelled).is_none());

        for (reason, expected) in [
            (StopReason::MaxTokens, TurnStopReason::MaxTokens),
            (StopReason::MaxTurnRequests, TurnStopReason::MaxTurnRequests),
            (StopReason::Refusal, TurnStopReason::Refusal),
        ] {
            match turn_ended_event(reason) {
                Some(UnifiedEventPayload::TurnEnded { reason }) => {
                    assert_eq!(reason, expected);
                }
                other => panic!("unexpected payload: {other:?}"),
            }
        }
    }

    #[test]
    fn maps_agent_and_terminal_auth_methods() {
        let agent = AuthMethod::Agent(
            AuthMethodAgent::new("agent-login", "Agent login").description("browser login"),
        );
        let mapped = map_auth_method(&agent);
        assert_eq!(mapped.id, "agent-login");
        assert_eq!(mapped.kind, AgentAuthMethodKind::Agent);
        assert_eq!(mapped.description.as_deref(), Some("browser login"));
        assert!(mapped.args.is_empty());

        let terminal = AuthMethod::Terminal(
            AuthMethodTerminal::new("terminal-login", "Terminal login")
                .args(vec!["--login".to_owned()]),
        );
        let mapped = map_auth_method(&terminal);
        assert_eq!(mapped.id, "terminal-login");
        assert_eq!(mapped.kind, AgentAuthMethodKind::Terminal);
        assert_eq!(mapped.args, vec!["--login"]);
    }

    #[test]
    fn client_capabilities_advertise_terminal_auth() {
        let capabilities = client_capabilities();
        assert!(capabilities.terminal);
        assert!(capabilities.auth.terminal);
    }

    #[test]
    fn prompt_blocks_sends_images_when_supported() {
        let input = PromptInput {
            text: "look".to_owned(),
            attachments: vec![fxg_protocol::common::AttachmentMeta {
                file_name: "shot.png".to_owned(),
                mime_type: "image/png".to_owned(),
                size_bytes: 3,
                local_path: None,
                data_b64: Some("YWJj".to_owned()),
            }],
        };
        let capabilities = PromptCapabilities::new().image(true).embedded_context(true);
        let blocks = prompt_blocks(input, &capabilities).expect("blocks");
        assert!(matches!(blocks[0], ContentBlock::Text(_)));
        assert!(matches!(blocks[1], ContentBlock::Image(_)));
    }

    #[test]
    fn prompt_blocks_falls_back_to_text_without_image_support() {
        let input = PromptInput {
            text: String::new(),
            attachments: vec![fxg_protocol::common::AttachmentMeta {
                file_name: "shot.png".to_owned(),
                mime_type: "image/png".to_owned(),
                size_bytes: 3,
                local_path: None,
                data_b64: Some("YWJj".to_owned()),
            }],
        };
        let capabilities = PromptCapabilities::default();
        let blocks = prompt_blocks(input, &capabilities).expect("blocks");
        assert!(matches!(blocks[0], ContentBlock::Text(_)));
    }

    #[cfg(unix)]
    #[test]
    fn prompt_blocks_links_non_image_files() {
        let input = PromptInput {
            text: "see this".to_owned(),
            attachments: vec![fxg_protocol::common::AttachmentMeta {
                file_name: "notes.md".to_owned(),
                mime_type: "text/markdown".to_owned(),
                size_bytes: 10,
                local_path: Some("/tmp/notes.md".to_owned()),
                data_b64: None,
            }],
        };
        let blocks = prompt_blocks(input, &PromptCapabilities::default()).expect("blocks");
        match &blocks[1] {
            ContentBlock::ResourceLink(link) => {
                assert_eq!(link.name, "notes.md");
                assert_eq!(link.uri, "file:///tmp/notes.md");
            }
            other => panic!("unexpected block: {other:?}"),
        }
    }

    #[test]
    fn tool_call_contents_extracts_text_output() {
        let contents = vec![ToolCallContent::Content(
            agent_client_protocol::v1::schema::Content::new(ContentBlock::Text(TextContent::new(
                "test output",
            ))),
        )];
        let (diff, locations, texts) = tool_call_contents(&contents);
        assert!(diff.is_none());
        assert!(locations.is_empty());
        assert_eq!(texts, vec!["test output"]);
    }

    #[test]
    fn stderr_registry_is_bounded_and_per_agent() {
        let registry = StderrRegistry::default();
        for index in 0..(StderrRegistry::MAX_LINES + 5) {
            registry.push("agy", &format!("line {index}"));
        }
        registry.push("opencode", "other");
        let agy = registry.tail("agy");
        assert_eq!(agy.len(), StderrRegistry::MAX_LINES);
        assert_eq!(agy.last().map(String::as_str), Some("line 204"));
        assert_eq!(registry.tail("opencode"), vec!["other"]);
        assert_eq!(registry.snapshot().len(), 2);
    }
}
