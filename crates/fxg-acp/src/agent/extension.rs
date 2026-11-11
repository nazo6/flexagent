//! ACP extension traits and capability overrides (Tier 2).
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.1 and
//! `docs/changelog/2026-11-11-agent-extension-architecture.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use fxg_protocol::common::{
    CommandInfo, ConfigOptionInfo, ConfigOptionValue, ModeInfo, PromptInput,
};

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

    /// Override / augment what ACP advertises (models, modes, config options).
    ///
    /// Applied to both capability probe results and live `CapabilitiesUpdated`
    /// events. The overlay is computed once per ACP session.
    async fn capabilities(
        &self,
        _ctx: &SessionContext,
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
}
