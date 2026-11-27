-- Composer drafts (node.db / hub.db).
--
-- Design: docs/proposals/2026-11-27-composer-drafts-and-sync.md
--
-- Drafts are user-authored, mutable data and are intentionally NOT part of the
-- session event log: they are not execution history, they change on every
-- keystroke (debounced), and they can exist without a session. Revision-based
-- optimistic locking keeps concurrent edits from silently overwriting each
-- other. Deletes are tombstones so a future node <-> hub replication can
-- propagate them.
CREATE TABLE drafts (
    draft_id         TEXT PRIMARY KEY,                     -- UUID v7 (stable across devices)
    title            TEXT NOT NULL DEFAULT '',             -- Empty = derive from the first line of body
    body             TEXT NOT NULL DEFAULT '',             -- Prompt body
    session_id       TEXT,                                 -- Optional target session (no FK; drafts outlive sessions)
    project_id       TEXT,                                 -- Optional context for a session-less draft
    node_id          TEXT,                                 -- Optional execution node for a session-less draft
    attachments_json TEXT NOT NULL DEFAULT '[]',           -- AttachmentMeta[] (v1: metadata only)
    revision         INTEGER NOT NULL DEFAULT 1,           -- Optimistic-lock revision
    origin_device_id TEXT NOT NULL,                        -- Device that made the last change
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    deleted_at       INTEGER                               -- NULL = active; otherwise tombstone time
);

CREATE INDEX idx_drafts_updated
    ON drafts(updated_at DESC)
 WHERE deleted_at IS NULL;

CREATE INDEX idx_drafts_session
    ON drafts(session_id, updated_at DESC)
 WHERE deleted_at IS NULL;
