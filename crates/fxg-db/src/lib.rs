//! FlexAgent SQLite スキーマ管理・イベント適用エンジン・共通クエリ層 (`fxg-db`)
//!
//! `node.db` (各ノード) と `server.db` (中央サーバー) は
//! 単一のマイグレーションセットから生成される同一スキーマを共有する。
//! 違いは「行のスコープ」と「実行ロール」のみ。
//!
//! 設計仕様は
//! [`docs/02-database-schema.md`](https://github.com/nazo6/flexagent/blob/master/docs/02-database-schema.md)
//! を参照。

#![warn(missing_docs)]
