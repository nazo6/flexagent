//! ノード自己登録・論理プロジェクト紐付けの upsert。
//!
//! ノードは起動時に `nodes` テーブルへ自分自身の行を upsert し、プロジェクト解決結果を
//! `projects` / `project_node_bindings` へ upsert する (ハブへの `NodeHello` 報告と
//! 同一の情報源)。設計: `docs/02-database-schema.md` §3。

use fxg_protocol::common::{NodeLifecycleStatus, SessionStatus};
use fxg_protocol::util::now_ms;
use sqlx::SqlitePool;

use crate::error::DbError;

/// `nodes` テーブルへの登録内容 (ノード自己登録 / `NodeHello` 更新)。
///
/// `token_hash` / `token_issued_at` は [`set_node_token_hash`] で管理する
/// (通常の upsert では変更しない)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRecord {
    /// ノードID
    pub node_id: String,
    /// 表示名
    pub name: String,
    /// OS (`windows` / `linux` / `macos`)
    pub os: String,
    /// アーキテクチャ (`x86_64` / `aarch64`)
    pub arch: String,
    /// fxg バイナリバージョン
    pub version: String,
    /// 利用可能なエージェントID一覧
    pub installed_agents: Vec<String>,
    /// 一時VM / コンテナノードか
    pub is_ephemeral: bool,
    /// 一時ノードのプロビジョナー識別子
    pub provisioner: Option<String>,
    /// ライフサイクル状態
    pub lifecycle_status: NodeLifecycleStatus,
    /// アイドル自動破棄までの秒数 (一時ノード用)
    pub idle_timeout_secs: Option<u64>,
    /// 接続中か
    pub is_online: bool,
}

impl NodeRecord {
    /// 必須項目のみ指定して常駐・ready・オンラインの登録内容を作る。
    pub fn new(
        node_id: impl Into<String>,
        name: impl Into<String>,
        os: impl Into<String>,
        arch: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            name: name.into(),
            os: os.into(),
            arch: arch.into(),
            version: version.into(),
            installed_agents: Vec::new(),
            is_ephemeral: false,
            provisioner: None,
            lifecycle_status: NodeLifecycleStatus::Ready,
            idle_timeout_secs: None,
            is_online: true,
        }
    }

    /// 利用可能エージェント一覧を設定する。
    pub fn with_agents(mut self, agents: Vec<String>) -> Self {
        self.installed_agents = agents;
        self
    }
}

/// `projects` テーブルへの登録内容 (論理プロジェクト)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    /// 論理プロジェクトID (正規化キー)
    pub project_id: String,
    /// 表示名
    pub name: String,
    /// 正規化元Git URL
    pub canonical_git_url: Option<String>,
}

/// `project_node_bindings` への登録内容 (プロジェクト × ノードのローカルパス)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectBindingRecord {
    /// 論理プロジェクトID
    pub project_id: String,
    /// ノードID
    pub node_id: String,
    /// ノード上のローカルパス
    pub local_path: String,
    /// Git Worktree か
    pub is_worktree: bool,
    /// 最終確認時のGitブランチ
    pub git_branch: Option<String>,
}

/// ノード情報を upsert する (`last_seen_at` は現在時刻で更新)。
///
/// `token_hash` / `token_issued_at` / `created_at` は既存値を保持する。
/// `provisioner` / `idle_timeout_secs` は既存値を `NULL` で上書きしない
/// (サーバーが一時ノードを事前登録した後、NodeHello が `provisioner = None`
/// で upsert しても失わない)。
pub async fn upsert_node(pool: &SqlitePool, record: &NodeRecord) -> Result<(), DbError> {
    let now = now_ms();
    let installed_agents_json = serde_json::to_string(&record.installed_agents)?;
    let idle_timeout_secs = record
        .idle_timeout_secs
        .map(|secs| i64::try_from(secs).unwrap_or(i64::MAX));
    sqlx::query!(
        r#"
        INSERT INTO nodes (
            node_id, name, os, arch, version, installed_agents_json, is_ephemeral,
            provisioner, lifecycle_status, idle_timeout_secs, is_online, last_seen_at, created_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(node_id) DO UPDATE SET
            name = excluded.name,
            os = excluded.os,
            arch = excluded.arch,
            version = excluded.version,
            installed_agents_json = excluded.installed_agents_json,
            is_ephemeral = excluded.is_ephemeral,
            -- 一時ノードの provisioner / idle_timeout はサーバーが事前登録した値を
            -- NodeHello の upsert (provisioner = NULL) で失わないよう保持する
            provisioner = COALESCE(excluded.provisioner, nodes.provisioner),
            lifecycle_status = excluded.lifecycle_status,
            idle_timeout_secs = COALESCE(excluded.idle_timeout_secs, nodes.idle_timeout_secs),
            is_online = excluded.is_online,
            last_seen_at = excluded.last_seen_at
        "#,
        record.node_id,
        record.name,
        record.os,
        record.arch,
        record.version,
        installed_agents_json,
        record.is_ephemeral,
        record.provisioner.as_deref(),
        record.lifecycle_status.as_str(),
        idle_timeout_secs,
        record.is_online,
        now,
        now,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// ノードのオンライン状態を更新する (`last_seen_at` も更新)。
pub async fn set_node_online(
    pool: &SqlitePool,
    node_id: &str,
    is_online: bool,
) -> Result<(), DbError> {
    let result = sqlx::query!(
        r#"UPDATE nodes SET is_online = ?, last_seen_at = ? WHERE node_id = ?"#,
        is_online,
        now_ms(),
        node_id,
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NodeNotFound(node_id.to_owned()));
    }
    Ok(())
}

/// ノード個別トークンのハッシュを設定する (`None` で失効)。
///
/// 平文トークンは保存しない (設計: `docs/01-architecture-and-sync.md` §7.3)。
pub async fn set_node_token_hash(
    pool: &SqlitePool,
    node_id: &str,
    token_hash: Option<&str>,
) -> Result<(), DbError> {
    let issued_at = token_hash.map(|_| now_ms());
    let result = sqlx::query!(
        r#"UPDATE nodes SET token_hash = ?, token_issued_at = ? WHERE node_id = ?"#,
        token_hash,
        issued_at,
        node_id,
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NodeNotFound(node_id.to_owned()));
    }
    Ok(())
}

/// ノード個別トークンのハッシュ一覧 `(node_id, token_hash)` を返す (ハブ認証用)。
///
/// 平文トークンは保存しないため、ハブは提示されたトークンをハッシュ化し、
/// 定数時間比較で照合する (設計: `docs/01-architecture-and-sync.md` §7.3)。
pub async fn list_node_tokens(pool: &SqlitePool) -> Result<Vec<(String, String)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT node_id, token_hash
          FROM nodes
         WHERE token_hash IS NOT NULL
        "#,
    )
    .fetch_all(pool)
    .await?;
    let mut tokens = Vec::with_capacity(rows.len());
    for row in rows {
        // SQLite のプリペアドステートメントからは NOT NULL を判別できないため
        // sqlx は Option を生成する (WHERE 句の IS NOT NULL は実行時に保証される)
        if let (Some(node_id), Some(hash)) = (row.node_id, row.token_hash) {
            tokens.push((node_id, hash));
        }
    }
    Ok(tokens)
}

/// プロジェクト × ノードのローカルパス紐付けを削除する (Worktree 削除時など)。
///
/// 削除された行数を返す。
pub async fn delete_project_binding(
    pool: &SqlitePool,
    node_id: &str,
    local_path: &str,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        DELETE FROM project_node_bindings
         WHERE node_id = ? AND local_path = ?
        "#,
        node_id,
        local_path,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// 同じ `(node_id, local_path)` を持つ他プロジェクトの紐付けを削除する。
///
/// プロジェクトの再リンク (`fxg project link`) 時に古い紐付けを残さないために使う。
/// 削除行数を返す。
pub async fn delete_other_project_bindings(
    pool: &SqlitePool,
    node_id: &str,
    local_path: &str,
    keep_project_id: &str,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        DELETE FROM project_node_bindings
         WHERE node_id = ? AND local_path = ? AND project_id != ?
        "#,
        node_id,
        local_path,
        keep_project_id,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// 紐付けもセッションも持たない孤立プロジェクトを削除する (削除行数を返す)。
///
/// ハブ側ではセッションから参照されるプロジェクトを削除しないようガードする。
pub async fn delete_orphan_projects(pool: &SqlitePool) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        DELETE FROM projects
         WHERE NOT EXISTS (
                   SELECT 1 FROM project_node_bindings b
                    WHERE b.project_id = projects.project_id
               )
           AND NOT EXISTS (
                   SELECT 1 FROM sessions s
                    WHERE s.project_id = projects.project_id
               )
        "#,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// 論理プロジェクトを upsert する。
///
/// 既存行の `canonical_git_url` は `None` で上書きしない
/// (フォールバック解決から後で Git URL が判明するケースに対応)。
pub async fn upsert_project(pool: &SqlitePool, record: &ProjectRecord) -> Result<(), DbError> {
    let now = now_ms();
    sqlx::query!(
        r#"
        INSERT INTO projects (project_id, name, canonical_git_url, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(project_id) DO UPDATE SET
            name = excluded.name,
            canonical_git_url = COALESCE(excluded.canonical_git_url, projects.canonical_git_url),
            updated_at = excluded.updated_at
        "#,
        record.project_id,
        record.name,
        record.canonical_git_url.as_deref(),
        now,
        now,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// プロジェクト × ノードのローカルパス紐付けを upsert する。
///
/// `projects` / `nodes` の行が存在しない場合は FK 制約違反となるため、
/// 呼び出し側で先に [`upsert_project`] / [`upsert_node`] を実行すること。
pub async fn upsert_project_binding(
    pool: &SqlitePool,
    record: &ProjectBindingRecord,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO project_node_bindings
            (project_id, node_id, local_path, is_worktree, git_branch, last_used_at)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(project_id, node_id, local_path) DO UPDATE SET
            is_worktree = excluded.is_worktree,
            git_branch = excluded.git_branch,
            last_used_at = excluded.last_used_at
        "#,
        record.project_id,
        record.node_id,
        record.local_path,
        record.is_worktree,
        record.git_branch.as_deref(),
        now_ms(),
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// 一時VMプロビジョニング中の仮セッション行 (`provisional session`)。
///
/// 一時VMは `bootstrap-workspace` (git clone + ツール導入) を経てから
/// `fxg daemon --stdio` が `SessionCreated` を送るため、それまでの間
/// クライアント (UI) にセッションの存在と `provisioning` 状態を見せる必要がある。
/// 中央サーバーはイベントではなく**投影行の直接挿入**でこれを行い
/// (書き込み権威の一元化は維持)、後続の `SessionCreated`
/// 投影 upsert が派生カラムを上書きする (`status` は `StatusChanged` まで保持)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionalSessionRecord {
    /// セッションID (サーバー採番)
    pub session_id: String,
    /// 論理プロジェクトID
    pub project_id: String,
    /// プロジェクト表示名 (`projects` 行が無い場合の作成用)
    pub project_name: String,
    /// 実行ノードID (一時VMノードの事前採番ID)
    pub node_id: String,
    /// 実行ディレクトリ (起動要求時のローカルパス。UI 表示用)
    pub local_path: String,
    /// Gitブランチ (任意)
    pub git_branch: Option<String>,
    /// 起動するエージェントID
    pub agent_id: String,
    /// 表示タイトル
    pub title: String,
}

/// 一時VMプロビジョニング中の仮セッション行を挿入する (冪等)。
///
/// `sessions` 行が既に存在する場合 (ノードが既に `SessionCreated` を送信済み)
/// は何もしない。呼び出し側は事前に [`upsert_node`] で一時ノードIDを登録すること。
pub async fn upsert_provisional_session(
    pool: &SqlitePool,
    record: &ProvisionalSessionRecord,
) -> Result<(), DbError> {
    let now = now_ms();
    // `sessions.project_id` の FK を満たすため、未知のプロジェクトを用意する
    sqlx::query!(
        r#"
        INSERT INTO projects (project_id, name, canonical_git_url, created_at, updated_at)
        VALUES (?, ?, NULL, ?, ?)
        ON CONFLICT(project_id) DO NOTHING
        "#,
        record.project_id,
        record.project_name,
        now,
        now,
    )
    .execute(pool)
    .await?;

    sqlx::query!(
        r#"
        INSERT INTO sessions (
            session_id, project_id, node_id, local_path, git_branch, is_worktree,
            agent_id, agent_session_id, parent_session_id, fork_from_node_seq,
            title, status, current_mode, available_modes_json,
            available_commands_json, config_options_json,
            git_bundle_path, last_node_seq, synced_up_to_node_seq, created_at, updated_at
        )
        VALUES (?, ?, ?, ?, ?, 0, ?, NULL, NULL, NULL, ?, ?, NULL, '[]', '[]', '[]', NULL, 0, 0, ?, ?)
        ON CONFLICT(session_id) DO NOTHING
        "#,
        record.session_id,
        record.project_id,
        record.node_id,
        record.local_path,
        record.git_branch.as_deref(),
        record.agent_id,
        record.title,
        SessionStatus::Provisioning.as_str(),
        now,
        now,
    )
    .execute(pool)
    .await?;
    Ok(())
}
