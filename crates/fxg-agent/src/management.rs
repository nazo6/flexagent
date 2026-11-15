//! Generic agent-native management seam.
//!
//! Session lifecycle is described by [`crate::driver::AgentDriver`]. Some
//! agents additionally expose configuration outside the session protocol
//! (providers, MCP servers, skills, commands). Those integrations implement
//! [`AgentManagement`] and let their [`BuiltinAgent`] expose it, so the node
//! API and UI do not need agent-specific route names.
//!
//! [`BuiltinAgent`]: crate::catalog::builtin::BuiltinAgent

use async_trait::async_trait;
use fxg_protocol::client_api::{AgentManagementSnapshot, AgentMcpAction, AgentMcpDefinition};

/// Agent-native configuration / catalog operations.
#[async_trait]
pub trait AgentManagement: Send + Sync {
    /// Stable agent ID this integration belongs to.
    fn agent_id(&self) -> &str;

    /// Reads the current management snapshot.
    async fn snapshot(&self) -> anyhow::Result<AgentManagementSnapshot>;

    /// Connects or disconnects an MCP server.
    async fn mcp_action(&self, name: &str, action: AgentMcpAction) -> anyhow::Result<()>;

    /// Adds or replaces an MCP server definition.
    async fn upsert_mcp(&self, name: &str, definition: &AgentMcpDefinition) -> anyhow::Result<()>;

    /// Removes an MCP server.
    async fn remove_mcp(&self, name: &str) -> anyhow::Result<()>;
}
