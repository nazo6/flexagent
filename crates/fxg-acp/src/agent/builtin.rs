//! Built-in agent providers.
//!
//! Keep this list small and verified. A provider is needed when the ACP
//! Registry launch recipe differs from the user-facing CLI (for example
//! `antigravity-acp` expects a bundled `agy_acp_server.par`, while the `agy`
//! CLI exposes `agy acp`), when a native CLI should be used instead of `npx`,
//! or when fxg applies an extension.

use std::sync::Arc;

use super::opencode::OpenCodeExtension;
use super::provider::{AgentBackend, AgentDefinition, AgentProvider, LaunchPreset, StaticProvider};
use crate::opencode_native::OpenCodeNativeDriver;

/// OpenCode native driver (Tier 3, OpenCode v2 HTTP + SSE).
pub const OPENCODE: &str = "opencode";
/// OpenCode through ACP, kept as a fallback for older CLI builds.
pub const OPENCODE_ACP: &str = "opencode-acp";
/// Extension ID applied to [`OPENCODE_ACP`].
pub const OPENCODE_EXTENSION: &str = "opencode";
/// ACP-only Antigravity CLI (`agy acp`).
pub const AGY: &str = "agy";
/// ACP-only GitHub Copilot CLI (`copilot --acp`).
pub const COPILOT: &str = "github-copilot-cli";
/// ACP-only Qwen Code CLI (`qwen --acp`).
pub const QWEN: &str = "qwen-code";
/// ACP-only Cline CLI (`cline --acp`).
pub const CLINE: &str = "cline";
/// ACP-only Auggie CLI (`auggie --acp`).
pub const AUGGIE: &str = "auggie";
/// ACP-only Kimi Code CLI (`kimi acp`).
pub const KIMI: &str = "kimi-code";

/// Returns the built-in providers.
pub fn providers() -> Vec<Arc<dyn AgentProvider>> {
    vec![
        Arc::new(
            StaticProvider::new(
                AgentDefinition {
                    id: OPENCODE.to_owned(),
                    name: "OpenCode".to_owned(),
                    description: Some(
                        "OpenCode v2 native driver (revert, fork, steering, MCP)".to_owned(),
                    ),
                    icon: Some("builtin:opencode".to_owned()),
                },
                Some(LaunchPreset {
                    program: OPENCODE,
                    args: &["serve"],
                }),
            )
            .with_backend(AgentBackend::Custom(Arc::new(OpenCodeNativeDriver::new()))),
        ),
        Arc::new(
            StaticProvider::new(
                AgentDefinition {
                    id: OPENCODE_ACP.to_owned(),
                    name: "OpenCode (ACP)".to_owned(),
                    description: Some("OpenCode through ACP (fallback)".to_owned()),
                    icon: Some("builtin:opencode".to_owned()),
                },
                Some(LaunchPreset {
                    program: OPENCODE,
                    args: &["acp"],
                }),
            )
            .with_extension(OPENCODE_EXTENSION, Arc::new(OpenCodeExtension)),
        ),
        acp_provider(
            AGY,
            "Antigravity",
            "Known ACP CLI",
            AGY,
            &["acp"],
            "builtin:antigravity",
        ),
        acp_provider(
            COPILOT,
            "GitHub Copilot",
            "GitHub Copilot CLI in ACP mode",
            "copilot",
            &["--acp"],
            "builtin:copilot",
        ),
        acp_provider(
            QWEN,
            "Qwen Code",
            "Qwen Code CLI in ACP mode",
            "qwen",
            &["--acp"],
            "builtin:qwen",
        ),
        acp_provider(
            CLINE,
            "Cline",
            "Cline CLI in ACP mode",
            "cline",
            &["--acp"],
            "builtin:cline",
        ),
        acp_provider(
            AUGGIE,
            "Auggie CLI",
            "Augment Code Auggie in ACP mode",
            "auggie",
            &["--acp"],
            "builtin:auggie",
        ),
        acp_provider(
            KIMI,
            "Kimi Code",
            "Moonshot Kimi Code ACP server",
            "kimi",
            &["acp"],
            "builtin:kimi",
        ),
    ]
}

/// Builds an ACP-only provider with a native CLI preset.
fn acp_provider(
    id: &str,
    name: &str,
    description: &str,
    program: &'static str,
    args: &'static [&'static str],
    icon: &str,
) -> Arc<dyn AgentProvider> {
    Arc::new(StaticProvider::new(
        AgentDefinition {
            id: id.to_owned(),
            name: name.to_owned(),
            description: Some(description.to_owned()),
            icon: Some(icon.to_owned()),
        },
        Some(LaunchPreset { program, args }),
    ))
}
