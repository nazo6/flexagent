//! ACP Registry JSON types and catalog display entries.

use std::collections::BTreeMap;
use std::path::PathBuf;

use fxg_protocol::client_api::AgentBackendKind;
use fxg_protocol::settings::CustomAgentConfig;
use serde::{Deserialize, Serialize};

/// `agents[].distribution`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RegistryDistribution {
    /// Binary distribution per OS/Arch key (`darwin-aarch64` / `windows-x86_64`, etc.)
    pub binary: BTreeMap<String, BinaryDistribution>,
    /// Distribution via `npx`
    pub npx: Option<PackageDistribution>,
    /// Distribution via `uvx`
    pub uvx: Option<PackageDistribution>,
}

impl RegistryDistribution {
    /// Names of the available distribution kinds (for display).
    pub fn kinds(&self) -> Vec<&'static str> {
        let mut kinds = Vec::new();
        if !self.binary.is_empty() {
            kinds.push("binary");
        }
        if self.npx.is_some() {
            kinds.push("npx");
        }
        if self.uvx.is_some() {
            kinds.push("uvx");
        }
        kinds
    }
}

/// Binary distribution entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BinaryDistribution {
    /// Archive URL (`.tar.gz` / `.zip`)
    pub archive: String,
    /// Executable inside the archive (relative path; may have the `./name` form)
    pub cmd: String,
    /// Additional arguments at launch
    #[serde(default)]
    pub args: Vec<String>,
    /// Additional environment variables
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// SHA-256 of the archive (optional)
    #[serde(default)]
    pub sha256: Option<String>,
}

/// `npx` / `uvx` distribution entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageDistribution {
    /// Package specification (may include `name@version`)
    pub package: String,
    /// Additional arguments passed to the package
    #[serde(default)]
    pub args: Vec<String>,
    /// Additional environment variables
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Agent definition in the registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryAgent {
    /// Agent ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Version
    pub version: String,
    /// Description
    #[serde(default)]
    pub description: Option<String>,
    /// Repository URL
    #[serde(default)]
    pub repository: Option<String>,
    /// Website
    #[serde(default)]
    pub website: Option<String>,
    /// Authors
    #[serde(default)]
    pub authors: Vec<String>,
    /// License
    #[serde(default)]
    pub license: Option<String>,
    /// Icon URL
    #[serde(default)]
    pub icon: Option<String>,
    /// Distribution kinds
    #[serde(default)]
    pub distribution: RegistryDistribution,
}

/// The whole `registry.json` file.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct RegistryIndex {
    /// Index schema version
    #[serde(default)]
    pub version: String,
    /// Agent list
    #[serde(default)]
    pub agents: Vec<RegistryAgent>,
}

impl RegistryIndex {
    /// Looks up an agent by ID.
    pub fn find(&self, id: &str) -> Option<&RegistryAgent> {
        self.agents.iter().find(|agent| agent.id == id)
    }
}

/// Launch sources available on this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalSource {
    /// An executable already present on `PATH`
    External(PathBuf),
    /// Run via `npx` (fetched at run time)
    Npx,
    /// Run via `uvx` (fetched at run time)
    Uvx,
}

/// Agent information for the catalog display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentListEntry {
    /// Agent ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Version (`-` for built-in agents, registry version for imports)
    pub version: String,
    /// Description
    pub description: Option<String>,
    /// Launch sources available on this node (in priority order)
    pub sources: Vec<LocalSource>,
    /// Custom definition (node.db / UI-managed)
    pub custom: Option<CustomAgentConfig>,
    /// Built-in extension ID applied to this agent (`None` = ACP only)
    pub extension: Option<String>,
    /// Effective ACP client capabilities advertised on connections
    pub capabilities: fxg_protocol::settings::AgentAcpCapabilities,
    /// How the agent runtime is implemented
    pub backend: AgentBackendKind,
    /// Whether this agent exposes out-of-session native management
    pub has_management: bool,
    /// Icon URL or built-in asset key
    pub icon: Option<String>,
}

impl AgentListEntry {
    /// Whether it can be launched on this node.
    pub fn is_available(&self) -> bool {
        self.custom.is_some()
            || !self.sources.is_empty()
            || self.backend == AgentBackendKind::Custom
    }
}
