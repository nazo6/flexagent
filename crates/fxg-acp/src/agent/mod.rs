//! Agent catalog and provider abstraction.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.1 (planned in
//! `docs/proposals/2026-11-11-agent-extension-architecture.md`).
//!
//! - [`provider`]: static definitions, launch presets and backend selection
//! - [`builtin`]: built-in providers for well-known ACP CLIs
//! - [`catalog`]: launch resolution and driver selection
//! - [`extension`]: Tier 2 extension traits and capability overlays
//! - [`extended`]: `ExtendedAcpDriver`, the ACP + extension adapter

pub mod builtin;
pub mod catalog;
pub mod extended;
pub mod extension;
pub mod opencode;
pub mod provider;

pub use catalog::AgentCatalog;
pub use extended::ExtendedAcpDriver;
pub use extension::{
    AgentExtension, AgentMetadata, CapabilitiesSnapshot, CapabilityOverlay, ConfigOptionPatch,
    EventDisposition, Hook, SessionContext, SessionExtension,
};
pub use provider::{AgentBackend, AgentDefinition, AgentProvider, LaunchPreset, StaticProvider};
