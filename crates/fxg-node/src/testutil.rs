//! Test helpers (`cfg(test)` only).
//!
//! Collects utilities shared by tests in several modules, such as building a Git
//! repository in a temporary directory.

#![cfg(test)]
#![allow(
    clippy::disallowed_types,
    reason = "test helpers build raw JSON payloads; see docs/06-type-safety.md"
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use fxg_acp::{
    ActiveSessionHandle, AgentDriver, AgentLaunchSpec, AgentSessionInfo, AgentSessionList,
    DriverEvent, DriverKind, ResumeRequest, StartSessionRequest, StartedSession,
};
use fxg_protocol::client_api::AgentCapabilitiesPreview;
use fxg_protocol::common::{InitialConfig, SessionStatus};
use fxg_protocol::events::UnifiedEventPayload;
use tokio::sync::mpsc;

use crate::git;
use crate::session_manager::DriverFactory;

/// Creates a Git repository in a temporary directory with a single commit.
pub(crate) async fn init_test_repo(dir: &Path) {
    git::run_git(dir, &["init", "-q", "-b", "main"])
        .await
        .expect("git init");
    git::run_git(dir, &["config", "user.email", "test@example.com"])
        .await
        .expect("git config email");
    git::run_git(dir, &["config", "user.name", "fxg test"])
        .await
        .expect("git config name");
    std::fs::write(dir.join("README.md"), "# test\n").expect("write readme");
    git::run_git(dir, &["add", "-A"]).await.expect("git add");
    git::run_git(dir, &["commit", "-q", "-m", "initial"])
        .await
        .expect("git commit");
}

/// Creates a Git repository and configures an `origin` remote.
pub(crate) async fn init_test_repo_with_remote(dir: &Path, remote_url: &str) {
    init_test_repo(dir).await;
    git::run_git(dir, &["remote", "add", "origin", remote_url])
        .await
        .expect("git remote add");
}

/// Registers a test custom agent (`mock`) in `node.db` under the role paths.
///
/// `EnsureSession` through the daemon resolves launch specs from the DB-owned
/// catalog, so even mock driver tests need an agent definition.
pub(crate) async fn write_mock_agent_config(paths: &fxg_protocol::paths::Paths) {
    paths.ensure_dirs().expect("create role dirs");
    let db = fxg_db::Db::open(&paths.db_path(), fxg_protocol::paths::Role::Node)
        .await
        .expect("open node db");
    db.upsert_custom_agent(
        "mock",
        &fxg_protocol::settings::CustomAgentConfig {
            name: Some("Mock Agent".to_owned()),
            command: "mock-agent".to_owned(),
            ..Default::default()
        },
    )
    .await
    .expect("register custom agent");
}

/// Shared state of the test mock agent driver (also serves as the driver itself).
///
/// It does not spawn a real process; it records sent prompts, permission
/// responses and mode changes, and lets tests inject driver events (turn
/// progress, permission requests, etc.) via [`Self::emit`]. It is passed to
/// `SessionManager` via [`Self::factory`].
#[derive(Clone, Default)]
pub(crate) struct MockAgent {
    inner: Arc<MockAgentInner>,
}

/// Recorded `start_session` invocation for tests.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MockStart {
    /// Target session ID
    pub session_id: String,
    /// Working directory
    pub cwd: PathBuf,
    /// Resume request (`None` means a new session)
    pub resume: Option<ResumeRequest>,
    /// Source ACP session ID for a native fork
    pub fork_from: Option<String>,
    /// Native fork boundary (before this agent message)
    pub fork_before_message_id: Option<String>,
    /// Initial configuration applied after `session/new`
    pub initial_config: Vec<InitialConfig>,
}

#[derive(Default)]
struct MockAgentInner {
    prompts: Mutex<Vec<String>>,
    permissions: Mutex<Vec<(String, String)>>,
    elicitations: Mutex<
        Vec<(
            String,
            fxg_protocol::common::ElicitationAction,
            Option<fxg_protocol::common::ElicitationContent>,
        )>,
    >,
    modes: Mutex<Vec<String>>,
    events: Mutex<Vec<mpsc::UnboundedSender<DriverEvent>>>,
    starts: Mutex<Vec<MockStart>>,
    /// Whether native restore (resume) is supported
    resume_supported: AtomicBool,
    /// Capabilities emitted right after `session/new` (capability probe tests)
    start_capabilities: Mutex<Option<AgentCapabilitiesPreview>>,
    /// Sessions reported by `session/list` (external session discovery tests)
    external_sessions: Mutex<Vec<AgentSessionInfo>>,
    /// Whether the mock agent can replay history (`loadSession`)
    external_history: AtomicBool,
    /// Events emitted during `start_session` when `capture_history` is requested
    start_replay_events: Mutex<Vec<UnifiedEventPayload>>,
    /// Whether `capture_history` requests fail because the agent replays nothing
    history_capture_fails: AtomicBool,
    /// Whether the mock accepts prompts while a turn is running
    steering: AtomicBool,
    /// Whether the mock assigns agent-side message IDs
    assigns_message_ids: AtomicBool,
    /// Recorded reverts (`agent_message_id`, `files`)
    reverts: Mutex<Vec<(String, bool)>>,
    /// Number of compaction requests
    compactions: AtomicUsize,
    /// Fork scope emitted in the start capabilities
    fork_scope: Mutex<Option<fxg_protocol::common::ForkScope>>,
}

impl MockAgent {
    /// Injects a driver event into running sessions.
    pub(crate) fn emit(&self, event: DriverEvent) {
        for tx in self.inner.events.lock().expect("events").iter() {
            fxg_protocol::discard_with!(
                tx.send(event.clone()),
                "send a message to a channel that may be closed"
            );
        }
    }

    /// List of `start_session` invocations.
    pub(crate) fn starts(&self) -> Vec<MockStart> {
        self.inner.starts.lock().expect("starts").clone()
    }

    /// Sets whether native restore (resume) is supported.
    pub(crate) fn set_resume_supported(&self, supported: bool) {
        self.inner
            .resume_supported
            .store(supported, Ordering::SeqCst);
    }

    /// Sets the capabilities emitted immediately after `session/new`.
    pub(crate) fn set_start_capabilities(&self, capabilities: AgentCapabilitiesPreview) {
        *self
            .inner
            .start_capabilities
            .lock()
            .expect("start capabilities") = Some(capabilities);
    }

    /// Sets the sessions returned by `session/list`.
    pub(crate) fn set_external_sessions(
        &self,
        sessions: Vec<AgentSessionInfo>,
        supports_history: bool,
    ) {
        *self
            .inner
            .external_sessions
            .lock()
            .expect("external sessions") = sessions;
        self.inner
            .external_history
            .store(supports_history, Ordering::SeqCst);
    }

    /// Sets replay events emitted before `Idle` when `capture_history` is true.
    pub(crate) fn set_start_replay_events(&self, events: Vec<UnifiedEventPayload>) {
        *self
            .inner
            .start_replay_events
            .lock()
            .expect("start replay events") = events;
    }

    /// Sets whether prompts are accepted while a turn is running.
    pub(crate) fn set_steering(&self, steering: bool) {
        self.inner.steering.store(steering, Ordering::SeqCst);
    }

    /// Sets whether prompts receive agent-side message IDs.
    pub(crate) fn set_assigns_message_ids(&self, assigns: bool) {
        self.inner
            .assigns_message_ids
            .store(assigns, Ordering::SeqCst);
    }

    /// Recorded native reverts.
    pub(crate) fn reverts(&self) -> Vec<(String, bool)> {
        self.inner.reverts.lock().expect("reverts").clone()
    }

    /// Number of compaction requests.
    pub(crate) fn compactions(&self) -> usize {
        self.inner.compactions.load(Ordering::SeqCst)
    }

    /// Sets the fork scope advertised by the start capabilities.
    pub(crate) fn set_fork_scope(&self, scope: fxg_protocol::common::ForkScope) {
        *self.inner.fork_scope.lock().expect("fork scope") = Some(scope);
    }

    /// Makes `capture_history` fail as if `session/load` replayed no history.
    pub(crate) fn set_history_capture_fails(&self, fails: bool) {
        self.inner
            .history_capture_fails
            .store(fails, Ordering::SeqCst);
    }

    /// List of sent prompts.
    pub(crate) fn prompts(&self) -> Vec<String> {
        self.inner.prompts.lock().expect("prompts").clone()
    }

    /// Closes the event channels of running sessions (simulates driver exit).
    pub(crate) fn close_events(&self) {
        self.inner.events.lock().expect("events").clear();
    }

    /// List of permission responses (`request_id`, `option_id`).
    pub(crate) fn permissions(&self) -> Vec<(String, String)> {
        self.inner.permissions.lock().expect("permissions").clone()
    }

    /// List of elicitation responses (`elicitation_id`, `action`, `content`).
    pub(crate) fn elicitations(
        &self,
    ) -> Vec<(
        String,
        fxg_protocol::common::ElicitationAction,
        Option<fxg_protocol::common::ElicitationContent>,
    )> {
        self.inner
            .elicitations
            .lock()
            .expect("elicitations")
            .clone()
    }

    /// List of applied mode changes.
    pub(crate) fn modes(&self) -> Vec<String> {
        self.inner.modes.lock().expect("modes").clone()
    }

    /// Driver factory passed to `SessionManager`.
    pub(crate) fn factory(&self) -> DriverFactory {
        let agent = self.clone();
        Arc::new(move |_spec: &AgentLaunchSpec| Ok(Arc::new(agent.clone()) as Arc<dyn AgentDriver>))
    }
}

#[async_trait]
impl AgentDriver for MockAgent {
    fn driver_kind(&self) -> DriverKind {
        DriverKind::Other("mock".to_owned())
    }

    async fn list_sessions(
        &self,
        _launch: &AgentLaunchSpec,
        _cwd: &std::path::Path,
    ) -> anyhow::Result<AgentSessionList> {
        Ok(AgentSessionList {
            sessions: self
                .inner
                .external_sessions
                .lock()
                .expect("external sessions")
                .clone(),
            supports_history: self.inner.external_history.load(Ordering::SeqCst),
        })
    }

    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession> {
        // Restore succeeds only when native resume support is enabled and an agent-side ID exists
        let can_restore = req.resume.as_ref().is_some_and(|resume| {
            resume.agent_session_id.is_some() && self.inner.resume_supported.load(Ordering::SeqCst)
        });
        // If restore is impossible in native-only mode (`allow_fresh = false`), return
        // `NativeResumeUnavailable` like the real driver (no session is created)
        if req
            .resume
            .as_ref()
            .is_some_and(|resume| !resume.allow_fresh)
            && !can_restore
        {
            return Err(
                fxg_acp::NativeResumeUnavailable("mock resume unsupported".to_owned()).into(),
            );
        }
        let capturing_history = req
            .resume
            .as_ref()
            .is_some_and(|resume| resume.capture_history);
        if capturing_history && self.inner.history_capture_fails.load(Ordering::SeqCst) {
            return Err(fxg_acp::HistoryCaptureUnavailable(
                "mock agent replayed no history".to_owned(),
            )
            .into());
        }
        self.inner
            .events
            .lock()
            .expect("events")
            .push(event_tx.clone());
        let bound_agent_session = req
            .resume
            .as_ref()
            .and_then(|resume| resume.agent_session_id.clone());
        self.inner.starts.lock().expect("starts").push(MockStart {
            session_id: req.session_id,
            cwd: req.cwd,
            resume: req.resume,
            fork_from: req.fork_from,
            fork_before_message_id: req.fork_before_message_id,
            initial_config: req.initial_config,
        });
        if let Some(agent_session_id) = bound_agent_session.clone() {
            fxg_protocol::discard_with!(
                event_tx.send(DriverEvent::Event(UnifiedEventPayload::SessionAgentBound {
                    agent_session_id
                })),
                "send the agent session binding"
            );
        }
        let emitted_capabilities = self
            .inner
            .start_capabilities
            .lock()
            .expect("start capabilities")
            .clone();
        let fork_scope = *self.inner.fork_scope.lock().expect("fork scope");
        if let Some(capabilities) = emitted_capabilities.clone() {
            fxg_protocol::discard_with!(
                event_tx.send(DriverEvent::Event(
                    UnifiedEventPayload::CapabilitiesUpdated {
                        current_mode: capabilities.current_mode,
                        available_modes: capabilities.available_modes,
                        available_commands: Vec::new(),
                        config_options: capabilities.config_options,
                        fork_scope,
                        native_features: Vec::new(),
                    }
                )),
                "send capabilities to the probe"
            );
        } else if fork_scope.is_some() {
            fxg_protocol::discard_with!(
                event_tx.send(DriverEvent::Event(
                    UnifiedEventPayload::CapabilitiesUpdated {
                        current_mode: None,
                        available_modes: Vec::new(),
                        available_commands: Vec::new(),
                        config_options: Vec::new(),
                        fork_scope,
                        native_features: Vec::new(),
                    }
                )),
                "send the fork scope capability"
            );
        }
        if capturing_history {
            let replay = self
                .inner
                .start_replay_events
                .lock()
                .expect("start replay events")
                .clone();
            for payload in replay {
                fxg_protocol::discard_with!(
                    event_tx.send(DriverEvent::Event(payload)),
                    "send replay event"
                );
            }
        }
        if emitted_capabilities.is_some() || capturing_history || fork_scope.is_some() {
            fxg_protocol::discard_with!(
                event_tx.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Idle,
                    error_message: None,
                })),
                "send idle status to the probe"
            );
        }
        Ok(StartedSession {
            handle: Box::new(self.clone()),
            context_restored: can_restore,
            agent_session_id: bound_agent_session,
        })
    }
}

#[async_trait]
impl ActiveSessionHandle for MockAgent {
    fn supports_steering(&self) -> bool {
        self.inner.steering.load(Ordering::SeqCst)
    }

    fn assigns_message_ids(&self) -> bool {
        self.inner.assigns_message_ids.load(Ordering::SeqCst)
    }

    async fn send_prompt(&self, input: fxg_protocol::common::PromptInput) -> anyhow::Result<()> {
        self.inner.prompts.lock().expect("prompts").push(input.text);
        Ok(())
    }

    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()> {
        self.inner
            .permissions
            .lock()
            .expect("permissions")
            .push((request_id, selected_option_id));
        Ok(())
    }

    async fn respond_elicitation(
        &self,
        elicitation_id: String,
        action: fxg_protocol::common::ElicitationAction,
        content: Option<fxg_protocol::common::ElicitationContent>,
    ) -> anyhow::Result<()> {
        self.inner.elicitations.lock().expect("elicitations").push((
            elicitation_id,
            action,
            content,
        ));
        Ok(())
    }

    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()> {
        self.inner.modes.lock().expect("modes").push(mode_id);
        Ok(())
    }

    async fn set_config(
        &self,
        _key: String,
        _value: fxg_protocol::common::ConfigOptionValue,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn revert_to_message(&self, agent_message_id: &str, files: bool) -> anyhow::Result<()> {
        self.inner
            .reverts
            .lock()
            .expect("reverts")
            .push((agent_message_id.to_owned(), files));
        Ok(())
    }

    async fn compact_context(&self) -> anyhow::Result<()> {
        self.inner.compactions.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        // Close the event sender so the session manager's pump observes the end
        // (the real ACP driver drops its sender on shutdown).
        let sender = self.inner.events.lock().expect("events").pop();
        drop(sender);
        Ok(())
    }
}
