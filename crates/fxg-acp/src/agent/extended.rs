//! ACP driver with an extension layer (Tier 2).
//!
//! `ExtendedAcpDriver` wraps an existing [`AgentDriver`] and inserts a relay
//! between the driver and the session manager. The relay applies a
//! [`CapabilityOverlay`] and forwards events through an optional
//! [`SessionExtension`].
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.1 and
//! `docs/changelog/2026-11-11-agent-extension-architecture.md`.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use fxg_protocol::common::{ConfigOptionValue, ElicitationAction, ElicitationContent, PromptInput};
use fxg_protocol::events::UnifiedEventPayload;
use tokio::sync::mpsc;

use super::extension::{
    AgentExtension, CapabilitiesSnapshot, EventDisposition, Hook, SessionContext, SessionExtension,
};
use crate::driver::{
    ActiveSessionHandle, AgentDriver, DriverEvent, DriverKind, StartSessionRequest, StartedSession,
};

/// ACP driver with an extension layer.
pub struct ExtendedAcpDriver {
    base: Arc<dyn AgentDriver>,
    extension: Arc<dyn AgentExtension>,
}

impl ExtendedAcpDriver {
    /// Wraps `base` with `extension`.
    pub fn new(base: Arc<dyn AgentDriver>, extension: Arc<dyn AgentExtension>) -> Self {
        Self { base, extension }
    }
}

#[async_trait]
impl AgentDriver for ExtendedAcpDriver {
    fn driver_kind(&self) -> DriverKind {
        DriverKind::AcpExtended
    }

    async fn list_sessions(
        &self,
        launch: &crate::driver::AgentLaunchSpec,
        cwd: &std::path::Path,
    ) -> anyhow::Result<crate::driver::AgentSessionList> {
        self.base.list_sessions(launch, cwd).await
    }

    async fn start_session(
        &self,
        mut req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession> {
        req.acp_parser = self.extension.notification_parser();
        let session_id = req.session_id.clone();
        let agent_id = req.launch.agent_id.clone();
        let cwd = req.cwd.clone();
        let launch = req.launch.clone();
        let (tap_tx, tap_rx) = mpsc::unbounded_channel();
        let started = self.base.start_session(req, tap_tx).await?;

        let context = SessionContext {
            session_id,
            agent_id,
            agent_session_id: started.agent_session_id.clone(),
            cwd,
            launch,
        };
        let session = match self.extension.open_session(&context).await {
            Ok(session) => session,
            Err(err) => {
                // Extensions are additive: a failing extension must not prevent
                // the ACP session from running.
                tracing::warn!(
                    agent_id = %context.agent_id,
                    "agent extension failed to open the session: {err:#}"
                );
                None
            }
        };

        spawn_relay(
            tap_rx,
            event_tx,
            Arc::clone(&self.extension),
            session.clone(),
            context,
        );

        Ok(StartedSession {
            handle: Box::new(ExtendedSessionHandle {
                inner: started.handle,
                session,
            }),
            context_restored: started.context_restored,
            agent_session_id: started.agent_session_id,
        })
    }

    async fn shutdown_idle(&self) -> usize {
        self.base.shutdown_idle().await
    }

    fn stderr_tail(&self, agent_id: &str) -> Vec<String> {
        self.base.stderr_tail(agent_id)
    }

    fn stderr_snapshot(&self) -> BTreeMap<String, Vec<String>> {
        self.base.stderr_snapshot()
    }
}

/// Handle that consults the session extension before delegating to ACP.
struct ExtendedSessionHandle {
    inner: Box<dyn ActiveSessionHandle>,
    session: Option<Arc<dyn SessionExtension>>,
}

#[async_trait]
impl ActiveSessionHandle for ExtendedSessionHandle {
    async fn send_prompt(&self, mut input: PromptInput) -> anyhow::Result<()> {
        if let Some(session) = &self.session
            && session.before_prompt(&mut input).await? == Hook::Handled
        {
            return Ok(());
        }
        self.inner.send_prompt(input).await
    }

    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()> {
        self.inner
            .respond_permission(request_id, selected_option_id)
            .await
    }

    async fn respond_elicitation(
        &self,
        elicitation_id: String,
        action: ElicitationAction,
        content: Option<ElicitationContent>,
    ) -> anyhow::Result<()> {
        self.inner
            .respond_elicitation(elicitation_id, action, content)
            .await
    }

    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()> {
        self.inner.set_mode(mode_id).await
    }

    async fn set_config(&self, key: String, value: ConfigOptionValue) -> anyhow::Result<()> {
        if let Some(session) = &self.session
            && session.before_config(&key, &value).await? == Hook::Handled
        {
            return Ok(());
        }
        self.inner.set_config(key, value).await
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        self.inner.cancel_turn().await
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        if let Some(session) = &self.session
            && let Err(err) = session.shutdown().await
        {
            tracing::warn!("agent session extension shutdown failed: {err:#}");
        }
        self.inner.shutdown().await
    }
}

/// Starts the event relay in the background.
fn spawn_relay(
    tap_rx: mpsc::UnboundedReceiver<DriverEvent>,
    event_tx: mpsc::UnboundedSender<DriverEvent>,
    extension: Arc<dyn AgentExtension>,
    session: Option<Arc<dyn SessionExtension>>,
    context: SessionContext,
) {
    fxg_protocol::spawn_guarded!(
        "agent extension relay",
        run_relay(tap_rx, event_tx, extension, session, context),
    );
}

/// Applies the capability overlay and session hooks to driver events.
async fn run_relay(
    mut tap_rx: mpsc::UnboundedReceiver<DriverEvent>,
    event_tx: mpsc::UnboundedSender<DriverEvent>,
    extension: Arc<dyn AgentExtension>,
    session: Option<Arc<dyn SessionExtension>>,
    context: SessionContext,
) {
    while let Some(mut event) = tap_rx.recv().await {
        if let DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
            current_mode,
            available_modes,
            available_commands,
            config_options,
            ..
        }) = &mut event
        {
            // Recompute for every update: a `Replace` patch must be based on
            // the values currently advertised, not on an earlier snapshot.
            let snapshot = CapabilitiesSnapshot {
                current_mode: current_mode.clone(),
                available_modes: available_modes.clone(),
                available_commands: available_commands.clone(),
                config_options: config_options.clone(),
            };
            match extension.capabilities(&context, &snapshot).await {
                Ok(Some(overlay)) => overlay.apply(
                    current_mode,
                    available_modes,
                    available_commands,
                    config_options,
                ),
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(
                        agent_id = %context.agent_id,
                        "agent extension capability overlay failed: {err:#}"
                    );
                }
            }
        }

        let Some(session) = &session else {
            forward(&event_tx, event);
            continue;
        };
        let original = event.clone();
        match session.on_event(event).await {
            Ok(EventDisposition::Emit) => forward(&event_tx, original),
            Ok(EventDisposition::Replace(event)) => forward(&event_tx, *event),
            Ok(EventDisposition::EmitMany(events)) => {
                for event in events {
                    forward(&event_tx, event);
                }
            }
            Ok(EventDisposition::Drop) => {}
            Err(err) => {
                // Event enrichment is best-effort; never drop the ACP event.
                tracing::warn!(
                    agent_id = %context.agent_id,
                    "agent session extension failed to handle an event: {err:#}"
                );
                forward(&event_tx, original);
            }
        }
    }
}

/// Forwards one event to the session manager.
fn forward(event_tx: &mpsc::UnboundedSender<DriverEvent>, event: DriverEvent) {
    fxg_protocol::discard_with!(event_tx.send(event), "forward agent extension event");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use fxg_protocol::common::{ConfigOptionCategory, ConfigOptionInfo, ModeInfo};

    use crate::agent::extension::{CapabilityOverlay, ConfigOptionPatch};
    use crate::driver::{AgentLaunchSpec, DriverKind};

    fn launch() -> AgentLaunchSpec {
        AgentLaunchSpec {
            agent_id: "demo".to_owned(),
            display_name: "Demo".to_owned(),
            driver_kind: DriverKind::Acp,
            extension: None,
            program: Some(PathBuf::from("demo")),
            args: Vec::new(),
            env: Vec::new(),
            acp: Default::default(),
        }
    }

    fn request() -> StartSessionRequest {
        StartSessionRequest {
            session_id: "fxg-session".to_owned(),
            title: Some("Demo".to_owned()),
            cwd: PathBuf::from("."),
            launch: launch(),
            extra_args: Vec::new(),
            initial_mode: None,
            resume: None,
            fork_from: None,
            initial_config: Vec::new(),
            acp_parser: None,
        }
    }

    fn capability_event() -> UnifiedEventPayload {
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode: Some("code".to_owned()),
            available_modes: vec![ModeInfo {
                mode_id: "code".to_owned(),
                name: "Code".to_owned(),
                description: None,
            }],
            available_commands: Vec::new(),
            config_options: vec![ConfigOptionInfo {
                key: "model".to_owned(),
                name: "Model".to_owned(),
                description: None,
                category: Some(ConfigOptionCategory::Model),
                current_value: ConfigOptionValue::Select("fast".to_owned()),
                options: vec![ConfigOptionValue::Select("fast".to_owned())],
            }],
            fork_scope: None,
        }
    }

    #[derive(Default)]
    struct FakeBase {
        prompts: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl AgentDriver for FakeBase {
        fn driver_kind(&self) -> DriverKind {
            DriverKind::Acp
        }

        async fn start_session(
            &self,
            _req: StartSessionRequest,
            event_tx: mpsc::UnboundedSender<DriverEvent>,
        ) -> anyhow::Result<StartedSession> {
            let agent_session_id = "agent-session".to_owned();
            fxg_protocol::discard_with!(
                event_tx.send(DriverEvent::Event(UnifiedEventPayload::SessionAgentBound {
                    agent_session_id: agent_session_id.clone(),
                })),
                "send session binding in the test"
            );
            fxg_protocol::discard_with!(
                event_tx.send(DriverEvent::Event(capability_event())),
                "send capabilities in the test"
            );
            Ok(StartedSession {
                handle: Box::new(FakeHandle {
                    prompts: Arc::clone(&self.prompts),
                }),
                context_restored: false,
                agent_session_id: Some(agent_session_id),
            })
        }
    }

    struct FakeHandle {
        prompts: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ActiveSessionHandle for FakeHandle {
        async fn send_prompt(&self, input: PromptInput) -> anyhow::Result<()> {
            self.prompts.lock().expect("prompts").push(input.text);
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

        async fn set_config(&self, _key: String, _value: ConfigOptionValue) -> anyhow::Result<()> {
            Ok(())
        }

        async fn cancel_turn(&self) -> anyhow::Result<()> {
            Ok(())
        }

        async fn shutdown(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct FakeExtension {
        seen_ids: Arc<Mutex<Vec<Option<String>>>>,
        fail_capabilities: bool,
    }

    #[async_trait]
    impl AgentExtension for FakeExtension {
        async fn capabilities(
            &self,
            ctx: &SessionContext,
            _current: &CapabilitiesSnapshot,
        ) -> anyhow::Result<Option<CapabilityOverlay>> {
            self.seen_ids
                .lock()
                .expect("seen ids")
                .push(ctx.agent_session_id.clone());
            if self.fail_capabilities {
                anyhow::bail!("capability API is unavailable");
            }
            Ok(Some(CapabilityOverlay {
                modes: Some(vec![ModeInfo {
                    mode_id: "enriched".to_owned(),
                    name: "Enriched".to_owned(),
                    description: None,
                }]),
                current_mode: Some("enriched".to_owned()),
                config_options: vec![ConfigOptionPatch::Merge {
                    key: "model".to_owned(),
                    name: Some("Model (enriched)".to_owned()),
                    options: Some(vec![
                        ConfigOptionValue::Select("fast".to_owned()),
                        ConfigOptionValue::Select("smart".to_owned()),
                    ]),
                    current: Some(ConfigOptionValue::Select("smart".to_owned())),
                }],
                add_commands: Vec::new(),
            }))
        }

        async fn open_session(
            &self,
            _ctx: &SessionContext,
        ) -> anyhow::Result<Option<Arc<dyn SessionExtension>>> {
            Ok(Some(Arc::new(FakeSession {
                seen_ids: Arc::clone(&self.seen_ids),
            })))
        }
    }

    struct FakeSession {
        seen_ids: Arc<Mutex<Vec<Option<String>>>>,
    }

    #[async_trait]
    impl SessionExtension for FakeSession {
        async fn before_prompt(&self, input: &mut PromptInput) -> anyhow::Result<Hook> {
            self.seen_ids
                .lock()
                .expect("seen ids")
                .push(Some("prompt".to_owned()));
            input.text.push('!');
            Ok(Hook::Fallback)
        }
    }

    struct FakeHarness {
        driver: ExtendedAcpDriver,
        prompts: Arc<Mutex<Vec<String>>>,
        seen_ids: Arc<Mutex<Vec<Option<String>>>>,
    }

    fn fake_driver(fail_capabilities: bool) -> FakeHarness {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let seen_ids = Arc::new(Mutex::new(Vec::new()));
        let base = Arc::new(FakeBase {
            prompts: Arc::clone(&prompts),
        });
        let extension = Arc::new(FakeExtension {
            seen_ids: Arc::clone(&seen_ids),
            fail_capabilities,
        });
        FakeHarness {
            driver: ExtendedAcpDriver::new(base, extension),
            prompts,
            seen_ids,
        }
    }

    async fn next_capabilities(
        events: &mut mpsc::UnboundedReceiver<DriverEvent>,
    ) -> UnifiedEventPayload {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let event = tokio::time::timeout_at(deadline, events.recv())
                .await
                .expect("capability event timed out")
                .expect("event channel closed");
            if let DriverEvent::Event(payload @ UnifiedEventPayload::CapabilitiesUpdated { .. }) =
                event
            {
                return payload;
            }
        }
    }

    #[tokio::test]
    async fn applies_overlay_and_session_hooks() {
        let harness = fake_driver(false);
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let started = harness
            .driver
            .start_session(request(), event_tx)
            .await
            .expect("start");
        assert_eq!(started.agent_session_id.as_deref(), Some("agent-session"));

        let payload = next_capabilities(&mut event_rx).await;
        match payload {
            UnifiedEventPayload::CapabilitiesUpdated {
                current_mode,
                available_modes,
                config_options,
                ..
            } => {
                assert_eq!(current_mode.as_deref(), Some("enriched"));
                assert_eq!(available_modes[0].mode_id, "enriched");
                assert_eq!(config_options[0].name, "Model (enriched)");
                assert_eq!(config_options[0].options.len(), 2);
                assert_eq!(
                    config_options[0].current_value,
                    ConfigOptionValue::Select("smart".to_owned())
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }

        started
            .handle
            .send_prompt(PromptInput::text("hello"))
            .await
            .expect("prompt");
        assert_eq!(
            harness.prompts.lock().expect("prompts").as_slice(),
            ["hello!"]
        );
        assert!(
            harness
                .seen_ids
                .lock()
                .expect("seen ids")
                .contains(&Some("agent-session".to_owned()))
        );
    }

    #[tokio::test]
    async fn keeps_the_acp_event_when_the_overlay_fails() {
        let harness = fake_driver(true);
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let _started = harness
            .driver
            .start_session(request(), event_tx)
            .await
            .expect("start");

        let payload = next_capabilities(&mut event_rx).await;
        match payload {
            UnifiedEventPayload::CapabilitiesUpdated {
                current_mode,
                config_options,
                ..
            } => {
                assert_eq!(current_mode.as_deref(), Some("code"));
                assert_eq!(config_options[0].name, "Model");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn shutdown_calls_the_base_handle() {
        let harness = fake_driver(false);
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        let started = harness
            .driver
            .start_session(request(), event_tx)
            .await
            .expect("start");
        started.handle.shutdown().await.expect("shutdown");
    }

    #[test]
    fn mode_enum_is_stable_for_logs() {
        assert_eq!(DriverKind::AcpExtended.as_str(), "acp_extended");
    }
}
