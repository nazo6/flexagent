//! Built-in agent providers.
//!
//! Keep this list small and verified. Agents distributed through the ACP
//! Registry do not need a provider unless their registry `cmd` differs from the
//! user-facing CLI (for example `antigravity-acp` expects a bundled
//! `agy_acp_server.par`, while the `agy` CLI exposes `agy acp`).

use std::sync::Arc;

use super::opencode::OpenCodeExtension;
use super::provider::{AgentBackend, AgentDefinition, AgentProvider, LaunchPreset, StaticProvider};

/// OpenCode CLI (`opencode acp`) with the OpenCode extension.
pub const OPENCODE: &str = "opencode";
/// ACP-only Antigravity CLI (`agy acp`).
pub const AGY: &str = "agy";

/// Returns the built-in providers.
pub fn providers() -> Vec<Arc<dyn AgentProvider>> {
    vec![
        Arc::new(
            StaticProvider::new(
                AgentDefinition {
                    id: OPENCODE.to_owned(),
                    name: "OpenCode".to_owned(),
                    description: Some("Known ACP CLI".to_owned()),
                    icon: Some("builtin:opencode".to_owned()),
                },
                Some(LaunchPreset {
                    program: OPENCODE,
                    args: &["acp"],
                }),
            )
            .with_backend(AgentBackend::AcpExtended(Arc::new(OpenCodeExtension))),
        ),
        Arc::new(StaticProvider::new(
            AgentDefinition {
                id: AGY.to_owned(),
                name: "Antigravity".to_owned(),
                description: Some("Known ACP CLI".to_owned()),
                icon: None,
            },
            Some(LaunchPreset {
                program: AGY,
                args: &["acp"],
            }),
        )),
    ]
}
