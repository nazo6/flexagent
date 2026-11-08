//! FlexAgent central server crate (`fxg-server`).
//!
//! Contains the Axum Node Hub (`/api/v1/node/ws`), the client REST/WS API (`:8080`), event
//! projection application, VAPID Web Push delivery, and temporary VM provisioner management.
//!
//! # Module layout
//!
//! - [`api`]: the shared router and the
//!   [`ClientApiBackend`](api::ClientApiBackend) trait that let the local node (`fxg daemon`)
//!   and the central server (`fxg server`) expose a **completely identical** REST / Client WS
//!   / PTY WS surface (design: `docs/03-protocol-and-api.md` §3)
//! - [`hub`]: Node Hub (per-node token auth, `NodeHello`/`ResyncRequest`, idempotent
//!   `EventBatchPush` application and ACK, command relay, PTY relay)
//! - [`push`][]: VAPID Web Push (key generation, subscription management, fan-out of
//!   approval request notifications)
//! - [`provisioner`][]: provisioner management for temporary VM / sandbox nodes (launches
//!   `fxg daemon --stdio` as a child process and uses stdin/stdout as the JSON Lines
//!   transport)
//! - [`state`][]: shared state combining `server.db` with the node connection registry
//! - [`Server`] is the startup entry point (CLI `fxg server`)

pub mod api;
mod credentials;
mod error;
pub mod hub;
pub mod provisioner;
pub mod push;
mod server;
pub mod state;

pub use error::ServerError;
pub use server::Server;
pub use state::{ServerOptions, ServerState};
