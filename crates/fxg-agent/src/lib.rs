//! FlexAgent agent driver crate (`fxg-agent`)
//!
//! Design: `docs/04-agent-drivers-and-windows.md`
//!
//! - [`driver`][]: [`AgentDriver`] / [`ActiveSessionHandle`] traits — the
//!   abstraction for adding agents with custom protocols without changing
//!   `fxg-node` itself
//! - [`catalog`][]: agent definitions, built-in agents and the catalog used for
//!   launch resolution and driver selection
//! - [`acp`][]: `AcpDriver` (standard ACP agent control via
//!   `agent-client-protocol`) and the Tier 2 extensions in
//!   [`acp::extensions`] module
//! - [`acp::registry`]: fetching and caching the ACP Registry (`registry.json`)
//! - [`opencode`][]: `OpenCodeNativeDriver` (OpenCode v2 HTTP + SSE)

// Migration baseline: remove this crate-level allow as Phase 1 of
// `docs/proposals/2026-11-08-type-safety-plan.md` lands.
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "type-safety migration baseline; see docs/06-type-safety.md"
)]

pub mod acp;
pub mod auth;
pub mod catalog;
pub mod driver;
pub mod management;
pub mod opencode;
mod usage;

pub use acp::AcpDriver;
pub use acp::extensions::{AgentExtension, AgentMetadata, EventDisposition};
pub use catalog::AgentCatalog;
pub use catalog::builtin::{
    AgentAuth, AgentBackend, AgentCommand, AgentDefinition, BuiltinAgent, LaunchPreset,
};
pub use driver::{
    AcpLaunchOptions, ActiveSessionHandle, AgentAuthRequired, AgentDriver, AgentLaunchSpec,
    AgentSessionInfo, AgentSessionList, DriverEvent, DriverKind, DriverSessionCapabilities,
    ExternalSessionsUnsupported, HistoryCaptureUnavailable, LaunchBackend, NativeForkUnavailable,
    NativeResumeUnavailable, NativeRevertUnavailable, ResumeRequest, StartSessionRequest,
    StartedSession,
};
