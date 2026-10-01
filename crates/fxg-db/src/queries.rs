//! 共通クライアント向けクエリ層。
//!
//! `node.db` (ローカル直結 UI / CLI) と `server.db` (中央サーバー UI) の双方が
//! **同一のレスポンス型** ([`SessionSummary`] / [`PermissionRequestEntry`] 等) を
//! 返せるようにする (設計: `docs/02-database-schema.md` §3)。
//!
//! # 方針: 動的 SQL の回避
//!
//! 型安全マクロ (`query!` / `query_as!` / `query_scalar!`) を維持するため、
//! 一覧系クエリの絞り込みは SQL を動的組み立てせず、取得後に Rust 側で適用する
//! (セルフホスト・単一テナント規模を前提とした単純化)。
//!
//! 例外は FTS5 `MATCH` と、3文字未満の検索語に対する LIKE フォールバックのみ。
//! どちらもプレースホルダを介せない特殊構文のため、理由コメント付きで
//! `query_as` (ランタイム文字列) を使用する (規約上の例外。単体テストで担保)。
//!
//! # 型オーバーライド
//!
//! SQLite では `TEXT PRIMARY KEY` 列に暗黙の NOT NULL が付かないため、
//! 主キー列は `AS "col!"` で非 NULL を、`INTEGER` の真偽値列は
//! `AS "col: bool"` で bool を明示する。

use std::str::FromStr;

use fxg_protocol::common::{
    AuditLogEntry, NodeLifecycleStatus, NodeSummary, PermissionRequestEntry,
    PermissionRequestStatus, ProjectBindingSummary, ProjectSummary, SearchHit, SessionStatus,
    SessionSummary,
};
use fxg_protocol::events::SessionEventBatch;
use sqlx::SqlitePool;

use crate::error::DbError;
use crate::events::{StoredPermissionDetails, envelope_from_row};

/// セッション一覧の絞り込み条件。
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// 論理プロジェクトIDで絞り込む
    pub project_id: Option<String>,
    /// ノードIDで絞り込む
    pub node_id: Option<String>,
    /// セッション状態で絞り込む (空なら全状態)
    pub statuses: Vec<SessionStatus>,
    /// 最大件数
    pub limit: Option<u32>,
}

/// LIKE 検索・スニペット生成のコンテキスト文字数。
const SNIPPET_CONTEXT_CHARS: usize = 40;

/// セッション一覧を取得する (更新日時降順)。
pub async fn list_sessions(
    pool: &SqlitePool,
    filter: &SessionFilter,
) -> Result<Vec<SessionSummary>, DbError> {
    let rows = sqlx::query_as!(
        SessionRow,
        r#"
        SELECT session_id AS "session_id!",
               project_id, node_id, local_path, git_branch,
               is_worktree AS "is_worktree: bool",
               agent_id, agent_session_id, parent_session_id, fork_from_node_seq,
               title, status, current_mode, last_node_seq, created_at, updated_at
          FROM sessions
         ORDER BY updated_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut sessions = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(project_id) = filter.project_id.as_deref()
            && row.project_id != project_id
        {
            continue;
        }
        if let Some(node_id) = filter.node_id.as_deref()
            && row.node_id != node_id
        {
            continue;
        }
        let summary = row.into_summary()?;
        if !filter.statuses.is_empty() && !filter.statuses.contains(&summary.status) {
            continue;
        }
        sessions.push(summary);
    }
    sessions.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    if let Some(limit) = filter.limit {
        sessions.truncate(limit as usize);
    }
    Ok(sessions)
}

/// セッションを1件取得する。
pub async fn get_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<SessionSummary>, DbError> {
    let row = sqlx::query_as!(
        SessionRow,
        r#"
        SELECT session_id AS "session_id!",
               project_id, node_id, local_path, git_branch,
               is_worktree AS "is_worktree: bool",
               agent_id, agent_session_id, parent_session_id, fork_from_node_seq,
               title, status, current_mode, last_node_seq, created_at, updated_at
          FROM sessions
         WHERE session_id = ?
        "#,
        session_id,
    )
    .fetch_optional(pool)
    .await?;
    row.map(SessionRow::into_summary).transpose()
}

/// ハブ専用: 一時VM破棄時に退避された git bundle のパスを記録する。
///
/// `None` で消去 (バンドル復元後など)。
pub async fn set_git_bundle_path(
    pool: &SqlitePool,
    session_id: &str,
    path: Option<&str>,
) -> Result<(), DbError> {
    let result = sqlx::query!(
        r#"UPDATE sessions SET git_bundle_path = ? WHERE session_id = ?"#,
        path,
        session_id,
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::SessionNotFound(session_id.to_owned()));
    }
    Ok(())
}

/// ハブ専用: 退避済み git bundle のパスを取得する。
pub async fn get_git_bundle_path(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<String>, DbError> {
    let row = sqlx::query!(
        r#"SELECT git_bundle_path FROM sessions WHERE session_id = ?"#,
        session_id,
    )
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => Ok(row.git_bundle_path),
        None => Err(DbError::SessionNotFound(session_id.to_owned())),
    }
}

/// `after_cursor` より後のイベント履歴をバッチで取得する (差分同期の再開用)。
///
/// 返却されるバッチの `cursor` は「取得できた最後のイベントのカーソル」であり、
/// 該当イベントが無い場合は `after_cursor` をそのまま返す。
pub async fn session_events_after(
    pool: &SqlitePool,
    session_id: &str,
    after_cursor: u64,
    limit: u32,
) -> Result<SessionEventBatch, DbError> {
    let after = i64::try_from(after_cursor).map_err(|_| DbError::CursorOutOfRange(after_cursor))?;
    let rows = sqlx::query!(
        r#"
        SELECT cursor, event_id, session_id, node_seq, payload_json, created_at
          FROM session_events
         WHERE session_id = ? AND cursor > ?
         ORDER BY cursor ASC
         LIMIT ?
        "#,
        session_id,
        after,
        i64::from(limit),
    )
    .fetch_all(pool)
    .await?;

    let mut cursor = after_cursor;
    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        cursor = u64::try_from(row.cursor).map_err(|_| DbError::NegativeNodeSeq(row.cursor))?;
        events.push(envelope_from_row(
            row.event_id,
            row.session_id,
            row.node_seq,
            row.payload_json,
            row.created_at,
        )?);
    }
    Ok(SessionEventBatch { events, cursor })
}

/// `(cursor, event_id, session_id, node_seq, payload_json, created_at)` 行から
/// [`SessionEventBatch`] を組み立てる (共通処理)。
fn batch_from_rows(
    rows: Vec<(i64, String, String, i64, String, i64)>,
    after_cursor: u64,
) -> Result<SessionEventBatch, DbError> {
    let mut cursor = after_cursor;
    let mut events = Vec::with_capacity(rows.len());
    for (row_cursor, event_id, session_id, node_seq, payload_json, created_at) in rows {
        cursor = u64::try_from(row_cursor).map_err(|_| DbError::NegativeNodeSeq(row_cursor))?;
        events.push(envelope_from_row(
            event_id,
            session_id,
            node_seq,
            payload_json,
            created_at,
        )?);
    }
    Ok(SessionEventBatch { events, cursor })
}

/// 全セッション横断で `after_cursor` より後のイベントを取得する。
///
/// Client WS のリプレイ (`Subscribe { since_cursor }`) や Outbox の再開で使用する。
/// 返却するバッチの `cursor` は最後に取得できたイベントのカーソル。
pub async fn events_after_cursor(
    pool: &SqlitePool,
    after_cursor: u64,
    limit: u32,
) -> Result<SessionEventBatch, DbError> {
    let after = i64::try_from(after_cursor).map_err(|_| DbError::CursorOutOfRange(after_cursor))?;
    let rows = sqlx::query!(
        r#"
        SELECT cursor, event_id, session_id, node_seq, payload_json, created_at
          FROM session_events
         WHERE cursor > ?
         ORDER BY cursor ASC
         LIMIT ?
        "#,
        after,
        i64::from(limit),
    )
    .fetch_all(pool)
    .await?;

    let rows = rows
        .into_iter()
        .map(|row| {
            (
                row.cursor,
                row.event_id,
                row.session_id,
                row.node_seq,
                row.payload_json,
                row.created_at,
            )
        })
        .collect();
    batch_from_rows(rows, after_cursor)
}

/// `session_events` の最新カーソル (イベントが1件も無い場合は 0) を返す。
///
/// Client WS の `Subscribe { since_cursor }` がストアの末尾を超える場合
/// (DB 再作成・リセット後に残った古いカーソル) の検知に使用する。
pub async fn latest_cursor(pool: &SqlitePool) -> Result<u64, DbError> {
    let cursor = sqlx::query_scalar!(
        r#"
        SELECT MAX(cursor)
          FROM session_events
        "#,
    )
    .fetch_one(pool)
    .await?;
    match cursor {
        Some(value) => u64::try_from(value).map_err(|_| DbError::NegativeNodeSeq(value)),
        None => Ok(0),
    }
}

/// 論理プロジェクト一覧を取得する (ノード・Worktree 紐付け含む)。
pub async fn list_projects(pool: &SqlitePool) -> Result<Vec<ProjectSummary>, DbError> {
    let project_rows = sqlx::query_as!(
        ProjectRow,
        r#"
        SELECT project_id AS "project_id!",
               name, canonical_git_url, created_at, updated_at
          FROM projects
         ORDER BY updated_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let binding_rows = sqlx::query_as!(
        BindingRow,
        r#"
        SELECT project_id, node_id, local_path,
               is_worktree AS "is_worktree: bool",
               path_exists AS "path_exists: bool",
               git_branch, last_used_at
          FROM project_node_bindings
         ORDER BY last_used_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut projects: Vec<ProjectSummary> = project_rows
        .into_iter()
        .map(|row| ProjectSummary {
            project_id: row.project_id,
            name: row.name,
            canonical_git_url: row.canonical_git_url,
            created_at: row.created_at,
            updated_at: row.updated_at,
            bindings: Vec::new(),
        })
        .collect();

    for binding in binding_rows {
        if let Some(project) = projects
            .iter_mut()
            .find(|project| project.project_id == binding.project_id)
        {
            project.bindings.push(ProjectBindingSummary {
                node_id: binding.node_id,
                local_path: binding.local_path,
                is_worktree: binding.is_worktree,
                path_exists: binding.path_exists,
                git_branch: binding.git_branch,
                last_used_at: binding.last_used_at,
            });
        }
    }
    Ok(projects)
}

/// 実行ディレクトリの既定パスを解決する。
///
/// # 解決順序
///
/// 1. `(project_id, node_id)` の紐付けのうち**最終使用が最も新しい**もの
///    (Worktree を含む。[`select_default_binding_path`] 参照)
/// 2. 紐付けが無ければ `(project_id, node_id)` の最新セッションの `local_path`
///    (セッション開始時の紐付け登録を実装する前に実行されたセッションへのフォールバック)
///
/// セッション開始要求で `local_path` を省略した場合の既定として、ノード・
/// 中央サーバーの双方が共有する (設計: docs/01-architecture-and-sync.md §4)。
pub async fn resolve_default_local_path(
    pool: &SqlitePool,
    project_id: &str,
    node_id: &str,
) -> Result<Option<String>, DbError> {
    let projects = list_projects(pool).await?;
    if let Some(project) = projects
        .iter()
        .find(|project| project.project_id == project_id)
        && let Some(path) = select_default_binding_path(&project.bindings, node_id)
    {
        return Ok(Some(path));
    }

    let sessions = list_sessions(
        pool,
        &SessionFilter {
            project_id: Some(project_id.to_owned()),
            node_id: Some(node_id.to_owned()),
            limit: Some(1),
            ..SessionFilter::default()
        },
    )
    .await?;
    Ok(sessions
        .into_iter()
        .next()
        .map(|session| session.local_path))
}

/// 紐付け一覧から既定として使うパスを選ぶ。
///
/// **実在する**紐付けのうち最終使用が最も新しいものを選ぶ (`last_used_at`)。
/// Worktree も候補に含める (直近の作業場所を既定にする)。同時刻の場合は
/// メインリポジトリ (`is_worktree = false`) を優先する。
///
/// 実在する紐付けが無い場合は最終使用が最も新しい紐付けを返す
/// (存在しないパスを暗黙に使わないための UX 警告は UI 側で行う)。
pub fn select_default_binding_path(
    bindings: &[ProjectBindingSummary],
    node_id: &str,
) -> Option<String> {
    let candidates: Vec<&ProjectBindingSummary> = bindings
        .iter()
        .filter(|binding| binding.node_id == node_id)
        .collect();
    let latest_existing = candidates
        .iter()
        .filter(|binding| binding.path_exists)
        .max_by_key(|binding| (binding.last_used_at, !binding.is_worktree));
    latest_existing
        .copied()
        .or_else(|| {
            candidates
                .into_iter()
                .max_by_key(|binding| (binding.last_used_at, !binding.is_worktree))
        })
        .map(|binding| binding.local_path.clone())
}

/// ノード一覧を取得する (オンライン状態・一時ノード属性含む)。
pub async fn list_nodes(pool: &SqlitePool) -> Result<Vec<NodeSummary>, DbError> {
    let rows = sqlx::query_as!(
        NodeRow,
        r#"
        SELECT node_id AS "node_id!",
               name, os, arch, version, installed_agents_json,
               is_ephemeral AS "is_ephemeral: bool",
               provisioner, lifecycle_status,
               is_online AS "is_online: bool",
               last_seen_at
          FROM nodes
         ORDER BY last_seen_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let installed_agents: Vec<String> =
                serde_json::from_str(&row.installed_agents_json).unwrap_or_default();
            Ok(NodeSummary {
                node_id: row.node_id,
                name: row.name,
                os: row.os,
                arch: row.arch,
                version: row.version,
                is_ephemeral: row.is_ephemeral,
                provisioner: row.provisioner,
                lifecycle_status: NodeLifecycleStatus::from_str(&row.lifecycle_status)
                    .map_err(|err| DbError::InvalidEnumValue(err.to_string()))?,
                is_online: row.is_online,
                last_seen_at: row.last_seen_at,
                installed_agents,
            })
        })
        .collect()
}

/// 未解決 (`pending`) の承認リクエスト一覧を取得する (グローバル承認 Inbox)。
pub async fn pending_permissions(
    pool: &SqlitePool,
) -> Result<Vec<PermissionRequestEntry>, DbError> {
    let rows = sqlx::query_as!(
        PermissionRow,
        r#"
        SELECT request_id AS "request_id!",
               session_id, node_id, tool_name, summary, details_json,
               status, created_at, resolved_at, resolved_by
          FROM permission_requests
         WHERE status = 'pending'
         ORDER BY created_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(PermissionRow::into_entry).collect()
}

/// 承認リクエストを1件取得する (`PermissionResolved` の応答判定などに使用)。
pub async fn find_permission_request(
    pool: &SqlitePool,
    request_id: &str,
) -> Result<Option<PermissionRequestEntry>, DbError> {
    let row = sqlx::query_as!(
        PermissionRow,
        r#"
        SELECT request_id AS "request_id!",
               session_id, node_id, tool_name, summary, details_json,
               status, created_at, resolved_at, resolved_by
          FROM permission_requests
         WHERE request_id = ?
        "#,
        request_id,
    )
    .fetch_optional(pool)
    .await?;
    row.map(PermissionRow::into_entry).transpose()
}

/// FTS5 全文検索。3文字未満の検索語は LIKE フォールバックする。
///
/// trigram トークナイザは 3 文字未満の検索語ではヒットしないため
/// (設計: `docs/02-database-schema.md` §3)。
pub async fn search(pool: &SqlitePool, query: &str, limit: u32) -> Result<Vec<SearchHit>, DbError> {
    let needle = query.trim();
    if needle.is_empty() {
        return Err(DbError::EmptySearchQuery);
    }
    let limit = i64::from(limit.clamp(1, 500));

    if needle.chars().count() < 3 {
        return search_like(pool, needle, limit).await;
    }

    // FTS5 の MATCH はフレーズ引用符を SQL 文字列へ埋め込む必要があり、
    // プレースホルダだけでは構文を表現できない (規約上の例外)。
    // 検索語はダブルクォートで囲んだフレーズとして渡し、
    // `-` や `"` 等の FTS5 演算子解釈によるエラーを避ける。
    let match_query = format!("\"{}\"", needle.replace('"', "\"\""));
    let rows = sqlx::query_as::<_, SearchRow>(
        r#"
        SELECT e.event_id AS event_id,
               e.session_id AS session_id,
               e.node_seq AS node_seq,
               e.event_type AS event_type,
               e.created_at AS created_at,
               snippet(session_events_fts, 2, '[', ']', '…', 12) AS snippet
          FROM session_events_fts
          JOIN session_events e ON e.cursor = session_events_fts.rowid
         WHERE session_events_fts MATCH ?
         ORDER BY e.created_at DESC
         LIMIT ?
        "#,
    )
    .bind(match_query)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().filter_map(SearchRow::into_hit).collect())
}

/// 3文字未満の検索語用の LIKE フォールバック。
async fn search_like(
    pool: &SqlitePool,
    needle: &str,
    limit: i64,
) -> Result<Vec<SearchHit>, DbError> {
    // LIKE のパターンもプレースホルダで表現できない (ワイルドカードの連結が必要)。
    let pattern = format!("%{}%", escape_like_pattern(needle));
    let rows = sqlx::query_as::<_, SearchRow>(
        r#"
        SELECT event_id, session_id, node_seq, event_type, created_at,
               searchable_text AS snippet
          FROM session_events
         WHERE searchable_text LIKE ? ESCAPE '\'
         ORDER BY created_at DESC
         LIMIT ?
        "#,
    )
    .bind(pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let text = row.snippet.as_deref()?;
            let snippet = excerpt_around(text, needle, SNIPPET_CONTEXT_CHARS);
            Some(SearchHit {
                event_id: row.event_id,
                session_id: row.session_id,
                node_seq: u64::try_from(row.node_seq).ok()?,
                event_type: row.event_type,
                snippet,
                created_at: row.created_at,
            })
        })
        .collect())
}

/// 監査ログ一覧を取得する (新しい順)。
pub async fn audit_logs(pool: &SqlitePool, limit: u32) -> Result<Vec<AuditLogEntry>, DbError> {
    let rows = sqlx::query_as!(
        AuditRow,
        r#"
        SELECT id, action, session_id, node_id, client_ip, client_user_agent,
               auth_subject, details_json, created_at
          FROM audit_logs
         ORDER BY created_at DESC, id DESC
         LIMIT ?
        "#,
        i64::from(limit.clamp(1, 1000)),
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| AuditLogEntry {
            id: row.id,
            action: row.action,
            session_id: row.session_id,
            node_id: row.node_id,
            client_ip: row.client_ip,
            client_user_agent: row.client_user_agent,
            auth_subject: row.auth_subject,
            details: serde_json::from_str(&row.details_json)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
            created_at: row.created_at,
        })
        .collect())
}

// ----------------------------------------------------------------------
// 行構造体 (sqlx の型安全マクロ用)
// ----------------------------------------------------------------------

struct SessionRow {
    session_id: String,
    project_id: String,
    node_id: String,
    local_path: String,
    git_branch: Option<String>,
    is_worktree: bool,
    agent_id: String,
    agent_session_id: Option<String>,
    parent_session_id: Option<String>,
    fork_from_node_seq: Option<i64>,
    title: String,
    status: String,
    current_mode: Option<String>,
    last_node_seq: i64,
    created_at: i64,
    updated_at: i64,
}

impl SessionRow {
    fn into_summary(self) -> Result<SessionSummary, DbError> {
        Ok(SessionSummary {
            session_id: self.session_id,
            project_id: self.project_id,
            node_id: self.node_id,
            local_path: self.local_path,
            git_branch: self.git_branch,
            is_worktree: self.is_worktree,
            agent_id: self.agent_id,
            agent_session_id: self.agent_session_id,
            parent_session_id: self.parent_session_id,
            fork_from_node_seq: self
                .fork_from_node_seq
                .map(|value| u64::try_from(value).map_err(|_| DbError::NegativeNodeSeq(value)))
                .transpose()?,
            title: self.title,
            status: SessionStatus::from_str(&self.status)
                .map_err(|err| DbError::InvalidEnumValue(err.to_string()))?,
            current_mode: self.current_mode,
            last_node_seq: u64::try_from(self.last_node_seq)
                .map_err(|_| DbError::NegativeNodeSeq(self.last_node_seq))?,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

struct ProjectRow {
    project_id: String,
    name: String,
    canonical_git_url: Option<String>,
    created_at: i64,
    updated_at: i64,
}

struct BindingRow {
    project_id: String,
    node_id: String,
    local_path: String,
    is_worktree: bool,
    path_exists: bool,
    git_branch: Option<String>,
    last_used_at: i64,
}

struct NodeRow {
    node_id: String,
    name: String,
    os: String,
    arch: String,
    version: String,
    installed_agents_json: String,
    is_ephemeral: bool,
    provisioner: Option<String>,
    lifecycle_status: String,
    is_online: bool,
    last_seen_at: i64,
}

struct PermissionRow {
    request_id: String,
    session_id: String,
    node_id: String,
    tool_name: String,
    summary: String,
    details_json: String,
    status: String,
    created_at: i64,
    resolved_at: Option<i64>,
    resolved_by: Option<String>,
}

impl PermissionRow {
    fn into_entry(self) -> Result<PermissionRequestEntry, DbError> {
        let stored: StoredPermissionDetails =
            serde_json::from_str(&self.details_json).unwrap_or_default();
        Ok(PermissionRequestEntry {
            request_id: self.request_id,
            session_id: self.session_id,
            node_id: self.node_id,
            tool_name: self.tool_name,
            summary: self.summary,
            options: stored.options,
            details: stored.details,
            status: PermissionRequestStatus::from_str(&self.status)
                .map_err(|err| DbError::InvalidEnumValue(err.to_string()))?,
            created_at: self.created_at,
            resolved_at: self.resolved_at,
            resolved_by: self.resolved_by,
        })
    }
}

#[derive(sqlx::FromRow)]
struct SearchRow {
    event_id: String,
    session_id: String,
    node_seq: i64,
    event_type: String,
    created_at: i64,
    snippet: Option<String>,
}

impl SearchRow {
    fn into_hit(self) -> Option<SearchHit> {
        Some(SearchHit {
            event_id: self.event_id,
            session_id: self.session_id,
            node_seq: u64::try_from(self.node_seq).ok()?,
            event_type: self.event_type,
            snippet: self.snippet?,
            created_at: self.created_at,
        })
    }
}

struct AuditRow {
    id: i64,
    action: String,
    session_id: Option<String>,
    node_id: Option<String>,
    client_ip: String,
    client_user_agent: Option<String>,
    auth_subject: String,
    details_json: String,
    created_at: i64,
}

// ----------------------------------------------------------------------
// ヘルパー
// ----------------------------------------------------------------------

/// LIKE パターンのメタ文字 (`%` / `_` / `\`) をエスケープする。
///
/// `ESCAPE '\'` を指定した LIKE と併用すること。
fn escape_like_pattern(needle: &str) -> String {
    let mut escaped = String::with_capacity(needle.len());
    for ch in needle.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// マッチ位置を中心に前後 `context_chars` 文字を抜き出してスニペットを作る。
///
/// 見つからない場合は先頭から切り出す (大文字小文字変換で長さが変わる言語等の
/// フォールバック)。バイト境界は常に文字境界へスナップする。
fn excerpt_around(text: &str, needle: &str, context_chars: usize) -> String {
    let lower_text = text.to_lowercase();
    let lower_needle = needle.to_lowercase();

    let Some(found) = lower_text.find(&lower_needle) else {
        return text.chars().take(context_chars * 2).collect();
    };
    let found_end = (found + needle.len()).min(text.len());

    let mut start = found.min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = found_end;
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }

    let before: String = text[..start]
        .chars()
        .rev()
        .take(context_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let after: String = text[end..].chars().take(context_chars).collect();

    let mut snippet = String::new();
    if start > 0 {
        snippet.push('…');
    }
    snippet.push_str(&before);
    snippet.push_str(&text[start..end]);
    snippet.push_str(&after);
    if end < text.len() {
        snippet.push('…');
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_like_pattern_escapes_metacharacters() {
        assert_eq!(escape_like_pattern("50%_off"), "50\\%\\_off");
        assert_eq!(escape_like_pattern("back\\slash"), "back\\\\slash");
        assert_eq!(escape_like_pattern("plain"), "plain");
    }

    #[test]
    fn excerpt_around_centers_on_match() {
        let text = "0123456789あいうえおかきくけこabcdefghij";
        let snippet = excerpt_around(text, "あいうえお", 4);
        assert_eq!(snippet, "…6789あいうえおかきくけ…");
    }

    #[test]
    fn excerpt_around_handles_missing_match() {
        let snippet = excerpt_around("abcdef", "zzz", 2);
        assert_eq!(snippet, "abcd");
    }

    #[test]
    fn excerpt_around_keeps_multibyte_intact() {
        let text = "こんにちは世界、これはテストです";
        let snippet = excerpt_around(text, "世界", 3);
        assert!(snippet.contains("世界"));
        assert!(!snippet.is_empty());
    }
}
