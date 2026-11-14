# 診断基盤の永続化: インシデント・コンポーネントヘルス・サポートバンドル

- **日付**: 2026-11-10
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-server`, `fxg-node`,
  `fxg-cli`, `ui`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run types`,
  `mise run sqlx:prepare`, `mise run lint:errors`, `cargo test --workspace`,
  `pnpm --dir ui run check`, `pnpm --dir ui run test`

## 背景

[`2026-11-08-error-handling-and-recovery-plan.md`](../../proposals/2026-11-08-error-handling-and-recovery-plan.md)
の Phase 1 / 2 / 2.5 と、daily-driver 側の診断取得経路
（`/healthz` / `/readyz` / `/api/v1/diagnostics` / `fxg doctor` /
`fxg logs` / `fxg session diagnose` / エージェント stderr リング）は
すでに実装済みだった。一方で、次の「後から原因を追う」ための要素が
未着手だった。

- 障害の永続化（`incidents`）と同一原因のグルーピング
- コンポーネント単位のヘルスと劣化理由
- 同期・配信のドロップ / リトライ回数の可視化
- 秘密情報を redaction したサポートバンドル

本変更で Phase 3 の残りを実装した。

## 設計

### インシデント

- 単一マイグレーションセットに `incidents` を追加
  (`crates/fxg-db/migrations/0011_incidents.sql`)。
  `node.db` / `server.db` の双方で同じ DDL を使い、同期対象にはしない。
- 原因の同一性は `(code, source, session_id, node_id)` で判定する。
  未解決のまま 10 分以内に再発した場合は行を増やさず
  `occurrence_count` と `last_seen_at` を更新する。
- 保持期間は 14 日、最大 1000 行（`fxg_db::incidents::prune`）。
  解決は `resolved_at` で表現し、`POST
  /api/v1/diagnostics/incidents/:id/resolve` から行える。

### 記録境界

- REST: `ApiError::into_response` が `x-fxg-error-code` ヘッダを付ける。
  診断ミドルウェアはヘッダだけを読み、レスポンスボディをバッファせずに
  5xx / 409 / 408 / 429 を記録する。400 / 401 / 403 / 404 は通常操作として
  除外する。
- Client WS: コマンド失敗と replay 失敗を記録し、警告
  (`CommandResult.warnings`) は `warning` として記録する。
- ノード RPC: `rpc::execute` のエラーを `NodeError::incident_source()` /
  `incident_level()` で分類して記録する。busy / duplicate / already-resolved
  は制御フローなので除外する。
- エージェント: `SessionEventBus::record` が
  `StatusChanged(Error)` を観測した時点で記録する。

### ヘルスとカウンタ

- `SystemStatus` は `db` / `migrations` / `http` / `ipc` / `sync` / `hub` /
  `disk` を常時測定し、`registry` / `agent spawn` / `git` / `pty` /
  `provisioner` / `push` は直近 24 時間の未解決インシデントから
  劣化として導出する。
- `disk` は `fs4::available_space` で 1 GiB 未満を degraded、
  100 MiB 未満を error とする。
- `DropCounters` はプロセス内の `AtomicU64` で
  `dropped_events` / `lagged_broadcasts` / `replay_failures` /
  `outbox_retries` / `push_failures` を集計する。

### サポートバンドル

- `GET /api/v1/diagnostics/bundle` が `SupportBundle` を返す。
  内容はバージョン、安全な設定項目、DB 統計、ヘルス、インシデント、
  セッション概要、ログ末尾。
- redaction は `fxg-server::api::diagnostics` に集約し、ノード / サーバーの
  双方が同じ実装を使う。
  - Bearer / `fxg_session` / `token=` / `password=` / `secret=`
  - `authorization` 行
  - JSON の `*token*` / `*password*` / `*secret*` / `*authorization*` /
    `*cookie*` キー
  - ホームディレクトリを `~` に置換
- CLI は `fxg diagnostics export`（`-o -` で stdout）と
  `fxg diagnostics incidents` を追加。ローカル IPC に
  `SupportBundle` / `Incidents` を追加した。
- Web UI は設定 > Bootstrap の Diagnostics パネルで
  コンポーネントヘルス・カウンタ・直近インシデントを表示し、
  サポートバンドルをダウンロードできるようにした。

## 変更ファイル（主なもの）

- `crates/fxg-protocol/src/common/diagnostics.rs`（新規）
- `crates/fxg-protocol/src/client_api.rs` / `ipc.rs`
- `crates/fxg-db/migrations/0011_incidents.sql`（新規）
- `crates/fxg-db/src/incidents.rs`（新規） / `db.rs` / `error.rs`
- `crates/fxg-server/src/api/diagnostics.rs`（新規） /
  `counters.rs`（新規） / `backend.rs` / `error.rs` / `mod.rs` /
  `handlers/system.rs` / `ws.rs` / `request_context.rs`
- `crates/fxg-server/src/state/{mod,api_impl}.rs` / `push.rs` / `hub.rs`
- `crates/fxg-node/src/daemon/{mod,api,ipc}.rs` / `rpc.rs` / `sync.rs` /
  `session.rs` / `error.rs`
- `crates/fxg-cli/src/commands/diagnostics.rs`（新規） / `client.rs` /
  `main.rs` / `commands/mod.rs`
- `ui/src/lib/api/client.ts` /
  `ui/src/lib/components/settings/DiagnosticsPanel.svelte`（新規） /
  `ui/src/routes/settings/+page.svelte` / `ui/messages/en.json`
- `docs/02-database-schema.md` / `docs/03-protocol-and-api.md`

## 検証

- `mise run fmt:check`
- `mise run lint:errors`（`-D clippy::disallowed_types` を含む）
- `cargo test --workspace --no-fail-fast`
- `mise run sqlx:prepare`（`0011` を含む `.sqlx` を更新）
- `pnpm --dir ui run check`
- `pnpm --dir ui run lint`
- `pnpm --dir ui run test`（94 tests）
- 追加テスト:
  - `fxg-db::incidents`: グルーピング / 解決後の新規行 / 期限切れ剪定 /
    カウント
  - `fxg-server::api::diagnostics`: token / cookie / ホームディレクトリ /
    JSON キーの redaction、セッション ID 抽出
  - `fxg-server::api::counters`: カウンタのスナップショット

## 注意・今後

- `SystemInfoResponse` は軽量な接続情報のままにし、詳細なヘルスは
  `/api/v1/diagnostics` に集約した。
- `fxg status --verbose` と `stale_registry_served` の計測は未着手。
- インシデントは各ストアが自分の観測事実のみを保持し、ノード ⇔
  サーバー間で同期しない（監査ログと同じ方針）。
- サポートバンドルは `/api/v1/diagnostics` を拡張した JSON であり、
  zip ではない。バイナリ添付が必要になった時点で別エンドポイントにする。
