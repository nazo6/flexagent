# ノード側ペアリングの DB 所有化と GUI 化（計画）

- **日付**: 2026-11-11
- **状態**: 計画（実装は同じ PR で継続）
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-node`, `fxg-hub`,
  `fxg-cli`, `ui`, `docs`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run lint`,
  `mise run lint:errors`, `mise run types`, `mise run sqlx:prepare`,
  `mise run test`, `pnpm --dir ui run check`
- **関連ドキュメント**:
  - [`docs/01-architecture-and-sync.md`](../01-architecture-and-sync.md) §2.2, §7.3
  - [`docs/02-database-schema.md`](../02-database-schema.md)
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md)
  - [`docs/05-cli-and-pwa-ui.md`](../05-cli-and-pwa-ui.md) §2, §3.2
  - [`2026-11-09-settings-consolidation-and-project-identity.md`](2026-11-09-settings-consolidation-and-project-identity.md) §3.3
  - [`2026-11-11-node-pairing-ux.md`](2026-11-11-node-pairing-ux.md)

---

## 0. この文書の位置づけ

- 本計画は **1 PR（計画 + 実装）** で進める。§5 のフェーズ順にコミットを積み、
  §8 のチェックリストだけを更新する。
- 設計判断を変える場合は §3 の決定表に追記し、実装と同時に更新する。
- 完了後も本文は残す（後から背景を追えるようにする）。

---

## 1. 背景と現状

### 1.1 ハブ側は既に DB 所有 + GUI

| 項目         | 現状                                                                          |
| :----------- | :---------------------------------------------------------------------------- |
| トークン保存 | `hub.db.nodes.token_hash` に SHA-256 ハッシュのみ（平文は保存しない）         |
| API          | `GET/POST /api/v1/nodes/tokens`, `DELETE /api/v1/nodes/tokens/{id}`           |
| Web UI       | 設定 > 一般 でノード ID を指定して発行・失効                                  |
| 監査         | `node_token_manage`                                                           |
| CLI          | `fxg hub pairing-token issue\|revoke\|list`（同じ DB を操作する薄いラッパー） |

つまり「設定ファイルベース」なのは **ノード側の半分だけ** である。

### 1.2 ノード側はファイル + 再起動前提

| 項目     | 現状                                                                                    |
| :------- | :-------------------------------------------------------------------------------------- |
| 接続先   | `~/.flexagent/config.toml` の `[node] hub_url`                                          |
| トークン | `~/.flexagent/node_token`（`0600`）                                                     |
| 変更経路 | `fxg node pair\|unpair` がファイルを直接書く                                            |
| 反映     | `fxg node restart` が必要（sync worker が起動時に URL / トークンを値で保持）            |
| 接続状態 | `hub_connected: bool` のみ。401 も到達不能も同じ `hub connection failed` 警告 + backoff |
| 上書き   | `FXG_HUB_URL` / `FXG_NODE_TOKEN` / `fxg node start --hub-url`                           |

### 1.3 課題

1. **GUI からペアリングできない**。`fxg web` を主インターフェースとする設計
   （`docs/05` §1）と矛盾する。CLI を開けない環境では手順が成立しない。
2. **`fxg node pair` が唯一ノード API を通らないコマンド**。「CLI はループバック
   HTTP/WS のみを使う」という原則（`AGENTS.md` §1）から外れており、CLI と GUI で
   実装が二重化する。
3. **失敗理由が見えない**。トークン失効（401）と到達不能が同じ扱いなので、
   ユーザーは何を直せばよいか分からない。
4. **config.toml が実行時に書き換わる**。`config.toml` は「プロセス起動時に
   必要な値だけ」を持つ方針（`docs/05` §2）なのに、ペアリング操作がこれを
   書き換えている。所有権が曖昧。
5. **ハブ切り替え時の再同期が場当たり**。新ハブは全履歴のフル再送を受け取るが、
   その間の Web Push 洪水・セッション数上限での欠落が未処理（§4.9）。
6. **環境変数が 2 つ目の真実になっている**。`FXG_HUB_URL` / `FXG_NODE_TOKEN` は
   `config.toml` の値と独立に効くため、GUI から変更しても環境変数が勝ち、
   Docker では画面が読み取り専用になる。秘密が `docker inspect` に出る。

### 1.4 変えないもの

- Web access token（`auth_token`）の仕組み。
- ノード → ハブの outbound WS、`Authorization: Bearer <node_token>`、
  ハブが `node_id` 一致を検証する方式。
- 一時 VM（`fxg internal stdio`）のパイプ直結。ネットワーク越しのトークンは
  引き続き発生しない。
- `FXG_NODE_ID` / `FXG_NODE_NAME`（DB 行を初めて作るときの既定値）と、
  `FXG_NODE_LISTEN_ADDR` / `FXG_HUB_LISTEN_ADDR` などの起動設定。

---

## 2. 目的と非目標

### 目的

- ノードのペアリング状態（接続先 + トークン）を **ノードが所有する実行時設定**
  として扱い、GUI / CLI / API のどれからでも同じ経路で変更できるようにする。
- ペアリング変更を **ノード再起動なしで反映** し、接続状態と失敗理由を
  GUI に表示する。
- ハブ切り替え時の再同期を、少なくとも「何が起きるか」が分かる状態にする。

### 非目標

- ハブからノードへの push 型ペアリング（ノードは outbound のみなので不可能）。
- 旧ハブに残ったデータの自動削除・移行。切り替えは「新ハブへ全履歴を再送」
  というセマンティクスに固定する。
- トークン平文の DB 保存、API からの再表示。
- ペアリング処理の i18n 以外の UI 大改修（設定画面の 1 タブに閉じる）。

---

## 3. 設計決定

| ID  | 決定                                                                                                                                                    |
| :-- | :------------------------------------------------------------------------------------------------------------------------------------------------------ |
| D1  | `hub_url` の所有者を `node.db` に移す。`config.toml` の `[node] hub_url` は廃止し、互換読み込みはしない。                                               |
| D2  | ペアリングは `node_pairing`（単一行テーブル）に置き、`node_settings` とは分ける。設定 PATCH と副作用（再接続）を混ぜない。                              |
| D3  | トークン平文は DB に入れない。固定パス `~/.flexagent/node_token`（`0600`）を維持し、`secrets` テーブルには `system.node_token` のメタデータだけを置く。 |
| D4  | `FXG_HUB_URL` / `FXG_NODE_TOKEN` / `fxg node start --hub-url` を **廃止** する。ノードのペアリングは `node.db` が唯一の保存先で、上書き層は持たない。   |
| D5  | ペアリングはノード API（`PUT/DELETE /api/v1/nodes/{node_id}/settings/pairing`）に集約し、CLI はこれを呼ぶ。                                             |
| D6  | sync worker を `HubLink` supervisor に置き換え、`watch` チャネルで再起動不要の再設定を行う。                                                            |
| D7  | 接続状態は `hub_connected: bool` を `HubLinkStatus`（型付き enum）へ置き換える。401/403 は `Unauthorized` として区別する。                              |
| D8  | ペアリング変更を監査する。ハブ側は既存 `node_token_manage`、ノード側は `pairing_update` を追加し、トークン値は記録しない。                              |
| D9  | ハブ切り替え時のフル再送では Web Push を抑止する（`EventBatchPush.resync`）。クライアント WS 配信は通常どおり行う。                                     |
| D10 | セッション数上限（1000）で移送が欠ける場合は、`NodeHello.sessions_truncated` で通知し GUI に表示する。ページングは今回は行わない。                      |
| D11 | 別ノード所有のセッションに対する `ResyncRequest`（無限リトライになる分岐）は、再送要求ではなく競合として記録しスキップする。                            |

---

## 4. 設計

### 4.1 所有権と優先順位

| 設定         | 所有者 / 保存先                                        | 変更手段                                    |
| :----------- | :----------------------------------------------------- | :------------------------------------------ |
| `hub_url`    | `node.db` (`node_pairing.hub_url`, HTTP origin で保存) | Client API / Web UI / `fxg node pair`       |
| `node_token` | `~/.flexagent/node_token`（`0600`、固定パス）          | Client API（書き込み専用）/ `fxg node pair` |
| `node_id`    | `config.toml` または DB 自動採番                       | 変更なし（読み取り専用 + コピー）           |
| 接続状態     | メモリ（`HubLinkStatus`）、再起動で初期化              | 読み取り専用                                |

ノードのペアリングに優先順位は無い。`node.db` の値だけを見る。

```text
node.db の node_pairing.hub_url + ~/.flexagent/node_token → ペアあり
どちらかが欠けている                                       → standalone
```

- 環境変数と `--hub-url` は廃止する。`DaemonConfig` から `hub_url` / `node_token`
  フィールド自体を削除し、テストも DB を seed する。
- `config.toml` に `[node] hub_url` が残っていれば **警告ログ** を出し、
  `fxg doctor` と設定カタログにも「無視されている（削除してください）」と表示する
  （読み込みはしない）。
- 保存形式は既存の `fxg node pair` に合わせて **HTTP origin**
  （`http://host:port`）とし、WS エンドポイントは `node_ws_url()` で導出する。

#### CLI のハブ探索（ノードのペアリングとは別物）

`fxg session new --provisioner` と `fxg web hub` は、ハブの REST API を直接叩く
ためにハブの場所を知る必要がある。ここは**ノードのペアリング状態ではなく
CLI の関心事**なので、次の順で解決する。

```text
1. --hub <URL>                                   (CLI フラグ)
2. hub.addr                                      (ローカルで稼働中のハブ)
3. ローカルノードの pairing.hub_url（ノード API）
4. [hub] listen_addr
```

- 3 を追加するため、CLI は既存どおりノードを自動起動して
  `GET /api/v1/nodes/{node_id}/settings` を読む（`crates/fxg-cli/src/hub_api.rs`）。
- ノードが未ペアリングのホストから `--provisioner` を使う場合は 1 を指定する。
- ここでの `--hub` は CLI の接続先であり、ノードのペアリングを変更しない。
  `fxg node pair --hub` とは別コマンドなので混同しない。

### 4.2 永続化

```sql
-- 0011_node_pairing.sql (node.db only)
CREATE TABLE node_pairing (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    hub_url     TEXT,
    revision    INTEGER NOT NULL DEFAULT 1,
    updated_at  INTEGER NOT NULL
);
```

- `updated_at` 更新と `revision` のインクリメントは `node_settings` と同じ流儀。
- トークンは既存の固定パス `~/.flexagent/node_token` を維持する
  （Docker 手順・`fxg doctor`・既存ドキュメントがこのパスを参照しているため）。
  `secrets` テーブルには `system.node_token` のメタデータ行を起動時に upsert し、
  設定画面の秘密一覧に「ノードペアリングトークン（システム）」として
  `configured` 付きで表示する。
- `fxg-db::secrets` に「システム秘密は固定パスへ解決する」関数を 1 つ置き、
  `is_configured` / `read_secret` をそこに通す（`system.auth_token` も同じ扱いに
  できるが、今回の対象は `system.node_token` のみ）。
- 汎用 Secret API（`PUT /api/v1/nodes/{id}/secrets/{secret_id}`）は `system.` 接頭辞の
  ID を拒否する（`SECRET_INVALID`）。トークンの書き込みはペアリング API だけが行う。
- 起動時の解決順で読むのは `node_pairing.hub_url` とトークンファイルのみ。
  ファイルが無ければ `token_configured = false`。

### 4.3 型と API 契約

`fxg-protocol/src/settings.rs`:

```rust
pub struct PairingSettings {
    pub hub_url: Option<String>,  // node_pairing.hub_url（HTTP origin）
    pub token_configured: bool,   // ~/.flexagent/node_token が存在する
    pub revision: u64,
    pub connection: HubLinkStatus,
    pub sessions_truncated: bool, // 直近の NodeHello で上限に達した
}

pub struct HubLinkStatus {
    pub state: HubLinkState,
    pub hub_url: Option<String>,
    pub since: Option<i64>,
    pub message: Option<String>,
}

pub enum HubLinkState {
    Standalone,
    Connecting,
    Connected,
    Unauthorized,
    Failed,
}
```

`NodeSettingsSnapshot` に `pairing: PairingSettings` を追加する
（`bootstrap` / `node` / `agents` と同じ階層）。`NodeBootstrapView` からは
`hub_url` を削除する（出所を偽らないため）。

| メソッド | パス                                            | 内容                                                          |
| :------- | :---------------------------------------------- | :------------------------------------------------------------ |
| `PUT`    | `/api/v1/nodes/{node_id}/settings/pairing`      | `{ hub_url, token?, expected_revision }` を保存し、即時再接続 |
| `DELETE` | `/api/v1/nodes/{node_id}/settings/pairing`      | トークン削除 + `hub_url = NULL`。outbox は保持                |
| `POST`   | `/api/v1/nodes/{node_id}/settings/pairing/test` | 保存せずに WS ハンドシェイクだけ試す（GUI の接続テスト）      |

- ペアリングはノード所有の状態なので、`NodeRpcRequest` に `UpdatePairing` /
  `ClearPairing` / `TestPairing` を追加し、`GetNodeSettings` の応答
  （`NodeSettingsSnapshot`）に `pairing` を載せる。更新系は既存の
  `NodeRpcResponse::NodeSettings` を返し、`TestPairing` だけ
  `NodeRpcResponse::HubLink` を返す。ハンドラは `execute_node_rpc` を呼ぶだけで、
  ハブ経由は既存のプロキシがそのまま働く（新しい `ClientApiBackend`
  メソッドは不要）。
- `token` は `Option`。`None` かつ接続先変更なしなら既存トークンを維持し、
  接続先が変わった場合は `AUTH_REQUIRED` を返す（別ハブに旧トークンを送らない）。
- `expected_revision` は楽観ロック（`REVISION_MISMATCH`）。`DELETE` も同じ
  revision を要求する。
- 保存先が 1 つしかないので `INVALID_STATE` の「上書き中」分岐は不要。
  汎用 Secret API の `system.*` への書き込みは `SECRET_INVALID`。
  **`ErrorCode` の新規バリアントは追加しない**（`ALL` / i18n カタログの契約を
  増やさないため）。
- `GET` はトークンを返さない。`token_configured` と `secrets` メタデータの
  ハッシュ先頭 12 文字まで。

### 4.4 接続ライフサイクル

`crates/fxg-node/src/daemon/mod.rs` の `start_sync_worker` を置き換える。

```text
DaemonState
  └─ hub_link: HubLink
       ├─ watch::Sender<Option<HubTarget>>   // None = standalone
       └─ supervisor task
            └─ sync::run(state, target_rx)
                 ├─ target 変更 / 切断で接続を落として再接続
                 ├─ 401/403 → HubLinkState::Unauthorized（max backoff で再試行）
                 └─ それ以外 → HubLinkState::Failed(message)
```

- `NodeDaemon` 終了時は supervisor を shutdown する（既存 `watch` を流用）。
- 状態変化は `HubWsMessage::SystemStatus` で Client WS に流す。このメッセージは
  定義だけがあって送信されておらず、UI の同期バッジが常に「未接続」になっていた
  ため、`ClientApiBackend::hub_link_updates()`（`watch::Receiver`）を足して
  `ws.rs` の `select!` から送るようにする。
- `SystemInfoResponse.hub_connected` を `hub_link: Option<HubLinkStatus>` に置き換え、
  `ui/src/lib/stores/sync.svelte.ts` の `centralConnected: bool` も
  `hubLink: HubLinkStatus` に置き換える（bool を残さない）。
- `role_health` の Sync コンポーネントは `HubLinkState` をそのまま写像する。
- **接続テスト**は `HubLink` の実装（ハンドシェイク）を共有し、
  401/403 と到達不能を同じ分類で返す。

### 4.5 CLI

- `fxg node pair --hub <URL> --token <TOKEN>`:
  ノードが起動していなければ他コマンドと同様に自動起動し、ループバック API の
  `PUT .../settings/pairing` を呼ぶ。出力は「即時反映済み」に変更。
- `fxg node unpair`: `DELETE .../settings/pairing`。
- `set_hub_url`（`toml_edit` による config.toml 書き換え）を削除する。
- `fxg node start --hub-url` と `FXG_HUB_URL` / `FXG_NODE_TOKEN` を削除する
  （`DaemonConfig` の該当フィールドも削除）。
- `crates/fxg-cli/src/hub_api.rs` のハブ探索を §4.1 の順
  （`--hub` → `hub.addr` → ノード API の `pairing.hub_url` → `[hub] listen_addr`）
  に付け替える。`fxg session new --provisioner` と `fxg web hub` に `--hub` を追加。
- `fxg node status` と `fxg doctor` に `HubLinkStatus` と
  「config.toml の `[node] hub_url` は無視されている」を追加する。

### 4.6 UI

設定画面（ノード接続時のみ、`ui/src/routes/settings/+page.svelte`）:

- **ハブ接続** タブ:
  - 状態バッジ（未設定 / 接続中 / 接続済み / 認証エラー / 接続失敗）
  - ハブ URL 入力（`http://host:port`、`ws://.../api/v1/node/ws` も受理）
  - トークン入力（`type=password`、`autocomplete="off"`、既存時は
    「設定済み（変更する場合のみ入力）」）
  - 「接続テスト」→ 「ペアリング」/「切断」
  - 自分の `node_id` を大きなコード + コピー（ハブ側での発行に必要）
  - `sessions_truncated` 時は「1000 セッションを超える分は移送されません」
- **切り替え確認ダイアログ**: 接続先が変わる場合に
  「新しいハブへ全履歴を再送します。旧ハブのデータは残ります。未同期分は
  新しいハブにだけ送られます」を表示。
- ハブ設定 > 一般 のトークン発行ダイアログに、ノード側の導線
  （`http://localhost:7860` → 設定 → ハブ接続）を追記。
- 設定カタログ（`SettingsCatalog.svelte`）から `bootstrap.hub_url` の行を削除し、
  ペアリングタブへ誘導する。

### 4.7 Docker / コンテナ

環境変数を廃止しても、コンテナの宣言性は次のどちらかで維持する。

- **トークンをファイルでマウント**（推奨）:
  `- ./node_token:/data/node_token`（rw）。`docker inspect` や
  `/proc/<pid>/environ` に秘密が出ない。ハブ URL はコンテナ DNS 名
  （`http://hub:8080`）を初回だけ GUI で入力する。
- **`docker compose exec` で一度だけペアリング**:
  `docker compose exec node node pair --hub http://hub:8080 --token <TOKEN>`
  （entrypoint が `fxg` なので引数は `node pair ...`）。ホット適用なので
  `restart` は不要。`node.db` は named volume に残る。

`compose.yaml` から `FXG_HUB_URL` と `FXG_NODE_TOKEN` を削除し、
コメントを上記手順に差し替える。`FXG_NODE_ID` / `FXG_NODE_NAME` は
「初回起動時に DB 行を作るときの既定値」という既存の意味のまま残す。

### 4.8 監査

- `AuditAction::PairingUpdate => "pairing_update"` を追加。
- ノード側 `node.db` に「ペアリング変更（接続先の origin、トークン変更の有無、
  接続元）」を記録する。トークン値・ハッシュは記録しない。
- ハブ側の発行・失効は既存 `NodeTokenManage` のまま。

### 4.9 ハブ切り替え時の同期セマンティクス

現状の挙動（`crates/fxg-hub/src/hub.rs::compute_resync`）:

- 新ハブはセッション未知名 → `from_node_seq = 1` → `SessionCreated` から全再送。
- 水位 `sessions.synced_up_to_node_seq` はノード側にあり、ハブを識別しない。
  再送起点はハブ側の `last_node_seq` なので、A → B → A も差分で自己修復する。
- 旧ハブはオフライン表示になるだけで、データは残る。

追加する対応:

1. `EventBatchPush { events, #[serde(default)] resync: bool }` を追加し、
   `ResyncRequest` 由来の送信を `resync = true` にする。ハブは `resync` の間、
   Web Push（`notify_events`）を抑止する。クライアント WS への配信は行う。
2. `NodeHello { #[serde(default)] sessions_truncated: bool }` を追加。
   ノードは `sessions.len() == 1000` のとき true を立て、GUI と `fxg doctor` に出す。
3. `compute_resync` の「別ノード所有」分岐は `ResyncRequest` を出さず、
   `tracing::warn` + 診断カウンタで競合として記録する（現状は
   `SessionCreated.node_id` 検証で拒否され続け、再接続のたびに無限リトライ）。
4. ペアリング切り替え確認ダイアログ（§4.6）に上記セマンティクスを明記する。

---

## 5. 実装フェーズ

### Phase 1: 接続状態の型付き化と 401 の区別

- `fxg-protocol`: `HubLinkState` / `HubLinkStatus`、`SystemInfoResponse` /
  `HubWsMessage::SystemStatus` の置き換え。
- `fxg-node`: `AtomicBool` を `watch::Sender<HubLinkStatus>` へ。`sync.rs` で
  `tungstenite::Error::Http` の status を見て `Unauthorized` を判定。
  失効トークンの e2e（`revoked_pairing_token_is_reported_as_unauthorized`）を追加。
- `fxg-hub`: `SystemExtras` と Client WS の `hub_link_updates` 対応。
- `ui`: `sync.svelte.ts` と接続状態バッジの追随（`shell_hub_*` の文言を追加）。
- 完了条件: 失効トークンで `Unauthorized` が UI に出る。既存テストが通る。

### Phase 2: DB 所有化 + ペアリング API + ホット適用

- `fxg-db`: `0011_node_pairing.sql`、`settings.rs` / `db.rs`、
  `.sqlx` 再生成（`mise run sqlx:prepare`）。
- `fxg-protocol`: `PairingSettings`、`NodeSettingsSnapshot`、
  `NodeRpcRequest::{UpdatePairing, ClearPairing, TestPairing}`、
  `NodeRpcResponse::HubLink`。`DaemonConfig` から `hub_url` / `node_token` を削除し、
  `NodeBootstrapView` から `hub_url` を削除。
- `fxg-hub`: ルート 3 本を `execute_node_rpc` に接続（プロキシは既存のまま）。
- `fxg-node`: `DaemonState::pairing_settings` / `update_pairing` / `clear_pairing` /
  `test_pairing`、`rpc.rs` のディスパッチ、`HubLink` supervisor、
  トークンファイル + `secrets` メタデータ、`AUTH_REQUIRED` の返却。
- `config.toml` の `[node] hub_url` を読まない + 残留時の警告。
- `compose.yaml` から `FXG_HUB_URL` / `FXG_NODE_TOKEN` を削除（§4.7 の手順に差し替え）。
- 完了条件: ペアリング API で接続が即時確立し、unpair で standalone に戻る。
  再起動後も維持される。

### Phase 3: GUI

- `client.ts` に 3 メソッド、設定画面のハブ接続タブ、切り替え確認、
  `sessions_truncated` 表示。
- `mise run types` で bindings を同期。
- 完了条件: ブラウザだけでペアリング → 接続済み → 失効 → 認証エラー表示が回る。

### Phase 4: CLI 付け替え + 診断

- `fxg node pair|unpair` を API 経由へ。`set_hub_url` と `--hub-url` を削除。
- `hub_api.rs` のハブ探索を `--hub` → `hub.addr` → ノード API → `[hub] listen_addr`
  へ付け替え、`fxg session new --provisioner` / `fxg web hub` に `--hub` を追加。
- `fxg node status` / `fxg doctor` / support bundle にペアリング情報。
- 完了条件: CLI と GUI で同じ結果になる。config.toml が書き換わらない。
  コンテナ内の `fxg node pair` でペアリングできる。

### Phase 5: ハブ切り替えの同期セマンティクス

- `resync` フラグ + Push 抑止、`sessions_truncated`、`compute_resync` の競合分岐修正。
- 完了条件: 新ハブへの初回接続で Push が飛ばず、上限到達が UI に出る。

### Phase 6: ドキュメント

- `docs/05` §2 の所有者表 / §2.2 config.toml スキーマ / §3.2 設定画面。
- `docs/01` §7.3 のペアリング手順（GUI を主、CLI を従）。
- `docs/02` に `node_pairing`、`docs/03` に API 3 本。
- 本文書の §8 を更新。

---

## 6. 検証

- **DB**: `node_pairing` の revision 楽観ロック、unpair 後の再ペアリング。
- **同期 e2e**（`crates/fxg-node/tests/sync_e2e.rs`）:
  - 既存テストのセットアップを「`node_pairing` + トークンファイルを seed」へ変更
    （`DaemonConfig` から `hub_url` / `node_token` を削除する）。
  - 追加: 失効トークンで `Unauthorized`、ペアリング API 変更後の即時再接続、
    新ハブ接続で全再送 + Push 抑止（`notify_events` の抑止を単体で確認）。
- **ハブ**: `compute_resync` の別ノード所有分岐が再送を要求しないこと。
- **CLI**:
  - `fxg node pair` が `config.toml` を変更しないこと。
  - ハブ探索が `--hub` → `hub.addr` → ノード API → `[hub] listen_addr` の順に
    解決されること。
  - `FXG_HUB_URL` / `FXG_NODE_TOKEN` / `--hub-url` が無視される（存在しない）
    こと。
- **UI**: `pnpm --dir ui run check`, `pnpm --dir ui run test`。
- **全体**: `mise run fmt`, `mise run lint`, `mise run lint:errors`,
  `mise run test`。
- **手動**: Xvfb 上で `fxg web` を開き、ペアリング → 切断 → 再接続、
  別ハブへの切り替え確認ダイアログ、失効後の表示を確認する。

---

## 7. リスクと未決事項

| 項目                             | 内容                                                            | 対応                                                                                                 |
| :------------------------------- | :-------------------------------------------------------------- | :--------------------------------------------------------------------------------------------------- |
| 誤った接続先の即時反映           | 保存した瞬間に同期が切れる                                      | 「接続テスト」を保存前の任意操作として用意し、失敗時は保存しない導線を既定にする                     |
| 全履歴再送のコスト               | セッションが多いと初回接続が長時間・大容量になる                | `sessions_truncated` と進捗表示。ページングは将来課題                                                |
| 旧ハブの扱い                     | 「移行した」と誤解されやすい                                    | ダイアログと docs で「旧ハブは残る」を明記。削除は将来の `fxg hub detach`                            |
| `node_id` の使い回し             | 同名 `node_id` を別マシンで使うとトークン・セッションが衝突する | GUI で `node_id` を明示し、`fxg doctor` で警告                                                       |
| コンテナの宣言性                 | env を消すと compose が「起動するだけ」でペアリングされない     | トークンのファイルマウントと `docker compose exec` の 2 手順を初回セットアップとして docs 化（§4.7） |
| 秘密が env に載っていた          | `docker inspect` や `/proc/<pid>/environ` から見えていた        | 廃止により 0600 のファイルのみになる（副次的な改善）                                                 |
| トークン受け渡しの UX            | 平文を一度だけ表示する現行方式はコピー漏れしやすい              | 今回は貼り付け方式を維持。短命ワンタイムコードは別計画                                               |
| 未決: `fxg node pair` の自動起動 | 停止中のノードを勝手に起動する挙動になる                        | 他コマンドと一貫させる方針で実装し、不都合なら「停止中は DB 直書き」を別途検討                       |

---

## 8. 進捗チェックリスト

- [x] Phase 1: 接続状態の型付き化と 401 の区別
- [x] Phase 2: DB 所有化 + ペアリング API + ホット適用
- [x] Phase 3: GUI
- [x] Phase 4: CLI 付け替え + 診断
- [ ] Phase 5: ハブ切り替えの同期セマンティクス
- [x] Phase 6: ドキュメント
- [ ] 検証（§6）完了（Phase 5 の項目を除く）

### 実装メモ

- `node_pairing` は migration `0019_node_pairing.sql`（`0011` は既存の
  `incidents`、以降も既存番号が埋まっていたため末尾に追加）。
- ペアリング API は `NodeRpcRequest::{UpdatePairing, ClearPairing, TestPairing}`
  としてノード RPC に載せ、REST ハンドラは既存の `execute_node_rpc` を呼ぶだけ。
  ハブ経由のプロキシは追加実装なしで動く（未ペアリングのノードには元々届かない）。
- トークンは固定パス `~/.flexagent/node_token` のまま。`secrets` テーブルに
  `system.node_token` のメタデータを登録し、汎用 Secret API からは
  `SECRET_INVALID` で拒否する。
- `DaemonConfig` から `hub_url` / `node_token` を削除し、環境変数
  (`FXG_HUB_URL` / `FXG_NODE_TOKEN`) と `--hub-url` も削除した。テストは
  `node.db` + トークンファイルを seed する。
- ハブ探索は `--hub` → `hub.addr` → ノード API の `pairing.hub_url` →
  `[hub] listen_addr`。`fxg web` と `fxg session new` に `--hub` を追加した。
- 実機確認: hub を起動してトークン発行 → 未起動のノードを `fxg node pair` で
  ペアリング → `hub: connected`、`unpair` で即 `standalone`、誤トークンで
  `unauthorized (HTTP 401)` を確認。`config.toml` は生成されず、`node_token` は
  `0600`。
