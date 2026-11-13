//! FlexAgent agent driver crate (`fxg-acp`)
//!
//! Design: `docs/04-agent-drivers-and-windows.md`
//!
//! - [`driver`]: [`AgentDriver`] / [`ActiveSessionHandle`] traits — the
//!   abstraction for adding agents with custom protocols without changing
//!   `fxg-node` itself
//! - [`agent`]: agent definitions, built-in providers and the catalog used for
//!   launch resolution and driver selection
//! - [`registry`]: fetching and caching the ACP Registry (`registry.json`),
//!   plus custom agent management used by the Web UI
//! - [`acp`][]: `AcpDriver` (standard ACP agent control via `agent-client-protocol`)

// Migration baseline: remove this crate-level allow as Phase 1 of
// `docs/changelog/2026-11-08-type-safety-plan.md` lands.
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "type-safety migration baseline; see docs/06-type-safety.md"
)]

pub mod acp;
pub mod agent;
pub mod auth;
pub mod driver;
pub mod registry;
mod warm;

pub use acp::AcpDriver;
pub use agent::AgentCatalog;
pub use driver::{
    ActiveSessionHandle, AgentAuthRequired, AgentDriver, AgentLaunchSpec, AgentSessionInfo,
    AgentSessionList, DriverEvent, DriverKind, ExternalSessionsUnsupported,
    HistoryCaptureUnavailable, NativeForkUnavailable, NativeResumeUnavailable, ResumeRequest,
    StartSessionRequest, StartedSession,
};
