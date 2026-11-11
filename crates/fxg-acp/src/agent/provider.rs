//! Agent definitions, launch presets and backend selection.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.1 (planned in
//! `docs/changelog/2026-11-11-agent-extension-architecture.md`).

use std::sync::Arc;

use fxg_protocol::client_api::AgentBackendKind;

use super::extension::AgentExtension;

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

/// How an agent is implemented at runtime.
#[derive(Clone)]
pub enum AgentBackend {
    /// ACP only (`AcpDriver`)
    Acp,
    /// ACP plus an fxg-side extension (`ExtendedAcpDriver`)
    AcpExtended(Arc<dyn AgentExtension>),
}

impl AgentBackend {
    /// Protocol-level representation exposed to clients.
    pub fn kind(&self) -> AgentBackendKind {
        match self {
            Self::Acp => AgentBackendKind::Acp,
            Self::AcpExtended(_) => AgentBackendKind::AcpExtended,
        }
    }
}

/// One built-in agent provider. This is the only registration seam.
pub trait AgentProvider: Send + Sync {
    /// Static definition (ID, display name, icon, ...).
    fn definition(&self) -> &AgentDefinition;

    /// Built-in launch preset, or `None` to use the ACP Registry launch.
    fn launch_preset(&self) -> Option<&LaunchPreset> {
        None
    }

    /// Runtime backend used for this agent.
    fn backend(&self) -> AgentBackend {
        AgentBackend::Acp
    }
}

/// A provider built from static data.
pub struct StaticProvider {
    definition: AgentDefinition,
    preset: Option<LaunchPreset>,
    backend: AgentBackend,
}

impl StaticProvider {
    /// Creates a static ACP-only provider.
    pub fn new(definition: AgentDefinition, preset: Option<LaunchPreset>) -> Self {
        Self {
            definition,
            preset,
            backend: AgentBackend::Acp,
        }
    }

    /// Replaces the runtime backend (for example an ACP extension).
    pub fn with_backend(mut self, backend: AgentBackend) -> Self {
        self.backend = backend;
        self
    }
}

impl AgentProvider for StaticProvider {
    fn definition(&self) -> &AgentDefinition {
        &self.definition
    }

    fn launch_preset(&self) -> Option<&LaunchPreset> {
        self.preset.as_ref()
    }

    fn backend(&self) -> AgentBackend {
        self.backend.clone()
    }
}
