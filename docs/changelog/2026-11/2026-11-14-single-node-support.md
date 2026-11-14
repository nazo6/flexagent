# シングルノード構成対応（Web Push・Standalone Outbox / UI 正常化）

- **日付**: 2026-11-14
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-hub`, `fxg-node`, `fxg-cli`, `ui`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run types`, `mise run lint:errors`,
  `cargo test --workspace`, `pnpm --dir ui run check`
- **関連ドキュメント**:
  - [`docs/01-architecture-and-sync.md`](../../01-architecture-and-sync.md) §1 / §3.1
  - [`docs/02-database-schema.md`](../../02-database-schema.md) §0.2 / §1
  - [`docs/03-protocol-and-api.md`](../../03-protocol-and-api.md) §3.1
  - [`docs/05-cli-and-pwa-ui.md`](../../05-cli-and-config.md) §2 / §3.3

## 背景

`node.toml` の `allowed_hosts` / `allowed_origins` 対応により、`fxg node start`
単体をリバースプロキシ経由で他端末へ公開できるようになりました。しかし、ハブなしで
常時運用するシングルノード構成では、以下の点がハブ前提のまま残っていました：

1. Web Push（`vapid_public_key` / `POST /api/v1/push/subscribe` / `vapid_subject`）が
   ハブ専用になっており、ノード単体では PWA 通知を受けられない。
2. ハブ未設定のスタンドアロン運用（`HubLinkState::Standalone`）では
   `synced_up_to_node_seq` が進まないため、イベント数が 1000 件を超えると
   `role_health` が `Degraded` になり、UI 接続バッジでも `Disconnected` /
   `Unsynced: N` と誤解を招く表示になる。

なお、**Provisioner（一時VM / `fxg internal stdio`）およびノードペアリングトークン管理は
引き続きハブ専用**とします。

## 設計決定

1. **Web Push はロール単位（オリジン単位）で独立運用する**
   - `fxg-node` も `<data>/node/vapid.json` と `node.db` の `push_subscriptions` を
     用いて `PushService` を動かす。
   - 購読はブラウザが接続したオリジンごとに各 DB へ登録されるため、ノードが後から
     ハブとペアリングされた状態（`hub_link == Connected`）でも、ノード自身の
     `push_subscriptions` への送信は独立して継続する。
   - 通知選別・配信ロジック（`RunningTurns`・`notification_events`・`push_subscribe`）は
     `fxg-hub` 内の共通ヘルパーとして切り出し、ハブとノードの双方から再利用する。
2. **`vapid_subject` は `node.db` (`node_settings`) に持つ**
   - 実行時変更可能な設定は所有者の DB に置く原則と `hub_settings` との対称性に
     合わせ、`node_settings` に `vapid_subject` を追加してノード設定画面から
     編集可能にする（`node.toml` は起動設定のみ）。
3. **Standalone 時の Outbox は保持しつつ通知・警告だけ抑止する**
   - 将来ハブとペアリングしたときに過去セッションを含めて同期できるよう、DB 上の
     Outbox（`synced_up_to_node_seq`）自体はそのまま維持する。
   - `HubLinkState::Standalone` の間は `role_health` でバックログによる `Degraded`
     判定を行わず、`system_extras` の `unsynced_event_count` は `None`（Client WS 上は
     `0`）を返す。UI の接続バッジも `Disconnected` ではなく `Standalone` として扱い、
     未同期件数を表示しない。

## 実装計画（概要）

- **`fxg-hub` 共通化**: `push` モジュールの通知選別・ディスパッチ・購読登録ヘルパーを
  公開し、`HubState` と `DaemonState` の両方から呼べるようにする。
- **`fxg-db` / `fxg-protocol`**: `node_settings` へ `vapid_subject` を追加する
  マイグレーションと型更新（`mise run types` で `bindings.ts` を同期）。
- **`fxg-node`**: `DaemonState` へ `PushService` / `RunningTurns` を統合し、
  `system_extras`・`push_subscribe`・`local client event forwarder`・`role_health`・
  `update_node_settings` を対応させる。
- **`fxg-cli` / `ui`**: `fxg doctor` の `Role::Node` パス一覧へ `vapid.json` を追加。
  `SettingsCatalog.svelte`・`settings-search.ts` にノード用 `vapid_subject` と
  `vapid.json` を追加し、`ConnectionScopeBadge.svelte` と `ui/messages/en.json` の
  Standalone 表示・ロール中立表現を整える。

## 注意（変更しない不変条件）

- ハブ用（`hub.db` / `<data>/hub/vapid.json`）とノード用（`node.db` /
  `<data>/node/vapid.json`）の VAPID 鍵と購読ストアは分離したまま維持する。
- Provisioner（`GET/PUT/DELETE /api/v1/provisioners`, `--provisioner`）および
  ノードペアリングトークン管理（`/api/v1/nodes/tokens`）はハブ専用とする。
