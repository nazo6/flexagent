# ハブ経由とノード直結の機能差を解消

- **日付**: 2026-11-18
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-api`, `fxg-hub`, `fxg-node`, `ui`, `docs`
- **対象スクリプト**: `mise run types`, `mise run fmt`, `cargo clippy --workspace --all-targets`,
  `cargo test -p fxg-node`, `pnpm --dir ui run check`, `mise run lint:ui`

---

## 概要

「ノード直結では使えるがハブ経由では使えない」機能をなくした。以前は
workspace のファイル / Git 操作を `via_hub` で一律拒否し、エージェントの
ネイティブ管理とノード単位の診断・監査・停止には RPC 経路自体がなかった。

細かい権限ビット（読み取り / 書き込み / 操作を per-node で許可する設定）は
追加しない。実運用では全許可になりがちで、設定・DB・API・UI の出し分けが
増える割に効果が薄いため、**ハブの認証トークンを持つクライアントは対象
ノードと同じ操作ができる**モデルに揃えた。ハブは LAN / Tailscale など信頼
できるネットワークに置く前提とし、認証・Host / Origin 検証・監査ログは従来
どおり維持する。

## 変更

### プロトコル / Node

- `NodeRpcRequest::requires_local_workspace_access()` と、ノード側の
  `via_hub` 拒否を削除した。`AuditSource::via_hub` も不要になったため削除。
- workspace の access root 検証（登録済み workspace 配下のみ、`.git` と
  シンボリックリンク脱出の拒否、UTF-8 / 2 MiB 上限）はノード側で継続する。
- `NodeRpcRequest` / `NodeRpcResponse` に以下を追加した。
  - `AgentManagement` / `AgentMcpAction` / `AgentMcpUpsert` / `AgentMcpRemove`
    （応答は `AgentManagementSnapshot`）
  - `Diagnostics`（`DiagnosticsResponse`）
  - `ListAuditLogs { limit }`（`Vec<AuditLogEntry>`）
  - `Shutdown`
- `Shutdown` は応答がハブへ届いてから停止するよう、500ms 後に
  `request_shutdown()` を実行する。監査ログ `node_shutdown` を記録する。
- エージェント管理 RPC はハブ中継でも同じ検証を通す。MCP の `environment` は
  書き込み専用で、読み取りの `AgentManagementSnapshot` には含めない。

### Client API / Hub

- ハブの `ClientApiBackend` にエージェント管理 4 メソッドの中継を追加した。
- ノード単位のエンドポイントを追加した（ハブは `NodeRpcRequest` で中継し、
  ローカルノードは同一プロセスで実行する）。
  - `GET /api/v1/nodes/:node_id/diagnostics`
  - `GET /api/v1/nodes/:node_id/audit/logs?limit=...`
  - `POST /api/v1/nodes/:node_id/shutdown`
- `AuditAction::NodeShutdown`（`node_shutdown`）を追加した。

### UI

- `Files` / `Changes` のハブ制限を撤去した。ハブ経由でもツリー閲覧・
  プレビュー・`git status` / diff / commit / pull / push を使える。
- チャットのファイルリンクとツールカードからの Dock オープンを接続種別に
  依存させない。
- エージェント管理（providers / models / MCP / skills / commands）の歯車を
  ハブ接続時にも表示する。
- 設定の Process / Audit タブをハブ接続時にも表示し、対象ノードの診断と
  監査ログを取得する。ハブ自身の Process タブは従来どおりハブの診断を出す。
- ノード単位の Process タブでは、ハブ全体に作用する kill switch を隠す。

## 不変条件

- 認証（Bearer / Cookie）、Host 検証、WS の Origin 検証はハブ・ノードで同一。
- workspace のパス検証と読み取り上限はハブ経由でもノードが実施する。
- 監査ログはノード側が中継操作を記録し、ハブ側も自身の操作を記録する。
- エージェントの credential / MCP の環境変数の値は API から返さない。

## 関連ドキュメント

- [`docs/03-protocol-and-api.md`](../../03-protocol-and-api.md) §3.1
- [`docs/07-web-ui-and-pwa.md`](../../07-web-ui-and-pwa.md) §2
- [`docs/backlog.md`](../../backlog.md) §6
