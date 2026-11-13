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
    SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigOptionValue, SessionId, SessionModeState, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason, TerminalExitStatus,
    TerminalId, TerminalOutputRequest, TerminalOutputResponse, TextContent, ToolCall,
    ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolKind, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::v1::{ActiveSession, ConnectionTo, SessionMessage, SessionSetup};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, Client};
use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use fxg_protocol::common::{
    AgentAuthMethodInfo, AgentAuthMethodKind, CommandInfo, ConfigOptionCategory, ConfigOptionInfo,
    ConfigOptionValue, ElicitationContent, ElicitationMode as FxgElicitationMode,
    ElicitationPropertyKind, ElicitationSchema, ElicitationValue, FileDiff, ForkScope, ModeInfo,
    PermissionOption, PermissionOptionKind as FxgPermissionOptionKind, PlanEntry,
    PlanEntryStatus as FxgPlanEntryStatus, PromptInput, StreamDeltaPayload,
    ToolCallKind as FxgToolCallKind, ToolCallStatus as FxgToolCallStatus, TurnStopReason,
    UsageCost,
};
use fxg_protocol::events::UnifiedEventPayload;
use fxg_protocol::util::uuid_v7;
use tokio::sync::{mpsc, oneshot, watch};

use crate::driver::{
    ActiveSessionHandle, AgentAuthRequired, AgentDriver, DriverEvent, HistoryCaptureUnavailable,
    NativeForkUnavailable, NativeResumeUnavailable, ResumeRequest, StartSessionRequest,
    StartedSession,
};
use crate::warm::{DEFAULT_WARM_IDLE, LaunchKey, WarmPool, WarmProcess};

mod client;
mod list;
mod mapping;
mod process;
mod session;
mod subagent;
mod terminal;

pub(crate) use mapping::map_auth_method;
pub(crate) use session::client_capabilities;

use self::client::AcpSessionHandle;
use self::process::{ProcessHandle, ProcessLease, spawn_agent_process};

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
    /// it (`0` keeps processes until node shutdown).
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
        params: Box<SessionParams>,
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
    /// Parser for vendor subagent notifications (set by the extension layer).
    acp_parser: Option<Arc<dyn crate::agent::extension::AcpNotificationParser>>,
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
    /// Subagent that owns this context (`None` = the root session)
    subagent_id: Option<String>,
    /// Ingress channel to flush buffered streams on the turn loop before blocking requests
    ingress: Option<mpsc::UnboundedSender<subagent::SessionIngress>>,
}

/// ACP session ID → session information.
type SessionContexts = Arc<Mutex<HashMap<String, SessionContext>>>;

/// Looks up session information by ACP session ID.
fn session_context(contexts: &SessionContexts, session_id: &SessionId) -> Option<SessionContext> {
    let contexts = contexts.lock().expect("session contexts poisoned");
    if let Some(context) = contexts.get(&session_id.to_string()) {
        return Some(context.clone());
    }
    // Some agents send requests with a child session ID. A process hosts at most one parent session, so fall back to it when
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

    async fn list_sessions(
        &self,
        launch: &crate::driver::AgentLaunchSpec,
        cwd: &Path,
    ) -> anyhow::Result<crate::driver::AgentSessionList> {
        list::list_sessions(launch, cwd).await
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
        let program_path =
            req.launch.program.as_deref().ok_or_else(|| {
                anyhow!("agent {} has no executable configured", req.launch.agent_id)
            })?;
        let program =
            fxg_pty::resolve_command(&program_path.to_string_lossy(), &req.cwd).map_err(|err| {
                anyhow!(
                    "failed to resolve agent program {}: {err}",
                    program_path.display()
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
            acp_parser: req.acp_parser.clone(),
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
                .new_session(Box::new(session_params.clone()), event_tx.clone(), lease)
                .await
            {
                Ok((handle, context_restored)) => {
                    tracing::debug!(
                        session_id = %req.session_id,
                        agent = %key.agent_id(),
                        "reusing idle acp agent process"
                    );
                    let agent_session_id = Some(handle.session_id().to_string());
                    return Ok(StartedSession {
                        handle: Box::new(handle),
                        context_restored,
                        agent_session_id,
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
            req.launch.acp.clone(),
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
            .new_session(Box::new(session_params), event_tx.clone(), lease)
            .await?;
        let agent_session_id = Some(handle.session_id().to_string());
        Ok(StartedSession {
            handle: Box::new(handle),
            context_restored,
            agent_session_id,
        })
    }
}

#[cfg(test)]
mod tests;
