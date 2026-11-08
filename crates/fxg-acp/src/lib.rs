//! FlexAgent agent driver crate (`fxg-acp`)
//!
//! Design: `docs/04-agent-drivers-and-windows.md`
//!
//! - [`driver`]: [`AgentDriver`] / [`ActiveSessionHandle`] traits — the
//!   abstraction for adding agents with custom protocols without changing
//!   `fxg-node` itself
//! - [`registry`]: fetching, caching and launch resolution for the ACP Registry
//!   (`registry.json`), plus install management (`fxg agents ...`)
//! - [`acp`][]: `AcpDriver` (standard ACP agent control via `agent-client-protocol`)
//! - [`opencode2`][]: `OpenCode2Driver` (`opencode serve` bridge + native TUI attach)

// Migration baseline: remove this crate-level allow as Phase 1 of
// `docs/changelog/2026-11-08-type-safety-plan.md` lands.
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "type-safety migration baseline; see docs/06-type-safety.md"
)]

pub mod acp;
pub mod driver;
pub mod registry;
mod warm;

pub use acp::AcpDriver;
pub use driver::{
    ActiveSessionHandle, AgentDriver, AgentLaunchSpec, DriverEvent, NativeResumeUnavailable,
    ResumeRequest, StartSessionRequest, StartedSession,
};
