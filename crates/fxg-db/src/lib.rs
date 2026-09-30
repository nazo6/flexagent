//! FlexAgent SQLite スキーマ管理・イベント適用エンジン・共通クエリ層 (`fxg-db`)
//!
//! `node.db` (各ノード) と `server.db` (中央サーバー) は
//! [単一のマイグレーションセット](./migrations) から生成される同一スキーマを共有する。
//! 違いは「行のスコープ」と「実行ロール ([`DbRole`])」のみ。
//!
//! 設計仕様は
//! [`docs/02-database-schema.md`](https://github.com/nazo6/flexagent/blob/master/docs/02-database-schema.md)
//! を参照。
//!
//! # モジュール構成
//!
//! - `events`: イベント適用エンジン (冪等追記 + 投影更新) と投影再構築
//! - `outbox`: 水位ベース Outbox 抽出と ACK 水位更新
//! - `queries`: Local Node / Central Server 共通のクライアント向けクエリ層
//! - `registration`: ノード自己登録・プロジェクト紐付けの upsert
//! - `audit`: 監査ログの記録
//! - `push`: Web Push 購読の管理
//! - `searchable`: FTS5 `searchable_text` 生成ロジック
//!
//! # SQL 規約
//!
//! SQL は `sqlx::query!` / `query_as!` / `query_scalar!` (コンパイル時検証) を
//! 必須とし、ランタイム文字列の `sqlx::query()` は例外規定
//! (FTS5 `MATCH` 等の特殊構文) のみとする。オフラインビルド用の `.sqlx/` は
//! `cargo sqlx prepare` で生成してコミットする (`SQLX_OFFLINE=true`)。

#![warn(missing_docs)]

pub mod audit;
pub mod error;
pub mod events;
pub mod outbox;
pub mod push;
pub mod queries;
pub mod registration;
pub mod searchable;

mod db;

pub use db::{Db, DbRole, hub_db_path, node_db_path};
pub use error::DbError;

// 型の再エクスポート (利用側の import を短くする)
pub use audit::AuditLogRecord;
pub use events::{AppendedEvent, ApplyOutcome};
pub use outbox::SessionOutboxBatch;
pub use push::PushSubscriptionRecord;
pub use queries::SessionFilter;
pub use registration::{NodeRecord, ProjectBindingRecord, ProjectRecord};
