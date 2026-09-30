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

use fxg_protocol::common::{DiffScope, FileDiff, WorkspaceDiffResponse};
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

/// 指定ディレクトリが Git Worktree (メインリポジトリ以外) かどうか。
///
/// `--git-dir` (Worktree では `<repo>/.git/worktrees/<name>`) と
/// `--git-common-dir` (`<repo>/.git`) の差分で判定する。
pub async fn is_worktree(repo: &Path) -> Result<bool, NodeError> {
    let git_dir = match try_git(repo, &["rev-parse", "--path-format=absolute", "--git-dir"]).await?
    {
        Some(value) => Some(value),
        None => try_git(repo, &["rev-parse", "--git-dir"]).await?,
    };
    let common_dir = match try_git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .await?
    {
        Some(value) => Some(value),
        None => try_git(repo, &["rev-parse", "--git-common-dir"]).await?,
    };

    Ok(matches!((git_dir, common_dir), (Some(a), Some(b)) if a != b))
}

/// ワークスペースの Git 差分を取得する (`GET /api/v1/sessions/:id/diff` /
/// `GetGitDiff`)。
///
/// - [`DiffScope::Uncommitted`] は未コミット差分 (`git diff HEAD`)
/// - [`DiffScope::BranchBase`] はベースブランチとの累積差分
///   (`git diff <base>...HEAD`。`base_branch` 省略時は `origin/HEAD` の
///   デフォルトブランチ、それも無ければ `main` を使う)
pub async fn workspace_diff(
    cwd: &Path,
    scope: DiffScope,
    base_branch: Option<&str>,
) -> Result<WorkspaceDiffResponse, NodeError> {
    let head = head_commit(cwd).await?.unwrap_or_default();
    let base = match scope {
        DiffScope::Uncommitted => None,
        DiffScope::BranchBase => Some(match base_branch.filter(|base| !base.trim().is_empty()) {
            Some(base) => base.to_owned(),
            None => default_branch(cwd).await?,
        }),
    };

    let args: Vec<String> = match &base {
        Some(base) => vec![
            "diff".to_owned(),
            "--no-color".to_owned(),
            "--no-ext-diff".to_owned(),
            format!("{base}...HEAD"),
        ],
        None => vec![
            "diff".to_owned(),
            "--no-color".to_owned(),
            "--no-ext-diff".to_owned(),
            "HEAD".to_owned(),
        ],
    };
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = capture(cwd, &arg_refs, &[]).await?;
    if !output.success {
        return Err(NodeError::Git {
            cwd: cwd.to_path_buf(),
            args: args.join(" "),
            message: output.stderr,
        });
    }

    Ok(WorkspaceDiffResponse {
        scope,
        base_branch: base,
        head_commit: head,
        files: parse_unified_diff(&String::from_utf8_lossy(&output.stdout)),
    })
}

/// デフォルトブランチ (`origin/HEAD`) を解決する。未知の場合は `main`。
async fn default_branch(repo: &Path) -> Result<String, NodeError> {
    if let Some(value) = try_git(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .await?
        && let Some(name) = value.strip_prefix("origin/")
        && !name.is_empty()
    {
        return Ok(name.to_owned());
    }
    Ok("main".to_owned())
}

/// Unified Diff (`git diff` 出力) をファイル単位の [`FileDiff`] に分解する。
pub fn parse_unified_diff(diff: &str) -> Vec<FileDiff> {
    struct FileAcc {
        path: String,
        old_path: Option<String>,
        header: String,
        additions: u32,
        deletions: u32,
        text: String,
    }

    let mut files: Vec<FileAcc> = Vec::new();
    let mut current_old_path: Option<String> = None;

    for line in diff.lines() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            files.push(FileAcc {
                path: String::new(),
                old_path: None,
                header: header.to_owned(),
                additions: 0,
                deletions: 0,
                text: String::new(),
            });
            current_old_path = None;
            if let Some(file) = files.last_mut() {
                file.text.push_str(line);
                file.text.push('\n');
            }
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("--- ") {
            current_old_path = (rest != "/dev/null").then(|| strip_prefix_path(rest, "a/"));
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            let new_path = (rest != "/dev/null").then(|| strip_prefix_path(rest, "b/"));
            file.path = new_path.or(current_old_path.clone()).unwrap_or_default();
        }
        if let Some(file) = files.last_mut() {
            if line.starts_with('+') && !line.starts_with("+++") {
                file.additions += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                file.deletions += 1;
            }
            file.text.push_str(line);
            file.text.push('\n');
        }
    }

    files
        .into_iter()
        .map(|mut file| {
            if file.path.is_empty() {
                // バイナリ差分等で `+++` 行が無い場合は `diff --git` ヘッダから
                // パスを推定する (`a/<path> b/<path>`)
                if let Some((left, right)) = file.header.rsplit_once(" b/") {
                    let _ = left;
                    file.old_path = Some(strip_prefix_path(left, "a/"));
                    file.path = right.to_owned();
                } else {
                    file.path = file.header.clone();
                }
            }
            FileDiff {
                path: file.path,
                old_text: None,
                new_text: None,
                unified_diff: file.text,
                additions: file.additions,
                deletions: file.deletions,
            }
        })
        .collect()
}

/// `a/` / `b/` プレフィックスを剥がす。
fn strip_prefix_path(path: &str, prefix: &str) -> String {
    path.strip_prefix(prefix).unwrap_or(path).to_owned()
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

    #[test]
    fn parse_unified_diff_splits_files_and_counts_lines() {
        let diff = "diff --git a/src/main.rs b/src/main.rs\n\
index 1234567..89abcde 100644\n\
--- a/src/main.rs\n\
+++ b/src/main.rs\n\
@@ -1,3 +1,4 @@\n\
 fn main() {\n\
-    old();\n\
+    new();\n\
+    extra();\n\
 }\n\
diff --git a/new.txt b/new.txt\n\
new file mode 100644\n\
--- /dev/null\n\
+++ b/new.txt\n\
@@ -0,0 +1 @@\n\
+hello\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/main.rs");
        assert_eq!(files[0].additions, 2);
        assert_eq!(files[0].deletions, 1);
        assert!(files[0].unified_diff.contains("-    old();"));
        assert_eq!(files[1].path, "new.txt");
        assert_eq!(files[1].additions, 1);
        assert_eq!(files[1].deletions, 0);
    }

    #[tokio::test]
    async fn workspace_diff_reports_uncommitted_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_test_repo(dir.path()).await;
        std::fs::write(dir.path().join("README.md"), "# test\n\nchanged\n").expect("write");
        std::fs::write(dir.path().join("added.txt"), "new file\n").expect("write");
        // 未追跡ファイルは `git diff HEAD` に出ないため stage しておく
        run_git(dir.path(), &["add", "added.txt"])
            .await
            .expect("git add");

        let diff = workspace_diff(dir.path(), DiffScope::Uncommitted, None)
            .await
            .expect("diff");
        assert_eq!(diff.scope, DiffScope::Uncommitted);
        assert!(diff.base_branch.is_none());
        assert!(!diff.head_commit.is_empty());
        let paths: Vec<&str> = diff.files.iter().map(|file| file.path.as_str()).collect();
        assert!(paths.contains(&"README.md"), "files: {paths:?}");
        assert!(paths.contains(&"added.txt"), "files: {paths:?}");
        let readme = diff
            .files
            .iter()
            .find(|file| file.path == "README.md")
            .expect("readme diff");
        // 空行 + "changed" の2行が追加される
        assert_eq!(readme.additions, 2);
    }
}
