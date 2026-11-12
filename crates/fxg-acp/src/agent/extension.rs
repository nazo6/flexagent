//! ACP extension traits and capability overrides (Tier 2).
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.1 and
//! `docs/changelog/2026-11-11-agent-extension-architecture.md`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use fxg_protocol::common::{
    CommandInfo, ConfigOptionInfo, ConfigOptionValue, ModeInfo, PromptInput, RawJson,
};
use fxg_protocol::events::SubagentStatus;

use crate::driver::{AgentLaunchSpec, DriverEvent};

/// Display metadata that an extension may override.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentMetadata {
    /// Display name
    pub name: Option<String>,
    /// Description shown in the catalog
    pub description: Option<String>,
    /// Icon URL or built-in asset key
    pub icon: Option<String>,
}

/// Capability information currently advertised by ACP.
///
/// Passed to [`AgentExtension::capabilities`] so an extension can transform
/// existing values (for example model labels) instead of only adding static
/// patches.
#[derive(Debug, Clone, Default)]
pub struct CapabilitiesSnapshot {
    /// Current mode
    pub current_mode: Option<String>,
    /// Available modes
    pub available_modes: Vec<ModeInfo>,
    /// Available slash commands
    pub available_commands: Vec<CommandInfo>,
    /// Configuration entries
    pub config_options: Vec<ConfigOptionInfo>,
}

/// Session information passed to an extension.
#[derive(Debug, Clone)]
pub struct SessionContext {
    /// fxg-side session ID
    pub session_id: String,
    /// Agent ID
    pub agent_id: String,
    /// Agent-side session ID (when known)
    pub agent_session_id: Option<String>,
    /// Session working directory
    pub cwd: PathBuf,
    /// Resolved launch spec (final args / env)
    pub launch: AgentLaunchSpec,
}

/// ACP client capability that an extension can advertise.
///
/// Unknown names are preserved so a newer configuration does not silently
/// enable a capability that this build does not understand.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpCapability {
    /// ACP subagents RFD (`clientCapabilities.subagents`).
    Subagents,
    /// A capability added by a newer fxg / ACP schema.
    Unknown(String),
}

impl AcpCapability {
    /// Parses a capability name (`subagents`, ...).
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "subagents" => Self::Subagents,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Capability name used on the wire and in logs.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Subagents => "subagents",
            Self::Unknown(name) => name,
        }
    }
}

/// Set of ACP client capabilities advertised on a connection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcpCapabilitySet(BTreeSet<AcpCapability>);

impl AcpCapabilitySet {
    /// Creates an empty capability set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a capability.
    pub fn insert(&mut self, capability: AcpCapability) -> bool {
        self.0.insert(capability)
    }

    /// Whether the capability is enabled.
    #[must_use]
    pub fn contains(&self, capability: &AcpCapability) -> bool {
        self.0.contains(capability)
    }

    /// Removes a capability (no-op when it was not enabled).
    pub fn remove(&mut self, capability: &AcpCapability) -> bool {
        self.0.remove(capability)
    }

    /// Iterates over the enabled capabilities.
    pub fn iter(&self) -> impl Iterator<Item = &AcpCapability> {
        self.0.iter()
    }

    /// Whether no capability is enabled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<AcpCapability> for AcpCapabilitySet {
    fn from_iter<T: IntoIterator<Item = AcpCapability>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl From<&fxg_protocol::settings::AgentAcpCapabilities> for AcpCapabilitySet {
    fn from(capabilities: &fxg_protocol::settings::AgentAcpCapabilities) -> Self {
        let mut set = Self::new();
        if capabilities.subagents {
            set.insert(AcpCapability::Subagents);
        }
        set
    }
}

/// ACP connection options contributed by an extension.
///
/// This is plain data so it can live in [`AgentLaunchSpec`] and the warm-pool
/// key. The agent-specific parser is passed separately per session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpExtensionOptions {
    /// Additional `clientCapabilities._meta` entries.
    pub client_meta: Option<RawJson>,
    /// Custom notification method names the extension wants to receive.
    pub notifications: Vec<String>,
    /// Client capabilities to advertise.
    pub capabilities: AcpCapabilitySet,
}

impl Default for AcpExtensionOptions {
    fn default() -> Self {
        let mut capabilities = AcpCapabilitySet::new();
        capabilities.insert(AcpCapability::Subagents);
        Self {
            client_meta: None,
            notifications: Vec::new(),
            capabilities,
        }
    }
}

/// A child-session update parsed from a vendor notification.
#[derive(Debug, Clone)]
pub struct AcpChildUpdate {
    /// Agent-side session ID of the child.
    pub child_session_id: String,
    /// Agent-side session ID of the parent (when reported).
    pub parent_session_id: Option<String>,
    /// Nesting depth reported by the vendor (0 = unknown).
    pub depth: u32,
    /// Display title for the child.
    pub title: Option<String>,
    /// Working directory of the child (when reported).
    pub cwd: Option<PathBuf>,
    /// Lifecycle state (status-only updates).
    pub state: Option<SubagentStatus>,
    /// Inner ACP update to normalize.
    pub update: Option<agent_client_protocol::v1::schema::SessionUpdate>,
}

/// Parses vendor-specific subagent notifications into a generic child update.
pub trait AcpNotificationParser: Send + Sync + std::fmt::Debug {
    /// Notification methods handled by this parser.
    fn methods(&self) -> &'static [&'static str];
    /// Parses one notification. Returns `None` when the payload is unknown.
    fn parse(&self, method: &str, params: &RawJson) -> Option<AcpChildUpdate>;
}

/// ACP extension (Tier 2): metadata, launch adjustments and capability overrides.
///
/// One extension instance is shared across sessions. Keep per-session state in
/// the [`SessionExtension`] returned by [`Self::open_session`].
#[async_trait]
pub trait AgentExtension: Send + Sync {
    /// Display overrides (name, icon, description).
    fn metadata(&self) -> AgentMetadata {
        AgentMetadata::default()
    }

    /// Adjust launch args / env before the ACP process starts.
    ///
    /// The resulting spec is also the warm-pool key, so keep it plain data.
    fn prepare_launch(&self, _cwd: &Path, _launch: &mut AgentLaunchSpec) -> anyhow::Result<()> {
        Ok(())
    }

    /// ACP connection options (client capabilities, vendor notification methods).
    ///
    /// The returned value is stored in [`AgentLaunchSpec`] so the ACP driver can
    /// apply it without knowing the extension. `capabilities` is the effective
    /// per-agent selection, so a vendor feature can follow a capability toggle
    /// (for example, OpenCode's child-session updates follow `subagents`).
    fn acp_options(
        &self,
        _capabilities: &fxg_protocol::settings::AgentAcpCapabilities,
    ) -> AcpExtensionOptions {
        AcpExtensionOptions::default()
    }

    /// Parser for vendor-specific subagent notifications.
    fn notification_parser(&self) -> Option<Arc<dyn AcpNotificationParser>> {
        None
    }

    /// Override / augment what ACP advertises (models, modes, config options).
    ///
    /// Called for every `CapabilitiesUpdated` (including probe sessions) with
    /// the values currently advertised. Expensive lookups should be cached by
    /// the extension.
    async fn capabilities(
        &self,
        _ctx: &SessionContext,
        _current: &CapabilitiesSnapshot,
    ) -> anyhow::Result<Option<CapabilityOverlay>> {
        Ok(None)
    }

    /// Called after the ACP session is established.
    async fn open_session(
        &self,
        _ctx: &SessionContext,
    ) -> anyhow::Result<Option<Arc<dyn SessionExtension>>> {
        Ok(None)
    }
}

/// Per-session hooks layered over an ACP session.
#[async_trait]
pub trait SessionExtension: Send + Sync {
    /// Preprocess a prompt. Return [`Hook::Handled`] to skip the ACP prompt.
    async fn before_prompt(&self, _input: &mut PromptInput) -> anyhow::Result<Hook> {
        Ok(Hook::Fallback)
    }

    /// Intercept a config change if the extension provides values ACP does not know.
    async fn before_config(&self, _key: &str, _value: &ConfigOptionValue) -> anyhow::Result<Hook> {
        Ok(Hook::Fallback)
    }

    /// Observe / rewrite an event from the ACP driver.
    async fn on_event(&self, _event: DriverEvent) -> anyhow::Result<EventDisposition> {
        Ok(EventDisposition::Emit)
    }

    /// Clean up session-scoped resources.
    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Whether an extension handled an operation or left it to ACP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    /// The extension handled the operation.
    Handled,
    /// Fall through to the ACP driver.
    Fallback,
}

/// How an extension wants an event forwarded.
#[derive(Debug, Clone)]
pub enum EventDisposition {
    /// Forward the original event.
    Emit,
    /// Replace the event with this one.
    Replace(Box<DriverEvent>),
    /// Replace the event with multiple events.
    EmitMany(Vec<DriverEvent>),
    /// Drop the event.
    Drop,
}

/// Typed capability overrides applied to `CapabilitiesUpdated`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CapabilityOverlay {
    /// Replace the available modes when set.
    pub modes: Option<Vec<ModeInfo>>,
    /// Replace the current mode when set.
    pub current_mode: Option<String>,
    /// Configuration option patches.
    pub config_options: Vec<ConfigOptionPatch>,
    /// Slash commands to add (existing names are kept).
    pub add_commands: Vec<CommandInfo>,
}

/// One configuration option change.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigOptionPatch {
    /// Replace the option with the same key, or add it when absent.
    Replace(ConfigOptionInfo),
    /// Merge labels / choices into an existing option.
    Merge {
        /// Configuration key
        key: String,
        /// New display name (when set)
        name: Option<String>,
        /// New selectable values (when set)
        options: Option<Vec<ConfigOptionValue>>,
        /// New current value (when set)
        current: Option<ConfigOptionValue>,
    },
    /// Remove the option with this key.
    Remove {
        /// Configuration key
        key: String,
    },
}

impl CapabilityOverlay {
    /// Applies the overlay to a `CapabilitiesUpdated` payload.
    pub(crate) fn apply(
        &self,
        current_mode: &mut Option<String>,
        available_modes: &mut Vec<ModeInfo>,
        available_commands: &mut Vec<CommandInfo>,
        config_options: &mut Vec<ConfigOptionInfo>,
    ) {
        if let Some(modes) = &self.modes {
            *available_modes = modes.clone();
        }
        if let Some(mode) = &self.current_mode {
            *current_mode = Some(mode.clone());
        }

        for patch in &self.config_options {
            match patch {
                ConfigOptionPatch::Replace(option) => {
                    if let Some(existing) = config_options
                        .iter_mut()
                        .find(|existing| existing.key == option.key)
                    {
                        *existing = option.clone();
                    } else {
                        config_options.push(option.clone());
                    }
                }
                ConfigOptionPatch::Merge {
                    key,
                    name,
                    options,
                    current,
                } => {
                    let Some(existing) = config_options
                        .iter_mut()
                        .find(|existing| &existing.key == key)
                    else {
                        tracing::debug!(
                            key,
                            "capability overlay merge target is missing; ignoring"
                        );
                        continue;
                    };
                    if let Some(name) = name {
                        existing.name = name.clone();
                    }
                    if let Some(options) = options {
                        existing.options = options.clone();
                    }
                    if let Some(current) = current {
                        existing.current_value = current.clone();
                    }
                }
                ConfigOptionPatch::Remove { key } => {
                    config_options.retain(|existing| &existing.key != key);
                }
            }
        }

        for command in &self.add_commands {
            if !available_commands
                .iter()
                .any(|existing| existing.name == command.name)
            {
                available_commands.push(command.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::common::ConfigOptionCategory;

    fn option(key: &str, name: &str, values: &[&str]) -> ConfigOptionInfo {
        ConfigOptionInfo {
            key: key.to_owned(),
            name: name.to_owned(),
            description: None,
            category: Some(ConfigOptionCategory::Model),
            current_value: ConfigOptionValue::Select(values[0].to_owned()),
            options: values
                .iter()
                .map(|value| ConfigOptionValue::Select((*value).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn applies_modes_and_commands() {
        let overlay = CapabilityOverlay {
            modes: Some(vec![ModeInfo {
                mode_id: "plan".to_owned(),
                name: "Plan".to_owned(),
                description: None,
            }]),
            current_mode: Some("plan".to_owned()),
            config_options: Vec::new(),
            add_commands: vec![CommandInfo {
                name: "compact".to_owned(),
                description: "Compact context".to_owned(),
                input_hint: None,
            }],
        };
        let mut current = None;
        let mut modes = Vec::new();
        let mut commands = Vec::new();
        let mut config = Vec::new();
        overlay.apply(&mut current, &mut modes, &mut commands, &mut config);
        assert_eq!(current.as_deref(), Some("plan"));
        assert_eq!(modes.len(), 1);
        assert_eq!(commands[0].name, "compact");
    }

    #[test]
    fn merges_replaces_and_removes_config_options() {
        let overlay = CapabilityOverlay {
            modes: None,
            current_mode: None,
            config_options: vec![
                ConfigOptionPatch::Merge {
                    key: "model".to_owned(),
                    name: Some("Model (enriched)".to_owned()),
                    options: Some(vec![
                        ConfigOptionValue::Select("fast".to_owned()),
                        ConfigOptionValue::Select("smart".to_owned()),
                    ]),
                    current: Some(ConfigOptionValue::Select("smart".to_owned())),
                },
                ConfigOptionPatch::Replace(option("reasoning", "Reasoning", &["low", "high"])),
                ConfigOptionPatch::Remove {
                    key: "obsolete".to_owned(),
                },
            ],
            add_commands: Vec::new(),
        };
        let mut config = vec![
            option("model", "Model", &["fast", "smart"]),
            option("obsolete", "Obsolete", &["x"]),
        ];
        overlay.apply(&mut None, &mut Vec::new(), &mut Vec::new(), &mut config);
        assert_eq!(config.len(), 2);
        let model = config
            .iter()
            .find(|entry| entry.key == "model")
            .expect("model");
        assert_eq!(model.name, "Model (enriched)");
        assert_eq!(model.options.len(), 2);
        assert!(config.iter().any(|entry| entry.key == "reasoning"));
        assert!(!config.iter().any(|entry| entry.key == "obsolete"));
    }

    #[test]
    fn default_capabilities_are_per_launch_and_keep_unknown_names() {
        // Subagents are advertised by default; an agent definition can replace
        // the set, and a name from a newer build survives the round trip.
        let default = AcpExtensionOptions::default();
        assert!(default.capabilities.contains(&AcpCapability::Subagents));

        let disabled = AcpExtensionOptions {
            capabilities: AcpCapabilitySet::new(),
            ..Default::default()
        };
        assert!(!disabled.capabilities.contains(&AcpCapability::Subagents));

        assert_eq!(
            AcpCapability::from_name("future_thing"),
            AcpCapability::Unknown("future_thing".to_owned())
        );
    }
}
