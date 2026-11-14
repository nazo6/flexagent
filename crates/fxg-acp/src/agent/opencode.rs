//! OpenCode-specific extension (Tier 2).
//!
//! OpenCode reports model labels through ACP, so this extension only adds the
//! vendor pieces that ACP does not expose: subagent child updates and the
//! child id on permission requests.
//!
//! Design: `docs/changelog/2026-11/2026-11-11-agent-extension-phase4.md`.

use std::sync::Arc;

use async_trait::async_trait;
use fxg_protocol::common::RawJson;
use fxg_protocol::events::{SubagentStatus, UnifiedEventPayload};

use super::extension::{
    AcpChildUpdate, AcpExtensionOptions, AcpNotificationParser, AgentExtension, AgentMetadata,
    EventDisposition, SessionExtension,
};
use crate::driver::DriverEvent;

/// OpenCode ACP extension.
#[derive(Debug, Default)]
pub struct OpenCodeExtension;

#[async_trait]
impl AgentExtension for OpenCodeExtension {
    fn metadata(&self) -> AgentMetadata {
        AgentMetadata {
            name: Some("OpenCode".to_owned()),
            description: Some(
                "OpenCode via ACP, with subagent updates and permission tagging".to_owned(),
            ),
            icon: Some("builtin:opencode".to_owned()),
        }
    }

    fn acp_options(
        &self,
        capabilities: &fxg_protocol::settings::AgentAcpCapabilities,
    ) -> AcpExtensionOptions {
        opencode_acp_options(capabilities)
    }

    fn notification_parser(&self) -> Option<Arc<dyn AcpNotificationParser>> {
        Some(Arc::new(OpenCodeNotificationParser))
    }

    async fn open_session(
        &self,
        _ctx: &super::extension::SessionContext,
    ) -> anyhow::Result<Option<Arc<dyn SessionExtension>>> {
        Ok(Some(Arc::new(OpenCodeSessionExtension)))
    }
}

/// OpenCode vendor capability and notification declaration.
///
/// OpenCode exposes child sessions through its vendor `_meta` rather than the
/// standard ACP capability, so the per-agent `subagents` toggle gates both.
fn opencode_acp_options(
    capabilities: &fxg_protocol::settings::AgentAcpCapabilities,
) -> AcpExtensionOptions {
    if !capabilities.subagents {
        return AcpExtensionOptions::default();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_uses_the_opencode_name() {
        let metadata = OpenCodeExtension.metadata();
        assert_eq!(metadata.name.as_deref(), Some("OpenCode"));
        assert_eq!(metadata.icon.as_deref(), Some("builtin:opencode"));
    }
}
