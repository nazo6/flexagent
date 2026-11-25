//! Initial config / mode application and client capabilities for ACP sessions.
//!
//! ACP supersedes session modes with session config options: agents such as
//! OpenCode only report the mode selector as `configOptions[category = mode]`
//! and reject those IDs in `session/set_mode`, so both channels are supported.

use fxg_protocol::common::InitialConfig;

use super::*;

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
        .elicitation(ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()))
        // Boolean config options are rendered and written back by the UI, so
        // advertise them; without this agents cannot expose `type: "boolean"`.
        .session(ClientSessionCapabilities::new().config_options(
            SessionConfigOptionsCapabilities::new().boolean(BooleanConfigOptionCapabilities::new()),
        ));

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

/// Applies the initial configuration (model / thinking level, etc.).
///
/// The response may contain the refreshed option list: selecting a model can
/// add dependent options (for example effort levels) that are not part of the
/// `session/new` payload.
pub(super) async fn apply_initial_config(
    cx: &ConnectionTo<Agent>,
    acp_session_id: &SessionId,
    initial_config: &[InitialConfig],
) -> Option<Vec<SessionConfigOption>> {
    let mut applied_config_options = None;
    for setting in initial_config {
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
    applied_config_options
}

/// Applies the initial mode through whichever channel the agent reports.
///
/// Returns whether the mode was applied. `applied_config_options` is updated
/// when `session/set_config_option` returns a refreshed option list.
pub(super) async fn apply_initial_mode(
    cx: &ConnectionTo<Agent>,
    acp_session_id: &SessionId,
    initial_mode: Option<&str>,
    acquired_config_options: Option<&[SessionConfigOption]>,
    applied_config_options: &mut Option<Vec<SessionConfigOption>>,
) -> bool {
    let mode_option = mode_config_option(acquired_config_options.unwrap_or_default());
    let Some(mode_id) = initial_mode else {
        return false;
    };
    let result = match mode_option {
        Some(option) => cx
            .send_request(SetSessionConfigOptionRequest::new(
                acp_session_id.clone(),
                option.id.clone(),
                SessionConfigOptionValue::value_id(mode_id.to_owned()),
            ))
            .block_task()
            .await
            .map(|response| {
                if !response.config_options.is_empty() {
                    *applied_config_options = Some(response.config_options);
                }
            }),
        None => cx
            .send_request(SetSessionModeRequest::new(
                acp_session_id.clone(),
                mode_id.to_owned(),
            ))
            .block_task()
            .await
            .map(|_| ()),
    };
    match result {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!("failed to set initial mode {mode_id}: {err}");
            false
        }
    }
}

/// Builds the capabilities published after the initial config / mode.
///
/// `session/new` reports the agent defaults; reflect the initial values that
/// were just applied so the first `CapabilitiesUpdated` matches the session
/// state (the agent usually does not emit a config update for
/// `set_config_option`), and report the fork scope the agent supports.
pub(super) fn session_capabilities_after_initial(
    agent_capabilities: &AgentCapabilities,
    acquired_modes: Option<&SessionModeState>,
    applied_config_options: Option<&[SessionConfigOption]>,
    acquired_config_options: Option<&[SessionConfigOption]>,
    initial_config: &[InitialConfig],
    initial_mode: Option<&str>,
    initial_mode_applied: bool,
) -> SessionCapabilitiesState {
    let (modes, mut current_mode, mut config_options) = session_capabilities(
        acquired_modes,
        applied_config_options.or(acquired_config_options),
    );
    for setting in initial_config {
        if let Some(option) = config_options
            .iter_mut()
            .find(|option| option.key == setting.key)
        {
            option.current_value = setting.value.clone();
        }
    }
    // `session/new` reports the mode from before the initial mode was applied.
    if initial_mode_applied && let Some(mode_id) = initial_mode {
        current_mode = Some(mode_id.to_owned());
        if mode_config_option(acquired_config_options.unwrap_or_default()).is_some()
            && let Some(option) = config_options
                .iter_mut()
                .find(|option| is_mode_option(option))
        {
            // Keep the label from the agent's option list when the mode exists.
            option.current_value = option
                .options
                .iter()
                .find(|choice| choice.select_value() == Some(mode_id))
                .cloned()
                .unwrap_or_else(|| {
                    ConfigOptionValue::Choice(ConfigOptionChoice::from_value(mode_id.to_owned()))
                });
        }
    }
    SessionCapabilitiesState {
        current_mode,
        available_modes: modes,
        available_commands: Vec::new(),
        config_options,
        fork_scope: Some(
            if agent_capabilities.session_capabilities.fork.is_some()
                && agent_capabilities.load_session
            {
                ForkScope::Last
            } else {
                ForkScope::None
            },
        ),
    }
}
