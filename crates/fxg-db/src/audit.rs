//! 監査ログの記録 (`audit_logs`)。
//!
//! 各ストアが「自分の観測した操作」のみを記録する (同期対象外)。
//! 中央サーバー経由の操作は `server.db`、ローカル直結 (CLI / `localhost:7860`) の
//! 操作は実行ノードの `node.db` にも記録する
//! (設計: `docs/01-architecture-and-sync.md` §7.5)。

use fxg_protocol::util::now_ms;
use sqlx::SqlitePool;

use crate::error::DbError;

/// 監査ログの `action` 種別。
pub mod actions {
    /// セッション起動
    pub const SESSION_START: &str = "session_start";
    /// セッション巻き戻し (Shadow Git Tree Revert)
    pub const SESSION_REVERT: &str = "session_revert";
    /// セッション再開 (停止済みセッションのレジューム)
    pub const SESSION_RESUME: &str = "session_resume";
    /// セッションのアーカイブ/復元
    pub const SESSION_ARCHIVE: &str = "session_archive";
    /// セッション削除 (イベント本文のパージ)
    pub const SESSION_DELETE: &str = "session_delete";
    /// 承認解決 (Approve / Reject)
    pub const PERMISSION_RESOLVED: &str = "permission_resolved";
    /// Web PTY 起動
    pub const PTY_SPAWN: &str = "pty_spawn";
    /// 緊急キルスイッチ実行
    pub const KILL_SWITCH: &str = "kill_switch";
    /// Worktree 操作 (作成・削除)
    pub const WORKTREE_MANAGE: &str = "worktree_manage";
    /// Worktree クリーンアップ (Prune)
    pub const WORKTREE_PRUNE: &str = "worktree_prune";
    /// プロジェクト紐付け (手動 Link / 一括 Scan)
    pub const PROJECT_LINK: &str = "project_link";
    /// エージェント管理 (インストール / 更新 / 削除)
    pub const AGENT_MANAGE: &str = "agent_manage";
    /// ノードトークン発行・失効
    pub const NODE_TOKEN_MANAGE: &str = "node_token_manage";
    /// クライアント認証トークン再生成
    pub const AUTH_TOKEN_ROTATE: &str = "auth_token_rotate";
}

/// 監査ログの記録内容。
#[derive(Debug, Clone)]
pub struct AuditLogRecord {
    /// 操作種別 ([`actions`] の定数)
    pub action: String,
    /// 関連セッションID (任意)
    pub session_id: Option<String>,
    /// 対象ノードID (任意。ノード側は自分自身)
    pub node_id: Option<String>,
    /// 送信元IPアドレス
    pub client_ip: String,
    /// クライアントUser-Agent
    pub client_user_agent: Option<String>,
    /// トークン識別子または認証主体
    pub auth_subject: String,
    /// 実行内容詳細 (実行コマンド・承認オプション等)
    pub details: serde_json::Value,
}

impl AuditLogRecord {
    /// 必須項目から記録内容を作る (詳細は空オブジェクト)。
    pub fn new(
        action: impl Into<String>,
        client_ip: impl Into<String>,
        auth_subject: impl Into<String>,
    ) -> Self {
        Self {
            action: action.into(),
            session_id: None,
            node_id: None,
            client_ip: client_ip.into(),
            client_user_agent: None,
            auth_subject: auth_subject.into(),
            details: serde_json::Value::Object(serde_json::Map::new()),
        }
    }
}

/// 監査ログを1件記録し、採番された `id` を返す。
pub async fn append_audit_log(pool: &SqlitePool, record: &AuditLogRecord) -> Result<i64, DbError> {
    let details_json = serde_json::to_string(&record.details)?;
    let result = sqlx::query!(
        r#"
        INSERT INTO audit_logs
            (action, session_id, node_id, client_ip, client_user_agent, auth_subject,
             details_json, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
        record.action,
        record.session_id.as_deref(),
        record.node_id.as_deref(),
        record.client_ip,
        record.client_user_agent.as_deref(),
        record.auth_subject,
        details_json,
        now_ms(),
    )
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}
