//! Shadow Git Tree (`GIT_INDEX_FILE` + `git write-tree`) によるターン単位スナップショット。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §4.1。
//!
//! ユーザーの実インデックス (ステージング状態) やブランチ履歴を一切汚さずに、
//! 任意のターン時点のファイル状態をミリ秒単位で保存・復元するため、Git のプラミング
//! コマンドとセッション専用インデックスファイルを用いる:
//!
//! 1. 環境変数 `GIT_INDEX_FILE=~/.flexagent/snapshots/<session-id>.index` を指定して
//!    `git add -A` → `git write-tree` を実行し、Tree Hash を得る。
//!    この Hash は `UnifiedEventPayload::UserMessage.snapshot_tree_hash` として
//!    イベント payload に保存する (専用カラムへの複製は行わない)。
//! 2. 復元は「現状を退避するバックアップ Tree の作成 → 余分なファイルの削除 →
//!    `git read-tree` + `git checkout-index` による展開」の順で行う。
//!
//! シャドウ Index は**セッション単位で分離**するため、同一リポジトリで並行する
//! 複数セッション (Worktree) が Index ファイルを奪い合うことはない。
//! また、巨大な未追跡ファイル（ビルド成果物等）によるスナップショット肥大化を防ぐため、
//! サイズ上限 ([`DEFAULT_SNAPSHOT_SIZE_LIMIT_BYTES`]) を超える場合はスキップする。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::NodeError;
use crate::git;

/// デフォルトのスナップショットサイズ上限 (100 MB)。
pub const DEFAULT_SNAPSHOT_SIZE_LIMIT_BYTES: u64 = 100 * 1024 * 1024;

/// スナップショット取得結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotOutcome {
    /// 取得成功
    Created {
        /// 40文字の Tree Hash (`snapshot_tree_hash` として保存する値)
        tree_hash: String,
        /// 対象ファイル数
        file_count: usize,
        /// 合計サイズ (bytes)
        total_bytes: u64,
    },
    /// サイズ上限超過によりスキップした (警告ログを残し、イベントには記録しない)
    SkippedTooLarge {
        /// 見積もり合計サイズ (bytes)
        total_bytes: u64,
        /// 設定された上限 (bytes)
        limit_bytes: u64,
    },
}

impl SnapshotOutcome {
    /// 取得できた Tree Hash (スキップ時は `None`)。
    pub fn tree_hash(&self) -> Option<&str> {
        match self {
            Self::Created { tree_hash, .. } => Some(tree_hash),
            Self::SkippedTooLarge { .. } => None,
        }
    }
}

/// 復元結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreOutcome {
    /// 復元直前の状態を退避したバックアップ Tree Hash (データロスト防止)
    pub backup_tree_hash: Option<String>,
    /// 復元したファイル数
    pub restored_files: usize,
    /// 削除したファイル数
    pub removed_files: usize,
}

/// セッション単位の Shadow Git Tree。
#[derive(Debug, Clone)]
pub struct ShadowGitTree {
    repo: PathBuf,
    index_path: PathBuf,
    index_path_env: String,
    session_id: String,
    size_limit_bytes: u64,
}

impl ShadowGitTree {
    /// リポジトリとシャドウ Index パスから作成する。
    ///
    /// Index パスは [`crate::paths::NodePaths::snapshot_index_path`]
    /// (`~/.flexagent/snapshots/<session-id>.index`) を想定する。
    pub fn new(
        repo: impl Into<PathBuf>,
        index_path: impl Into<PathBuf>,
        session_id: impl Into<String>,
    ) -> Self {
        let index_path = index_path.into();
        Self {
            repo: repo.into(),
            index_path_env: index_path.to_string_lossy().into_owned(),
            index_path,
            session_id: session_id.into(),
            size_limit_bytes: DEFAULT_SNAPSHOT_SIZE_LIMIT_BYTES,
        }
    }

    /// サイズ上限を変更する。
    pub fn with_size_limit(mut self, bytes: u64) -> Self {
        self.size_limit_bytes = bytes;
        self
    }

    /// シャドウ Index ファイルのパス。
    pub fn index_path(&self) -> &Path {
        &self.index_path
    }

    /// 対象リポジトリ。
    pub fn repo(&self) -> &Path {
        &self.repo
    }

    /// セッションID。
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 現在のワークスペース状態の Tree Hash を取得する。
    pub async fn snapshot(&self) -> Result<SnapshotOutcome, NodeError> {
        let env = self.index_env();
        let candidates = self.collect_candidate_files(&env).await?;

        let mut total_bytes: u64 = 0;
        for relative in &candidates {
            let path = self.repo.join(relative);
            if let Ok(metadata) = std::fs::metadata(&path)
                && metadata.is_file()
            {
                total_bytes = total_bytes.saturating_add(metadata.len());
            }
        }

        if total_bytes > self.size_limit_bytes {
            tracing::warn!(
                session_id = %self.session_id,
                total_bytes,
                limit_bytes = self.size_limit_bytes,
                "shadow git snapshot skipped: working tree exceeds size limit"
            );
            return Ok(SnapshotOutcome::SkippedTooLarge {
                total_bytes,
                limit_bytes: self.size_limit_bytes,
            });
        }

        if let Some(parent) = self.index_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| NodeError::io(parent, err))?;
        }

        git::run_git_with_env(&self.repo, &["add", "-A"], &env).await?;
        let tree_hash = git::run_git_with_env(&self.repo, &["write-tree"], &env).await?;

        Ok(SnapshotOutcome::Created {
            tree_hash,
            file_count: candidates.len(),
            total_bytes,
        })
    }

    /// 指定 Tree Hash の状態へワークスペースを復元する。
    ///
    /// 復元前に現状をバックアップ Tree として退避するため、
    /// [`RestoreOutcome::backup_tree_hash`] を使えば元に戻せる。
    /// ユーザーの実インデックス・ブランチ・コミット履歴は変更しない。
    pub async fn restore(&self, tree_hash: &str) -> Result<RestoreOutcome, NodeError> {
        let env = self.index_env();

        // 0. 現状の退避 (データロスト防止)
        let backup_tree_hash = self.snapshot().await?.tree_hash().map(str::to_owned);

        // 1. 目標ツリーのファイル一覧
        let target_files = self.list_tree_files(tree_hash).await?;

        // 2. 目標ツリーに存在しないファイルを削除 (.gitignore 対象は候補に含まれない)
        let current_files = self.collect_candidate_files(&env).await?;
        let mut removed_files: usize = 0;
        for relative in &current_files {
            if target_files.contains(relative) {
                continue;
            }
            let path = self.repo.join(relative);
            if path.is_file() {
                std::fs::remove_file(&path).map_err(|err| NodeError::io(&path, err))?;
                removed_files += 1;
            }
        }

        // 3. 目標ツリーをシャドウ Index に読み込み、ワークツリーへ展開
        git::run_git_with_env(&self.repo, &["read-tree", tree_hash], &env).await?;
        git::run_git_with_env(&self.repo, &["checkout-index", "-a", "-f"], &env).await?;

        Ok(RestoreOutcome {
            backup_tree_hash,
            restored_files: target_files.len(),
            removed_files,
        })
    }

    /// シャドウ Index ファイルを削除する (セッション終了時の後片付け)。
    pub fn remove_index(&self) -> Result<(), NodeError> {
        match std::fs::remove_file(&self.index_path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(NodeError::io(&self.index_path, err)),
        }
    }

    fn index_env(&self) -> [(&str, &str); 1] {
        [("GIT_INDEX_FILE", self.index_path_env.as_str())]
    }

    /// スナップショット対象になり得るファイル (リポジトリ相対パス) を列挙する。
    ///
    /// 以下の和集合を取る (`.gitignore` 対象は含まれない):
    /// - 未追跡ファイル (`git ls-files -o --exclude-standard`)
    /// - シャドウ Index に登録済みのファイル (`git ls-files -c`)
    /// - HEAD にコミット済みのファイル (`git ls-tree -r HEAD`。初回スナップショット用)
    async fn collect_candidate_files(
        &self,
        env: &[(&str, &str)],
    ) -> Result<BTreeSet<String>, NodeError> {
        let mut candidates = BTreeSet::new();
        let listings = [
            vec!["ls-files", "-z", "--others", "--exclude-standard"],
            vec!["ls-files", "-z", "--cached"],
            vec!["ls-tree", "-r", "-z", "--name-only", "HEAD"],
        ];
        for args in listings {
            let Ok(output) = git::capture(&self.repo, &args, env).await else {
                continue;
            };
            if !output.success {
                continue; // HEAD が無い (コミット前) 等は無視する
            }
            for path in output.stdout.split(|byte| *byte == 0) {
                if path.is_empty() {
                    continue;
                }
                candidates.insert(String::from_utf8_lossy(path).into_owned());
            }
        }
        Ok(candidates)
    }

    /// Tree に含まれるファイル一覧を取得する。
    async fn list_tree_files(&self, tree_hash: &str) -> Result<BTreeSet<String>, NodeError> {
        let output = git::run_git_bytes(
            &self.repo,
            &["ls-tree", "-r", "-z", "--name-only", tree_hash],
        )
        .await?;
        Ok(output
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    async fn repo_with_snapshot() -> (tempfile::TempDir, PathBuf, ShadowGitTree) {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        testutil::init_test_repo(&repo).await;
        let tree = ShadowGitTree::new(
            &repo,
            testutil::snapshot_index_path(dir.path(), "session-1"),
            "session-1",
        );
        (dir, repo, tree)
    }

    #[tokio::test]
    async fn snapshot_and_restore_roundtrip() {
        let (dir, repo, tree) = repo_with_snapshot().await;

        std::fs::write(repo.join("file.txt"), "v1\n").expect("write v1");
        let first = tree.snapshot().await.expect("snapshot 1");
        let first_hash = first.tree_hash().expect("hash").to_owned();
        assert_eq!(first_hash.len(), 40);

        std::fs::write(repo.join("file.txt"), "v2\n").expect("write v2");
        std::fs::write(repo.join("extra.txt"), "extra\n").expect("write extra");
        let second = tree.snapshot().await.expect("snapshot 2");
        let second_hash = second.tree_hash().expect("hash").to_owned();
        assert_ne!(first_hash, second_hash);

        // 復元: file.txt は v1 に戻り、extra.txt は削除される
        let outcome = tree.restore(&first_hash).await.expect("restore");
        assert_eq!(
            outcome.backup_tree_hash.as_deref(),
            Some(second_hash.as_str())
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("file.txt")).expect("read"),
            "v1\n"
        );
        assert!(
            !repo.join("extra.txt").exists(),
            "extra file must be removed"
        );
        assert_eq!(outcome.removed_files, 1);

        // バックアップ Tree から復元すれば元に戻せる
        tree.restore(&second_hash).await.expect("restore backup");
        assert_eq!(
            std::fs::read_to_string(repo.join("file.txt")).expect("read"),
            "v2\n"
        );
        assert!(repo.join("extra.txt").is_file());

        let _ = dir;
    }

    #[tokio::test]
    async fn does_not_touch_real_index_or_history() {
        let (_dir, repo, tree) = repo_with_snapshot().await;
        std::fs::write(repo.join("file.txt"), "v1\n").expect("write");
        let hash = tree
            .snapshot()
            .await
            .expect("snapshot")
            .tree_hash()
            .unwrap()
            .to_owned();

        // 実インデックスには何もステージされていない
        let staged = git::run_git(&repo, &["diff", "--cached", "--name-only"])
            .await
            .expect("diff cached");
        assert!(
            staged.is_empty(),
            "real index must stay untouched: {staged}"
        );

        // ブランチ履歴も増えていない
        let log = git::run_git(&repo, &["log", "--oneline"])
            .await
            .expect("log");
        assert_eq!(log.lines().count(), 1);

        tree.restore(&hash).await.expect("restore");
        let staged = git::run_git(&repo, &["diff", "--cached", "--name-only"])
            .await
            .expect("diff cached");
        assert!(staged.is_empty());
    }

    #[tokio::test]
    async fn skips_snapshot_when_over_size_limit() {
        let (_dir, repo, tree) = repo_with_snapshot().await;
        let tree = tree.with_size_limit(1);
        std::fs::write(repo.join("big.txt"), "0123456789\n").expect("write");
        let outcome = tree.snapshot().await.expect("snapshot");
        match outcome {
            SnapshotOutcome::SkippedTooLarge {
                total_bytes,
                limit_bytes,
            } => {
                assert!(total_bytes > limit_bytes);
                assert_eq!(limit_bytes, 1);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert!(outcome.tree_hash().is_none());
    }

    #[tokio::test]
    async fn ignored_files_are_untouched() {
        let (_dir, repo, tree) = repo_with_snapshot().await;
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n").expect("write ignore");
        std::fs::write(repo.join("ignored.txt"), "keep me\n").expect("write ignored");
        std::fs::write(repo.join("tracked.txt"), "v1\n").expect("write tracked");
        let hash = tree
            .snapshot()
            .await
            .expect("snapshot")
            .tree_hash()
            .unwrap()
            .to_owned();

        std::fs::write(repo.join("tracked.txt"), "v2\n").expect("write v2");
        tree.restore(&hash).await.expect("restore");

        assert_eq!(
            std::fs::read_to_string(repo.join("ignored.txt")).expect("read ignored"),
            "keep me\n",
            ".gitignore 対象は復元の削除対象にならない"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("tracked.txt")).expect("read tracked"),
            "v1\n"
        );
    }

    #[tokio::test]
    async fn index_file_is_created_and_removable() {
        let (_dir, repo, tree) = repo_with_snapshot().await;
        assert!(!tree.index_path().exists());
        tree.snapshot().await.expect("snapshot");
        assert!(tree.index_path().is_file(), "shadow index must be created");
        tree.remove_index().expect("remove");
        assert!(!tree.index_path().exists());
        // 二重削除はエラーにしない
        tree.remove_index().expect("remove idempotent");
        let _ = repo;
    }
}
