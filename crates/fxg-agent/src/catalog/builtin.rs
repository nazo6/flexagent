//! Built-in agent definitions and launch presets.
//!
//! Keep this list small and verified. A built-in agent is needed when the ACP
//! Registry launch recipe differs from the user-facing CLI (for example
//! `antigravity-acp` expects a bundled `agy_acp_server.par`, while the `agy`
//! CLI exposes `agy acp`), when a native CLI should be used instead of `npx`,
//! or when fxg applies an extension.

use std::sync::Arc;

use fxg_protocol::client_api::AgentBackendKind;
use fxg_protocol::common::AgentAuthMethodInfo;

use crate::acp::extensions::AgentExtension;
use crate::acp::extensions::antigravity::AntigravityExtension;
use crate::acp::extensions::opencode::OpenCodeExtension;
use crate::driver::AgentDriver;
use crate::management::AgentManagement;
use crate::opencode::{OpenCodeNativeDriver, auth as opencode_auth};

/// OpenCode native driver (Tier 3, OpenCode v2 HTTP + SSE).
pub const OPENCODE: &str = "opencode";
/// OpenCode through ACP, kept as a fallback for older CLI builds.
pub const OPENCODE_ACP: &str = "opencode-acp";
/// Extension ID applied to [`OPENCODE_ACP`].
pub const OPENCODE_EXTENSION: &str = "opencode";
/// ACP-only Antigravity CLI (`agy acp`).
pub const AGY: &str = "agy";
/// Extension ID applied to [`AGY`].
pub const ANTIGRAVITY_EXTENSION: &str = "antigravity";
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

/// Static information needed even when the agent is not running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    /// Stable agent ID (used by `fxg session new` and `AgentSummary.id`)
    pub id: String,
    /// Display name
    pub name: String,
    /// Description shown in the catalog
    pub description: Option<String>,
    /// Icon URL or built-in asset key
    pub icon: Option<String>,
}

/// A built-in launch preset for a well-known ACP CLI.
///
/// A preset is not a distribution channel: it only supplies the correct
/// arguments when the CLI is already installed on `PATH` (for example
/// `agy acp`). User custom definitions always take priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchPreset {
    /// Executable name on `PATH` (resolved with `which` / `PATHEXT`)
    pub program: &'static str,
    /// Arguments that start the CLI's ACP server
    pub args: &'static [&'static str],
}

/// An interactive command owned by an agent (native login / logout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCommand {
    /// Executable name on `PATH`
    pub program: &'static str,
    /// Command arguments
    pub args: &'static [&'static str],
}

/// Authentication surface of a native agent.
///
/// ACP agents discover their methods from `initialize`; native drivers have no
/// handshake, so they declare the same terminal login / logout commands here.
#[derive(Debug, Clone)]
pub struct AgentAuth {
    /// Methods shown by `fxg agents login`
    pub methods: Vec<AgentAuthMethodInfo>,
    /// Interactive login command
    pub login: Option<AgentCommand>,
    /// Interactive logout command
    pub logout: Option<AgentCommand>,
}

/// How an agent is implemented at runtime.
#[derive(Clone)]
pub enum AgentBackend {
    /// ACP only (`AcpDriver`)
    Acp,
    /// ACP plus an fxg-side extension (`AcpDriver` with an extension)
    AcpExtended {
        /// Stable built-in extension ID (custom definitions reference extensions by this ID)
        id: &'static str,
        /// Extension implementation
        extension: Arc<dyn AgentExtension>,
    },
    /// Fully custom driver
    Custom(Arc<dyn AgentDriver>),
}

impl AgentBackend {
    /// Protocol-level representation exposed to clients.
    pub fn kind(&self) -> AgentBackendKind {
        match self {
            Self::Acp => AgentBackendKind::Acp,
            Self::AcpExtended { .. } => AgentBackendKind::AcpExtended,
            Self::Custom(_) => AgentBackendKind::Custom,
        }
    }

    /// Stable built-in extension ID when [`Self::AcpExtended`] is used.
    pub fn extension_id(&self) -> Option<&'static str> {
        match self {
            Self::AcpExtended { id, .. } => Some(id),
            Self::Acp | Self::Custom(_) => None,
        }
    }
}

/// One built-in agent: its definition, launch preset and runtime backend.
pub struct BuiltinAgent {
    definition: AgentDefinition,
    preset: Option<LaunchPreset>,
    backend: AgentBackend,
    auth: Option<AgentAuth>,
}

impl BuiltinAgent {
    /// Creates an ACP-only built-in agent.
    pub fn new(definition: AgentDefinition, preset: Option<LaunchPreset>) -> Self {
        Self {
            definition,
            preset,
            backend: AgentBackend::Acp,
            auth: None,
        }
    }

    /// Replaces the runtime backend (for example a custom driver).
    ///
    /// Agents that declare an extension should prefer [`Self::with_extension`]
    /// so the adapters and the extension ID stay in sync.
    pub fn with_backend(mut self, backend: AgentBackend) -> Self {
        self.backend = backend;
        self
    }

    /// Applies a built-in extension and records its stable ID.
    pub fn with_extension(
        mut self,
        extension_id: &'static str,
        extension: Arc<dyn AgentExtension>,
    ) -> Self {
        self.backend = AgentBackend::AcpExtended {
            id: extension_id,
            extension,
        };
        self
    }

    /// Declares native authentication (terminal login / logout).
    pub fn with_auth(mut self, auth: AgentAuth) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Static definition (ID, display name, icon, ...).
    pub fn definition(&self) -> &AgentDefinition {
        &self.definition
    }

    /// Built-in launch preset, or `None` to use the ACP Registry launch.
    pub fn preset(&self) -> Option<&LaunchPreset> {
        self.preset.as_ref()
    }

    /// Runtime backend used for this agent.
    pub fn backend(&self) -> &AgentBackend {
        &self.backend
    }

    /// Native management integration (`None` = no out-of-session configuration).
    pub fn management(&self) -> Option<Arc<dyn AgentManagement>> {
        match &self.backend {
            AgentBackend::Custom(driver) => driver.management(),
            AgentBackend::Acp | AgentBackend::AcpExtended { .. } => None,
        }
    }

    /// Native authentication surface (`None` = discover ACP auth methods).
    pub fn auth(&self) -> Option<&AgentAuth> {
        self.auth.as_ref()
    }
}

/// Returns the built-in agents.
pub fn agents() -> Vec<BuiltinAgent> {
    let opencode = Arc::new(OpenCodeNativeDriver::new());
    vec![
        BuiltinAgent::new(
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
        .with_backend(AgentBackend::Custom(opencode))
        .with_auth(opencode_auth()),
        BuiltinAgent::new(
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
        acp_agent(
            AGY,
            "Antigravity",
            "Known ACP CLI",
            AGY,
            &["acp"],
            "builtin:antigravity",
        )
        .with_extension(
            ANTIGRAVITY_EXTENSION,
            Arc::new(AntigravityExtension::default()),
        ),
        acp_agent(
            COPILOT,
            "GitHub Copilot",
            "GitHub Copilot CLI in ACP mode",
            "copilot",
            &["--acp"],
            "builtin:copilot",
        ),
        acp_agent(
            QWEN,
            "Qwen Code",
            "Qwen Code CLI in ACP mode",
            "qwen",
            &["--acp"],
            "builtin:qwen",
        ),
        acp_agent(
            CLINE,
            "Cline",
            "Cline CLI in ACP mode",
            "cline",
            &["--acp"],
            "builtin:cline",
        ),
        acp_agent(
            AUGGIE,
            "Auggie CLI",
            "Augment Code Auggie in ACP mode",
            "auggie",
            &["--acp"],
            "builtin:auggie",
        ),
        acp_agent(
            KIMI,
            "Kimi Code",
            "Moonshot Kimi Code ACP server",
            "kimi",
            &["acp"],
            "builtin:kimi",
        ),
    ]
}

/// Builds an ACP-only agent with a native CLI preset.
fn acp_agent(
    id: &str,
    name: &str,
    description: &str,
    program: &'static str,
    args: &'static [&'static str],
    icon: &str,
) -> BuiltinAgent {
    BuiltinAgent::new(
        AgentDefinition {
            id: id.to_owned(),
            name: name.to_owned(),
            description: Some(description.to_owned()),
            icon: Some(icon.to_owned()),
        },
        Some(LaunchPreset { program, args }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_opencode_declares_terminal_login() {
        let agents = agents();
        let opencode = agents
            .iter()
            .find(|agent| agent.definition().id == OPENCODE)
            .expect("opencode agent");
        let auth = opencode.auth().expect("native auth");
        assert_eq!(auth.methods.len(), 1);
        assert_eq!(
            auth.methods[0].kind,
            fxg_protocol::common::AgentAuthMethodKind::Terminal
        );
        assert_eq!(auth.login.expect("login").args, &["auth", "login"]);
        assert_eq!(auth.logout.expect("logout").args, &["auth", "logout"]);
    }

    #[test]
    fn acp_agents_have_no_native_auth() {
        let agents = agents();
        let acp = agents
            .iter()
            .find(|agent| agent.definition().id == OPENCODE_ACP)
            .expect("opencode-acp agent");
        assert!(acp.auth().is_none());
    }
}
