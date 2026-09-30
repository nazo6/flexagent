# 02. データベーススキーマ設計 (`server.db` / `node.db`)

FlexAgentは外部DBミドルウェアを必要としない **SQLite (`sqlx`)**
を採用し、WALモード (`PRAGMA journal_mode = WAL;`) で動作します。
E2E暗号化を行わないため、中央サーバー側で会話履歴の構造化クエリおよび
**FTS5（全文検索）** がそのまま利用可能です。

---

## 1. 中央サーバー DB (`server.db`)

中央サーバーが保持するスキーマです。全ノードの情報、論理プロジェクト、全セッション履歴、およびWeb
Push購読情報を管理します。

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

-- 1. ノード管理テーブル (常駐ノードおよび一時VMノード)
CREATE TABLE nodes (
    node_id         TEXT PRIMARY KEY,               -- UUID または 固定スラッグ (例: "win-desktop", "eph-0195...")
    name            TEXT NOT NULL,                  -- 表示名 (例: "Home Windows PC", "Colab Pro (T4)")
    os              TEXT NOT NULL,                  -- "windows" | "linux" | "macos"
    arch            TEXT NOT NULL,                  -- "x86_64" | "aarch64"
    version         TEXT NOT NULL,                  -- fxg バイナリバージョン
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
    git_branch      TEXT,                           -- 最終確認時のGitブランチ (例: "main", "feat/auth")
    last_used_at    INTEGER NOT NULL,
    PRIMARY KEY (project_id, node_id, local_path)
);

-- 4. セッションテーブル
CREATE TABLE sessions (
    session_id      TEXT PRIMARY KEY,               -- UUID v7
    project_id      TEXT NOT NULL REFERENCES projects(project_id),
    node_id         TEXT NOT NULL REFERENCES nodes(node_id),
    local_path      TEXT NOT NULL,                  -- 実行ディレクトリ (Worktree パス含む)
    git_branch      TEXT,                           -- 起動時・現在のブランチ名
    is_worktree     INTEGER NOT NULL DEFAULT 0,     -- Worktree 内での実行か
    agent_id        TEXT NOT NULL,                  -- "opencode2" | "antigravity-acp" 等
    agent_session_id TEXT,                          -- エージェント内部のセッションID (ACP session_id / opencode id)
    parent_session_id TEXT REFERENCES sessions(session_id), -- Fork元のセッションID
    fork_from_node_seq INTEGER,                     -- 親セッションのどのイベント(node_seq)時点からFork/Revertしたか
    title           TEXT NOT NULL DEFAULT 'New Session',
    status          TEXT NOT NULL,                  -- 'provisioning' | 'bootstrapping' | 'idle' | 'running' | 'waiting_permission' | 'stopped' | 'error'
    current_mode    TEXT,                           -- ACP SessionMode (例: 'code', 'plan')
    available_commands_json TEXT NOT NULL DEFAULT '[]', -- ACP AvailableCommand[]
    config_options_json     TEXT NOT NULL DEFAULT '[]', -- ACP ConfigOption[]
    git_bundle_path TEXT,                           -- 一時VM破棄時に退避された git bundle ファイルパス (別ノードでのFork/復元用)
    last_node_seq   INTEGER NOT NULL DEFAULT 0,     -- ノードから受信済みの最大 node_seq
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE INDEX idx_sessions_project_updated ON sessions(project_id, updated_at DESC);
CREATE INDEX idx_sessions_status ON sessions(status);

-- 5. セッションイベント履歴 (Append-Only Event Log)
CREATE TABLE session_events (
    global_seq      INTEGER PRIMARY KEY AUTOINCREMENT, -- サーバー全体の単調増加シーケンス (クライアント差分同期用)
    event_id        TEXT NOT NULL UNIQUE,           -- UUID v7 (再送時の重複排除用)
    session_id      TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    node_seq        INTEGER NOT NULL,               -- セッション内の順序番号 (1, 2, 3...)
    event_type      TEXT NOT NULL,                  -- 'user_message' | 'agent_message' | 'agent_thought' | 'tool_call' | 'tool_update' | 'plan' | 'permission_request' | 'permission_resolved' | 'terminal_output' | 'status_change' | 'turn_snapshot'
    snapshot_tree_hash TEXT,                        -- Revert用: ターン開始時の Shadow Git Tree Hash (git write-tree)
    payload_json    TEXT NOT NULL,                  -- 構造化ペイロード (UnifiedEventPayload のJSON)
     searchable_text TEXT,                           -- FTS5全文検索用のプレーンテキスト抽出
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
    content_rowid='global_seq',
    tokenize='trigram'                              -- 日本語・コード識別子の部分一致に強い trigram トークナイザ
);

-- 7. 承認Inbox (未解決のPermission Requestを高速一覧取得するためのビュー/テーブル)
CREATE TABLE permission_requests (
    request_id      TEXT PRIMARY KEY,               -- ACP request id
    session_id      TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    node_id         TEXT NOT NULL REFERENCES nodes(node_id),
    tool_name       TEXT NOT NULL,                  -- 例: "terminal/create", "fs/write_text_file"
    summary         TEXT NOT NULL,                  -- 例: "Run command: cargo test"
    details_json    TEXT NOT NULL,                  -- 引数やDiff詳細、選択肢 (options)
    status          TEXT NOT NULL DEFAULT 'pending',-- 'pending' | 'approved' | 'rejected' | 'cancelled'
    resolved_by     TEXT,                           -- 'cli' | 'web' | 'android_push'
    created_at      INTEGER NOT NULL,
    resolved_at     INTEGER
);

CREATE INDEX idx_permission_pending ON permission_requests(status) WHERE status = 'pending';

-- 8. Web Push (VAPID) サブスクリプション管理
CREATE TABLE push_subscriptions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    endpoint        TEXT NOT NULL UNIQUE,
    p256dh          TEXT NOT NULL,
    auth            TEXT NOT NULL,
    device_name     TEXT,                           -- 例: "Pixel 9 Chrome PWA"
    created_at      INTEGER NOT NULL
);

-- 9. 監査ログ (Audit Log)
CREATE TABLE audit_logs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    action          TEXT NOT NULL,                  -- 'session_start' | 'permission_resolved' | 'pty_spawn' | 'kill_switch' | 'worktree_manage'
    session_id      TEXT,                           -- 関連セッションID (任意)
    node_id         TEXT,                           -- 対象ノードID (任意)
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

## 2. ノードデーモン DB (`node.db`)

各ノード（`~/.flexagent/node.db`）がローカルに保持するスキーマです。
中央サーバーとほぼ同じ `sessions` / `session_events` / `permission_requests`
テーブル構造を持ち、さらに **未送信イベント管理用の `synced` フラグ**
を備えます。

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE local_sessions (
    session_id      TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL,
    project_name    TEXT NOT NULL,
    local_path      TEXT NOT NULL,
    git_branch      TEXT,                           -- 起動時・現在のブランチ名
    is_worktree     INTEGER NOT NULL DEFAULT 0,     -- Worktree 内での実行か
    agent_id        TEXT NOT NULL,
    agent_session_id TEXT,
    title           TEXT NOT NULL DEFAULT 'New Session',
    status          TEXT NOT NULL,
    current_mode    TEXT,
    available_commands_json TEXT NOT NULL DEFAULT '[]',
    config_options_json     TEXT NOT NULL DEFAULT '[]',
    next_node_seq   INTEGER NOT NULL DEFAULT 1,
     metadata_synced INTEGER NOT NULL DEFAULT 0,     -- 0: 中央サーバーへ未同期, 1: 同期済
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE TABLE local_session_events (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id        TEXT NOT NULL UNIQUE,           -- UUID v7
    session_id      TEXT NOT NULL REFERENCES local_sessions(session_id) ON DELETE CASCADE,
    node_seq        INTEGER NOT NULL,
    event_type      TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    synced          INTEGER NOT NULL DEFAULT 0,     -- 0: 未送信 (Outbox), 1: 中央サーバーACK済
    created_at      INTEGER NOT NULL,
    UNIQUE(session_id, node_seq)
);

-- Outbox ワーカーが未送信イベントを順番に取り出すためのインデックス
CREATE INDEX idx_local_events_unsynced ON local_session_events(synced, id ASC) WHERE synced = 0;
```

### クエリ・同期のポイント

- **共通クレート `fxg-db`**: `server.db` と `node.db`
  のイベント構造を共通化しておくことで、ローカルWeb
  UI（`http://localhost:7860`）が `node.db` を読む際も、中央サーバーWeb UIが
  `server.db` を読む際も、同じJSONレスポンス型（`SessionDetail`,
  `SessionEvent`）を返却できます。
- **日本語・ソースコード検索に強い `trigram` トークナイザ**: SQLite FTS5の
  `tokenize='trigram'`
  を使うことで、形態素解析器なしで日本語の会話（「認証エラー」「データベース」）も関数名・識別子（`OpenCode2Driver`）も高速に全文検索できます。
