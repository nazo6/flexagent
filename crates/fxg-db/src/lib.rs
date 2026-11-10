//! FlexAgent SQLite schema management, event application engine, and shared query layer (`fxg-db`)
//!
//! `node.db` (each node) and `server.db` (the central server) share the same schema
//! generated from [a single migration set](./migrations). The only differences are the
//! "row scope" and the "execution role" ([`DbRole`]).
//!
//! See
//! [`docs/02-database-schema.md`](https://github.com/nazo6/flexagent/blob/master/docs/02-database-schema.md)
//! for the design specification.
//!
//! # Module layout
//!
//! - `events`: event application engine (idempotent append + projection update) and rebuild
//! - `outbox`: watermark-based outbox extraction and ACK watermark updates
//! - `queries`: client-facing query layer shared by Local Node / Central Server
//! - `registration`: upsert for node self-registration and project binding
//! - `audit`: audit log recording
//! - `push`: Web Push subscription management
//! - `searchable`: FTS5 `searchable_text` generation logic
//!
//! # SQL conventions
//!
//! SQL must use `sqlx::query!` / `query_as!` / `query_scalar!` (compile-time verification).
//! Runtime string `sqlx::query()` is only an exception for special syntax such as FTS5
//! `MATCH`. The `.sqlx/` directory for offline builds is generated with `cargo sqlx prepare`
//! and committed (`SQLX_OFFLINE=true`).

#![warn(missing_docs)]

pub mod audit;
pub mod error;
pub mod events;
pub mod incidents;
pub mod outbox;
pub mod push;
pub mod queries;
pub mod registration;
pub mod searchable;
pub mod secrets;
pub mod settings;

mod db;

pub use db::{Db, DbRole, hub_db_path, node_db_path};
pub use error::DbError;

// Re-exported types (keeps imports on the caller side short)
pub use audit::AuditLogRecord;
pub use events::{AppendedEvent, ApplyOutcome};
pub use incidents::{DatabaseCounts, IncidentRecord};
pub use outbox::SessionOutboxBatch;
pub use push::PushSubscriptionRecord;
pub use queries::SessionFilter;
pub use registration::{NodeRecord, ProjectBindingRecord, ProjectRecord};
