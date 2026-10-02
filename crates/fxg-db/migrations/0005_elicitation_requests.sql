-- elicitation (ACP `elicitation/create` の構造化入力リクエスト) の投影。
--
-- エージェントの「質問」ツールを `ElicitationRequest` / `ElicitationResolved`
-- イベントから適用する。未解決リクエストは Inbox / Web Push / UI の回答
-- フォームに表示される (設計: docs/02-database-schema.md §1)。
CREATE TABLE elicitation_requests (
    elicitation_id  TEXT PRIMARY KEY,               -- ACP elicitation id
    session_id      TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    node_id         TEXT NOT NULL REFERENCES nodes(node_id), -- ノード側は自分自身の node_id
    message         TEXT NOT NULL,                  -- ユーザーへ提示するメッセージ
    mode            TEXT NOT NULL DEFAULT 'form',   -- 'form' | 'url' (Phase 1 は form のみ)
    schema_json     TEXT NOT NULL DEFAULT '{}',     -- form モードの requestedSchema
    tool_call_id    TEXT,                           -- 関連するツール呼び出しID (任意)
    status          TEXT NOT NULL DEFAULT 'pending',-- 'pending' | 'accepted' | 'declined' | 'cancelled'
    content_json    TEXT NOT NULL DEFAULT 'null',   -- accept 時の回答内容
    resolved_by     TEXT,                           -- 'cli' | 'web' | 'android_push'
    created_at      INTEGER NOT NULL,
    resolved_at     INTEGER
);

CREATE INDEX idx_elicitation_pending ON elicitation_requests(status) WHERE status = 'pending';
