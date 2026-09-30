-- FlexAgent 単一スキーマ (node.db / server.db 共通)
--
-- 設計: docs/02-database-schema.md §1
--   node.db (各ノード: ~/.flexagent/node.db) と server.db (中央サーバー:
--   ~/.flexagent/server.db) は「同一DDL・同一テーブル名・単一マイグレーションセット」
--   から生成される1つのスキーマを共有する。両者の違いは「行のスコープ」と
--   「実行ロール」だけであり、スキーマ対称性は単一定義によって保証する。
--
--   PRAGMA journal_mode = WAL / PRAGMA foreign_keys = ON はマイグレーション
--   (トランザクション内) では効果がないため、接続オプション側
--   (crates/fxg-db/src/db.rs の SqliteConnectOptions) で全接続に適用する。

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
    provisioner     TEXT,                           -- 一時ノードのプロビジョナー識別子 (例: "local-docker")
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
    event_type      TEXT NOT NULL,                  -- 'session_created' | ... | 'status_change'
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
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
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
