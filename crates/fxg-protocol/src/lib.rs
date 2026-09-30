//! FlexAgent 共通プロトコル型定義 (`fxg-protocol`)
//!
//! 全クレートと UI が参照する「型」を一元定義する。設計仕様は
//! [`docs/03-protocol-and-api.md`](https://github.com/nazo6/flexagent/blob/master/docs/03-protocol-and-api.md)
//! を参照。
//!
//! - `events`: 正規化セッションイベント (`SessionEventEnvelope` / `UnifiedEventPayload`)
//! - `common`: 共通補助型・エラーコード
//! - `node_server`: Node ⇔ Central Server メッセージ (WS / Stdio 共通)
//! - `client_api`: Client REST / Client WS / PTY WS メッセージ
//! - `ipc`: CLI ⇔ Local Daemon ローカルIPC メッセージ
//! - `config`: グローバル設定 (`config.toml`) / プロジェクト設定 (`.fxg.toml`)
//! - `util`: 時刻・ID 生成ユーティリティ
//!
//! クライアント (UI / CLI) が直接扱う型には `#[derive(ts_rs::TS)]` +
//! `#[ts(export)]` を付与し、`ui/src/lib/generated/` へ TypeScript 型定義を
//! 自動出力する (設定ファイルスキーマ等の内部型は export 対象外)。
//!
//! **Phase 1 で実装予定** (現時点ではプレースホルダ)。
