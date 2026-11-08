//! Agent driver abstraction (`AgentDriver` / `ActiveSessionHandle`).
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.
//!
//! The `fxg-node` session manager depends only on this trait and hides the
//! differences between ACP (`AcpDriver`) and OpenCode2 (`OpenCode2Driver`).
//! Drivers emit normalized events ([`UnifiedEventPayload`] /
//! [`StreamDeltaPayload`]) as [`DriverEvent`]; persistence and delivery are
//! handled centrally by the session manager (event bus).

use std::path::PathBuf;

use async_trait::async_trait;
use fxg_protocol::common::{ElicitationAction, StreamDeltaPayload};
use fxg_protocol::events::UnifiedEventPayload;
use tokio::sync::mpsc;

/// How to launch the agent process (resolved from the ACP Registry / custom definitions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLaunchSpec {
    /// Resolved agent ID (registry id / custom name / `opencode2`)
    pub agent_id: String,
    /// Display name
    pub display_name: String,
    /// Driver kind (a value of [`AgentDriver::driver_kind`]: `"acp"` / `"opencode2"`)
    pub driver_kind: String,
    /// Executable (absolute path, or a command name on `PATH`).
    /// `PATH` resolution (`which` / `PATHEXT`) is done by the driver right before launch.
    pub program: PathBuf,
    /// Command arguments (base; pass-through arguments go in [`StartSessionRequest::extra_args`])
    pub args: Vec<String>,
    /// Additional environment variables injected into the process (overrides inherited ones)
    pub env: Vec<(String, String)>,
}

/// Resume selection from an existing agent session ([`StartSessionRequest::resume`]).
///
/// Design: `docs/04-agent-drivers-and-windows.md` §4.3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeRequest {
    /// Recorded internal agent session ID (the `SessionAgentBound` value).
    /// `None` means create a fresh session (the caller is expected to inject history replay).
    pub agent_session_id: Option<String>,
    /// Whether creating a fresh session is allowed when native restore is unavailable
    /// (`false` returns [`NativeResumeUnavailable`]).
    pub allow_fresh: bool,
}

/// Resume was aborted without creating a fresh session because native restore
/// (`session/resume` / `session/load` / opencode2 existing-session bind) is unavailable.
///
/// Returned only when [`ResumeRequest::allow_fresh`] is `false`. The caller (the
/// node) downcasts to this type to detect it and directs the user to an explicit
/// resume via history replay (`ErrorCode::RESUME_REQUIRED`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("native session resume is not available: {0}")]
pub struct NativeResumeUnavailable(pub String);

/// Session start request.
#[derive(Debug, Clone)]
pub struct StartSessionRequest {
    /// fxg-side session ID (UUID v7)
    pub session_id: String,
    /// Session display name (identical to `fxg`'s `SessionCreated.title`).
    /// Only drivers that can use it as the agent-side session name apply it.
    pub title: Option<String>,
    /// Working directory (the session's `local_path` / worktree path)
    pub cwd: PathBuf,
    /// Launch spec (resolved)
    pub launch: AgentLaunchSpec,
    /// Pass-through arguments for the agent process (`fxg run <agent> -- ...`)
    pub extra_args: Vec<String>,
    /// Initial mode (when `--mode` is given; applied via ACP `session/set_mode`)
    pub initial_mode: Option<String>,
    /// Resume selection from an existing agent session (`None` means a fresh session)
    pub resume: Option<ResumeRequest>,
}

/// Result of [`AgentDriver::start_session`].
pub struct StartedSession {
    /// Handle for operating a started session
    pub handle: Box<dyn ActiveSessionHandle>,
    /// `true` = the agent-side context was restored natively
    /// (`session/resume` / `session/load` / opencode2 existing-session bind).
    /// `false` = a fresh agent session was created
    /// (the caller injects history replay to carry the context over).
    pub context_restored: bool,
}

/// Events sent from a driver to the session manager.
///
/// This uses [`mpsc::UnboundedSender`] instead of the `mpsc::Sender<DriverEvent>`
/// from the design (docs/04 §1); drivers run on the connection event loop, so
/// sends must never block.
#[derive(Debug, Clone)]
pub enum DriverEvent {
    /// Complete event to persist (`node_seq` assignment and delivery are done by the manager)
    Event(UnifiedEventPayload),
    /// In-progress streaming delta (in-memory delivery only; not persisted)
    Delta(StreamDeltaPayload),
    /// Driver-side failure (moves the session to the `error` state)
    Failed {
        /// Error message
        message: String,
    },
}

/// Agent driver (ACP / OpenCode2, etc.).
#[async_trait]
pub trait AgentDriver: Send + Sync {
    /// Driver identifier (`"acp"` / `"opencode2"`).
    fn driver_kind(&self) -> &'static str;

    /// Starts a new session and returns an operation handle.
    ///
    /// When `req.resume` is set, the driver attempts native restore
    /// (`session/resume` / `session/load` / opencode2 existing-session bind) and
    /// reports success via [`StartedSession::context_restored`].
    /// Injecting history replay when native restore is unavailable is the caller's responsibility.
    ///
    /// Subsequent events are sent to `event_tx`.
    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession>;

    /// Disposes all idle agent processes (those not attached to a session).
    ///
    /// Called by `fxg kill-all` / daemon shutdown. Drivers that keep processes alive
    /// after session end must terminate them here. The return value is the number disposed.
    /// The default implementation for drivers that hold no processes returns 0 (no-op).
    async fn shutdown_idle(&self) -> usize {
        0
    }
}

/// Operation handle for a started session.
#[async_trait]
pub trait ActiveSessionHandle: Send + Sync {
    /// Send a prompt (including slash commands).
    async fn send_prompt(&self, text: String) -> anyhow::Result<()>;
    /// Respond to a permission request (`ALREADY_RESOLVED` detection is handled by the manager).
    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()>;
    /// Respond to an elicitation (structured input request) (`accept` / `decline` / `cancel`).
    ///
    /// The `content` for `accept` must conform to the request's `requested_schema`
    /// (form mode). The default implementation for unsupported drivers returns an error.
    async fn respond_elicitation(
        &self,
        _elicitation_id: String,
        _action: ElicitationAction,
        _content: serde_json::Value,
    ) -> anyhow::Result<()> {
        anyhow::bail!("respond_elicitation is not supported by this driver")
    }
    /// Change mode (`plan` / `code`, etc.).
    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()>;
    /// Change configuration (model selection, etc.).
    async fn set_config(&self, key: String, value: serde_json::Value) -> anyhow::Result<()>;
    /// Cancel the current turn.
    async fn cancel_turn(&self) -> anyhow::Result<()>;
    /// Fully terminates the session process (including the process tree).
    ///
    /// Drivers that keep processes alive after session end (`AcpDriver`) **discard
    /// them here instead of keeping them**. Used by `fxg kill-all` / daemon shutdown.
    async fn dispose(&self) -> anyhow::Result<()> {
        self.shutdown().await
    }

    /// End the session.
    ///
    /// Drivers that keep processes alive after session end close the session and
    /// return the process to the reuse pool (full termination is [`Self::dispose`]).
    async fn shutdown(&self) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// Verifies that the trait is object safe and that a mock implementation can
    /// return `Box<dyn ...>`.
    struct MockDriver;

    struct MockHandle;

    #[async_trait]
    impl ActiveSessionHandle for MockHandle {
        async fn send_prompt(&self, _text: String) -> anyhow::Result<()> {
            Ok(())
        }
        async fn respond_permission(
            &self,
            _request_id: String,
            _selected_option_id: String,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn set_mode(&self, _mode_id: String) -> anyhow::Result<()> {
            Ok(())
        }
        async fn set_config(&self, _key: String, _value: serde_json::Value) -> anyhow::Result<()> {
            Ok(())
        }
        async fn cancel_turn(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn shutdown(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl AgentDriver for MockDriver {
        fn driver_kind(&self) -> &'static str {
            "mock"
        }

        async fn start_session(
            &self,
            req: StartSessionRequest,
            event_tx: mpsc::UnboundedSender<DriverEvent>,
        ) -> anyhow::Result<StartedSession> {
            event_tx
                .send(DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                    message_id: "m1".to_owned(),
                    text: "hello".to_owned(),
                    is_complete: true,
                }))
                .expect("event_tx");
            assert_eq!(req.launch.driver_kind, "mock");
            Ok(StartedSession {
                handle: Box::new(MockHandle),
                context_restored: false,
            })
        }
    }

    #[tokio::test]
    async fn driver_trait_is_object_safe_and_emits_events() {
        let driver: Box<dyn AgentDriver> = Box::new(MockDriver);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let launch = AgentLaunchSpec {
            agent_id: "mock".to_owned(),
            display_name: "Mock".to_owned(),
            driver_kind: "mock".to_owned(),
            program: PathBuf::from("mock"),
            args: vec![],
            env: vec![],
        };
        let handle = driver
            .start_session(
                StartSessionRequest {
                    session_id: "s".to_owned(),
                    title: None,
                    cwd: PathBuf::from("."),
                    launch,
                    extra_args: vec![],
                    initial_mode: None,
                    resume: None,
                },
                tx,
            )
            .await
            .expect("start_session");
        assert!(!handle.context_restored);
        handle
            .handle
            .send_prompt("hi".to_owned())
            .await
            .expect("prompt");
        match rx.recv().await.expect("event") {
            DriverEvent::Event(payload) => {
                assert_eq!(payload.event_type(), "agent_message");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
