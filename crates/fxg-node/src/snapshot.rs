//! Per-turn snapshots via a Shadow Git Tree (`GIT_INDEX_FILE` + `git write-tree`).
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §4.1.
//!
//! To save and restore the file state of an arbitrary turn in milliseconds without
//! ever dirtying the user's real index (staging state) or branch history, it uses
//! Git plumbing commands and a session-specific index file:
//!
//! 1. With `GIT_INDEX_FILE=~/.flexagent/snapshots/<session-id>.index`, run
//!    `git add -A` -> `git write-tree` to obtain a tree hash.
//!    This hash is stored in the event payload as
//!    `UnifiedEventPayload::UserMessage.snapshot_tree_hash` (it is not duplicated into
//!    a dedicated column).
//! 2. Restore proceeds as follows: "create a backup tree that saves the current
//!    state -> delete extra files -> expand with `git read-tree` + `git
//!    checkout-index`".
//!
//! The shadow index is **isolated per session**, so multiple concurrent sessions
//! (worktrees) in the same repository never contend for the index file. To prevent
//! snapshots from bloating due to huge untracked files (build artifacts and so on),
//! the snapshot is skipped when the size exceeds the limit
//! ([`DEFAULT_SNAPSHOT_SIZE_LIMIT_BYTES`]).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::NodeError;
use crate::git;

/// Default snapshot size limit (100 MB).
pub const DEFAULT_SNAPSHOT_SIZE_LIMIT_BYTES: u64 = 100 * 1024 * 1024;

/// Configuration arguments prefixed to every shadow Git operation (`git -c <key>=<value>`).
///
/// FXG snapshots and restores must round-trip the raw bytes of the worktree, so EOL
/// conversion (`core.autocrlf` / `core.eol`) is disabled. Under the Windows default
/// (`core.autocrlf=true`), restoring rewrites LF to CRLF and destroys the workspace
/// (detected in CI on windows-latest). `core.eol=lf` pins the checkout-side conversion
/// of files with the `text` attribute in `.gitattributes` to be platform-independent.
const SHADOW_GIT_CONFIG: &[&str] = &["-c", "core.autocrlf=false", "-c", "core.eol=lf"];

/// Prefixes the arguments of a shadow Git operation with [`SHADOW_GIT_CONFIG`].
fn shadow_git_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    SHADOW_GIT_CONFIG
        .iter()
        .copied()
        .chain(args.iter().copied())
        .collect()
}

/// Result of taking a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotOutcome {
    /// Successfully created
    Created {
        /// 40-character tree hash (the value stored as `snapshot_tree_hash`)
        tree_hash: String,
        /// Number of target files
        file_count: usize,
        /// Total size (bytes)
        total_bytes: u64,
    },
    /// Skipped because the size limit was exceeded (a warning is logged and nothing
    /// is recorded in the event)
    SkippedTooLarge {
        /// Estimated total size (bytes)
        total_bytes: u64,
        /// Configured limit (bytes)
        limit_bytes: u64,
    },
}

impl SnapshotOutcome {
    /// The tree hash, if obtained (`None` when skipped).
    pub fn tree_hash(&self) -> Option<&str> {
        match self {
            Self::Created { tree_hash, .. } => Some(tree_hash),
            Self::SkippedTooLarge { .. } => None,
        }
    }
}

/// Result of a restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreOutcome {
    /// Tree hash of the backup that saved the state immediately before the restore
    /// (prevents data loss)
    pub backup_tree_hash: Option<String>,
    /// Number of restored files
    pub restored_files: usize,
    /// Number of removed files
    pub removed_files: usize,
}

/// Session-scoped Shadow Git Tree.
#[derive(Debug, Clone)]
pub struct ShadowGitTree {
    repo: PathBuf,
    index_path: PathBuf,
    index_path_env: String,
    session_id: String,
    size_limit_bytes: u64,
}

impl ShadowGitTree {
    /// Creates an instance from the repository and the shadow index path.
    ///
    /// The index path is expected to be
    /// [`crate::paths::NodePaths::snapshot_index_path`]
    /// (`~/.flexagent/snapshots/<session-id>.index`).
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

    /// Changes the size limit.
    pub fn with_size_limit(mut self, bytes: u64) -> Self {
        self.size_limit_bytes = bytes;
        self
    }

    /// Path of the shadow index file.
    pub fn index_path(&self) -> &Path {
        &self.index_path
    }

    /// Target repository.
    pub fn repo(&self) -> &Path {
        &self.repo
    }

    /// Session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Obtains the tree hash of the current workspace state.
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

        git::run_git_with_env(&self.repo, &shadow_git_args(&["add", "-A"]), &env).await?;
        let tree_hash =
            git::run_git_with_env(&self.repo, &shadow_git_args(&["write-tree"]), &env).await?;

        Ok(SnapshotOutcome::Created {
            tree_hash,
            file_count: candidates.len(),
            total_bytes,
        })
    }

    /// Restores the workspace to the state of the given tree hash.
    ///
    /// The current state is saved as a backup tree before restoring, so it can be
    /// reverted by passing [`RestoreOutcome::backup_tree_hash`].
    /// The user's real index, branches, and commit history are left unchanged.
    pub async fn restore(&self, tree_hash: &str) -> Result<RestoreOutcome, NodeError> {
        let env = self.index_env();

        // 0. Save the current state (prevent data loss)
        let backup_tree_hash = self.snapshot().await?.tree_hash().map(str::to_owned);

        // 1. File list of the target tree
        let target_files = self.list_tree_files(tree_hash).await?;

        // 2. Delete files that do not exist in the target tree (.gitignore'd files are
        //    not part of the candidates)
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

        // 3. Read the target tree into the shadow index and expand it into the worktree
        git::run_git_with_env(
            &self.repo,
            &shadow_git_args(&["read-tree", tree_hash]),
            &env,
        )
        .await?;
        git::run_git_with_env(
            &self.repo,
            &shadow_git_args(&["checkout-index", "-a", "-f"]),
            &env,
        )
        .await?;

        Ok(RestoreOutcome {
            backup_tree_hash,
            restored_files: target_files.len(),
            removed_files,
        })
    }

    /// Deletes the shadow index file (cleanup at session end).
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

    /// Lists files (repository-relative paths) that can be snapshot targets.
    ///
    /// Takes the union of the following (`.gitignore`'d files are excluded):
    /// - Untracked files (`git ls-files -o --exclude-standard`)
    /// - Files registered in the shadow index (`git ls-files -c`)
    /// - Files committed in HEAD (`git ls-tree -r HEAD`; for the first snapshot)
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
            let Ok(output) = git::capture(&self.repo, &shadow_git_args(&args), env).await else {
                continue;
            };
            if !output.success {
                continue; // Ignore cases such as no HEAD (before the first commit)
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

    /// Retrieves the list of files contained in a tree.
    async fn list_tree_files(&self, tree_hash: &str) -> Result<BTreeSet<String>, NodeError> {
        let output = git::run_git_bytes(
            &self.repo,
            &shadow_git_args(&["ls-tree", "-r", "-z", "--name-only", tree_hash]),
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

        // Restore: file.txt goes back to v1 and extra.txt is deleted
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

        // Restoring from the backup tree reverts the change
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

        // Nothing is staged in the real index
        let staged = git::run_git(&repo, &["diff", "--cached", "--name-only"])
            .await
            .expect("diff cached");
        assert!(
            staged.is_empty(),
            "real index must stay untouched: {staged}"
        );

        // Branch history does not grow either
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
            ".gitignore'd files are not removal targets of a restore"
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
        // Deleting twice must not be an error
        tree.remove_index().expect("remove idempotent");
        let _ = repo;
    }

    /// Snapshot -> restore must not convert the worktree bytes (LF / CRLF) even under
    /// the Windows default `core.autocrlf=true` environment (simulated via repo config).
    #[tokio::test]
    async fn restore_preserves_bytes_under_autocrlf_repo_config() {
        let (_dir, repo, tree) = repo_with_snapshot().await;
        git::run_git(&repo, &["config", "core.autocrlf", "true"])
            .await
            .expect("config autocrlf");

        std::fs::write(repo.join("lf.txt"), "lf line\n").expect("write lf");
        std::fs::write(repo.join("crlf.txt"), "crlf line\r\n").expect("write crlf");
        let hash = tree
            .snapshot()
            .await
            .expect("snapshot")
            .tree_hash()
            .unwrap()
            .to_owned();

        std::fs::write(repo.join("lf.txt"), "changed\n").expect("write changed");
        tree.restore(&hash).await.expect("restore");

        assert_eq!(
            std::fs::read(repo.join("lf.txt")).expect("read lf"),
            b"lf line\n",
            "files with LF are restored as LF"
        );
        assert_eq!(
            std::fs::read(repo.join("crlf.txt")).expect("read crlf"),
            b"crlf line\r\n",
            "files with CRLF are restored as CRLF"
        );
    }
}
