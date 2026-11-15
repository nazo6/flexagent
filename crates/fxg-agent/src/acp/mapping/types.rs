//! ACP <-> FXG type mapping (auth, config options, modes, tool vocabulary).

use super::*;

/// Extracts the session's mode state and config options.
///
/// ACP supersedes Session Modes with Session Config Options, and agents such as
/// OpenCode only report the selector as `configOptions[category = mode]`, so the
/// config option takes precedence over the legacy `modes` field.
pub(crate) fn session_capabilities(
    modes: Option<&SessionModeState>,
    config_options: Option<&[SessionConfigOption]>,
) -> (Vec<ModeInfo>, Option<String>, Vec<ConfigOptionInfo>) {
    let (available_modes, current_mode) = mode_config_option(config_options.unwrap_or_default())
        .and_then(mode_state_from_config)
        .unwrap_or_else(|| {
            (
                modes
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
                    .unwrap_or_default(),
                modes.map(|state| state.current_mode_id.to_string()),
            )
        });
    let config_options = config_options
        .map(|options| options.iter().map(map_config_option).collect())
        .unwrap_or_default();
    // Slash commands arrive via session/update (AvailableCommandsUpdate), so they
    // are not part of the session/new payload.
    (available_modes, current_mode, config_options)
}

/// The config option that carries the session mode, if the agent reports one.
///
/// Categories are UX hints (ACP: clients MUST handle missing or unknown
/// categories), so an uncategorized option named `mode` is accepted as well,
/// matching the Web UI.
pub(crate) fn mode_config_option(options: &[SessionConfigOption]) -> Option<&SessionConfigOption> {
    options.iter().find(|option| {
        option.category.as_ref() == Some(&SessionConfigOptionCategory::Mode)
            || (option.category.is_none() && option.id.to_string() == "mode")
    })
}

/// Projects a mode selector config option into FXG's mode list / current mode.
fn mode_state_from_config(option: &SessionConfigOption) -> Option<(Vec<ModeInfo>, Option<String>)> {
    let SessionConfigKind::Select(select) = &option.kind else {
        return None;
    };
    let to_mode = |value: &SessionConfigSelectOption| ModeInfo {
        mode_id: value.value.to_string(),
        name: value.name.clone(),
        description: value.description.clone(),
    };
    let mut modes = Vec::new();
    match &select.options {
        SessionConfigSelectOptions::Ungrouped(values) => {
            modes.extend(values.iter().map(to_mode));
        }
        SessionConfigSelectOptions::Grouped(groups) => {
            modes.extend(groups.iter().flat_map(|group| &group.options).map(to_mode));
        }
        // `SessionConfigSelectOptions` is non-exhaustive
        _ => {}
    }
    if modes.is_empty() {
        return None;
    }
    Some((modes, Some(select.current_value.to_string())))
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
pub(crate) fn map_config_option(option: &SessionConfigOption) -> ConfigOptionInfo {
    let (current_value, options) = match &option.kind {
        SessionConfigKind::Select(select) => {
            let values: Vec<&SessionConfigSelectOption> = match &select.options {
                SessionConfigSelectOptions::Ungrouped(values) => values.iter().collect(),
                SessionConfigSelectOptions::Grouped(groups) => {
                    groups.iter().flat_map(|group| &group.options).collect()
                }
                // `SessionConfigSelectOptions` is non-exhaustive
                _ => Vec::new(),
            };
            let current_id = select.current_value.to_string();
            let current_value = values
                .iter()
                .find(|value| value.value.to_string() == current_id)
                .map_or_else(
                    || ConfigOptionValue::Choice(ConfigOptionChoice::from_value(current_id)),
                    |value| ConfigOptionValue::Choice(map_choice(value)),
                );
            let options = values
                .iter()
                .map(|value| ConfigOptionValue::Choice(map_choice(value)))
                .collect();
            (current_value, options)
        }
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
        description: option.description.clone(),
        category: option.category.as_ref().map(map_config_category),
        current_value,
        options,
    }
}

/// Converts one ACP select option into FXG's display choice.
fn map_choice(value: &SessionConfigSelectOption) -> ConfigOptionChoice {
    ConfigOptionChoice {
        value: value.value.to_string(),
        name: value.name.clone(),
        description: value.description.clone(),
    }
}

/// Maps the ACP config option category to FXG's open enum.
fn map_config_category(category: &SessionConfigOptionCategory) -> ConfigOptionCategory {
    match category {
        SessionConfigOptionCategory::Mode => ConfigOptionCategory::Mode,
        SessionConfigOptionCategory::Model => ConfigOptionCategory::Model,
        SessionConfigOptionCategory::ModelConfig => ConfigOptionCategory::ModelConfig,
        SessionConfigOptionCategory::ThoughtLevel => ConfigOptionCategory::ThoughtLevel,
        SessionConfigOptionCategory::Other(raw) => ConfigOptionCategory::Unknown(raw.clone()),
        // `SessionConfigOptionCategory` is non-exhaustive; preserve future values as-is.
        _ => ConfigOptionCategory::Unknown(String::new()),
    }
}

/// Converts an FXG config value into the ACP wire value.
///
/// FXG exposes select values as bare strings (for example
/// `"provider/model"`), while ACP serializes the same value as
/// `{ "value": "..." }`. Deserializing the bare string directly into
/// [`SessionConfigOptionValue`] fails, so map the scalar shapes explicitly and
/// only fall back to serde for the tagged (object) forms.
pub(crate) fn parse_config_value(
    value: ConfigOptionValue,
) -> anyhow::Result<SessionConfigOptionValue> {
    match value {
        ConfigOptionValue::Choice(choice) => Ok(SessionConfigOptionValue::value_id(choice.value)),
        ConfigOptionValue::Boolean(enabled) => Ok(SessionConfigOptionValue::boolean(enabled)),
        ConfigOptionValue::Unknown(raw) => {
            serde_json::from_value(raw.into_inner()).map_err(Into::into)
        }
    }
}

/// Tracking state for session capabilities (modes, commands, config options).
#[derive(Debug, Default, Clone)]
pub(crate) struct SessionCapabilitiesState {
    pub(crate) current_mode: Option<String>,
    pub(crate) available_modes: Vec<ModeInfo>,
    pub(crate) available_commands: Vec<CommandInfo>,
    pub(crate) config_options: Vec<ConfigOptionInfo>,
    pub(crate) fork_scope: Option<ForkScope>,
}

impl SessionCapabilitiesState {
    pub(crate) fn to_payload(&self) -> UnifiedEventPayload {
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode: self.current_mode.clone(),
            available_modes: self.available_modes.clone(),
            available_commands: self.available_commands.clone(),
            config_options: self.config_options.clone(),
            fork_scope: self.fork_scope,
            native_features: Vec::new(),
        }
    }

    /// Replaces the config options, keeping the mode projection in sync.
    ///
    /// Agents that expose modes as a config option (ACP's current model) own the
    /// mode list and current mode, so `config_option_update` must refresh both.
    pub(crate) fn set_config_options(&mut self, options: &[SessionConfigOption]) {
        self.config_options = options.iter().map(map_config_option).collect();
        let Some((modes, current_mode)) =
            mode_config_option(options).and_then(mode_state_from_config)
        else {
            return;
        };
        self.available_modes = modes;
        if current_mode.is_some() {
            self.current_mode = current_mode;
        }
    }
}

/// Where a `session/update` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdateMode {
    /// A live turn / background update (user messages are recorded by the manager).
    Live,
    /// History replayed by `session/load` for an explicit import (user messages
    /// come from the replay and must be recorded).
    Import,
}

/// Whether the option is the session mode selector (the ACP category, or the
/// conventional `mode` key when the agent omits categories).
pub(crate) fn is_mode_option(option: &ConfigOptionInfo) -> bool {
    option.category.as_ref() == Some(&ConfigOptionCategory::Mode)
        || (option.category.is_none() && option.key == "mode")
}

pub(crate) fn tool_kind(kind: ToolKind) -> FxgToolCallKind {
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

pub(crate) fn tool_status(status: ToolCallStatus) -> FxgToolCallStatus {
    match status {
        ToolCallStatus::Pending => FxgToolCallStatus::Pending,
        ToolCallStatus::InProgress => FxgToolCallStatus::InProgress,
        ToolCallStatus::Completed => FxgToolCallStatus::Completed,
        ToolCallStatus::Failed => FxgToolCallStatus::Failed,
        _ => FxgToolCallStatus::Unknown(acp_wire_value(&status)),
    }
}

pub(crate) fn plan_status(status: &AcpPlanEntryStatus) -> FxgPlanEntryStatus {
    match status {
        AcpPlanEntryStatus::Pending => FxgPlanEntryStatus::Pending,
        AcpPlanEntryStatus::InProgress => FxgPlanEntryStatus::InProgress,
        AcpPlanEntryStatus::Completed => FxgPlanEntryStatus::Completed,
        _ => FxgPlanEntryStatus::Unknown(acp_wire_value(status)),
    }
}

pub(crate) fn permission_kind(kind: AcpPermissionOptionKind) -> FxgPermissionOptionKind {
    match kind {
        AcpPermissionOptionKind::AllowOnce => FxgPermissionOptionKind::AllowOnce,
        AcpPermissionOptionKind::AllowAlways => FxgPermissionOptionKind::AllowAlways,
        AcpPermissionOptionKind::RejectOnce => FxgPermissionOptionKind::RejectOnce,
        AcpPermissionOptionKind::RejectAlways => FxgPermissionOptionKind::RejectAlways,
        _ => FxgPermissionOptionKind::Unknown(acp_wire_value(&kind)),
    }
}
