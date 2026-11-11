//! Agent management actions and authentication methods.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use specta::Type;

crate::open_string_enum! {
    /// Authentication method kind (`agent` / `terminal`).
    pub enum AgentAuthMethodKind {
        /// The agent handles authentication through `authenticate`.
        Agent => "agent",
        /// The client runs the agent CLI interactively (`--login` etc.).
        Terminal => "terminal",
    }
}

/// An authentication method advertised by an ACP agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentAuthMethodInfo {
    /// Method ID passed to `authenticate` (agent methods) or selected locally (terminal)
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Optional description shown in the login UI
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How the method is executed
    pub kind: AgentAuthMethodKind,
    /// Additional arguments for terminal methods (appended to the launch command)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Additional environment variables for terminal methods
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// Agent management action (shared by the `ManageAgent` command and the Client API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "action", rename_all = "snake_case")]
#[specta(collect)]
pub enum AgentAction {
    /// Force a refresh of the ACP Registry catalog index (`fxg agents update`)
    RefreshCatalog,
    /// Create or update a UI-managed custom agent definition
    UpsertCustom {
        /// Agent ID
        agent_id: String,
        /// Command, arguments and environment variables
        config: Box<crate::settings::CustomAgentConfig>,
    },
    /// Delete a UI-managed custom agent definition
    RemoveCustom {
        /// Agent ID
        agent_id: String,
    },
    /// Import an ACP Registry entry as a custom agent definition
    ImportRegistryAgent {
        /// Registry agent ID
        agent_id: String,
        /// Built-in extension ID to apply (optional)
        extension: Option<String>,
    },
}
