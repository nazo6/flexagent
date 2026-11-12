//! OpenCode-specific extension (Tier 2).
//!
//! ACP already exposes OpenCode's models, but the model value is a raw
//! `provider/model` ID. This extension adds a human-readable label and a
//! secondary provider label without changing the value sent back to ACP.
//!
//! Design: `docs/changelog/2026-11-11-agent-extension-phase4.md`.

use std::sync::Arc;

use async_trait::async_trait;
use fxg_protocol::common::{ConfigOptionCategory, ConfigOptionInfo, ConfigOptionValue, RawJson};
use fxg_protocol::events::{SubagentStatus, UnifiedEventPayload};

use super::extension::{
    AcpChildUpdate, AcpExtensionOptions, AcpNotificationParser, AgentExtension, AgentMetadata,
    CapabilitiesSnapshot, CapabilityOverlay, ConfigOptionPatch, EventDisposition, SessionExtension,
};
use crate::driver::DriverEvent;

/// OpenCode ACP extension.
#[derive(Debug, Default)]
pub struct OpenCodeExtension;

/// OpenCode V2 ACP extension.
///
/// The v2 registry entry launches the same `opencode acp` binary, so it shares
/// the model label conversion but keeps its own display name.
#[derive(Debug, Default)]
pub struct OpenCodeV2Extension;

#[async_trait]
impl AgentExtension for OpenCodeExtension {
    fn metadata(&self) -> AgentMetadata {
        AgentMetadata {
            name: Some("OpenCode".to_owned()),
            description: Some(
                "OpenCode via ACP, with readable model labels provided by fxg".to_owned(),
            ),
            icon: Some("builtin:opencode".to_owned()),
        }
    }

    fn acp_options(&self) -> AcpExtensionOptions {
        opencode_acp_options()
    }

    fn notification_parser(&self) -> Option<Arc<dyn AcpNotificationParser>> {
        Some(Arc::new(OpenCodeNotificationParser))
    }

    async fn capabilities(
        &self,
        _ctx: &super::extension::SessionContext,
        current: &CapabilitiesSnapshot,
    ) -> anyhow::Result<Option<CapabilityOverlay>> {
        model_label_overlay(current)
    }

    async fn open_session(
        &self,
        _ctx: &super::extension::SessionContext,
    ) -> anyhow::Result<Option<Arc<dyn SessionExtension>>> {
        Ok(Some(Arc::new(OpenCodeSessionExtension)))
    }
}

#[async_trait]
impl AgentExtension for OpenCodeV2Extension {
    fn metadata(&self) -> AgentMetadata {
        AgentMetadata {
            name: Some("OpenCode V2".to_owned()),
            description: Some(
                "OpenCode V2 via ACP, with readable model labels provided by fxg".to_owned(),
            ),
            icon: Some("builtin:opencode".to_owned()),
        }
    }

    fn acp_options(&self) -> AcpExtensionOptions {
        opencode_acp_options()
    }

    fn notification_parser(&self) -> Option<Arc<dyn AcpNotificationParser>> {
        Some(Arc::new(OpenCodeNotificationParser))
    }

    async fn capabilities(
        &self,
        _ctx: &super::extension::SessionContext,
        current: &CapabilitiesSnapshot,
    ) -> anyhow::Result<Option<CapabilityOverlay>> {
        model_label_overlay(current)
    }

    async fn open_session(
        &self,
        _ctx: &super::extension::SessionContext,
    ) -> anyhow::Result<Option<Arc<dyn SessionExtension>>> {
        Ok(Some(Arc::new(OpenCodeSessionExtension)))
    }
}

/// OpenCode vendor capability and notification declaration.
fn opencode_acp_options() -> AcpExtensionOptions {
    AcpExtensionOptions {
        client_meta: Some(RawJson::new(serde_json::json!({
            "opencode/child-session-updates": true
        }))),
        notifications: vec!["opencode/session/child_update".to_owned()],
        ..Default::default()
    }
}

/// Parser for `opencode/session/child_update` notifications.
#[derive(Debug, Default)]
struct OpenCodeNotificationParser;

impl AcpNotificationParser for OpenCodeNotificationParser {
    fn methods(&self) -> &'static [&'static str] {
        &["opencode/session/child_update"]
    }

    fn parse(&self, _method: &str, params: &RawJson) -> Option<AcpChildUpdate> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct ChildUpdate {
            #[allow(dead_code)]
            root_session_id: String,
            child_session_id: String,
            parent_session_id: Option<String>,
            #[serde(default)]
            depth: u32,
            title: Option<String>,
            #[serde(flatten)]
            kind: ChildUpdateKind,
        }

        #[derive(serde::Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum ChildUpdateKind {
            Update {
                update: Box<agent_client_protocol::v1::schema::SessionUpdate>,
            },
            Status {
                status: String,
            },
        }

        let Ok(payload) = serde_json::from_value::<ChildUpdate>(params.as_value().clone()) else {
            return None;
        };
        let (state, update) = match payload.kind {
            ChildUpdateKind::Update { update } => (None, Some(*update)),
            ChildUpdateKind::Status { status } => (Some(map_child_status(&status)), None),
        };
        Some(AcpChildUpdate {
            child_session_id: payload.child_session_id,
            parent_session_id: payload.parent_session_id,
            depth: payload.depth,
            title: payload.title,
            cwd: None,
            state,
            update,
        })
    }
}

/// Maps OpenCode child status strings to the normalized status.
fn map_child_status(status: &str) -> SubagentStatus {
    match status {
        "created" | "running" => SubagentStatus::Running,
        "completed" => SubagentStatus::Completed,
        "interrupted" | "cancelled" => SubagentStatus::Cancelled,
        "failed" => SubagentStatus::Failed,
        other => SubagentStatus::Unknown(other.to_owned()),
    }
}

/// Tags child permission requests with the vendor `_meta` child ID.
#[derive(Debug, Default)]
struct OpenCodeSessionExtension;

#[async_trait]
impl SessionExtension for OpenCodeSessionExtension {
    async fn on_event(&self, event: DriverEvent) -> anyhow::Result<EventDisposition> {
        let DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
            subagent_id: None,
            details,
            ..
        }) = &event
        else {
            return Ok(EventDisposition::Emit);
        };
        let Some(child_id) = details
            .as_value()
            .get("_meta")
            .and_then(|meta| meta.get("opencode/child-session"))
            .and_then(|child| child.get("id"))
            .and_then(|id| id.as_str())
        else {
            return Ok(EventDisposition::Emit);
        };
        let child_id = child_id.to_owned();
        let DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
            request_id,
            tool_name,
            summary,
            options,
            details,
            subagent_id: _,
        }) = event
        else {
            unreachable!("matched above");
        };
        Ok(EventDisposition::Replace(Box::new(DriverEvent::Event(
            UnifiedEventPayload::PermissionRequest {
                request_id,
                tool_name,
                summary,
                options,
                details,
                subagent_id: Some(child_id),
            },
        ))))
    }
}

/// Replaces model option values with readable labels.
fn model_label_overlay(
    current: &CapabilitiesSnapshot,
) -> anyhow::Result<Option<CapabilityOverlay>> {
    let mut patches = Vec::new();
    for option in &current.config_options {
        if !is_model_option(option) {
            continue;
        }
        let mut patched = option.clone();
        patched.options = option.options.iter().map(transform_model_value).collect();
        patched.current_value = transform_model_value(&option.current_value);
        patches.push(ConfigOptionPatch::Replace(patched));
    }
    if patches.is_empty() {
        return Ok(None);
    }
    Ok(Some(CapabilityOverlay {
        config_options: patches,
        ..Default::default()
    }))
}

/// Whether the option is OpenCode's model selector.
fn is_model_option(option: &ConfigOptionInfo) -> bool {
    option.category.as_ref() == Some(&ConfigOptionCategory::Model)
        || (option.category.is_none() && option.key == "model")
}

/// Converts `provider/model` into a display value with a readable name.
///
/// The `value` field is kept verbatim so `session/set_config_option` receives
/// the original ACP ID. `name` is used by the UI for the primary label and
/// `providerID` for the secondary label.
fn transform_model_value(value: &ConfigOptionValue) -> ConfigOptionValue {
    let ConfigOptionValue::Select(id) = value else {
        return value.clone();
    };
    let Some((provider, model)) = id.split_once('/') else {
        return value.clone();
    };
    if provider.is_empty() || model.is_empty() {
        return value.clone();
    }
    ConfigOptionValue::Unknown(RawJson::new(serde_json::json!({
        "value": id,
        "name": display_model_name(model),
        "providerID": provider,
        "id": model,
    })))
}

/// Turns `gpt-6-luna` into `GPT 6 Luna` while keeping version tokens intact.
fn display_model_name(model: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for token in model.split('-') {
        let token = title_token(token);
        if token.chars().all(|ch| ch.is_ascii_digit())
            && parts
                .last()
                .is_some_and(|previous| previous.chars().all(|ch| ch.is_ascii_digit()))
        {
            if let Some(previous) = parts.last_mut() {
                previous.push('.');
                previous.push_str(&token);
            }
        } else {
            parts.push(token);
        }
    }
    parts.join(" ")
}

/// Title-cases one token, preserving common acronyms and brand casing.
fn title_token(token: &str) -> String {
    if token.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        && token
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
    {
        let mut result = token.to_owned();
        if let Some(last) = result.pop() {
            result.push(last.to_ascii_uppercase());
        }
        return result;
    }
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "gpt" => "GPT".to_owned(),
        "glm" => "GLM".to_owned(),
        "vl" => "VL".to_owned(),
        "deepseek" => "DeepSeek".to_owned(),
        "minimax" => "MiniMax".to_owned(),
        "mimo" => "MiMo".to_owned(),
        "longcat" => "LongCat".to_owned(),
        "qwen" => "Qwen".to_owned(),
        "claude" => "Claude".to_owned(),
        "gemini" => "Gemini".to_owned(),
        "kimi" => "Kimi".to_owned(),
        "grok" => "Grok".to_owned(),
        "nemotron" => "Nemotron".to_owned(),
        "inkling" => "Inkling".to_owned(),
        "flint" => "Flint".to_owned(),
        "iceman" => "Iceman".to_owned(),
        "muse" => "Muse".to_owned(),
        "spark" => "Spark".to_owned(),
        "argon" => "Argon".to_owned(),
        "luna" => "Luna".to_owned(),
        "preview" => "Preview".to_owned(),
        "contributor" => "Contributor".to_owned(),
        "lightning" => "Lightning".to_owned(),
        "super" => "Super".to_owned(),
        "small" => "Small".to_owned(),
        "flash" => "Flash".to_owned(),
        "pro" => "Pro".to_owned(),
        "plus" => "Plus".to_owned(),
        "max" => "Max".to_owned(),
        "mini" => "Mini".to_owned(),
        "free" => "Free".to_owned(),
        "big" => "Big".to_owned(),
        "pickle" => "Pickle".to_owned(),
        "beam" => "Beam".to_owned(),
        _ => {
            let mut chars = token.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            format!("{}{}", first.to_uppercase(), chars.as_str())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::v1::schema::SessionConfigOptionValue;
    use fxg_protocol::common::{ConfigOptionValue, ModeInfo};

    #[test]
    fn v2_metadata_keeps_its_own_name() {
        let metadata = OpenCodeV2Extension.metadata();
        assert_eq!(metadata.name.as_deref(), Some("OpenCode V2"));
        assert_eq!(metadata.icon.as_deref(), Some("builtin:opencode"));
    }

    fn option() -> ConfigOptionInfo {
        ConfigOptionInfo {
            key: "model".to_owned(),
            name: "Model".to_owned(),
            description: None,
            category: Some(ConfigOptionCategory::Model),
            current_value: ConfigOptionValue::Select("opencode-go/gpt-6-luna".to_owned()),
            options: vec![
                ConfigOptionValue::Select("opencode/beam-free".to_owned()),
                ConfigOptionValue::Select("opencode-go/gpt-6-luna".to_owned()),
            ],
        }
    }

    #[test]
    fn transforms_model_labels_without_changing_values() {
        let transformed = transform_model_value(&ConfigOptionValue::Select(
            "opencode-go/deepseek-v4.1-flash".to_owned(),
        ));
        let ConfigOptionValue::Unknown(raw) = transformed else {
            panic!("expected an object value");
        };
        let value = raw.as_value();
        assert_eq!(value["value"], "opencode-go/deepseek-v4.1-flash");
        assert_eq!(value["name"], "DeepSeek V4.1 Flash");
        assert_eq!(value["providerID"], "opencode-go");
        assert_eq!(value["id"], "deepseek-v4.1-flash");

        // The value must still deserialize into an ACP value for set_config.
        let parsed: SessionConfigOptionValue =
            serde_json::from_value(value.clone()).expect("ACP value");
        assert_eq!(
            parsed.as_value_id().map(ToString::to_string).as_deref(),
            Some("opencode-go/deepseek-v4.1-flash")
        );
    }

    #[test]
    fn leaves_non_model_values_untouched() {
        let value = ConfigOptionValue::Select("plan".to_owned());
        assert_eq!(transform_model_value(&value), value);
        let boolean = ConfigOptionValue::Boolean(true);
        assert_eq!(transform_model_value(&boolean), boolean);
    }

    #[test]
    fn display_name_keeps_version_tokens() {
        assert_eq!(display_model_name("gpt-6-luna"), "GPT 6 Luna");
        assert_eq!(display_model_name("glm-5.3-flash"), "GLM 5.3 Flash");
        assert_eq!(display_model_name("qwen3.8-max"), "Qwen3.8 Max");
        assert_eq!(display_model_name("claude-haiku-5-5"), "Claude Haiku 5.5");
        assert_eq!(display_model_name("flint-40b-free"), "Flint 40B Free");
    }

    #[tokio::test]
    async fn overlay_replaces_only_the_model_option() {
        let current = CapabilitiesSnapshot {
            config_options: vec![
                option(),
                ConfigOptionInfo {
                    key: "mode".to_owned(),
                    name: "Mode".to_owned(),
                    description: None,
                    category: Some(ConfigOptionCategory::Mode),
                    current_value: ConfigOptionValue::Select("code".to_owned()),
                    options: vec![ConfigOptionValue::Select("code".to_owned())],
                },
            ],
            available_modes: vec![ModeInfo {
                mode_id: "code".to_owned(),
                name: "Code".to_owned(),
                description: None,
            }],
            ..Default::default()
        };
        let overlay = OpenCodeExtension
            .capabilities(
                &super::super::extension::SessionContext {
                    session_id: "s".to_owned(),
                    agent_id: "opencode".to_owned(),
                    agent_session_id: None,
                    cwd: std::path::PathBuf::from("."),
                    launch: crate::driver::AgentLaunchSpec {
                        agent_id: "opencode".to_owned(),
                        display_name: "OpenCode".to_owned(),
                        driver_kind: crate::driver::DriverKind::AcpExtended,
                        extension: Some("opencode".to_owned()),
                        program: Some(std::path::PathBuf::from("opencode")),
                        args: vec!["acp".to_owned()],
                        env: Vec::new(),
                        acp: Default::default(),
                    },
                },
                &current,
            )
            .await
            .expect("capabilities")
            .expect("overlay");
        assert_eq!(overlay.config_options.len(), 1);
        let ConfigOptionPatch::Replace(patched) = &overlay.config_options[0] else {
            panic!("expected a replace patch");
        };
        assert_eq!(patched.key, "model");
        assert!(matches!(patched.options[0], ConfigOptionValue::Unknown(_)));
    }
}
