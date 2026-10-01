# 02. データベーススキーマ設計（`node.db` / `server.db` 単一スキーマ）

FlexAgentは外部DBミドルウェアを必要としない **SQLite (`sqlx`)**
を採用し、WALモード (`PRAGMA journal_mode = WAL;`) で動作します。
E2E暗号化を行わないため、中央サーバー側で会話履歴の構造化クエリおよび
**FTS5（全文検索）** がそのまま利用可能です。

`node.db`（各ノード: `~/.flexagent/node.db`）と `server.db`（中央サーバー:
`~/.flexagent/server.db`）は、**同一のDDL・同一のテーブル名・単一のマイグレーションセット**
（`crates/fxg-db/migrations/`）から生成される「1つのスキーマ」を共有します。
両者の違いは **「行のスコープ」と「実行ロール」だけ**
であり、スキーマ対称性は「規約」ではなく 「単一定義」によって保証されます。

---

## 0. 単一スキーマ原則

### 0.1 3つの原則

1. **スキーマは1本**: 役割専用のテーブル（`push_subscriptions` 等）やカラム
   （`synced_up_to_node_seq` / `git_bundle_path`
   等）も両DBに作成し、使わない側では 空 /
   未使用のまま放置する。DDL・クエリ・マイグレーションを1本に保つことを優先する
   （空テーブル・未使用カラムの実害はない）。
2. **同期はイベントログ1本**:
   セッションのメタデータ（作成・タイトル・ステータス・モード）も
   `session_events` 上のイベントとして記録する。中央サーバーの `sessions` /
   `permission_requests` はイベントから適用される**投影（projection）** であり、
   別系統のメタデータ同期（旧 `SessionUpsert` /
   `metadata_synced`）は存在しない。
3. **書き込み権威は1つ**: セッション状態を書き換えられるのは**実行ノードのみ**。
   中央サーバーは (a) クライアントコマンドの転送 (b)
   受信イベントの追記と投影適用
   だけを行い、クライアント要求に応じてセッション状態を直接書き換えない （例外は
   `git_bundle_path` のようなハブ固有の管理フィールドのみ）。

### 0.2 ロール差分（ノード / 中央サーバー）

| 項目                                 | ノード (`node.db`)                                   | 中央サーバー (`server.db`)                                         |
| :----------------------------------- | :--------------------------------------------------- | :----------------------------------------------------------------- |
| `nodes`                              | 自分自身の1行のみ（FK参照・ローカルUI用）            | 全ノード（`NodeHello` で更新。`token_hash` はハブのみ使用）        |
| `projects` / `project_node_bindings` | 自ノードが関与する分のみ                             | 全ノード分（ノード報告の集約。ハブが権威）                         |
| `sessions`                           | **権威**。自分のセッションのみ。直接書き込む         | **投影**。全ノード分。イベント適用でのみ更新                       |
| `session_events`                     | 自分のセッションのイベント。水位より後が Outbox 対象 | 全イベントの冪等追記。`cursor` はハブ配信用カーソル                |
| `session_events_fts`                 | ローカル直結UIの検索用                               | 全セッション横断検索用                                             |
| `permission_requests`                | 自分のセッション分の投影                             | 全会話分の投影（グローバル承認 Inbox）                             |
| `push_subscriptions`                 | 空（未使用）                                         | PWA 購読管理（VAPID）                                              |
| `audit_logs`                         | ローカル直結操作（CLI / `localhost:7860`）の記録     | ハブ経由操作の記録（同期対象外。各ストアが自分の観測事実のみ記録） |
| `sessions.synced_up_to_node_seq`     | 使用（ハブが ACK した水位）                          | 未使用（常に 0）                                                   |
| `sessions.git_bundle_path`           | 未使用（NULL）                                       | 使用（一時VMの退避 Git bundle パス）                               |
| `sessions.last_node_seq`             | 永続化済み最新の `node_seq`（次はこの値 + 1 を採番） | 投影に適用済みの最大 `node_seq`                                    |

### 0.3 イベント適用と投影

中央サーバーは `EventBatchPush` を受信すると、**同一トランザクション**で
イベントを `INSERT OR IGNORE`（`event_id` / `(session_id, node_seq)` の UNIQUE
制約で 冪等）し、続けて投影を更新します。投影の更新規則は以下で固定です：

```text
SessionCreated        → sessions 行の生成（project_id / node_id / local_path /
                        git_branch / is_worktree / agent_id /
                        parent_session_id / fork_from_node_seq / title / created_at）
                        ※ node_id は SessionCreated payload が保持する
                          (イベントログのみから投影を完全再構築可能にするため)
SessionTitleChanged   → sessions.title
SessionAgentBound     → sessions.agent_session_id
StatusChanged         → sessions.status
CapabilitiesUpdated   → sessions.current_mode / available_modes_json /
                        available_commands_json / config_options_json
すべてのイベント      → sessions.last_node_seq = 適用済み最大 node_seq
                        sessions.updated_at = 最新イベントの created_at
PermissionRequest     → permission_requests 行を upsert (status = 'pending')
PermissionResolved    → permission_requests.status / resolved_by / resolved_at
```

- 投影はイベントログから**完全に再構築可能**です。`fxg-db` は再構築関数
  （イベント全量からの再生）を提供し、テストで「イベント適用後の状態 ==
  全量再構築後の状態」を 検証します。
- 未知のセッションのイベントは、バッチ内の `SessionCreated`（`node_seq = 1`）を
  先に適用することで FK 制約を満たします（イベントは `node_seq`
  昇順で送信される）。

### 0.4 同期の水位（ACK）とカーソル

- **水位（Watermark）**:
  「送信済み」は行単位フラグではなく、セッション単位の水位
  `sessions.synced_up_to_node_seq` で表現します。`node_seq`
  は実行ノードだけが採番する
  欠番のない連番のため、水位による表現が成立します。Outbox 抽出は
  `node_seq > synced_up_to_node_seq` の単純な述語になり、ACK 時の更新も O(1)
  です （旧設計の `synced = 0` フラグ + partial index は廃止）。
- **カーソル (`session_events.cursor`)**: そのDB（`node.db` / `server.db`）が
  イベントを取り込んだ順の AUTOINCREMENT 連番です。クライアント（CLI / Web
  UI）の 差分再開カーソルとして使用します。**接続先ストア以外の cursor
  は意味を持ちません** （ハブ接続時はハブの cursor、ローカル直結時はノードの
  cursor を使用）。
  - 重複イベントは IGNORE されるため cursor には欠番が生じ得ますが、再開は
    `cursor > 最後に受信した cursor` で行うため欠番は問題になりません。

---

## 1. 共通スキーマ DDL

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

-- 1. ノード登録簿
--    ノード側: 自分自身の1行のみ（FK参照・ローカルUI表示用。起動時に upsert）
--    ハブ側: 全ノード（NodeHello で更新されるハブ権威データ。token_hash はハブのみ使用）
CREATE TABLE nodes (
    node_id         TEXT PRIMARY KEY,               -- UUID または 固定スラッグ (例: "win-desktop", "eph-0195...")
    name            TEXT NOT NULL,                  -- 表示名 (例: "Home Windows PC", "Colab Pro (T4)")
    os              TEXT NOT NULL,                  -- "windows" | "linux" | "macos"
    arch            TEXT NOT NULL,                  -- "x86_64" | "aarch64"
    version         TEXT NOT NULL,                  -- fxg バイナリバージョン
    token_hash      TEXT,                           -- ノード個別 node_token のハッシュ (SHA-256)。NULL は未発行
    token_issued_at INTEGER,                        -- トークン最終発行日時 (Unix epoch ms, 任意)
    installed_agents_json TEXT NOT NULL DEFAULT '[]', -- 利用可能なエージェントID一覧 (JSON配列)
    is_ephemeral    INTEGER NOT NULL DEFAULT 0,     -- 0: 常駐ノード, 1: 一時VM/コンテナノード
    provisioner     TEXT,                           -- 一時ノードのプロビジョナー識別子 (例: "local-docker", "local-incus", "colab-pro")
    lifecycle_status TEXT NOT NULL DEFAULT 'ready', -- 'provisioning' | 'bootstrapping' | 'ready' | 'draining' | 'terminated' | 'error'
    idle_timeout_secs INTEGER,                      -- アイドル自動破棄までの秒数 (一時ノード用、例: 900)
    is_online       INTEGER NOT NULL DEFAULT 0,     -- 1: 接続中, 0: 切断
    last_seen_at    INTEGER NOT NULL,               -- Unix epoch (ms)
    created_at      INTEGER NOT NULL
);

-- 2. 論理プロジェクトテーブル
--    ノード側: 自ノードが関与する分のみ / ハブ側: 全ノード分の集約（ハブ権威）
CREATE TABLE projects (
    project_id      TEXT PRIMARY KEY,               -- 正規化キー (例: "github.com/nazo6/flexagent")
    name            TEXT NOT NULL,                  -- 表示名 (例: "flexagent")
    canonical_git_url TEXT,                         -- 正規化元Git URL
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

-- 3. プロジェクト × ノードのローカルパス紐付け (Worktree 含む)
CREATE TABLE project_node_bindings (
    project_id      TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    node_id         TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    local_path      TEXT NOT NULL,                  -- 例: "D:\ghq\github.com\nazo6\flexagent" または Worktree パス
    is_worktree     INTEGER NOT NULL DEFAULT 0,     -- 1: Git Worktree, 0: メインリポジトリ
    path_exists     INTEGER NOT NULL DEFAULT 1,     -- 最終報告時点でノード上に実在するか (0: 見つからない)
    git_branch      TEXT,                           -- 最終確認時のGitブランチ (例: "main", "feat/auth")
    last_used_at    INTEGER NOT NULL,
    PRIMARY KEY (project_id, node_id, local_path)
);

-- 4. セッション
--    ノード側: 自分のセッション（権威。直接書き込む）
--    ハブ側: 全ノード分の投影（イベント適用と同一トランザクションでのみ更新）
CREATE TABLE sessions (
    session_id      TEXT PRIMARY KEY,               -- UUID v7
    project_id      TEXT NOT NULL REFERENCES projects(project_id),
    node_id         TEXT NOT NULL REFERENCES nodes(node_id),
    local_path      TEXT NOT NULL,                  -- 実行ディレクトリ (Worktree パス含む)
    git_branch      TEXT,                           -- 起動時点のスナップショット（ライブ値は Worktree API から取得）
    is_worktree     INTEGER NOT NULL DEFAULT 0,     -- Worktree 内での実行か
    agent_id        TEXT NOT NULL,                  -- "opencode2" | "antigravity-acp" 等
    agent_session_id TEXT,                          -- エージェント内部のセッションID (SessionAgentBound イベントで確定)
    parent_session_id TEXT REFERENCES sessions(session_id), -- Fork元のセッションID
    fork_from_node_seq INTEGER,                     -- 親セッションのどのイベント(node_seq)時点からFork/Revertしたか
    title           TEXT NOT NULL DEFAULT 'New Session',
    status          TEXT NOT NULL,                  -- 'provisioning' | 'bootstrapping' | 'idle' | 'running' | 'waiting_permission' | 'stopped' | 'error'
    current_mode    TEXT,                           -- ACP SessionMode (例: 'code', 'plan')
    available_modes_json    TEXT NOT NULL DEFAULT '[]', -- ACP ModeInfo[]
    available_commands_json TEXT NOT NULL DEFAULT '[]', -- ACP AvailableCommand[]
    config_options_json     TEXT NOT NULL DEFAULT '[]', -- ACP ConfigOption[]
    git_bundle_path TEXT,                           -- ハブ専用: 一時VM破棄時に退避された git bundle パス（ノード側は NULL）
    last_node_seq   INTEGER NOT NULL DEFAULT 0,     -- ノード: 永続化済み最新 node_seq（採番は +1）/ ハブ: 投影に適用済み最大 node_seq
    synced_up_to_node_seq INTEGER NOT NULL DEFAULT 0, -- ノード専用: ハブが ACK した水位（ハブ側は常に 0 で未使用）
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE INDEX idx_sessions_project_updated ON sessions(project_id, updated_at DESC);
CREATE INDEX idx_sessions_status ON sessions(status);
CREATE INDEX idx_sessions_node_updated ON sessions(node_id, updated_at DESC);

-- 5. セッションイベント履歴 (Append-Only Event Log)
--    ノード側: 自分のセッションのイベント / ハブ側: 全ノードのイベント（冪等追記）
CREATE TABLE session_events (
    cursor          INTEGER PRIMARY KEY AUTOINCREMENT, -- このDBへの取り込み順 (クライアント差分再開カーソル)
    event_id        TEXT NOT NULL UNIQUE,           -- UUID v7 (冪等適用・重複排除)
    session_id      TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    node_seq        INTEGER NOT NULL,               -- セッション内の順序番号 (1, 2, 3...)。実行ノードのみが採番
    event_type      TEXT NOT NULL,                  -- 'session_created' | 'session_title_changed' | 'session_agent_bound' | 'user_message' | 'agent_message' | 'agent_thought' | 'tool_call' | 'plan' | 'permission_request' | 'permission_resolved' | 'session_reverted' | 'terminal_output' | 'status_change' | 'capabilities_updated' | 'bootstrap_log'
    payload_json    TEXT NOT NULL,                  -- 構造化ペイロード (UnifiedEventPayload のJSON。正データ)
    searchable_text TEXT,                           -- FTS5全文検索用のプレーンテキスト抽出 (受信時に payload_json から生成。terminal_output 等のバイナリ系は対象外)
    created_at      INTEGER NOT NULL,
    UNIQUE(session_id, node_seq)
);

CREATE INDEX idx_session_events_session_seq ON session_events(session_id, node_seq ASC);

-- 6. FTS5 全文検索インデックス (会話・思考・ファイルパス・コマンド履歴の高速検索)
CREATE VIRTUAL TABLE session_events_fts USING fts5(
    session_id UNINDEXED,
    event_type UNINDEXED,
    searchable_text,
    content='session_events',
    content_rowid='cursor',
    tokenize='trigram'                              -- 日本語・コード識別子の部分一致に強い trigram トークナイザ
);

-- FTS5 外部コンテンツテーブルの同期トリガー (INSERT / DELETE / UPDATE)
CREATE TRIGGER session_events_fts_ai AFTER INSERT ON session_events BEGIN
    INSERT INTO session_events_fts(rowid, session_id, event_type, searchable_text)
    VALUES (new.cursor, new.session_id, new.event_type, new.searchable_text);
END;
CREATE TRIGGER session_events_fts_ad AFTER DELETE ON session_events BEGIN
    INSERT INTO session_events_fts(session_events_fts, rowid, session_id, event_type, searchable_text)
    VALUES ('delete', old.cursor, old.session_id, old.event_type, old.searchable_text);
END;
CREATE TRIGGER session_events_fts_au AFTER UPDATE ON session_events BEGIN
    INSERT INTO session_events_fts(session_events_fts, rowid, session_id, event_type, searchable_text)
    VALUES ('delete', old.cursor, old.session_id, old.event_type, old.searchable_text);
    INSERT INTO session_events_fts(rowid, session_id, event_type, searchable_text)
    VALUES (new.cursor, new.session_id, new.event_type, new.searchable_text);
END;

-- 7. 承認Inbox（イベントから適用される投影。未解決 Permission Request の高速一覧用）
CREATE TABLE permission_requests (
    request_id      TEXT PRIMARY KEY,               -- ACP request id
    session_id      TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    node_id         TEXT NOT NULL REFERENCES nodes(node_id), -- ノード側は自分自身の node_id
    tool_name       TEXT NOT NULL,                  -- 例: "terminal/create", "fs/write_text_file"
    summary         TEXT NOT NULL,                  -- 例: "Run command: cargo test"
    details_json    TEXT NOT NULL,                  -- 引数やDiff詳細、選択肢 (options)
    status          TEXT NOT NULL DEFAULT 'pending',-- 'pending' | 'approved' | 'rejected' | 'cancelled'
    resolved_by     TEXT,                           -- 'cli' | 'web' | 'android_push'
    created_at      INTEGER NOT NULL,
    resolved_at     INTEGER
);

CREATE INDEX idx_permission_pending ON permission_requests(status) WHERE status = 'pending';

-- 8. Web Push (VAPID) サブスクリプション管理（ハブ専用。ノード側は常に空）
CREATE TABLE push_subscriptions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    endpoint        TEXT NOT NULL UNIQUE,
    p256dh          TEXT NOT NULL,
    auth            TEXT NOT NULL,
    device_name     TEXT,                           -- 例: "Pixel 9 Chrome PWA"
    created_at      INTEGER NOT NULL
);

-- 9. 監査ログ (Audit Log)
--    各ストアが「自分の観測した操作」のみを記録する（同期対象外）
CREATE TABLE audit_logs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    action          TEXT NOT NULL,                  -- 'session_start' | 'permission_resolved' | 'pty_spawn' | 'kill_switch' | 'worktree_manage'
    session_id      TEXT,                           -- 関連セッションID (任意)
    node_id         TEXT,                           -- 対象ノードID (任意。ノード側は自分自身)
    client_ip       TEXT NOT NULL,                  -- 送信元IPアドレス (LAN/VPN IP)
    client_user_agent TEXT,                         -- クライアントUser-Agent
    auth_subject    TEXT NOT NULL,                  -- トークン識別子または認証主体
    details_json    TEXT NOT NULL DEFAULT '{}',     -- 実行内容詳細 (実行コマンド、承認オプション等)
    created_at      INTEGER NOT NULL
);

CREATE INDEX idx_audit_logs_action ON audit_logs(action, created_at DESC);
CREATE INDEX idx_audit_logs_session ON audit_logs(session_id, created_at DESC);
```

---

## 2. 正データと派生カラム

イベントログを唯一の正（Single Source of
Truth）とするため、テーブル上の以下のカラムは
すべて**派生データ**として扱います。派生カラムはクエリ性能のためにのみ存在し、
常にイベントログから再構築できることが要件です（再構築関数 + テストで保証）。

| 派生カラム / テーブル                                              | 生成源                                                                     |
| :----------------------------------------------------------------- | :------------------------------------------------------------------------- |
| `sessions.*`（`git_bundle_path` / `synced_up_to_node_seq` を除く） | `session_events` の適用（§0.3 の投影規則）                                 |
| `permission_requests`                                              | `PermissionRequest` / `PermissionResolved` イベント                        |
| `session_events.searchable_text`                                   | `payload_json` からのテキスト抽出（`TerminalOutput` 等のバイナリ系は除外） |

- **Shadow Git の `snapshot_tree_hash`** は専用カラムを持たず、
  `UnifiedEventPayload::UserMessage.snapshot_tree_hash`（`payload_json`
  内）を正とします。 Revert 時の参照は `(session_id, node_seq)`
  で対象イベントを直接引くため、複製カラムは不要です。
- `searchable_text` の生成責務はイベントを永続化する側（ノード /
  ハブ双方の受信ハンドラ）にあります。
- `git_bundle_path`
  はハブのみが書き込む成果物退避パスであり、イベント投影の対象外です。
- **一時VMの仮セッション行**: 一時VMプロビジョニング中（`NodeHello` 前）は、
  中央サーバーが `sessions` 行を `status = 'provisioning'`
  で先行挿入します（`upsert_provisional_session`。イベントではなく投影行）。
  ノードの `SessionCreated` が upsert で派生カラムを上書きし、以降は通常の
  イベント投影に合流します（`git_bundle_path` と同様、全量再構築時には
  失われ得るベストエフォートの表示用状態）。

---

## 3. クエリ・同期・運用のポイント

- **共通クレート `fxg-db`**:
  単一スキーマ・単一マイグレーションセット・単一のクエリ層を持ち、
  実行ロール（`Node` / `Hub`）パラメータで挙動（Outbox 送信 / 中継 / Push
  等）だけを分岐させます。 ローカルWeb UI（`http://localhost:7860`）が `node.db`
  を読む際も、中央サーバーWeb UIが `server.db`
  を読む際も、同じJSONレスポンス型（`SessionDetail`,
  `SessionEvent`）を返却できます。
- **ノードの自己登録**: ノードは起動時に `nodes` テーブルへ自分自身の行を upsert
  し、 プロジェクト解決結果を `projects` / `project_node_bindings` へ upsert
  します （ハブへの `NodeHello` 報告と同一の情報源）。
- **水位ベース Outbox**: Outbox Sync Worker は
  `SELECT ... FROM session_events e JOIN sessions s USING (session_id)
  WHERE e.node_seq > s.synced_up_to_node_seq ORDER BY e.node_seq`
  で未送信イベントを抽出し、 `EventBatchAck` 受信時に
  `sessions.synced_up_to_node_seq` を更新します。
- **ハンドシェイク再同期**: `NodeHello`（`SessionSyncState` 一覧）とハブの
  `last_node_seq` を比較し、 欠落・遅延のあるセッションは
  `ResyncRequest { sessions: [{ session_id, from_node_seq }] }` で
  再送を要求します（ハブDBを再構築した場合の自動復元）。
- **日本語・ソースコード検索に強い `trigram` トークナイザ**: SQLite FTS5の
  `tokenize='trigram'`
  を使うことで、形態素解析器なしで日本語の会話（「認証エラー」「データベース」）も関数名・識別子（`OpenCode2Driver`）も高速に全文検索できます。
  - 制約: trigram は 3 文字未満の検索語ではヒットしないため、検索 UI では 3
    文字以上を要求するか、LIKE 検索へフォールバックします。
