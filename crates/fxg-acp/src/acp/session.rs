use super::mapping::*;
use super::*;

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
        fork_supported: Some(
            agent_capabilities.session_capabilities.fork.is_some()
                && agent_capabilities.load_session,
        ),
    };
    if !modes.is_empty()
        || !commands.is_empty()
        || !config_options.is_empty()
        || current_mode.is_some()
        || capabilities_state.fork_supported.is_some()
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
pub(super) async fn serve_session(
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
/// discarded because fxg keeps the canonical history itself.
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
/// session. fxg keeps the canonical history in its own event log, so it drains and
/// drops the queue right after the response to avoid double recording.
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
pub(crate) fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
        .fs(FileSystemCapabilities::new()
            .read_text_file(true)
            .write_text_file(true))
        .terminal(true)
        .auth(AuthCapabilities::new().terminal(true))
        .elicitation(ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()))
}
