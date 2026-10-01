//! FlexAgent 中央サーバークレート (`fxg-server`)
//!
//! Axum による Node Hub (`/api/v1/node/ws`)、Client REST/WS API (`:8080`)、
//! イベント投影の適用、VAPID Web Push 送信、一時VMプロビジョナー管理を含む。
//!
//! # モジュール構成
//!
//! - [`api`]: ローカルノード (`fxg daemon`) と中央サーバー (`fxg server`) が
//!   **完全に同一**の REST / Client WS / PTY WS を提供するための共通ルーターと
//!   [`ClientApiBackend`](api::ClientApiBackend) トレイト
//!   (設計: `docs/03-protocol-and-api.md` §3)
//! - [`hub`]: Node Hub (ノード個別トークン認証・`NodeHello`/`ResyncRequest`・
//!   `EventBatchPush` の冪等適用と ACK・コマンド中継・PTY 中継)
//! - [`push`][]: VAPID Web Push (鍵生成・購読管理・承認リクエスト通知の fan-out)
//! - [`provisioner`][]: 一時VM・サンドボックスノードのプロビジョナー管理
//!   (`fxg daemon --stdio` を子プロセスとして起動し、stdin/stdout を
//!   JSON Lines トランスポートに使う)
//! - [`state`][]: `server.db` とノード接続レジストリを束ねる共有状態
//! - [`Server`] は起動エントリポイント (CLI の `fxg server`)

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
