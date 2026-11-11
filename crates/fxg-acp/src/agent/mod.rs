//! Agent catalog and provider abstraction.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.1 (planned in
//! `docs/changelog/2026-11-11-agent-extension-architecture.md`).
//!
//! - [`provider`]: static definitions, launch presets and backend selection
//! - [`builtin`]: built-in providers for well-known ACP CLIs
//! - [`catalog`]: launch resolution and driver selection

pub mod builtin;
pub mod catalog;
pub mod provider;

pub use catalog::AgentCatalog;
pub use provider::{AgentBackend, AgentDefinition, AgentProvider, LaunchPreset, StaticProvider};
