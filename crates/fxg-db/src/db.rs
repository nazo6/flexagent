//! SQLite 接続管理とマイグレーション適用。

use std::path::{Path, PathBuf};
use std::time::Duration;

use fxg_protocol::common::{
    AuditLogEntry, NodeSummary, PermissionRequestEntry, ProjectSummary, SearchHit, SessionSummary,
};
use fxg_protocol::config::{NODE_DB_FILE_NAME, SERVER_DB_FILE_NAME};
use fxg_protocol::events::{SessionEventBatch, SessionEventEnvelope, UnifiedEventPayload};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::error::DbError;
use crate::{audit, events, outbox, queries, registration};

/// DB の実行ロール。
///
/// `node.db` / `server.db` は同一スキーマを共有し、挙動差はこのロールで分岐する
/// (実行ロールによる分岐は Phase 4 以降で Outbox 同期・中継・Web Push などに現れる)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbRole {
    /// 各ノードのローカルDB (`node.db`)。セッション状態の**権威**。
    Node,
    /// 中央サーバーのDB (`server.db`)。イベント適用による**投影**のみ更新する。
    Hub,
}

impl DbRole {
    /// ロール名 (`node` / `hub`)。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Hub => "hub",
        }
    }
}

/// `~/.flexagent/node.db` のパスを返す。
pub fn node_db_path(fxg_home: &Path) -> PathBuf {
    fxg_home.join(NODE_DB_FILE_NAME)
}

/// `~/.flexagent/server.db` のパスを返す。
pub fn hub_db_path(fxg_home: &Path) -> PathBuf {
    fxg_home.join(SERVER_DB_FILE_NAME)
}

/// SQLite DB ハンドル (接続プール + 実行ロール)。
///
/// `SqlitePool` はハンドル (Arc) をクローンするだけなので、`Db` 自体も
/// 安価にクローンできる (デーモンの共有状態などで使用する)。
#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
    role: DbRole,
}

impl Db {
    /// ファイルDBを開く。親ディレクトリを作成し、マイグレーションを適用する。
    ///
    /// WAL モード / `foreign_keys = ON` / `busy_timeout` は全接続に適用する
    /// (マイグレーション内の `PRAGMA` はトランザクション中では効果がないため)。
    pub async fn open(path: &Path, role: DbRole) -> Result<Self, DbError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        Self::from_pool(pool, role).await
    }

    /// インメモリDBを開く (テスト用)。
    ///
    /// インメモリDBは接続ごとに独立するため、プールを1接続に固定する。
    pub async fn open_in_memory(role: DbRole) -> Result<Self, DbError> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        Self::from_pool(pool, role).await
    }

    async fn from_pool(pool: SqlitePool, role: DbRole) -> Result<Self, DbError> {
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool, role })
    }

    /// 接続プールへの参照を返す (共通クエリ層を直接利用する場合)。
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 実行ロールを返す。
    pub fn role(&self) -> DbRole {
        self.role
    }

    /// 接続プールを閉じる。
    pub async fn close(self) {
        self.pool.close().await;
    }

    // ------------------------------------------------------------------
    // イベント適用エンジン (crate::events)
    // ------------------------------------------------------------------

    /// イベントを冪等に追記し、同一トランザクションで投影を更新する。
    pub async fn append_events(
        &self,
        events: &[SessionEventEnvelope],
    ) -> Result<Vec<events::ApplyOutcome>, DbError> {
        events::append_events(&self.pool, events).await
    }

    /// 単一イベントを冪等に追記し、投影を更新する。
    pub async fn append_event(
        &self,
        event: &SessionEventEnvelope,
    ) -> Result<events::ApplyOutcome, DbError> {
        events::append_events(&self.pool, std::slice::from_ref(event))
            .await
            .map(|mut outcomes| outcomes.remove(0))
    }

    /// セッションの次の `node_seq` を採番してイベントを追記する (実行ノード専用)。
    pub async fn append_next_event(
        &self,
        session_id: &str,
        payload: UnifiedEventPayload,
    ) -> Result<events::AppendedEvent, DbError> {
        events::append_next_event(&self.pool, session_id, payload).await
    }

    /// イベントログ全量から `sessions` / `permission_requests` 投影を再構築する。
    pub async fn rebuild_projections(&self) -> Result<(), DbError> {
        events::rebuild_projections(&self.pool).await
    }

    /// `searchable_text` / `event_type` を payload から再生成する。
    pub async fn regenerate_searchable_text(&self) -> Result<u64, DbError> {
        events::regenerate_searchable_text(&self.pool).await
    }

    /// FTS5 外部コンテンツインデックスを content テーブルから再構築する。
    pub async fn rebuild_fts_index(&self) -> Result<(), DbError> {
        events::rebuild_fts_index(&self.pool).await
    }

    // ------------------------------------------------------------------
    // 水位ベース Outbox (crate::outbox)
    // ------------------------------------------------------------------

    /// 未送信イベントをセッション単位にまとめて抽出する。
    pub async fn extract_outbox(&self) -> Result<Vec<outbox::SessionOutboxBatch>, DbError> {
        outbox::extract_outbox(&self.pool).await
    }

    /// `ResyncRequest` 用: 指定セッションの `from_node_seq` 以降を水位に関係なく抽出する。
    pub async fn extract_outbox_after(
        &self,
        session_id: &str,
        from_node_seq: u64,
    ) -> Result<Vec<SessionEventEnvelope>, DbError> {
        outbox::extract_outbox_after(&self.pool, session_id, from_node_seq).await
    }

    /// `EventBatchAck` 受信時の水位更新 (`synced_up_to_node_seq`)。
    pub async fn ack_watermark(
        &self,
        session_id: &str,
        acked_up_to_node_seq: u64,
    ) -> Result<(), DbError> {
        outbox::ack_watermark(&self.pool, session_id, acked_up_to_node_seq).await
    }

    /// 未同期 Outbox イベント件数 (UI の同期バッジ用)。
    pub async fn unsynced_event_count(&self) -> Result<u64, DbError> {
        outbox::unsynced_event_count(&self.pool).await
    }

    /// 最後に ACK されたイベントの時刻 (最終同期時刻)。
    pub async fn last_synced_at(&self) -> Result<Option<i64>, DbError> {
        outbox::last_synced_at(&self.pool).await
    }

    // ------------------------------------------------------------------
    // ノード自己登録・プロジェクト紐付け (crate::registration)
    // ------------------------------------------------------------------

    /// ノード情報を upsert する (`token_hash` は保持)。
    pub async fn upsert_node(&self, record: &registration::NodeRecord) -> Result<(), DbError> {
        registration::upsert_node(&self.pool, record).await
    }

    /// ノードのオンライン状態を更新する。
    pub async fn set_node_online(&self, node_id: &str, is_online: bool) -> Result<(), DbError> {
        registration::set_node_online(&self.pool, node_id, is_online).await
    }

    /// ノード個別トークンのハッシュを設定 / 失効させる。
    pub async fn set_node_token_hash(
        &self,
        node_id: &str,
        token_hash: Option<&str>,
    ) -> Result<(), DbError> {
        registration::set_node_token_hash(&self.pool, node_id, token_hash).await
    }

    /// ノード個別トークンのハッシュ一覧 `(node_id, token_hash)` を返す。
    pub async fn list_node_tokens(&self) -> Result<Vec<(String, String)>, DbError> {
        registration::list_node_tokens(&self.pool).await
    }

    /// 論理プロジェクトを upsert する。
    pub async fn upsert_project(
        &self,
        record: &registration::ProjectRecord,
    ) -> Result<(), DbError> {
        registration::upsert_project(&self.pool, record).await
    }

    /// プロジェクト × ノードのローカルパス紐付けを upsert する。
    pub async fn upsert_project_binding(
        &self,
        record: &registration::ProjectBindingRecord,
    ) -> Result<(), DbError> {
        registration::upsert_project_binding(&self.pool, record).await
    }

    /// プロジェクト × ノードのローカルパス紐付けを削除する (削除行数を返す)。
    pub async fn delete_project_binding(
        &self,
        node_id: &str,
        local_path: &str,
    ) -> Result<u64, DbError> {
        registration::delete_project_binding(&self.pool, node_id, local_path).await
    }

    /// 同じ `(node_id, local_path)` を持つ他プロジェクトの紐付けを削除する。
    pub async fn delete_other_project_bindings(
        &self,
        node_id: &str,
        local_path: &str,
        keep_project_id: &str,
    ) -> Result<u64, DbError> {
        registration::delete_other_project_bindings(
            &self.pool,
            node_id,
            local_path,
            keep_project_id,
        )
        .await
    }

    /// 紐付けもセッションも持たない孤立プロジェクトを削除する。
    pub async fn delete_orphan_projects(&self) -> Result<u64, DbError> {
        registration::delete_orphan_projects(&self.pool).await
    }

    // ------------------------------------------------------------------
    // 共通クライアント向けクエリ (crate::queries)
    // ------------------------------------------------------------------

    /// セッション一覧 (更新日時降順)。
    pub async fn list_sessions(
        &self,
        filter: &queries::SessionFilter,
    ) -> Result<Vec<SessionSummary>, DbError> {
        queries::list_sessions(&self.pool, filter).await
    }

    /// セッションを1件取得する。
    pub async fn get_session(&self, session_id: &str) -> Result<Option<SessionSummary>, DbError> {
        queries::get_session(&self.pool, session_id).await
    }

    /// `after_cursor` より後のイベント履歴 (差分再開用バッチ) を取得する。
    pub async fn session_events_after(
        &self,
        session_id: &str,
        after_cursor: u64,
        limit: u32,
    ) -> Result<SessionEventBatch, DbError> {
        queries::session_events_after(&self.pool, session_id, after_cursor, limit).await
    }

    /// 全セッション横断で `after_cursor` より後のイベントを取得する
    /// (Client WS のリプレイ用)。
    pub async fn events_after_cursor(
        &self,
        after_cursor: u64,
        limit: u32,
    ) -> Result<SessionEventBatch, DbError> {
        queries::events_after_cursor(&self.pool, after_cursor, limit).await
    }

    /// 論理プロジェクト一覧 (ノード・Worktree 紐付け含む)。
    pub async fn list_projects(&self) -> Result<Vec<ProjectSummary>, DbError> {
        queries::list_projects(&self.pool).await
    }

    /// ノード一覧 (オンライン状態・一時ノード属性含む)。
    pub async fn list_nodes(&self) -> Result<Vec<NodeSummary>, DbError> {
        queries::list_nodes(&self.pool).await
    }

    /// 未解決 (`pending`) の承認リクエスト一覧 (グローバル承認 Inbox)。
    pub async fn pending_permissions(&self) -> Result<Vec<PermissionRequestEntry>, DbError> {
        queries::pending_permissions(&self.pool).await
    }

    /// 承認リクエストを1件取得する。
    pub async fn find_permission_request(
        &self,
        request_id: &str,
    ) -> Result<Option<PermissionRequestEntry>, DbError> {
        queries::find_permission_request(&self.pool, request_id).await
    }

    /// FTS5 全文検索 (3文字未満は LIKE フォールバック)。
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>, DbError> {
        queries::search(&self.pool, query, limit).await
    }

    /// 監査ログ一覧 (新しい順)。
    pub async fn audit_logs(&self, limit: u32) -> Result<Vec<AuditLogEntry>, DbError> {
        queries::audit_logs(&self.pool, limit).await
    }

    /// 監査ログを記録する。
    pub async fn append_audit_log(&self, record: &audit::AuditLogRecord) -> Result<i64, DbError> {
        audit::append_audit_log(&self.pool, record).await
    }

    // ------------------------------------------------------------------
    // Web Push 購読 (中央サーバー)
    // ------------------------------------------------------------------

    /// Web Push 購読を登録・更新する (同一 `endpoint` は鍵を上書き)。
    pub async fn upsert_push_subscription(
        &self,
        record: &crate::push::PushSubscriptionRecord,
    ) -> Result<(), DbError> {
        crate::push::upsert_push_subscription(&self.pool, record).await
    }

    /// 全 Web Push 購読を取得する。
    pub async fn list_push_subscriptions(
        &self,
    ) -> Result<Vec<crate::push::PushSubscriptionRecord>, DbError> {
        crate::push::list_push_subscriptions(&self.pool).await
    }

    /// Web Push 購読を削除する (失効購読のクリーンアップ)。
    pub async fn delete_push_subscription(&self, endpoint: &str) -> Result<(), DbError> {
        crate::push::delete_push_subscription(&self.pool, endpoint).await
    }
}
