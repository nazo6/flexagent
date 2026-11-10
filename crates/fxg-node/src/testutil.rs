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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use fxg_acp::{
    ActiveSessionHandle, AgentDriver, AgentLaunchSpec, DriverEvent, DriverKind, ResumeRequest,
    StartSessionRequest, StartedSession,
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

/// Returns a local IPC endpoint for tests.
///
/// - **Windows**: Named pipe (`\\.\pipe\fxg-test-<pid>-<n>`). Tests run in
///   parallel within the same process, so a sequence number keeps them unique.
/// - **Unix**: A temporary socket under `dir` (each test uses its own tempdir).
pub(crate) fn test_ipc_endpoint(dir: &Path) -> String {
    if cfg!(windows) {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!(r"\\.\pipe\fxg-test-{}-{n}", std::process::id())
    } else {
        dir.join("node.sock").to_string_lossy().into_owned()
    }
}

/// Registers a test custom agent (`mock`) in `node.db` under `fxg_home`.
///
/// `EnsureSession` through the daemon resolves launch specs from the DB-owned
/// catalog, so even mock driver tests need an agent definition.
pub(crate) async fn write_mock_agent_config(fxg_home: &Path) {
    std::fs::create_dir_all(fxg_home).expect("mkdir home");
    let db = fxg_db::Db::open(&fxg_db::node_db_path(fxg_home), fxg_db::DbRole::Node)
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
        self.inner
            .events
            .lock()
            .expect("events")
            .push(event_tx.clone());
        self.inner.starts.lock().expect("starts").push(MockStart {
            session_id: req.session_id,
            cwd: req.cwd,
            resume: req.resume,
            fork_from: req.fork_from,
            initial_config: req.initial_config,
        });
        if let Some(capabilities) = self
            .inner
            .start_capabilities
            .lock()
            .expect("start capabilities")
            .clone()
        {
            fxg_protocol::discard_with!(
                event_tx.send(DriverEvent::Event(
                    UnifiedEventPayload::CapabilitiesUpdated {
                        current_mode: capabilities.current_mode,
                        available_modes: capabilities.available_modes,
                        available_commands: Vec::new(),
                        config_options: capabilities.config_options,
                        fork_supported: None,
                    }
                )),
                "send capabilities to the probe"
            );
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
        })
    }
}

#[async_trait]
impl ActiveSessionHandle for MockAgent {
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
