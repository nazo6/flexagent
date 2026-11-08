//! Best-effort Git bundle backup and restore before an ephemeral VM is destroyed.
//!
//! Design: `docs/01-architecture-and-sync.md` §6.4.
//!
//! On drain, an ephemeral VM packs "unpushed commits + uncommitted changes (including
//! untracked files)" into a single `git bundle` and backs it up to the central server.
//! Restore is performed into a new workspace on another node (resident node / another
//! ephemeral VM), reproducing the backed-up working tree state as a commit on a branch.
//!
//! - Uncommitted changes are turned into a tree with the Shadow Git Tree
//!   (`GIT_INDEX_FILE` + `git write-tree`), made into a snapshot commit with
//!   `git commit-tree`, then pinned to `refs/heads/fxg-snapshot` and included in the
//!   bundle
//! - Restore has two paths: `git clone` if the target directory is empty or does not
//!   exist, `git fetch` if it is an existing repository. Both obtain `fxg-snapshot`
//!   and `checkout -f -B` it onto the current branch

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use crate::error::NodeError;
use crate::git;
use crate::snapshot::ShadowGitTree;

/// Temporary branch name that pins the snapshot to be backed up.
pub const SNAPSHOT_REF: &str = "refs/heads/fxg-snapshot";

/// Git configuration arguments that round-trip the worktree bytes verbatim.
///
/// Under the Windows default (`core.autocrlf=true`), LF is converted to CRLF on
/// clone / checkout, so the backed-up working tree does not match the restore result
/// (detected in CI on windows-latest; the same reason as the Shadow Git operations in
/// snapshot.rs).
const WORKTREE_GIT_CONFIG: &[&str] = &["-c", "core.autocrlf=false", "-c", "core.eol=lf"];

/// Prefixes the arguments of a worktree Git operation with [`WORKTREE_GIT_CONFIG`].
fn worktree_git_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    WORKTREE_GIT_CONFIG
        .iter()
        .copied()
        .chain(args.iter().copied())
        .collect()
}

/// Generated workspace bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceBundle {
    /// Bundle data (raw bytes of `git bundle create`)
    pub bytes: Vec<u8>,
    /// Current branch at backup time (`None` for a detached HEAD)
    pub branch: Option<String>,
    /// Commit to check out at the restore destination (snapshot including uncommitted
    /// changes)
    pub head_commit: Option<String>,
}

/// Creates a Git bundle of the workspace.
///
/// Returns [`NodeError::Git`] for a non-Git directory.
pub async fn create_workspace_bundle(
    repo: &Path,
    index_path: &Path,
    session_id: &str,
) -> Result<WorkspaceBundle, NodeError> {
    if git::toplevel(repo).await?.is_none() {
        return Err(NodeError::Git {
            cwd: repo.to_path_buf(),
            args: "bundle create".to_owned(),
            message: "workspace is not a git repository".to_owned(),
        });
    }

    let branch = git::try_git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await?
        .filter(|branch| branch != "HEAD");
    let original_head = git::try_git(repo, &["rev-parse", "HEAD"]).await?;

    // 1. Turn the working tree (including uncommitted changes and untracked files)
    //    into a tree and create a snapshot commit (the user's real index / HEAD are
    //    left unchanged)
    let tree_hash = ShadowGitTree::new(repo, index_path, session_id)
        .snapshot()
        .await?
        .tree_hash()
        .map(str::to_owned);
    let snapshot_commit = match tree_hash {
        Some(tree) => {
            let mut args = vec!["commit-tree", tree.as_str()];
            if let Some(head) = original_head.as_deref() {
                args.push("-p");
                args.push(head);
            }
            args.push("-m");
            args.push("fxg ephemeral workspace snapshot");
            Some(
                git::run_git_with_env(
                    repo,
                    &args,
                    &[
                        ("GIT_AUTHOR_NAME", "fxg"),
                        ("GIT_AUTHOR_EMAIL", "fxg@localhost"),
                        ("GIT_COMMITTER_NAME", "fxg"),
                        ("GIT_COMMITTER_EMAIL", "fxg@localhost"),
                    ],
                )
                .await?,
            )
        }
        // If it cannot be obtained, e.g. because the snapshot size limit was exceeded,
        // back up HEAD only
        None => original_head.clone(),
    };

    // 2. Pin the snapshot commit to a branch so it is included in the bundle
    //    (`git clone <bundle>` only imports refs/heads/*)
    if let Some(commit) = snapshot_commit.as_deref() {
        git::run_git(repo, &["update-ref", SNAPSHOT_REF, commit]).await?;
    }

    // 3. bundle create (include all refs)
    let bundle_path = std::env::temp_dir().join(format!("fxg-bundle-{session_id}.bundle"));
    let bundle_arg = bundle_path.to_string_lossy().into_owned();
    let created = git::run_git(repo, &["bundle", "create", &bundle_arg, "--all"]).await;
    if let Err(err) = created {
        fxg_protocol::discard_with!(
            std::fs::remove_file(&bundle_path),
            "remove the temporary bundle after a failed creation"
        );
        return Err(err);
    }
    let bytes = std::fs::read(&bundle_path).map_err(|source| NodeError::Io {
        path: bundle_path.clone(),
        source,
    })?;
    fxg_protocol::discard_with!(
        std::fs::remove_file(&bundle_path),
        "remove the temporary bundle file"
    );
    // Clean up the temporary branch (the VM is disposable, but this also covers
    // execution on a regular node)
    fxg_protocol::discard_with!(
        git::try_git(repo, &["update-ref", "-d", SNAPSHOT_REF]).await,
        "delete the temporary snapshot ref"
    );

    Ok(WorkspaceBundle {
        bytes,
        branch,
        head_commit: snapshot_commit,
    })
}

/// Restores a backed-up bundle into the target directory.
///
/// - Target does not exist / is an empty directory: `git clone <bundle> <target>`
/// - Existing Git repository: import with `git fetch <bundle>`
///
/// Then `checkout -f -B` the bundle snapshot (`fxg-snapshot`) onto the current branch
/// to reproduce the working tree state (if there is no snapshot, do nothing and keep
/// the normal clone result).
pub async fn restore_workspace_bundle(target: &Path, bundle_file: &Path) -> Result<(), NodeError> {
    let bundle_arg = bundle_file.to_string_lossy().into_owned();
    // git cannot run in a directory that does not exist (use it as the clone destination)
    let is_repo = target.is_dir() && git::toplevel(target).await?.is_some();

    if !is_repo {
        let parent = target.parent().ok_or_else(|| {
            NodeError::Server(format!(
                "cannot restore bundle: invalid target path {}",
                target.display()
            ))
        })?;
        std::fs::create_dir_all(parent).map_err(|source| NodeError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let target_arg = target.to_string_lossy().into_owned();
        git::run_git(
            parent,
            &worktree_git_args(&["clone", &bundle_arg, &target_arg]),
        )
        .await?;
    } else {
        git::run_git(
            target,
            &worktree_git_args(&[
                "fetch",
                &bundle_arg,
                "+refs/heads/fxg-snapshot:refs/fxg-bundle/fxg-snapshot",
            ]),
        )
        .await?;
    }

    // Resolve the snapshot commit (absorbing the clone / fetch path difference)
    let snapshot = if is_repo {
        git::try_git(target, &["rev-parse", "refs/fxg-bundle/fxg-snapshot"]).await?
    } else {
        git::try_git(target, &["rev-parse", "refs/remotes/origin/fxg-snapshot"]).await?
    };
    let Some(snapshot) = snapshot else {
        tracing::info!(
            target = %target.display(),
            "restored bundle has no workspace snapshot; keeping checked out branch"
        );
        return Ok(());
    };

    // Apply the snapshot to the current branch (right after clone, the bundle's HEAD
    // branch)
    let branch = git::try_git(target, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await?
        .filter(|branch| branch != "HEAD");
    match branch {
        Some(branch) => {
            git::run_git(
                target,
                &worktree_git_args(&["checkout", "-f", "-B", &branch, &snapshot]),
            )
            .await?;
        }
        None => {
            git::run_git(
                target,
                &worktree_git_args(&["checkout", "-f", "--detach", &snapshot]),
            )
            .await?;
        }
    }
    tracing::info!(
        target = %target.display(),
        snapshot,
        "restored workspace bundle snapshot"
    );
    Ok(())
}

/// Writes the bundle's Base64 data to a file (for restore).
pub fn write_bundle_file(dir: &Path, session_id: &str, bytes: &[u8]) -> Result<PathBuf, NodeError> {
    std::fs::create_dir_all(dir).map_err(|source| NodeError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let path = dir.join(format!("restore-{session_id}.bundle"));
    std::fs::write(&path, bytes).map_err(|source| NodeError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Whether the `git` version can be obtained (for detecting environments without git).
pub async fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn init_repo(dir: &Path) {
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "fxg test"],
            // Regression condition reproducing the Windows CI default (core.autocrlf=true)
            vec!["config", "core.autocrlf", "true"],
        ] {
            git::run_git(dir, &args).await.expect("git init");
        }
        std::fs::write(dir.join("committed.txt"), "committed\n").expect("write");
        git::run_git(dir, &["add", "-A"]).await.expect("add");
        git::run_git(dir, &["commit", "-q", "-m", "initial"])
            .await
            .expect("commit");
    }

    #[tokio::test]
    async fn bundle_roundtrip_restores_committed_and_uncommitted_state() {
        let source = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("tempdir");
        init_repo(source.path()).await;

        // Uncommitted changes + untracked files
        std::fs::write(source.path().join("committed.txt"), "modified\n").expect("write");
        std::fs::write(source.path().join("untracked.txt"), "new file\n").expect("write");

        let index_path = home.path().join("snap.index");
        let bundle = create_workspace_bundle(source.path(), &index_path, "test-session")
            .await
            .expect("create bundle");
        assert_eq!(bundle.branch.as_deref(), Some("main"));
        assert!(bundle.head_commit.is_some());
        assert!(!bundle.bytes.is_empty());

        let bundle_file = home.path().join("restore.bundle");
        std::fs::write(&bundle_file, &bundle.bytes).expect("write bundle");

        // Restore into a new directory
        let target = home.path().join("restored");
        restore_workspace_bundle(&target, &bundle_file)
            .await
            .expect("restore");
        assert_eq!(
            std::fs::read_to_string(target.join("committed.txt")).expect("read"),
            "modified\n"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("untracked.txt")).expect("read"),
            "new file\n"
        );
        let branch = git::try_git(&target, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await
            .expect("branch");
        assert_eq!(branch.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn bundle_reports_non_repo_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("tempdir");
        let index_path = home.path().join("snap.index");
        let err = create_workspace_bundle(dir.path(), &index_path, "s")
            .await
            .expect_err("must fail");
        assert!(err.to_string().contains("not a git repository"));
    }
}
