use super::mapping::*;
use super::subagent::map_state_update;
use super::subagent::{ChildNotificationHandler, ChildSessions, SessionIngress};
use super::*;
use crate::agent::extension::{AcpCapability, AcpExtensionOptions};
use fxg_protocol::events::SubagentStatus;

use agent_client_protocol::v1::schema::SubagentCapabilities;

/// Reason the turn loop ended.
pub(super) enum ServeOutcome {
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
pub(super) async fn build_session(
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
            subagent_id: None,
        },
    );
    fxg_protocol::discard_with!(
        events.send(DriverEvent::Event(UnifiedEventPayload::SessionAgentBound {
            agent_session_id: acp_session_id.to_string(),
        })),
        "notify event subscribers"
    );

    // Apply initial configuration (model / thinking level, etc.).
    //
    // The response may contain the refreshed option list: selecting a model can
    // add dependent options (for example effort levels) that are not
    // part of the `session/new` payload.
    let mut applied_config_options: Option<Vec<SessionConfigOption>> = None;
    for setting in &params.initial_config {
        let value = match parse_config_value(setting.value.clone()) {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(key = %setting.key, "invalid initial config value: {err}");
                continue;
            }
        };
        match cx
            .send_request(SetSessionConfigOptionRequest::new(
                acp_session_id.clone(),
                setting.key.clone(),
                value,
            ))
            .block_task()
            .await
        {
            Ok(response) if !response.config_options.is_empty() => {
                applied_config_options = Some(response.config_options);
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(key = %setting.key, "failed to set initial config: {err}");
            }
        }
    }

    // Apply the initial mode
    //
    // ACP supersedes Session Modes with Session Config Options: agents such as
    // OpenCode only report the selector as `configOptions[category = mode]` and
    // reject those IDs in `session/set_mode`, so use whichever channel the agent
    // reports.
    let mode_option = mode_config_option(acquired.config_options.as_deref().unwrap_or_default());
    let mode_via_config = mode_option.is_some();
    let mut initial_mode_applied = false;
    if let Some(mode_id) = params.initial_mode.as_ref() {
        let result = match mode_option {
            Some(option) => cx
                .send_request(SetSessionConfigOptionRequest::new(
                    acp_session_id.clone(),
                    option.id.clone(),
                    SessionConfigOptionValue::value_id(mode_id.clone()),
                ))
                .block_task()
                .await
                .map(|response| {
                    if !response.config_options.is_empty() {
                        applied_config_options = Some(response.config_options);
                    }
                }),
            None => cx
                .send_request(SetSessionModeRequest::new(
                    acp_session_id.clone(),
                    mode_id.clone(),
                ))
                .block_task()
                .await
                .map(|_| ()),
        };
        match result {
            Ok(()) => initial_mode_applied = true,
            Err(err) => tracing::warn!("failed to set initial mode {mode_id}: {err}"),
        }
    }

    // Publish modes, slash commands and config options.
    //
    // `session/new` reports the agent defaults; reflect the initial values we
    // just applied so the first `CapabilitiesUpdated` matches the session state
    // (the agent usually does not emit a config update for `set_config_option`).
    let (modes, mut current_mode, mut config_options) = session_capabilities(
        acquired.modes.as_ref(),
        applied_config_options
            .as_deref()
            .or(acquired.config_options.as_deref()),
    );
    for setting in &params.initial_config {
        if let Some(option) = config_options
            .iter_mut()
            .find(|option| option.key == setting.key)
        {
            option.current_value = setting.value.clone();
        }
    }
    // `session/new` reports the mode from before the initial mode was applied.
    if initial_mode_applied && let Some(mode_id) = params.initial_mode.as_ref() {
        current_mode = Some(mode_id.clone());
        if mode_via_config
            && let Some(option) = config_options
                .iter_mut()
                .find(|option| is_mode_option(option))
        {
            option.current_value = ConfigOptionValue::Select(mode_id.clone());
        }
    }
    let capabilities_state = SessionCapabilitiesState {
        current_mode: current_mode.clone(),
        available_modes: modes.clone(),
        available_commands: Vec::new(),
        config_options: config_options.clone(),
        fork_scope: Some(
            if agent_capabilities.session_capabilities.fork.is_some()
                && agent_capabilities.load_session
            {
                ForkScope::Last
            } else {
                ForkScope::None
            },
        ),
    };
    if !modes.is_empty()
        || !config_options.is_empty()
        || current_mode.is_some()
        || capabilities_state.fork_scope.is_some()
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
#[allow(clippy::too_many_arguments)]
pub(super) async fn serve_session(
    mut session: ActiveSession<'static, Agent>,
    mut cmd_rx: mpsc::UnboundedReceiver<AcpCommand>,
    events: &mpsc::UnboundedSender<DriverEvent>,
    capabilities_state: Arc<Mutex<SessionCapabilitiesState>>,
    prompt_capabilities: &PromptCapabilities,
    contexts: SessionContexts,
    root_cwd: PathBuf,
    ingress_tx: mpsc::UnboundedSender<SessionIngress>,
    mut ingress_rx: mpsc::UnboundedReceiver<SessionIngress>,
    guards: Vec<agent_client_protocol::DynamicHandlerGuard<Agent>>,
) -> ServeOutcome {
    let root_id = session.session_id().clone();
    let mut runtime = SessionRuntime::new(
        root_id,
        events,
        capabilities_state,
        contexts,
        ingress_tx,
        root_cwd,
        guards,
    );

    loop {
        #[allow(clippy::large_enum_variant, reason = "short-lived select result")]
        enum Selected {
            Command(Option<AcpCommand>),
            Ingress(Option<SessionIngress>),
        }
        let selected = tokio::select! {
            command = cmd_rx.recv() => Selected::Command(command),
            ingress = ingress_rx.recv() => Selected::Ingress(ingress),
        };
        match selected {
            Selected::Command(Some(AcpCommand::Prompt(input))) => {
                runtime.accumulator.reset();
                runtime.send_payload(UnifiedEventPayload::StatusChanged {
                    status: fxg_protocol::common::SessionStatus::Running,
                    error_message: None,
                });
                let blocks = match prompt_blocks(input, prompt_capabilities) {
                    Ok(blocks) => blocks,
                    Err(err) => {
                        runtime.send_payload(UnifiedEventPayload::StatusChanged {
                            status: fxg_protocol::common::SessionStatus::Error,
                            error_message: Some(format!("invalid prompt: {err}")),
                        });
                        return ServeOutcome::Fatal(err);
                    }
                };
                if let Err(err) = session.send_prompt_blocks(blocks) {
                    return ServeOutcome::Fatal(err.into());
                }
                loop {
                    #[allow(clippy::large_enum_variant, reason = "short-lived select result")]
                    enum TurnSelected {
                        Read(Result<SessionMessage, agent_client_protocol::Error>),
                        Ingress(Option<SessionIngress>),
                    }
                    let selected = tokio::select! {
                        result = session.read_update() => TurnSelected::Read(result),
                        ingress = ingress_rx.recv() => TurnSelected::Ingress(ingress),
                    };
                    match selected {
                        TurnSelected::Ingress(ingress) => {
                            if let Some(ingress) = ingress {
                                runtime.handle_ingress(ingress, &session);
                            }
                        }
                        TurnSelected::Read(Ok(SessionMessage::PromptResponse(response))) => {
                            // Drain child / vendor notifications that arrived before the
                            // prompt response but were not selected yet.
                            while let Ok(ingress) = ingress_rx.try_recv() {
                                runtime.handle_ingress(ingress, &session);
                            }
                            for payload in runtime.accumulator.finish_all() {
                                runtime.send_payload(payload);
                            }
                            match response {
                                Ok(resp) => {
                                    let reason = resp.stop_reason;
                                    tracing::debug!(?reason, "acp turn finished");
                                    if let Some(payload) = turn_ended_event(reason) {
                                        runtime.send_payload(payload);
                                    }
                                }
                                Err(err) => {
                                    let message = format!("agent turn failed: {err}");
                                    tracing::warn!("{message}");
                                    runtime.send_payload(UnifiedEventPayload::StatusChanged {
                                        status: fxg_protocol::common::SessionStatus::Error,
                                        error_message: Some(message),
                                    });
                                    break;
                                }
                            }
                            runtime.send_payload(UnifiedEventPayload::StatusChanged {
                                status: fxg_protocol::common::SessionStatus::Idle,
                                error_message: None,
                            });
                            break;
                        }
                        TurnSelected::Read(Ok(SessionMessage::Message(dispatch))) => {
                            let mut notification = None;
                            let result = MatchDispatch::new(dispatch.into_raw())
                                .if_notification(async |notification_in: SessionNotification| {
                                    notification = Some(notification_in);
                                    Ok(())
                                })
                                .await
                                .otherwise_ignore();
                            if let Err(err) = result {
                                return ServeOutcome::Fatal(err.into());
                            }
                            if let Some(notification) = notification {
                                runtime.handle_update(notification.update, None, &session);
                            }
                        }
                        // `SessionMessage` is #[non_exhaustive]
                        TurnSelected::Read(Ok(_)) => {}
                        TurnSelected::Read(Err(err)) => {
                            return ServeOutcome::Fatal(err.into());
                        }
                    }
                }
            }
            Selected::Command(Some(AcpCommand::Shutdown)) | Selected::Command(None) => break,
            Selected::Ingress(ingress) => {
                if let Some(ingress) = ingress {
                    runtime.handle_ingress(ingress, &session);
                }
            }
        }
    }
    ServeOutcome::Closed
}

/// Per-session state for the parent / child event streams.
struct SessionRuntime<'a> {
    events: &'a mpsc::UnboundedSender<DriverEvent>,
    capabilities: Arc<Mutex<SessionCapabilitiesState>>,
    ingress_tx: mpsc::UnboundedSender<SessionIngress>,
    accumulator: StreamingAccumulator,
    children: ChildSessions,
    guards: Vec<agent_client_protocol::DynamicHandlerGuard<Agent>>,
}

impl<'a> SessionRuntime<'a> {
    fn new(
        root_id: SessionId,
        events: &'a mpsc::UnboundedSender<DriverEvent>,
        capabilities: Arc<Mutex<SessionCapabilitiesState>>,
        contexts: SessionContexts,
        ingress_tx: mpsc::UnboundedSender<SessionIngress>,
        root_cwd: PathBuf,
        guards: Vec<agent_client_protocol::DynamicHandlerGuard<Agent>>,
    ) -> Self {
        Self {
            events,
            capabilities,
            ingress_tx,
            accumulator: StreamingAccumulator::default(),
            children: ChildSessions::new(root_id.to_string(), root_cwd, contexts),
            guards,
        }
    }

    fn send_payload(&self, payload: UnifiedEventPayload) {
        fxg_protocol::discard_with!(
            self.events.send(DriverEvent::Event(payload)),
            "notify event subscribers"
        );
    }

    fn send_driver_event(&self, event: DriverEvent) {
        fxg_protocol::discard_with!(self.events.send(event), "notify event subscribers");
    }

    /// Normalizes one `session/update` from the root or a child.
    fn handle_update(
        &mut self,
        update: SessionUpdate,
        subagent_id: Option<&str>,
        session: &ActiveSession<'static, Agent>,
    ) {
        if let SessionUpdate::SubagentUpdate(announcement) = update {
            self.announce(announcement, subagent_id, session);
            return;
        }

        let mapped = {
            let mut capabilities = self
                .capabilities
                .lock()
                .expect("session capabilities poisoned");
            map_session_update(
                update,
                subagent_id,
                &mut self.accumulator,
                &mut capabilities,
            )
        };
        for event in mapped {
            self.send_driver_event(event);
        }
    }

    /// Handles work routed from a dynamic handler.
    fn handle_ingress(&mut self, ingress: SessionIngress, session: &ActiveSession<'static, Agent>) {
        match ingress {
            SessionIngress::Child(notification) => {
                let child = notification.session_id.to_string();
                if self.children.get(&child).is_none() {
                    self.upsert_child(
                        &child,
                        None,
                        0,
                        None,
                        None,
                        None,
                        Some(SubagentStatus::Running),
                        session,
                    );
                }
                self.handle_update(notification.update, Some(&child), session);
            }
            SessionIngress::Vendor(update) => {
                let child = update.child_session_id.clone();
                self.upsert_child(
                    &child,
                    update.parent_session_id.as_deref(),
                    update.depth,
                    update.title.clone(),
                    None,
                    update.cwd.clone(),
                    update.state.clone(),
                    session,
                );
                if let Some(inner) = update.update {
                    self.handle_update(inner, Some(&child), session);
                }
            }
        }
    }

    /// UPserts child metadata and registers its routing / filesystem context.
    #[allow(clippy::too_many_arguments)]
    fn upsert_child(
        &mut self,
        child: &str,
        parent: Option<&str>,
        depth: u32,
        title: Option<String>,
        description: Option<String>,
        cwd: Option<PathBuf>,
        status: Option<SubagentStatus>,
        session: &ActiveSession<'static, Agent>,
    ) {
        let previous = self.children.get(child).cloned();
        let info = self
            .children
            .upsert(child, parent, depth, title, description, cwd, status);
        self.children
            .insert_context(child, info.cwd.clone(), self.events.clone());
        if self.children.mark_registered(child) {
            let handler = ChildNotificationHandler::new(child.to_owned(), self.ingress_tx.clone());
            match session.connection().add_dynamic_handler(handler) {
                Ok(guard) => self.guards.push(guard),
                Err(err) => {
                    tracing::warn!(
                        child = child,
                        "failed to register child session notification handler: {err}"
                    );
                }
            }
        }
        // Vendors resend status on every update; only announce real changes so the
        // parent log does not fill with duplicate `subagent_updated` events.
        if previous.as_ref() == Some(&info) {
            return;
        }
        self.send_payload(UnifiedEventPayload::SubagentUpdated {
            child_session_id: child.to_owned(),
            parent_session_id: Some(info.parent),
            depth: info.depth,
            title: info.title,
            description: info.description,
            status: info.status,
        });
    }

    /// Handles a standard ACP `subagent_update` announcement.
    fn announce(
        &mut self,
        update: agent_client_protocol::v1::schema::SubagentUpdate,
        enclosing: Option<&str>,
        session: &ActiveSession<'static, Agent>,
    ) {
        let child = update.session_id.to_string();
        let status = update.state.value().map(map_state_update);
        self.upsert_child(
            &child,
            enclosing,
            0,
            update.title.value().cloned(),
            update.description.value().cloned(),
            update.cwd.value().cloned(),
            status,
            session,
        );
    }
}

/// Decides how to acquire the session from the `resume` selection and agent capabilities.
#[derive(Debug)]
pub(super) enum ResumePlan {
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
/// (design: `docs/04-agent-drivers-and-windows.md` §4.1).
///
/// When `resume.allow_fresh` is `false` (automatic resume on send), it returns
/// [`NativeResumeUnavailable`] instead of creating a fresh session, prompting the
/// caller to perform an explicit resume via history replay.
pub(super) fn plan_resume(
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

pub(super) struct AcquiredSession {
    session: ActiveSession<'static, Agent>,
    is_resumed: bool,
    modes: Option<SessionModeState>,
    config_options: Option<Vec<SessionConfigOption>>,
}

/// Acquires a session (following [`ResumePlan`]).
///
/// The returned `is_resumed` indicates native restore success (`false` = fresh
/// session; the caller injects history replay).
pub(super) async fn acquire_session(
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
/// discarded because the node mirrors the parent's canonical event log into the
/// fork instead (see `SessionEventBus::copy_conversation_events`).
pub(super) async fn fork_and_load_session(
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
/// session. fxg keeps the canonical history in its own event log (resume already
/// has it; a fork mirrors the parent's log in the node), so it drains and drops
/// the queue right after the response to avoid double recording.
pub(super) fn discard_replay_updates(session: &mut ActiveSession<'static, Agent>) {
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
pub(crate) fn client_capabilities(acp: &AcpExtensionOptions) -> ClientCapabilities {
    let mut capabilities = ClientCapabilities::new()
        .fs(FileSystemCapabilities::new()
            .read_text_file(true)
            .write_text_file(true))
        .terminal(true)
        .auth(AuthCapabilities::new().terminal(true))
        .elicitation(ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()));

    if let Some(meta) = &acp.client_meta
        && let serde_json::Value::Object(map) = meta.as_value()
    {
        capabilities = capabilities.meta(map.clone());
    }

    for capability in acp.capabilities.iter() {
        match capability {
            AcpCapability::Subagents => {
                capabilities = capabilities.subagents(Some(SubagentCapabilities::new()));
            }
            AcpCapability::Unknown(name) => {
                tracing::warn!(capability = name, "unknown ACP capability ignored");
            }
        }
    }

    capabilities
}
