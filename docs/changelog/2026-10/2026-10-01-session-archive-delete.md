# セッションのアーカイブ / 削除機能 実装 (Session Archive / Delete)

- **日付**: 2026-10-01
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-node`, `fxg-server`,
  `fxg-cli`, `ui`
- **対象スクリプト**: `mise run check` / `mise run check:ui` / `mise run fmt` /
  `mise run sqlx:prepare` / `mise run types`
- **状態**: ✅ 実装完了 (2026-10-01)

---

## 0. ゴールとスコープ

セッションを**アーカイブ** (一覧から非表示。復元可能) および**削除**
(会話ログを消去。復元不能) できるようにする。

| 項目             | 決定                                                                              |
| ---------------- | --------------------------------------------------------------------------------- |
| アーカイブ       | `sessions.archived_at` を立てるだけの**可逆な可視性フラグ**。イベントログは保持   |
| 削除             | **tombstone 方式**: `sessions` 行と `SessionDeleted` イベントのみ残し、本文パージ |
| 実行中セッション | 削除時は**自動停止** (Kill → プロセス終了待ち) してから tombstone を追記          |
| Worktree         | 削除時も**残す** (ファイルの誤削除防止。`fxg worktree prune` に委ねる)            |
| 操作経路         | CLI + REST (ローカル/中央) + Web UI (サイドバー + セッションページ)               |

## 1. 背景・設計判断

### 1.1 なぜ tombstone 方式か

`node.db` (実行ノード) が一次ソースで、`server.db` (中央サーバー) は Outbox
同期で追いつく複製である。セッションを完全に物理削除すると:

1. サーバーは削除を知る手段がなく (ノードは「削除した」という情報すら送れない)、
   古いセッションが残り続ける。
2. 再接続時の Resync 判定 (`NodeHello` の `SessionSyncState` と
   `sessions.last_node_seq` の比較) が壊れる
   (削除済みか未送信かを区別できない)。
3. 他セッションの Fork 元参照 (`parent_session_id` の FK) が壊れ得る。

そこで削除は「`sessions` 行 + `SessionDeleted` イベント (tombstone) のみ残し、
**それ以前のイベント本文と `permission_requests` を物理 DELETE**」とする。
残るデータは数百バイトで、UI には一切表示されない
(`sessions.deleted_at IS NOT NULL` は一覧・取得の双方から除外)。

### 1.2 イベント型

- `SessionArchived { archived: bool }` (event_type: `session_archived`)
  - `archived = true` → `sessions.archived_at = created_at`
  - `archived = false` → `sessions.archived_at = NULL` (復元)
- `SessionDeleted {}` (event_type: `session_deleted`)
  - `sessions.deleted_at = created_at` + `archived_at = NULL`
  - `DELETE FROM session_events WHERE session_id = ? AND node_seq < <tombstone>`
    (tombstone 自身は残す)
  - `DELETE FROM permission_requests WHERE session_id = ?`

投影 (`apply_projections`) はイベント適用と全量再構築
(`rebuild_projections`) の双方から呼ばれるため、削除状態もイベントログから
再構築可能 (`archived_at` / `deleted_at` はリセット → 再生で復元)。

### 1.3 削除済みセッションへのガード

- 実行ノード: `append_next_event` の採番 UPDATE に `AND deleted_at IS NULL`
  を追加し、削除後に競合したイベント追記を `SessionNotFound` として拒否する。
- ハブ: `append_event_in_tx` で「削除済みセッションへは `SessionDeleted` のみ、
  未知セッションへは追記しない」。Resync 再送などでパージ済みの会話内容が
  復活するのを防ぐ。
  - **実装上の注意**: この判定は `INSERT` 文に `WHERE EXISTS (...)` を
    埋め込む**単文**で行う。`SELECT` → `INSERT` の 2 文に分けると、WAL の
    読み取りスナップショット取得後に書き込みへ昇格するため
    `SQLITE_BUSY_SNAPSHOT` (517, "database is locked"。
    `busy_timeout` では回復しない) を誘発し、イベント適用が恒久的に
    リトライ待ちになる (E2E で発見)。

### 1.4 同期

- 削除の伝播は通常の Outbox (`SessionDeleted` が `node_seq > 水位` で送信) に
  乗る。本文イベントはパージ済みのため、サーバーは tombstone のみを受け取り、
  自身のコピーもパージする。
- `NodeHello` はアーカイブ済みも報告する (`include_archived: true`) ことで、
  アーカイブ操作の Resync 安全網を維持する (削除済みは常に除外)。
- サーバーが削除済みセッションの `get_session` を `None` として扱うため、
  クライアント WS の `since_cursor` 巻き戻り時も削除済みセッションは復活しない。

### 1.5 実行中セッションの削除

`SessionManager::delete` は稼働中 (`sessions.active`) の場合に
`ControlSession { action: Kill }` を実行し、エージェントプロセスの終了
(最大 30 秒) を待ってから tombstone を追記する。タイムアウト時は警告のみで
続行する (以降のイベントは削除済みセッションへの追記として破棄される)。
再開処理中 (`resuming`) は `INVALID_STATE` で拒否する。

## 2. 変更点

### 2.1 `fxg-protocol`

- `UnifiedEventPayload::SessionArchived` / `SessionDeleted` (`events.rs`)
- `SessionSummary.archived_at: Option<i64>` (`ts-rs` 出力更新)
- `SessionArchiveRequest` / `SessionArchiveResponse` (`client_api.rs`)
- `ServerToNodeMsg::ArchiveSession` / `DeleteSession`、
  `NodeToServerMsg::ArchiveResult` / `DeleteResult` (`node_server.rs`)
- `IpcClientMessage::SessionArchive` / `SessionDelete`、
  `IpcResult::SessionArchived` / `SessionDeleted`、
  `ListSessions { include_archived }` (`ipc.rs`)

### 2.2 `fxg-db`

- migration `0004_sessions_archive_delete.sql`
  (`sessions.archived_at` / `sessions.deleted_at`)
- `SessionFilter.include_archived` (既定 `false` = 除外)。
  `list_sessions` / `get_session` は `deleted_at IS NULL` を常時適用
- `searchable.rs` は両イベントを FTS 対象外に追加
- 監査 action `session_archive` / `session_delete` (`audit.rs`)

### 2.3 `fxg-node`

- `SessionManager::{set_archived, delete, is_active}`。
  `delete` は自動停止 + tombstone 追記
- `DaemonState::{set_session_archived, purge_session}` (監査ログ付き共有実装。
  IPC / Client REST / 中央サーバー中継の 3 経路から利用)
- Client REST (`POST /sessions/:id/archive` / `DELETE /sessions/:id`) と
  IPC ハンドラ、`ServerToNodeMsg` ハンドラを配線

### 2.4 `fxg-server`

- `ClientApiBackend::{archive_session, delete_session}` と共通ルーターの
  ルート追加 (`archive` は `COMMAND_TIMEOUT`、`delete` は稼働中停止を待つため
  `SESSION_COMMAND_TIMEOUT`)
- `NodeHub::{archive_session, delete_session}` (pending/oneshot 相関)
- `GET /api/v1/sessions` に `include_archived` クエリ

### 2.5 `fxg-cli`

- `fxg session archive <id>` / `unarchive <id>` / `delete <id>`
- `fxg ps` / `fxg session list` に `--archived` (アーカイブ済みも表示)
- 内蔵 TUI はアーカイブ/削除イベントをシステム行として表示

### 2.6 `ui`

- `ApiClient::{archiveSession, deleteSession}` + `sessions({ includeArchived })`
- `SyncStore::refreshSessions` は `include_archived: true` で取得し、
  サイドバー側で「アーカイブ済み」セクションに分離。
  `session_deleted` 受信時はローカルのタイムライン/バッファも破棄
- `SessionActionsMenu` (アーカイブ/復元 + 削除確認ダイアログ) を新設し、
  サイドバー各行とセッションページヘッダーから利用
- セッションページにアーカイブ済みバナー (解除ボタン付き)。
  削除後は `/` へ遷移
- NEEDS ATTENTION は承認待ちをアーカイブ済みセッションからも拾う
  (承認の見落とし防止)

## 3. テスト

- `fxg-db`:
  - `session_archiving_hides_from_default_list_and_is_reversible`
    (一覧除外 / `include_archived` / 全量再構築での維持 / 復元)
  - `session_deletion_purges_content_and_keeps_tombstone`
    (本文・承認履歴のパージ、tombstone のみの Outbox、削除後の追記破棄、再構築)
  - `session_deleted_for_unknown_session_is_noop` (FK 安全)
- `fxg-node` E2E (`sync_e2e.rs`):
  `central_server_archives_and_deletes_session`
  (中央サーバー REST → ノード → Outbox 同期 → 両 DB で tombstone のみ +
  監査ログ)
- `mise run check` / `mise run check:ui` 通過
