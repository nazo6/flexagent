//! FlexAgent hub crate (`fxg-hub`).
//!
//! Contains the Axum Node Hub (`/api/v1/node/ws`), event projection application, and
//! temporary VM provisioner management. The client REST/WS API surface is shared with the
//! local node through [`fxg_api`] (design: `docs/03-protocol-and-api.md` §3).
//!
//! # Module layout
//!
//! - [`hub`]: Node Hub (node pairing token auth, `NodeHello`/`ResyncRequest`, idempotent
//!   `EventBatchPush` application and ACK, command relay, PTY relay)
//! - [`provisioner`][]: provisioner management for temporary VM / sandbox nodes (launches
//!   `fxg internal stdio` as a child process and uses stdin/stdout as the JSON Lines
//!   transport)
//! - [`state`][]: shared state combining `hub.db` with the node connection registry
//! - [`Hub`] is the startup entry point (CLI `fxg hub start`)

// Migration baseline: remove this crate-level allow as Phase 1 of
// `docs/proposals/2026-11-08-type-safety-plan.md` lands.
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "type-safety migration baseline; see docs/06-type-safety.md"
)]

mod error;
pub mod hub;
mod process;
pub mod provisioner;
pub mod state;

pub use error::HubError;
pub use process::Hub;
pub use state::{HubOptions, HubState};
