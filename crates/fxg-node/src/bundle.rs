//! 一時VM破棄前の Git バンドル退避・復元 (ベストエフォート)。
//!
//! 設計: `docs/01-architecture-and-sync.md` §6.4。
//!
//! 一時VMは Drain 時に「未プッシュコミット + 未コミット変更 (未追跡ファイル含む)」
//! を単一の `git bundle` に固めて中央サーバーへ退避する。復元は別ノード
//! (常駐ノード / 別の一時VM) の新規ワークスペースに対して行い、退避時の
//! 作業ツリー状態をブランチ上のコミットとして再現する。
//!
//! - 未コミット変更は Shadow Git Tree (`GIT_INDEX_FILE` + `git write-tree`) で
//!   ツリー化し、`git commit-tree` でスナップショットコミットにしてから
//!   `refs/heads/fxg-snapshot` に固定して bundle に含める
//! - 復元は「対象ディレクトリが空/未作成なら `git clone`、既存リポジトリなら
//!   `git fetch`」の 2 経路。どちらも `fxg-snapshot` を取得して
//!   現在ブランチへ `checkout -f -B` する

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use crate::error::NodeError;
use crate::git;
use crate::snapshot::ShadowGitTree;

/// 退避対象のスナップショットを固定する一時ブランチ名。
pub const SNAPSHOT_REF: &str = "refs/heads/fxg-snapshot";

/// ワークツリーのバイト列をそのまま往復させるための Git 設定引数。
///
/// Windows の既定 (`core.autocrlf=true`) では clone / checkout 時に LF が CRLF
/// へ変換され、退避した作業ツリーと復元結果が一致しない (CI: windows-latest で
/// 検出。snapshot.rs の Shadow Git 操作と同じ理由)。
const WORKTREE_GIT_CONFIG: &[&str] = &["-c", "core.autocrlf=false", "-c", "core.eol=lf"];

/// ワークツリー Git 操作の引数に [`WORKTREE_GIT_CONFIG`] を前置する。
fn worktree_git_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    WORKTREE_GIT_CONFIG
        .iter()
        .copied()
        .chain(args.iter().copied())
        .collect()
}

/// 生成したワークスペースバンドル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceBundle {
    /// bundle データ (`git bundle create` の生バイト列)
    pub bytes: Vec<u8>,
    /// 退避時の現在ブランチ (`detached HEAD` 時は `None`)
    pub branch: Option<String>,
    /// 復元先でチェックアウトすべきコミット (未コミット変更を含むスナップショット)
    pub head_commit: Option<String>,
}

/// ワークスペースの Git バンドルを作成する。
///
/// 非 Git ディレクトリの場合は [`NodeError::Git`] を返す。
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

    // 1. ワーキングツリー (未コミット変更・未追跡ファイル含む) をツリー化し、
    //    スナップショットコミットを作る (ユーザーの実 Index / HEAD は変更しない)
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
        // スナップショット上限超過等で取得できない場合は HEAD のみ退避する
        None => original_head.clone(),
    };

    // 2. スナップショットコミットをブランチに固定して bundle に含める
    //    (`git clone <bundle>` は refs/heads/* のみを取り込むため)
    if let Some(commit) = snapshot_commit.as_deref() {
        git::run_git(repo, &["update-ref", SNAPSHOT_REF, commit]).await?;
    }

    // 3. bundle create (全 ref を含める)
    let bundle_path = std::env::temp_dir().join(format!("fxg-bundle-{session_id}.bundle"));
    let bundle_arg = bundle_path.to_string_lossy().into_owned();
    let created = git::run_git(repo, &["bundle", "create", &bundle_arg, "--all"]).await;
    if let Err(err) = created {
        let _ = std::fs::remove_file(&bundle_path);
        return Err(err);
    }
    let bytes = std::fs::read(&bundle_path).map_err(|source| NodeError::Io {
        path: bundle_path.clone(),
        source,
    })?;
    let _ = std::fs::remove_file(&bundle_path);
    // 一時ブランチを後始末する (VM内は使い捨てだが、通常ノードでの実行にも備える)
    let _ = git::try_git(repo, &["update-ref", "-d", SNAPSHOT_REF]).await;

    Ok(WorkspaceBundle {
        bytes,
        branch,
        head_commit: snapshot_commit,
    })
}

/// 退避済みバンドルを対象ディレクトリへ復元する。
///
/// - 対象が存在しない / 空ディレクトリ: `git clone <bundle> <target>`
/// - 既存の Git リポジトリ: `git fetch <bundle>` で取り込み
///
/// その後、バンドルのスナップショット (`fxg-snapshot`) を現在ブランチへ
/// `checkout -f -B` して作業ツリー状態を再現する (スナップショットが無い場合は
/// 何もしない = 通常の clone 結果のまま)。
pub async fn restore_workspace_bundle(target: &Path, bundle_file: &Path) -> Result<(), NodeError> {
    let bundle_arg = bundle_file.to_string_lossy().into_owned();
    // 存在しないディレクトリでは git を実行できない (clone 先として使う)
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

    // スナップショットコミットを解決する (clone / fetch の経路差を吸収)
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

    // 現在ブランチ (clone 直後は bundle の HEAD ブランチ) へスナップショットを適用する
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

/// バンドルの Base64 データをファイルへ書き出す (復元用)。
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

/// `git` のバージョンを取得できるか (git 未導入環境の検出用)。
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
            // Windows CI の既定 (core.autocrlf=true) を再現する回帰条件
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

        // 未コミット変更 + 未追跡ファイル
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

        // 新規ディレクトリへ復元
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
