//! `git` CLI ラッパー。
//!
//! FlexAgent は Git のプラミングコマンド（`git worktree`, `git rev-parse`,
//! `git write-tree` 等）を前提とするため、`git2` バインディングではなく
//! **CLI (`git`) を呼び出す**方針を取る（ユーザーの Git 設定・フック・
//! `GIT_*` 環境変数と常に同じ挙動になる）。
//!
//! 実行ファイルは [`fxg_pty::resolve_command`] で解決する
//! (Windows では `git.exe` を `PATHEXT` 込みで解決する)。

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use crate::error::NodeError;

/// `git` 実行結果 (生バイト列を含む)。
#[derive(Debug, Clone)]
pub struct GitOutput {
    /// 終了ステータスが成功だったか
    pub success: bool,
    /// 標準出力 (生バイト列)
    pub stdout: Vec<u8>,
    /// 標準エラー (trim 済み文字列)
    pub stderr: String,
}

impl GitOutput {
    /// 標準出力を UTF-8 (lossy) 文字列として返す。
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// 標準出力を trim した文字列として返す。
    pub fn stdout_trimmed(&self) -> String {
        self.stdout_text().trim().to_owned()
    }
}

/// `git` を実行して生の出力を返す (終了コードは [`GitOutput::success`])。
pub async fn capture(
    cwd: &Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<GitOutput, NodeError> {
    let program = fxg_pty::resolve_command("git", cwd)
        .map_err(|err| NodeError::GitUnavailable(err.to_string()))?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().await.map_err(NodeError::Plain)?;
    Ok(GitOutput {
        success: output.status.success(),
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

/// `git` を実行し、成功時の標準出力 (trim 済み) を返す。
pub async fn run_git(cwd: &Path, args: &[&str]) -> Result<String, NodeError> {
    run_git_with_env(cwd, args, &[]).await
}

/// `git` を環境変数付きで実行し、成功時の標準出力 (trim 済み) を返す。
pub async fn run_git_with_env(
    cwd: &Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<String, NodeError> {
    let output = capture(cwd, args, env).await?;
    if !output.success {
        return Err(NodeError::Git {
            cwd: cwd.to_path_buf(),
            args: args.join(" "),
            message: output.stderr,
        });
    }
    Ok(output.stdout_trimmed())
}

/// `git` を実行し、失敗 (非ゼロ終了) は `None` として扱う。
///
/// 未設定の `remote.origin.url` や非 Git ディレクトリの判定に使用する。
pub async fn try_git(cwd: &Path, args: &[&str]) -> Result<Option<String>, NodeError> {
    let output = capture(cwd, args, &[]).await?;
    if output.success {
        Ok(Some(output.stdout_trimmed()))
    } else {
        Ok(None)
    }
}

/// `git` を実行し、成功時の標準出力 (生バイト列) を返す。
///
/// `-z` (NUL 区切り) 出力のパースなどに使用する。
pub async fn run_git_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, NodeError> {
    let output = capture(cwd, args, &[]).await?;
    if !output.success {
        return Err(NodeError::Git {
            cwd: cwd.to_path_buf(),
            args: args.join(" "),
            message: output.stderr,
        });
    }
    Ok(output.stdout)
}

/// Git リポジトリのルート (`git rev-parse --show-toplevel`)。非 Git なら `None`。
pub async fn toplevel(cwd: &Path) -> Result<Option<PathBuf>, NodeError> {
    let Some(root) = try_git(cwd, &["rev-parse", "--show-toplevel"]).await? else {
        return Ok(None);
    };
    if root.is_empty() {
        return Ok(None);
    }
    // Windows で `\\?\` が付かないよう dunce で正規化する
    Ok(Some(
        fxg_pty::canonicalize(&root).unwrap_or_else(|_| PathBuf::from(root)),
    ))
}

/// `remote.origin.url`。未設定なら `None`。
pub async fn remote_origin_url(repo: &Path) -> Result<Option<String>, NodeError> {
    match try_git(repo, &["config", "--get", "remote.origin.url"]).await? {
        Some(url) if !url.is_empty() => Ok(Some(url)),
        _ => Ok(None),
    }
}

/// 現在のブランチ名。detached HEAD または未コミットの場合は `None`。
pub async fn current_branch(repo: &Path) -> Result<Option<String>, NodeError> {
    match try_git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).await? {
        Some(branch) if !branch.is_empty() && branch != "HEAD" => Ok(Some(branch)),
        _ => Ok(None),
    }
}

/// 現在の HEAD コミット。コミットが無い場合は `None`。
pub async fn head_commit(repo: &Path) -> Result<Option<String>, NodeError> {
    match try_git(repo, &["rev-parse", "HEAD"]).await? {
        Some(commit) if !commit.is_empty() => Ok(Some(commit)),
        _ => Ok(None),
    }
}

/// Git リポジトリの中にいるか。
pub async fn is_inside_work_tree(cwd: &Path) -> Result<bool, NodeError> {
    Ok(try_git(cwd, &["rev-parse", "--is-inside-work-tree"])
        .await?
        .map(|value| value == "true")
        .unwrap_or(false))
}

/// 指定パス配下の Git リポジトリを再帰探索する (`.git` ディレクトリを持つディレクトリ)。
///
/// `fxg project scan` から使用する。シンボリックリンクは辿らない。
pub fn find_repositories(root: &Path, max_depth: usize) -> Result<Vec<PathBuf>, NodeError> {
    let mut repositories = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > max_depth {
            continue;
        }
        if dir.join(".git").exists() {
            repositories.push(dir.clone());
            continue; // リポジトリ内部のネスト探索は行わない
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            stack.push((path, depth + 1));
        }
    }
    repositories.sort();
    Ok(repositories)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::init_test_repo;

    #[tokio::test]
    async fn detects_repository_and_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_test_repo(dir.path()).await;

        assert!(is_inside_work_tree(dir.path()).await.expect("inside"));
        let root = toplevel(dir.path()).await.expect("toplevel").expect("repo");
        assert_eq!(
            fxg_pty::canonicalize(root).expect("canonical"),
            fxg_pty::canonicalize(dir.path()).expect("canonical tempdir")
        );
        assert_eq!(
            current_branch(dir.path()).await.expect("branch").as_deref(),
            Some("main")
        );
        assert!(head_commit(dir.path()).await.expect("head").is_some());
        assert!(
            remote_origin_url(dir.path())
                .await
                .expect("remote")
                .is_none()
        );
    }

    #[tokio::test]
    async fn non_repository_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!is_inside_work_tree(dir.path()).await.expect("inside"));
        assert!(toplevel(dir.path()).await.expect("toplevel").is_none());
        assert!(
            try_git(dir.path(), &["rev-parse", "HEAD"])
                .await
                .expect("try")
                .is_none()
        );
    }

    #[tokio::test]
    async fn failing_git_command_reports_context() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_test_repo(dir.path()).await;
        let err = run_git(dir.path(), &["rev-parse", "does-not-exist"])
            .await
            .expect_err("must fail");
        match err {
            NodeError::Git { args, .. } => assert!(args.contains("does-not-exist")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn find_repositories_scans_children() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo_a = dir.path().join("a");
        let repo_b = dir.path().join("nested/b");
        std::fs::create_dir_all(&repo_a).expect("mkdir a");
        std::fs::create_dir_all(&repo_b).expect("mkdir b");
        init_test_repo(&repo_a).await;
        init_test_repo(&repo_b).await;
        std::fs::write(dir.path().join("not-a-repo.txt"), "x").expect("write");

        let found = find_repositories(dir.path(), 3).expect("scan");
        assert_eq!(found.len(), 2, "found: {found:?}");
        assert!(found.iter().any(|path| path.ends_with("a")));
        assert!(found.iter().any(|path| path.ends_with("b")));
    }
}
