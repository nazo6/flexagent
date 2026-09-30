//! `fxg-db` のエラー型。

/// `fxg-db` の操作で発生するエラー。
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// SQLite 操作エラー
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// マイグレーション適用エラー
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// JSON 変換エラー
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// ファイルシステム操作エラー
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// DB 内のイベント payload がデコードできない (データ破損)
    #[error("invalid event payload in database: {0}")]
    InvalidPayload(String),
    /// 永続化してはいけないイベントを追記しようとした
    /// (ストリーミング途中のチャンク・キーストローク等)
    #[error("event is not persistable: {0}")]
    NonPersistableEvent(&'static str),
    /// `node_seq` が負値 (データ破損)
    #[error("node_seq {0} is negative")]
    NegativeNodeSeq(i64),
    /// `node_seq` が i64 に収まらない
    #[error("node_seq {node_seq} of session {session_id} does not fit into i64")]
    NodeSeqOutOfRange {
        /// 対象セッションID
        session_id: String,
        /// 変換に失敗した連番
        node_seq: u64,
    },
    /// セッションが見つからない
    #[error("session not found: {0}")]
    SessionNotFound(String),
    /// ノードが見つからない
    #[error("node not found: {0}")]
    NodeNotFound(String),
    /// DB 内の列挙値が未知 (データ破損)
    #[error("invalid enum value in database: {0}")]
    InvalidEnumValue(String),
    /// カーソル値が i64 に収まらない
    #[error("cursor {0} does not fit into i64")]
    CursorOutOfRange(u64),
    /// 承認リクエストが見つからない
    #[error("permission request not found: {0}")]
    PermissionRequestNotFound(String),
    /// 検索クエリが空
    #[error("search query must not be empty")]
    EmptySearchQuery,
}
