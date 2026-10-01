//! イベント適用エンジン (冪等追記 + 投影更新) と投影再構築。
//!
//! 設計: `docs/02-database-schema.md` §0.3 (イベント適用と投影) / §2 (正データと派生カラム)。
//!
//! - `append_events`: `INSERT OR IGNORE` による冪等追記 + **同一トランザクション**での
//!   `sessions` / `permission_requests` 投影更新
//! - `rebuild_projections`: イベントログ全量からの投影再構築
//!   (「イベント適用後の状態 == 全量再構築後の状態」をテストで保証する)
//! - `regenerate_searchable_text` / `rebuild_fts_index`: FTS5 検索データの再生成
//!
//! # 冪等性
//!
//! `session_events` の `event_id` UNIQUE / `UNIQUE(session_id, node_seq)` により
//! 重複イベントは `INSERT OR IGNORE` で破棄される。投影の更新は「実際に INSERT
//! された場合」のみ行うため、同一バッチの再送 (ACK 前の再送・Resync 再送) は
//! 何度適用しても同じ状態に収束する。

use fxg_protocol::common::{PermissionOption, PermissionRequestStatus};
use fxg_protocol::events::{SessionEventEnvelope, UnifiedEventPayload};
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqliteConnection, SqlitePool, Transaction};

use crate::error::DbError;
use crate::searchable;

/// 1イベント分の適用結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// イベントID
    pub event_id: String,
    /// 対象セッションID
    pub session_id: String,
    /// セッション内連番
    pub node_seq: u64,
    /// 新規に追記されたか (`false` は重複として破棄)
    pub inserted: bool,
    /// このイベントの `session_events.cursor`
    /// (配信元ストアの取り込み順。差分再開カーソルとしてクライアントへ返す)
    pub cursor: u64,
}

/// 採番付き追記 ([`append_next_event`]) の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct AppendedEvent {
    /// 追記されたイベント (node_seq / event_id 確定済み)
    pub envelope: SessionEventEnvelope,
    /// このイベントの `session_events.cursor`
    pub cursor: u64,
}

/// `permission_requests.details_json` に保存する構造化詳細。
///
/// `PermissionRequest` イベントの `details` と `options` の双方を保持し、
/// 承認 Inbox の表示と `PermissionResolved` の状態判定に使用する。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct StoredPermissionDetails {
    /// 引数や Diff 詳細
    #[serde(default)]
    pub details: serde_json::Value,
    /// 選択肢 (option_id / name / kind)
    #[serde(default)]
    pub options: Vec<PermissionOption>,
}

/// イベント列を冪等に追記し、同一トランザクションで投影を更新する。
///
/// 呼び出し側はバッチ内のイベントを `node_seq` 昇順で渡すこと
/// (未知セッションの `SessionCreated` (`node_seq = 1`) が最初に適用され、
/// `sessions` への FK 制約を満たす)。
pub async fn append_events(
    pool: &SqlitePool,
    events: &[SessionEventEnvelope],
) -> Result<Vec<ApplyOutcome>, DbError> {
    let mut tx = pool.begin().await?;
    let mut outcomes = Vec::with_capacity(events.len());
    for event in events {
        outcomes.push(append_event_in_tx(&mut tx, event).await?);
    }
    tx.commit().await?;
    Ok(outcomes)
}

/// セッションの次の `node_seq` を採番してイベントを追記する (**実行ノード専用**)。
///
/// `sessions.last_node_seq + 1` を同一トランザクション内で採番するため、
/// 同時実行でも欠番・重複が生じない。永続化対象外のイベント
/// ([`UnifiedEventPayload::is_persistable`] が `false`) は拒否する。
///
/// # Errors
///
/// 対象セッションが存在しない場合は [`DbError::SessionNotFound`]。
pub async fn append_next_event(
    pool: &SqlitePool,
    session_id: &str,
    payload: UnifiedEventPayload,
) -> Result<AppendedEvent, DbError> {
    let mut tx = pool.begin().await?;

    // 採番は必ずこの UPDATE で行い、トランザクション内でイベント追記まで完了させる
    // (ロールバック時は last_node_seq も巻き戻るため欠番が残らない)。
    let next_node_seq = sqlx::query_scalar!(
        r#"
        UPDATE sessions
           SET last_node_seq = last_node_seq + 1
         WHERE session_id = ?
        RETURNING last_node_seq
        "#,
        session_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(next_node_seq) = next_node_seq else {
        return Err(DbError::SessionNotFound(session_id.to_owned()));
    };
    let node_seq =
        u64::try_from(next_node_seq).map_err(|_| DbError::NegativeNodeSeq(next_node_seq))?;

    let envelope = SessionEventEnvelope {
        event_id: fxg_protocol::util::uuid_v7(),
        session_id: session_id.to_owned(),
        node_seq,
        created_at: fxg_protocol::util::now_ms(),
        payload,
    };
    let outcome = append_event_in_tx(&mut tx, &envelope).await?;
    debug_assert!(
        outcome.inserted,
        "newly allocated node_seq must be inserted"
    );
    tx.commit().await?;
    Ok(AppendedEvent {
        envelope,
        cursor: outcome.cursor,
    })
}

async fn append_event_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    envelope: &SessionEventEnvelope,
) -> Result<ApplyOutcome, DbError> {
    if !envelope.payload.is_persistable() {
        return Err(DbError::NonPersistableEvent(envelope.payload.event_type()));
    }
    let node_seq = node_seq_to_i64(&envelope.session_id, envelope.node_seq)?;
    let (event_type, searchable_text) = searchable::classify_payload(&envelope.payload);
    let payload_json = serde_json::to_string(&envelope.payload)?;

    // SessionCreated は `session_events.session_id -> sessions` の FK を満たすため、
    // イベント追記の**前**に sessions 行 (および未知プロジェクト行) を用意する。
    // (バッチ内のイベントは node_seq 昇順で渡されるため、未知セッションの最初の
    //  イベントは必ず SessionCreated = node_seq 1 になる)
    if let UnifiedEventPayload::SessionCreated { .. } = &envelope.payload {
        upsert_session_from_created(tx, envelope).await?;
    }

    let result = sqlx::query!(
        r#"
        INSERT OR IGNORE INTO session_events
            (event_id, session_id, node_seq, event_type, payload_json, searchable_text, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
        envelope.event_id,
        envelope.session_id,
        node_seq,
        event_type,
        payload_json,
        searchable_text,
        envelope.created_at,
    )
    .execute(&mut **tx)
    .await?;

    let inserted = result.rows_affected() > 0;
    let cursor = if inserted {
        result.last_insert_rowid()
    } else {
        // 重複として破棄されたイベントは既存行のカーソルを返す
        // (event_id 一致 → なければ (session_id, node_seq) 一致で引く)
        let by_event_id = sqlx::query_scalar!(
            r#"SELECT cursor FROM session_events WHERE event_id = ?"#,
            envelope.event_id,
        )
        .fetch_optional(&mut **tx)
        .await?;
        match by_event_id {
            Some(cursor) => cursor,
            None => sqlx::query_scalar!(
                r#"SELECT cursor FROM session_events WHERE session_id = ? AND node_seq = ?"#,
                envelope.session_id,
                node_seq,
            )
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| {
                DbError::InvalidPayload(format!(
                    "ignored event has no stored row: {}",
                    envelope.event_id
                ))
            })?,
        }
    };
    let cursor = u64::try_from(cursor).map_err(|_| DbError::NegativeNodeSeq(cursor))?;

    if inserted {
        apply_projections(tx, envelope).await?;
    }

    Ok(ApplyOutcome {
        event_id: envelope.event_id.clone(),
        session_id: envelope.session_id.clone(),
        node_seq: envelope.node_seq,
        inserted,
        cursor,
    })
}

/// `SessionCreated` イベントから `projects` / `sessions` 行を upsert する。
///
/// 冪等 (再構築時の再生・重複適用の双方で同じ状態に収束する)。
async fn upsert_session_from_created(
    conn: &mut SqliteConnection,
    envelope: &SessionEventEnvelope,
) -> Result<(), DbError> {
    let UnifiedEventPayload::SessionCreated {
        node_id,
        project_id,
        project_name,
        local_path,
        git_branch,
        is_worktree,
        agent_id,
        parent_session_id,
        fork_from_node_seq,
        title,
    } = &envelope.payload
    else {
        return Ok(());
    };

    let node_seq = node_seq_to_i64(&envelope.session_id, envelope.node_seq)?;

    // ハブ側で未知のプロジェクトだった場合に備えて行を用意する
    // (ノード側はプロジェクト解決時に正データが既に存在するため DO NOTHING)。
    sqlx::query!(
        r#"
        INSERT INTO projects (project_id, name, canonical_git_url, created_at, updated_at)
        VALUES (?, ?, NULL, ?, ?)
        ON CONFLICT(project_id) DO NOTHING
        "#,
        project_id,
        project_name,
        envelope.created_at,
        envelope.created_at,
    )
    .execute(&mut *conn)
    .await?;

    let fork_from_node_seq_value = fork_from_node_seq
        .map(|seq| {
            i64::try_from(seq).map_err(|_| DbError::NodeSeqOutOfRange {
                session_id: envelope.session_id.clone(),
                node_seq: seq,
            })
        })
        .transpose()?;

    // 再構築時に備えて全カラムを上書きする (INSERT OR REPLACE は
    // ON DELETE CASCADE でイベントログを消してしまうため使用しない)。
    sqlx::query!(
        r#"
        INSERT INTO sessions (
            session_id, project_id, node_id, local_path, git_branch, is_worktree,
            agent_id, agent_session_id, parent_session_id, fork_from_node_seq,
            title, status, current_mode, available_modes_json,
            available_commands_json, config_options_json,
            git_bundle_path, last_node_seq, synced_up_to_node_seq, created_at, updated_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, 'idle', NULL, '[]', '[]', '[]', NULL, ?, 0, ?, ?)
        ON CONFLICT(session_id) DO UPDATE SET
            project_id = excluded.project_id,
            node_id = excluded.node_id,
            local_path = excluded.local_path,
            git_branch = excluded.git_branch,
            is_worktree = excluded.is_worktree,
            agent_id = excluded.agent_id,
            parent_session_id = excluded.parent_session_id,
            fork_from_node_seq = excluded.fork_from_node_seq,
            title = excluded.title,
            created_at = excluded.created_at
        "#,
        envelope.session_id,
        project_id,
        node_id,
        local_path,
        git_branch.as_deref(),
        is_worktree,
        agent_id,
        parent_session_id.as_deref(),
        fork_from_node_seq_value,
        title,
        node_seq,
        envelope.created_at,
        envelope.created_at,
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// イベント1件分の投影更新を適用する (イベントログへの追記は行わない)。
///
/// [`append_events`] と [`rebuild_projections`] の双方から呼ばれる共通ルーチン。
pub(crate) async fn apply_projections(
    conn: &mut SqliteConnection,
    envelope: &SessionEventEnvelope,
) -> Result<(), DbError> {
    let node_seq = node_seq_to_i64(&envelope.session_id, envelope.node_seq)?;

    match &envelope.payload {
        UnifiedEventPayload::SessionCreated { .. } => {
            upsert_session_from_created(conn, envelope).await?;
        }
        UnifiedEventPayload::SessionTitleChanged { title } => {
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET title = ?, updated_at = MAX(updated_at, ?), last_node_seq = MAX(last_node_seq, ?)
                 WHERE session_id = ?
                "#,
                title,
                envelope.created_at,
                node_seq,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::SessionAgentBound { agent_session_id } => {
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET agent_session_id = ?, updated_at = MAX(updated_at, ?), last_node_seq = MAX(last_node_seq, ?)
                 WHERE session_id = ?
                "#,
                agent_session_id,
                envelope.created_at,
                node_seq,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::StatusChanged { status, .. } => {
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET status = ?, updated_at = MAX(updated_at, ?), last_node_seq = MAX(last_node_seq, ?)
                 WHERE session_id = ?
                "#,
                status.as_str(),
                envelope.created_at,
                node_seq,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode,
            available_modes,
            available_commands,
            config_options,
        } => {
            let available_modes_json = if available_modes.is_empty() {
                None
            } else {
                Some(serde_json::to_string(available_modes)?)
            };
            let available_commands_json = if available_commands.is_empty() {
                None
            } else {
                Some(serde_json::to_string(available_commands)?)
            };
            let config_options_json = if config_options.is_empty() {
                None
            } else {
                Some(serde_json::to_string(config_options)?)
            };
            // current_mode / available_modes / available_commands / config_options が未指定・空配列の場合は既存値を維持する
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET current_mode = COALESCE(?, current_mode),
                       available_modes_json = COALESCE(?, available_modes_json),
                       available_commands_json = COALESCE(?, available_commands_json),
                       config_options_json = COALESCE(?, config_options_json),
                       updated_at = MAX(updated_at, ?),
                       last_node_seq = MAX(last_node_seq, ?)
                 WHERE session_id = ?
                "#,
                current_mode.as_deref(),
                available_modes_json,
                available_commands_json,
                config_options_json,
                envelope.created_at,
                node_seq,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::PermissionRequest {
            request_id,
            tool_name,
            summary,
            options,
            details,
        } => {
            let details_json = serde_json::to_string(&StoredPermissionDetails {
                details: details.clone(),
                options: options.clone(),
            })?;
            sqlx::query!(
                r#"
                INSERT INTO permission_requests
                    (request_id, session_id, node_id, tool_name, summary, details_json,
                     status, created_at, resolved_at, resolved_by)
                SELECT ?, ?, s.node_id, ?, ?, ?, 'pending', ?, NULL, NULL
                  FROM sessions s
                 WHERE s.session_id = ?
                ON CONFLICT(request_id) DO UPDATE SET
                    tool_name = excluded.tool_name,
                    summary = excluded.summary,
                    details_json = excluded.details_json,
                    status = 'pending',
                    resolved_at = NULL,
                    resolved_by = NULL
                "#,
                request_id,
                envelope.session_id,
                tool_name,
                summary,
                details_json,
                envelope.created_at,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::PermissionResolved {
            request_id,
            selected_option_id,
            resolved_by,
        } => {
            let stored: Option<String> = sqlx::query_scalar!(
                r#"SELECT details_json FROM permission_requests WHERE request_id = ?"#,
                request_id,
            )
            .fetch_optional(&mut *conn)
            .await?;
            let status = resolve_permission_status(stored.as_deref(), selected_option_id);
            // Resync が途中の node_seq から行われた等で対応する
            // PermissionRequest 行が無い場合は 0 行更新となる (破棄)。
            sqlx::query!(
                r#"
                UPDATE permission_requests
                   SET status = ?, resolved_by = ?, resolved_at = ?
                 WHERE request_id = ?
                "#,
                status.as_str(),
                resolved_by,
                envelope.created_at,
                request_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        // sessions 行の派生カラム更新を持たないイベント
        // (会話・ツール実行・ターミナル出力・BootstrapLog 等)。
        // 共通の水位/更新時刻更新のみを行う。
        UnifiedEventPayload::UserMessage { .. }
        | UnifiedEventPayload::AgentMessage { .. }
        | UnifiedEventPayload::AgentThought { .. }
        | UnifiedEventPayload::ToolCall { .. }
        | UnifiedEventPayload::PlanUpdate { .. }
        | UnifiedEventPayload::TerminalOutput { .. }
        | UnifiedEventPayload::TerminalInput { .. }
        | UnifiedEventPayload::BootstrapLog { .. }
        | UnifiedEventPayload::SessionReverted { .. } => {}
    }

    // 全イベント共通: 投影に適用済みの最大 node_seq と更新時刻を反映する
    sqlx::query!(
        r#"
        UPDATE sessions
           SET last_node_seq = MAX(last_node_seq, ?), updated_at = MAX(updated_at, ?)
         WHERE session_id = ?
        "#,
        node_seq,
        envelope.created_at,
        envelope.session_id,
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// `selected_option_id` に対応する選択肢の kind から解決状態を判定する。
///
/// `details_json` に保存した `options` を参照し、`reject*` 系なら `rejected`、
/// それ以外は `approved` とする。options が取得できない場合 (Resync 途中など) は
/// `option_id` の命名規則 (`reject` / `deny` を含む) でフォールバックする。
fn resolve_permission_status(
    details_json: Option<&str>,
    selected_option_id: &str,
) -> PermissionRequestStatus {
    let kind = details_json
        .and_then(|json| serde_json::from_str::<StoredPermissionDetails>(json).ok())
        .and_then(|stored| {
            stored
                .options
                .into_iter()
                .find(|option| option.option_id == selected_option_id)
        })
        .map(|option| option.kind);

    match kind {
        Some(kind) if PermissionOption::is_reject_kind(&kind) => PermissionRequestStatus::Rejected,
        Some(_) => PermissionRequestStatus::Approved,
        None => {
            if selected_option_id.contains("reject") || selected_option_id.contains("deny") {
                PermissionRequestStatus::Rejected
            } else {
                PermissionRequestStatus::Approved
            }
        }
    }
}

/// イベントログ全量から `sessions` / `permission_requests` 投影を再構築する。
///
/// # 実装上の制約
///
/// - `session_events.session_id` は `ON DELETE CASCADE` で `sessions` を参照するため、
///   `sessions` 行を DELETE することはできない (イベントログごと消えてしまう)。
///   そこで「派生カラムを既定値にリセット → カーソル順に `SessionCreated` から再生」
///   という手順を取る。
/// - `git_bundle_path` (ハブ固有管理フィールド) と `synced_up_to_node_seq`
///   (ノード固有の水位) はイベント由来ではないため保持する。
/// - イベント全量をメモリに読み出す (セルフホスト・単一テナント規模を前提とした単純化)。
pub async fn rebuild_projections(pool: &SqlitePool) -> Result<(), DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT event_id, session_id, node_seq, payload_json, created_at
          FROM session_events
         ORDER BY cursor ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut tx = pool.begin().await?;

    sqlx::query!(r#"DELETE FROM permission_requests"#)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        r#"
        UPDATE sessions
           SET title = 'New Session',
               status = 'idle',
               current_mode = NULL,
               available_modes_json = '[]',
               available_commands_json = '[]',
               config_options_json = '[]',
               agent_session_id = NULL,
               last_node_seq = 0,
               updated_at = created_at
        "#,
    )
    .execute(&mut *tx)
    .await?;

    for row in rows {
        let envelope = envelope_from_row(
            row.event_id,
            row.session_id,
            row.node_seq,
            row.payload_json,
            row.created_at,
        )?;
        apply_projections(&mut tx, &envelope).await?;
    }

    tx.commit().await?;
    Ok(())
}

/// `searchable_text` / `event_type` を payload から再生成する
/// (抽出ロジックを変更した際の再計算用)。
///
/// `session_events` の UPDATE は FTS5 同期トリガー (`session_events_fts_au`) を
/// 経由して検索インデックスにも反映される。
pub async fn regenerate_searchable_text(pool: &SqlitePool) -> Result<u64, DbError> {
    let rows = sqlx::query!(
        r#"SELECT cursor, event_id, session_id, node_seq, payload_json, created_at FROM session_events"#,
    )
    .fetch_all(pool)
    .await?;

    let mut tx = pool.begin().await?;
    let mut updated_rows = 0u64;
    for row in rows {
        let envelope = envelope_from_row(
            row.event_id,
            row.session_id,
            row.node_seq,
            row.payload_json,
            row.created_at,
        )?;
        let (event_type, searchable_text) = searchable::classify_payload(&envelope.payload);
        let result = sqlx::query!(
            r#"UPDATE session_events SET event_type = ?, searchable_text = ? WHERE cursor = ?"#,
            event_type,
            searchable_text,
            row.cursor,
        )
        .execute(&mut *tx)
        .await?;
        updated_rows += result.rows_affected();
    }
    tx.commit().await?;
    Ok(updated_rows)
}

/// FTS5 外部コンテンツインデックスを content テーブルから再構築する。
///
/// `searchable_text` は content テーブル (`session_events`) から読み直されるため、
/// 適用前に [`regenerate_searchable_text`] を実行しておくこと。
pub async fn rebuild_fts_index(pool: &SqlitePool) -> Result<(), DbError> {
    // FTS5 の 'rebuild' コマンドはプレースホルダを持たない特殊構文のため
    // sqlx の型安全マクロ (`query!`) は使用できない (規約上の例外。単体テストで担保)。
    sqlx::query(r#"INSERT INTO session_events_fts(session_events_fts) VALUES('rebuild')"#)
        .execute(pool)
        .await?;
    Ok(())
}

/// DB 行 (payload_json) から [`SessionEventEnvelope`] を復元する。
pub(crate) fn envelope_from_row(
    event_id: String,
    session_id: String,
    node_seq: i64,
    payload_json: String,
    created_at: i64,
) -> Result<SessionEventEnvelope, DbError> {
    let payload: UnifiedEventPayload = serde_json::from_str(&payload_json)
        .map_err(|err| DbError::InvalidPayload(format!("event {event_id}: {err}")))?;
    let node_seq = u64::try_from(node_seq).map_err(|_| DbError::NegativeNodeSeq(node_seq))?;
    Ok(SessionEventEnvelope {
        event_id,
        session_id,
        node_seq,
        created_at,
        payload,
    })
}

fn node_seq_to_i64(session_id: &str, node_seq: u64) -> Result<i64, DbError> {
    i64::try_from(node_seq).map_err(|_| DbError::NodeSeqOutOfRange {
        session_id: session_id.to_owned(),
        node_seq,
    })
}
