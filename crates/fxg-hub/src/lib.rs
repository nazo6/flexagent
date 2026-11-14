//! FlexAgent hub crate (`fxg-server`).
//!
//! Contains the Axum Node Hub (`/api/v1/node/ws`), the client REST/WS API (`:8080`), event
//! projection application, VAPID Web Push delivery, and temporary VM provisioner management.
//!
//! # Module layout
//!
//! - [`api`]: the shared router and the
//!   [`ClientApiBackend`](api::ClientApiBackend) trait that let the local node (`fxg node start`)
//!   and the hub (`fxg hub start`) expose a **completely identical** REST / Client WS
//!   / PTY WS surface (design: `docs/03-protocol-and-api.md` §3)
//! - [`hub`]: Node Hub (node pairing token auth, `NodeHello`/`ResyncRequest`, idempotent
//!   `EventBatchPush` application and ACK, command relay, PTY relay)
//! - [`push`][]: VAPID Web Push (key generation, subscription management, fan-out of
//!   approval request notifications)
//! - [`provisioner`][]: provisioner management for temporary VM / sandbox nodes (launches
//!   `fxg internal stdio` as a child process and uses stdin/stdout as the JSON Lines
//!   transport)
//! - [`state`][]: shared state combining `hub.db` with the node connection registry
//! - [`Server`] is the startup entry point (CLI `fxg hub start`)

// Migration baseline: remove this crate-level allow as Phase 1 of
// `docs/proposals/2026-11-08-type-safety-plan.md` lands.
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "type-safety migration baseline; see docs/06-type-safety.md"
)]

pub mod api;
mod error;
pub mod hub;
mod process;
pub mod provisioner;
pub mod push;
pub mod state;

pub use error::HubError;
pub use process::{Hub, await_shutdown_task, spawn_ctrl_c_handler};
pub use state::{HubOptions, HubState};
