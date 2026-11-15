//! ACP extension traits (Tier 2).
//!
//! An extension contributes the vendor pieces that ACP does not expose:
//! display metadata, extra client capabilities / notification methods, a
//! vendor notification parser, and a synchronous event transform applied by
//! the ACP driver.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.1.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use fxg_protocol::common::RawJson;
use fxg_protocol::events::SubagentStatus;

use crate::driver::DriverEvent;

pub mod opencode;

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

/// ACP client capability that an extension can advertise.
///
/// Unknown names are preserved so a newer configuration does not silently
/// enable a capability that this build does not understand.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpCapability {
    /// ACP subagents RFD (`clientCapabilities.subagents`).
    Subagents,
    /// A capability added by a newer fxg / ACP schema.
    Unknown(String),
}

impl AcpCapability {
    /// Parses a capability name (`subagents`, ...).
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "subagents" => Self::Subagents,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Capability name used on the wire and in logs.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Subagents => "subagents",
            Self::Unknown(name) => name,
        }
    }
}

/// Set of ACP client capabilities advertised on a connection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcpCapabilitySet(BTreeSet<AcpCapability>);

impl AcpCapabilitySet {
    /// Creates an empty capability set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a capability.
    pub fn insert(&mut self, capability: AcpCapability) -> bool {
        self.0.insert(capability)
    }

    /// Whether the capability is enabled.
    #[must_use]
    pub fn contains(&self, capability: &AcpCapability) -> bool {
        self.0.contains(capability)
    }

    /// Removes a capability (no-op when it was not enabled).
    pub fn remove(&mut self, capability: &AcpCapability) -> bool {
        self.0.remove(capability)
    }

    /// Iterates over the enabled capabilities.
    pub fn iter(&self) -> impl Iterator<Item = &AcpCapability> {
        self.0.iter()
    }

    /// Whether no capability is enabled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<AcpCapability> for AcpCapabilitySet {
    fn from_iter<T: IntoIterator<Item = AcpCapability>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl From<&fxg_protocol::settings::AgentAcpCapabilities> for AcpCapabilitySet {
    fn from(capabilities: &fxg_protocol::settings::AgentAcpCapabilities) -> Self {
        let mut set = Self::new();
        if capabilities.subagents {
            set.insert(AcpCapability::Subagents);
        }
        set
    }
}

/// ACP connection options contributed by an extension.
///
/// This is plain data so it can live in [`crate::driver::AgentLaunchSpec`] and
/// the warm-pool key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpExtensionOptions {
    /// Additional `clientCapabilities._meta` entries.
    pub client_meta: Option<RawJson>,
    /// Custom notification method names the extension wants to receive.
    pub notifications: Vec<String>,
    /// Client capabilities to advertise.
    pub capabilities: AcpCapabilitySet,
}

impl Default for AcpExtensionOptions {
    fn default() -> Self {
        let mut capabilities = AcpCapabilitySet::new();
        capabilities.insert(AcpCapability::Subagents);
        Self {
            client_meta: None,
            notifications: Vec::new(),
            capabilities,
        }
    }
}

/// A child-session update parsed from a vendor notification.
#[derive(Debug, Clone)]
pub struct AcpChildUpdate {
    /// Agent-side session ID of the child.
    pub child_session_id: String,
    /// Agent-side session ID of the parent (when reported).
    pub parent_session_id: Option<String>,
    /// Nesting depth reported by the vendor (0 = unknown).
    pub depth: u32,
    /// Display title for the child.
    pub title: Option<String>,
    /// Working directory of the child (when reported).
    pub cwd: Option<PathBuf>,
    /// Lifecycle state (status-only updates).
    pub state: Option<SubagentStatus>,
    /// Inner ACP update to normalize.
    pub update: Option<agent_client_protocol::v1::schema::SessionUpdate>,
}

/// Parses vendor-specific subagent notifications into a generic child update.
pub trait AcpNotificationParser: Send + Sync + std::fmt::Debug {
    /// Notification methods handled by this parser.
    fn methods(&self) -> &'static [&'static str];
    /// Parses one notification. Returns `None` when the payload is unknown.
    fn parse(&self, method: &str, params: &RawJson) -> Option<AcpChildUpdate>;
}

/// ACP extension (Tier 2): metadata, connection options and event transforms.
///
/// One extension instance is shared across sessions and agents of the same
/// built-in extension ID.
pub trait AgentExtension: Send + Sync {
    /// Display overrides (name, icon, description).
    fn metadata(&self) -> AgentMetadata {
        AgentMetadata::default()
    }

    /// ACP connection options (client capabilities, vendor notification methods).
    ///
    /// The returned value is stored in [`crate::driver::AgentLaunchSpec`] so the
    /// ACP driver can apply it without knowing the extension. `capabilities` is
    /// the effective per-agent selection, so a vendor feature can follow a
    /// capability toggle (for example, OpenCode's child-session updates follow
    /// `subagents`).
    fn acp_options(
        &self,
        _capabilities: &fxg_protocol::settings::AgentAcpCapabilities,
    ) -> AcpExtensionOptions {
        AcpExtensionOptions::default()
    }

    /// Parser for vendor-specific subagent notifications.
    fn notification_parser(&self) -> Option<Arc<dyn AcpNotificationParser>> {
        None
    }

    /// Rewrites one driver event before it reaches the session manager.
    ///
    /// The event is passed by value so the common path does not clone it;
    /// return it wrapped in [`EventDisposition::Emit`] when unchanged.
    fn transform_event(&self, event: DriverEvent) -> EventDisposition {
        EventDisposition::Emit(event)
    }
}

/// How an extension wants an event forwarded.
///
/// The event is owned: an extension returns it unchanged by wrapping it in
/// [`Self::Emit`].
#[derive(Debug, Clone)]
#[allow(
    clippy::large_enum_variant,
    reason = "the event is moved through the transform; boxing it would add an allocation to every forwarded event"
)]
pub enum EventDisposition {
    /// Forward the event (unchanged, or rewritten).
    Emit(DriverEvent),
    /// Replace the event with multiple events.
    EmitMany(Vec<DriverEvent>),
    /// Drop the event.
    Drop,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_capabilities_are_per_launch_and_keep_unknown_names() {
        // Subagents are advertised by default; an agent definition can replace
        // the set, and a name from a newer build survives the round trip.
        let default = AcpExtensionOptions::default();
        assert!(default.capabilities.contains(&AcpCapability::Subagents));

        let disabled = AcpExtensionOptions {
            capabilities: AcpCapabilitySet::new(),
            ..Default::default()
        };
        assert!(!disabled.capabilities.contains(&AcpCapability::Subagents));

        assert_eq!(
            AcpCapability::from_name("future_thing"),
            AcpCapability::Unknown("future_thing".to_owned())
        );
    }
}
