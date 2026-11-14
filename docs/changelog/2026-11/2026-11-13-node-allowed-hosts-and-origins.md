# ノードの `allowed_hosts` / `allowed_origins` 対応（node 単体・リバースプロキシ運用）

- **日付**: 2026-11-13
- **対象パッケージ**: `fxg-protocol`, `fxg-node`, `fxg-hub`, `fxg-cli`, `ui`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run types`, `mise run lint:errors`,
  `cargo test --workspace`, `pnpm --dir ui run check`
- **関連ドキュメント**:
  - [`docs/01-architecture-and-sync.md`](../../01-architecture-and-sync.md) §7.1 / §7.2
  - [`docs/05-cli-and-pwa-ui.md`](../../05-cli-and-config.md) §2.2

## 背景

Host ヘッダ検証（DNS Rebinding 対策）と WebSocket `Origin` 検証（CSWSH 対策）は
ハブとノードで同じミドルウェアを使っていましたが、許可リストを設定できるのは
ハブだけでした。ノードは `ClientApiOptions::local()` でリストを空にしていたため、
リバースプロキシや LAN アドレス経由でノードの Web UI を開くと、ページ自体は
配信される（SPA の fallback はセキュリティ層の外側）ものの API が
`403 host not allowed` になっていました。

node 単体運用（ハブなし）でスマートフォンや他端末から PWA を使う場合に
この構成が必要になるため、ハブと同じ設定方法をノードにも用意します。

## 変更

- `NodeBootstrapConfig` に `allowed_hosts` / `allowed_origins` を追加
  （`FXG_NODE_ALLOWED_HOSTS` / `FXG_NODE_ALLOWED_ORIGINS` でも指定可能）。
  既定は空で、従来どおりループバック / 同一オリジンのみ許可します。
- `DaemonConfig` に同名フィールドを追加し、`fxg node start` が
  `[node]` の値を渡すようにしました。`fxg-node` の HTTP ルータは
  `ClientApiOptions::local()` ではなく、この値から
  `ClientApiOptions` を組み立てます（`ClientApiOptions::local` は削除）。
- `NodeBootstrapView` に `allowed_hosts` / `allowed_origins` を追加し、
  設定カタログ（Bootstrap タブ）にハブと同じ読み取り専用行を表示します。
  `sourced_list` ヘルパーを追加し、ハブ側の重複していた出所判定もこれに置き換えました。
- 出所表示のキー名を実際の `config.toml` に合わせて `server.*` から
  `hub.*` に修正（`hub.listen_addr` / `hub.allowed_hosts` / `hub.allowed_origins`）。
- サポートバンドルの `SupportBundleConfig.allowed_hosts` / `allowed_origins` は
  ハブ専用ではなく所有者（node / hub）それぞれの値を入れるようにしました。
- CLI フラグ（`--allowed-host` など）は追加しません。セキュリティ境界の設定は
  `config.toml` / 環境変数が唯一の所有者で、Web UI は読み取り専用という既存方針に
  合わせています（ハブにもフラグはありません）。
- `node.addr` にワイルドカードの bind アドレス（`0.0.0.0` / `[::]`）が記録された
  場合、CLI・`fxg web`・トレイはハブと同じくループバックへ読み替えます
  （`normalize_listen`）。LAN 公開のために `listen_addr = "0.0.0.0:7860"` に
  しても、ローカルの CLI は従来どおり動きます。

## 注意（変更しない不変条件）

- ループバック (`localhost` / `127.0.0.1` / `[::1]`) は常に許可されます。
  許可リストは「追加」であり、置き換えではありません。
- ノードに直接接続したブラウザの Web PTY は `allow_remote_pty` の対象外です
  （この設定はハブ経由の中継のみを制御します）。ノードを公開する場合も、
  ハブと同じく LAN / VPN / TLS リバースプロキシの内側に限定してください。
- 設定は起動時読み込みのため、変更には `fxg node restart` が必要です。

## 検証

- `cargo test -p fxg-protocol`（`[node] allowed_*` のパースと env 上書き）
- `cargo test -p fxg-node`（`accepts_configured_host_and_origin`:
  許可した Host / Origin で REST 200・WS 101、既定では 403）
- `mise run types` で `ui/src/lib/generated/bindings.ts` を同期
- `mise run fmt`, `mise run lint:errors`, `cargo test --workspace`,
  `pnpm --dir ui run check`
