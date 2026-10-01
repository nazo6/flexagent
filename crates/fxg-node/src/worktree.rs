//! Git Worktree の解決・作成・削除。
//!
//! 設計: `docs/01-architecture-and-sync.md` §5、`docs/05-cli-and-pwa-ui.md` §2.2/§2.3。
//!
//! - `fxg` が新規作成する Worktree は、親フォルダを散らかさないようデフォルトで
//!   `~/.flexagent/worktrees/<project>/<branch>` に集約する
//!   (`worktree_dir_template` で変更可能)。
//! - 作成時は `.fxg.toml` の `copy_files` (`.env` 等の未追跡ファイルコピー) と
//!   `post_create` (初期化コマンド) フックを実行する。
//! - 検出は `git worktree list --porcelain` を用い、メインリポジトリを含む
//!   すべての Worktree を列挙する。

use std::path::{Path, PathBuf};

use crate::error::NodeError;
use crate::git;

/// `git worktree list --porcelain` の1エントリ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    /// Worktree の絶対パス
    pub path: PathBuf,
    /// HEAD コミット
    pub head: Option<String>,
    /// チェックアウト中のブランチ名 (`refs/heads/` を除いた短縮名)
    pub branch: Option<String>,
    /// bare リポジトリか
    pub is_bare: bool,
    /// detached HEAD か
    pub is_detached: bool,
    /// メインリポジトリ (最初のエントリ) か
    pub is_main: bool,
}

/// Worktree 一覧を取得する (メインリポジトリを先頭に返す)。
pub async fn list_worktrees(repo: &Path) -> Result<Vec<WorktreeEntry>, NodeError> {
    ensure_repository(repo).await?;
    let output = git::run_git(repo, &["worktree", "list", "--porcelain"]).await?;
    Ok(parse_worktree_list(&output))
}

/// `git worktree list --porcelain` の出力をパースする。
pub fn parse_worktree_list(output: &str) -> Vec<WorktreeEntry> {
    let mut entries: Vec<WorktreeEntry> = Vec::new();
    let mut current = WorktreeEntry {
        path: PathBuf::new(),
        head: None,
        branch: None,
        is_bare: false,
        is_detached: false,
        is_main: false,
    };
    let mut has_current = false;

    let flush =
        |current: &mut WorktreeEntry, has_current: &mut bool, entries: &mut Vec<WorktreeEntry>| {
            if *has_current {
                current.is_main = entries.is_empty();
                entries.push(current.clone());
                *has_current = false;
            }
        };

    for line in output.lines() {
        if line.trim().is_empty() {
            flush(&mut current, &mut has_current, &mut entries);
            current = WorktreeEntry {
                path: PathBuf::new(),
                head: None,
                branch: None,
                is_bare: false,
                is_detached: false,
                is_main: false,
            };
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            current.path = PathBuf::from(rest.trim());
            has_current = true;
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            current.head = Some(rest.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            let branch = rest.trim();
            current.branch = Some(
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_owned(),
            );
        } else if line.trim() == "bare" {
            current.is_bare = true;
        } else if line.trim() == "detached" {
            current.is_detached = true;
        }
    }
    flush(&mut current, &mut has_current, &mut entries);
    entries
}

/// Worktree 配置先テンプレートのプレースホルダ解決に使う入力。
#[derive(Debug, Clone)]
pub struct WorktreePathContext {
    /// `~/.flexagent`
    pub fxg_home: PathBuf,
    /// 論理プロジェクトID (`github.com/nazo6/flexagent` 等)
    pub project_id: String,
    /// メインリポジトリのパス
    pub repo_path: PathBuf,
    /// 作成するブランチ名 (`feat/auth` 等)
    pub branch: String,
}

/// テンプレート中の `{project}` / `{branch}` 用にスラッシュ等をハイフンへ正規化する。
pub fn slugify_project(project_id: &str) -> String {
    project_id
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '#' => '-',
            other => other,
        })
        .collect()
}

/// ブランチ名をディレクトリ名向けに正規化する (`feat/auth` → `feat-auth`)。
pub fn slugify_branch(branch: &str) -> String {
    branch
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' => '-',
            other => other,
        })
        .collect()
}

/// `worktree_dir_template` を解決する。
///
/// 利用可能プレースホルダ: `{fxg_home}` / `{project}` / `{repo}` / `{repo_parent}` /
/// `{branch}`。
pub fn resolve_worktree_dir(template: &str, ctx: &WorktreePathContext) -> PathBuf {
    let repo_name = ctx
        .repo_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let repo_parent = ctx
        .repo_path
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();

    let resolved = template
        .replace("{fxg_home}", &ctx.fxg_home.to_string_lossy())
        .replace("{project}", &slugify_project(&ctx.project_id))
        .replace("{repo_parent}", &repo_parent)
        .replace("{repo}", &repo_name)
        .replace("{branch}", &slugify_branch(&ctx.branch));
    PathBuf::from(resolved)
}

/// `post_create` などフックコマンドの実行結果 ([`HookLogEntry`] を再公開)。
pub use fxg_protocol::common::HookLogEntry;

/// Worktree 作成要求。
#[derive(Debug, Clone)]
pub struct WorktreeAddRequest<'a> {
    /// メインリポジトリのパス (Git ルート)
    pub repo: &'a Path,
    /// 論理プロジェクトID
    pub project_id: &'a str,
    /// 作成するブランチ名
    pub branch: &'a str,
    /// 起点ブランチ (省略時は現在の HEAD)
    pub base_branch: Option<&'a str>,
    /// 配置先テンプレート (省略時はデフォルト)
    pub dir_template: &'a str,
    /// 配置先の明示指定 (テンプレートより優先)
    pub new_path: Option<&'a Path>,
    /// `~/.flexagent`
    pub fxg_home: &'a Path,
    /// メインリポジトリからコピーする未追跡ファイル
    pub copy_files: &'a [String],
    /// 作成直後に実行する初期化コマンド
    pub post_create: &'a [String],
}

/// Worktree 作成結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeAddOutcome {
    /// Worktree のパス
    pub path: PathBuf,
    /// ブランチ名
    pub branch: String,
    /// 新規作成されたか (`false` は既存 Worktree の再利用)
    pub created: bool,
    /// コピーしたファイル (リポジトリ相対パス)
    pub copied_files: Vec<String>,
    /// `post_create` フックの実行ログ
    pub hook_logs: Vec<HookLogEntry>,
}

/// Worktree を作成する (同一ブランチの Worktree が既にあれば再利用する)。
pub async fn ensure_worktree(
    request: &WorktreeAddRequest<'_>,
) -> Result<WorktreeAddOutcome, NodeError> {
    ensure_repository(request.repo).await?;

    // 既存 Worktree の再利用 (同一ブランチ)
    let existing = list_worktrees(request.repo).await?;
    if let Some(entry) = existing
        .iter()
        .find(|entry| entry.branch.as_deref() == Some(request.branch) && entry.path != request.repo)
    {
        // `git worktree list` はシンボリックリンクを解決したパスを返すため
        // (macOS の /var → /private/var 等)、返却パスを正規化して揃える
        let path = fxg_pty::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone());
        return Ok(WorktreeAddOutcome {
            path,
            branch: request.branch.to_owned(),
            created: false,
            copied_files: Vec::new(),
            hook_logs: Vec::new(),
        });
    }

    let path = match request.new_path {
        Some(path) => path.to_path_buf(),
        None => resolve_worktree_dir(
            request.dir_template,
            &WorktreePathContext {
                fxg_home: request.fxg_home.to_path_buf(),
                project_id: request.project_id.to_owned(),
                repo_path: request.repo.to_path_buf(),
                branch: request.branch.to_owned(),
            },
        ),
    };
    if path.exists()
        && path
            .read_dir()
            .map(|mut dir| dir.next().is_some())
            .unwrap_or(false)
    {
        return Err(NodeError::InvalidWorktree(format!(
            "worktree directory already exists and is not empty: {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| NodeError::io(parent, source))?;
    }

    // ブランチが既存なら checkout、未作成なら -b で新規作成
    let branch_exists = git::try_git(
        request.repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{}", request.branch),
        ],
    )
    .await?
    .is_some();

    let path_str = path.to_string_lossy().into_owned();
    if branch_exists {
        git::run_git(
            request.repo,
            &["worktree", "add", &path_str, request.branch],
        )
        .await?;
    } else {
        let mut args = vec!["worktree", "add", "-b", request.branch, path_str.as_str()];
        if let Some(base) = request.base_branch {
            args.push(base);
        }
        git::run_git(request.repo, &args).await?;
    }

    // copy_files: メインリポジトリから未追跡ファイルをコピー
    let mut copied_files = Vec::new();
    for relative in request.copy_files {
        let source = request.repo.join(relative);
        if !source.is_file() {
            continue;
        }
        let destination = path.join(relative);
        if destination.exists() {
            continue;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|err| NodeError::io(parent, err))?;
        }
        std::fs::copy(&source, &destination).map_err(|err| NodeError::io(&destination, err))?;
        copied_files.push(relative.clone());
    }

    // post_create: 新規 Worktree 内で初期化コマンドを実行
    let mut hook_logs = Vec::new();
    for command in request.post_create {
        let log = run_hook_command(&path, command).await?;
        if !log.success {
            tracing::warn!(
                worktree = %path.display(),
                command = %command,
                "post_create hook failed"
            );
        }
        hook_logs.push(log);
    }

    // シンボリックリンク解決後のパスへ揃える (git 側の記録と一致させる)
    let path = fxg_pty::canonicalize(&path).unwrap_or(path);

    Ok(WorktreeAddOutcome {
        path,
        branch: request.branch.to_owned(),
        created: true,
        copied_files,
        hook_logs,
    })
}

/// Worktree を削除する (`git worktree remove`)。
pub async fn remove_worktree(repo: &Path, path: &Path, force: bool) -> Result<(), NodeError> {
    ensure_repository(repo).await?;
    let path_str = path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path_str.as_str());
    git::run_git(repo, &args).await?;
    Ok(())
}

/// 削除済みディレクトリの Worktree 管理情報をクリーンアップする。
pub async fn prune_worktrees(repo: &Path) -> Result<String, NodeError> {
    ensure_repository(repo).await?;
    git::run_git(repo, &["worktree", "prune"]).await
}

/// フック実行のタイムアウト (秒)。
const HOOK_TIMEOUT_SECS: u64 = 60;

/// フックコマンドをシェル経由で実行する (失敗しても `Err` にはしない)。
pub async fn run_hook_command(dir: &Path, command: &str) -> Result<HookLogEntry, NodeError> {
    let (program, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let program_path = fxg_pty::resolve_command(program, dir)
        .map_err(|err| NodeError::GitUnavailable(err.to_string()))?;

    let guard = fxg_pty::ProcessTreeGuard::new().map_err(|err| {
        NodeError::Server(format!("failed to initialize process tree guard: {err}"))
    })?;

    let mut cmd = tokio::process::Command::new(program_path);
    cmd.arg(flag)
        .arg(command)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = cmd.spawn().map_err(NodeError::Plain)?;

    #[cfg(windows)]
    {
        if let Some(raw_handle) = child.raw_handle()
            && let Err(err) = guard.attach_raw_handle(raw_handle)
        {
            tracing::warn!("failed to attach hook process to job object: {err}");
        }
    }

    let wait_output = child.wait_with_output();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(HOOK_TIMEOUT_SECS),
        wait_output,
    )
    .await;

    match result {
        Ok(Ok(output)) => {
            let mut text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            if !stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            Ok(HookLogEntry {
                command: command.to_owned(),
                success: output.status.success(),
                output: text,
            })
        }
        Ok(Err(err)) => Err(NodeError::Plain(err)),
        Err(_elapsed) => {
            let message = format!("hook command timed out after {HOOK_TIMEOUT_SECS}s: {command}");
            tracing::warn!("{message}");
            Ok(HookLogEntry {
                command: command.to_owned(),
                success: false,
                output: message,
            })
        }
    }
}

async fn ensure_repository(repo: &Path) -> Result<(), NodeError> {
    if !git::is_inside_work_tree(repo).await? {
        return Err(NodeError::NotARepository(repo.to_path_buf()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    fn add_request<'a>(
        repo: &'a Path,
        fxg_home: &'a Path,
        branch: &'a str,
        template: &'a str,
        copy_files: &'a [String],
        post_create: &'a [String],
    ) -> WorktreeAddRequest<'a> {
        WorktreeAddRequest {
            repo,
            project_id: "github.com/nazo6/flexagent",
            branch,
            base_branch: None,
            dir_template: template,
            new_path: None,
            fxg_home,
            copy_files,
            post_create,
        }
    }

    #[test]
    fn parses_worktree_porcelain() {
        let output = "\
worktree /home/nazo/src/flexagent
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /home/nazo/.flexagent/worktrees/github.com-nazo6-flexagent/feat-auth
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feat/auth
";
        let entries = parse_worktree_list(output);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_main);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert!(!entries[1].is_main);
        assert_eq!(entries[1].branch.as_deref(), Some("feat/auth"));
        assert_eq!(
            entries[1].path,
            PathBuf::from("/home/nazo/.flexagent/worktrees/github.com-nazo6-flexagent/feat-auth")
        );
    }

    #[test]
    fn parses_detached_and_bare_entries() {
        let output = "\
worktree /srv/bare.git
bare

worktree /home/nazo/scratch
HEAD 3333333333333333333333333333333333333333
detached
";
        let entries = parse_worktree_list(output);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_bare);
        assert!(entries[1].is_detached);
        assert!(entries[1].branch.is_none());
    }

    #[test]
    fn resolves_template_placeholders() {
        let ctx = WorktreePathContext {
            fxg_home: PathBuf::from("/home/nazo/.flexagent"),
            project_id: "github.com/nazo6/flexagent".to_owned(),
            repo_path: PathBuf::from("/home/nazo/src/flexagent"),
            branch: "feat/auth".to_owned(),
        };
        assert_eq!(
            resolve_worktree_dir("{fxg_home}/worktrees/{project}/{branch}", &ctx),
            PathBuf::from("/home/nazo/.flexagent/worktrees/github.com-nazo6-flexagent/feat-auth")
        );
        assert_eq!(
            resolve_worktree_dir("{repo_parent}/{repo}-{branch}", &ctx),
            PathBuf::from("/home/nazo/src/flexagent-feat-auth")
        );
        assert_eq!(slugify_project("local:node:abc"), "local-node-abc");
        assert_eq!(slugify_branch("fix/bug/1"), "fix-bug-1");
    }

    #[tokio::test]
    async fn creates_and_removes_worktree_with_hooks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        let home = dir.path().join("fxg-home");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        testutil::init_test_repo(&repo).await;
        std::fs::write(repo.join(".env"), "SECRET=1\n").expect("write .env");

        let copy_files = vec![".env".to_owned()];
        let post_create = vec!["printf hook-ok".to_owned()];
        let request = add_request(
            &repo,
            &home,
            "feat/auth",
            "{fxg_home}/worktrees/{project}/{branch}",
            &copy_files,
            &post_create,
        );
        let outcome = ensure_worktree(&request).await.expect("ensure");

        assert!(outcome.created);
        assert_eq!(
            outcome.path,
            fxg_pty::canonicalize(
                home.join("worktrees")
                    .join("github.com-nazo6-flexagent")
                    .join("feat-auth")
            )
            .expect("canonicalize expected path")
        );
        assert!(outcome.path.join("README.md").is_file());
        assert_eq!(
            std::fs::read_to_string(outcome.path.join(".env")).expect(".env copied"),
            "SECRET=1\n"
        );
        assert_eq!(outcome.copied_files, vec![".env"]);
        assert_eq!(outcome.hook_logs.len(), 1);
        assert!(outcome.hook_logs[0].success);
        assert!(outcome.hook_logs[0].output.contains("hook-ok"));

        // 一覧に現れる
        let entries = list_worktrees(&repo).await.expect("list");
        assert!(
            entries
                .iter()
                .any(|entry| entry.branch.as_deref() == Some("feat/auth"))
        );

        // 2 回目は再利用 (作成しない)
        let again = ensure_worktree(&request).await.expect("ensure again");
        assert!(!again.created);
        assert_eq!(again.path, outcome.path);

        // 未追跡ファイル (.env) があるため force なしでは削除できない
        assert!(
            remove_worktree(&repo, &outcome.path, false).await.is_err(),
            "未追跡ファイルがある場合は force が必要"
        );
        remove_worktree(&repo, &outcome.path, true)
            .await
            .expect("remove");
        let entries = list_worktrees(&repo).await.expect("list");
        assert_eq!(entries.len(), 1);
        prune_worktrees(&repo).await.expect("prune");
    }

    #[tokio::test]
    async fn base_branch_is_used_for_new_branch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        let home = dir.path().join("fxg-home");
        std::fs::create_dir_all(&repo).expect("mkdir");
        testutil::init_test_repo(&repo).await;
        // main から派生した base ブランチを作る
        git::run_git(&repo, &["checkout", "-q", "-b", "develop"])
            .await
            .expect("checkout");
        std::fs::write(repo.join("dev.txt"), "dev\n").expect("write");
        git::run_git(&repo, &["add", "-A"]).await.expect("add");
        git::run_git(&repo, &["commit", "-q", "-m", "dev"])
            .await
            .expect("commit");
        git::run_git(&repo, &["checkout", "-q", "main"])
            .await
            .expect("checkout main");

        let request = WorktreeAddRequest {
            repo: &repo,
            project_id: "github.com/nazo6/flexagent",
            branch: "feat/from-develop",
            base_branch: Some("develop"),
            dir_template: "{fxg_home}/worktrees/{project}/{branch}",
            new_path: None,
            fxg_home: &home,
            copy_files: &[],
            post_create: &[],
        };
        let outcome = ensure_worktree(&request).await.expect("ensure");
        assert!(
            outcome.path.join("dev.txt").is_file(),
            "base_branch の内容がチェックアウトされる"
        );
    }

    #[tokio::test]
    async fn rejects_non_repository() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = list_worktrees(dir.path()).await.expect_err("must fail");
        assert!(matches!(err, NodeError::NotARepository(_)));
    }

    #[tokio::test]
    async fn hook_command_runs_and_captures_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let entry = run_hook_command(dir.path(), "echo hook_test_success")
            .await
            .expect("hook run");
        assert!(entry.success);
        assert!(entry.output.contains("hook_test_success"));
    }
}
