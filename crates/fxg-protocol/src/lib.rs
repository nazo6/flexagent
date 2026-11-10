//! Shared protocol type definitions for FlexAgent (`fxg-protocol`)
//!
//! Defines, in one place, the types referenced by every crate and by the UI.
//! See [`docs/03-protocol-and-api.md`](https://github.com/nazo6/flexagent/blob/master/docs/03-protocol-and-api.md)
//! for the design specification.
//!
//! - `events`: normalized session events (`SessionEventEnvelope` / `UnifiedEventPayload`)
//! - `common`: shared helper types and error codes
//! - `node_hub`: Node <-> Central Server messages (shared by WS / Stdio)
//! - `client_api`: Client REST / Client WS / PTY WS messages
//! - `ipc`: CLI <-> Local Daemon local IPC messages
//! - `config`: process bootstrap configuration (`config.toml`)
//! - `settings`: typed settings snapshots, secret metadata and project identity
//! - `util`: time and ID generation helpers
//!
//! Types handled directly by clients (UI / CLI) get `#[derive(specta::Type)]` +
//! `#[specta(collect)]` so that TypeScript definitions are emitted to
//! `ui/src/lib/generated/bindings.ts` automatically (internal types such as the
//! configuration file schemas are not exported).

// Protocol type definitions are the declared JSON boundary. Phase 2 replaces
// the remaining raw fields with `RawJson`.
#![allow(
    clippy::disallowed_types,
    clippy::wildcard_enum_match_arm,
    reason = "protocol boundary definitions; see docs/06-type-safety.md"
)]

mod string_enum;

pub mod client_api;
pub mod common;
pub mod config;
pub mod events;
pub mod ids;
pub mod ipc;
pub mod node_hub;
pub mod settings;
pub mod util;

pub use ids::{CommandId, EventId, NodeId, ProjectId, SecretId, SessionId};

/// Generates the TypeScript definitions used by the UI (`cargo test -p fxg-protocol export_bindings`).
///
/// It lives in a lib unit test rather than an integration test because the
/// `#[specta(collect)]` types of this crate must be collected from the same crate.
#[cfg(test)]
mod export_bindings;
