//! サブコマンドの引数定義と実行処理。
//!
//! CLI の宣言 (`usage` の derive) が単一のソースであり、`__usage_spec__` から
//! KDL spec・シェル補完・manpage・Markdown リファレンスを生成できる
//! (設計: `docs/05-cli-and-pwa-ui.md` §1)。

use anyhow::{Context, Result, bail};
use usage::Args;

use crate::client::{DaemonClient, format_unix_ms_utc, print_table, short_id, truncate};

// ----------------------------------------------------------------------
// fxg daemon
// ----------------------------------------------------------------------

/// `fxg daemon` の引数。
#[derive(Debug, Args)]
pub struct DaemonArgs {
    /// ローカルHTTP/WSサーバーのバインド先 (既定: 127.0.0.1:7860)
    #[usage(long)]
    listen: Option<String>,
    /// 中央サーバーの Node Hub WebSocket URL (Outbox 同期。Phase 4 で実装)
    #[usage(long)]
    server_url: Option<String>,
    /// リモート (中央サーバー経由) からの Web PTY 起動を許可する
    #[usage(long)]
    allow_remote_pty: bool,
    /// 標準入出力パイプ (JSON Lines) モード (一時VM。Phase 6 で実装)
    #[usage(long)]
    stdio: bool,
    /// 一時VMモード (Phase 6 で実装)
    #[usage(long)]
    ephemeral: bool,
    /// `--stdio` 時の初期対象ディレクトリ (Phase 6 で実装)
    #[usage(long)]
    workspace: Option<String>,
}

impl usage::RunAsync for DaemonArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        if self.stdio || self.ephemeral || self.workspace.is_some() {
            bail!("--stdio / --ephemeral / --workspace は Phase 6 (一時VM) で実装します");
        }
        if self.server_url.is_some() {
            bail!("--server-url (Outbox 同期) は Phase 4 で実装します");
        }

        let env = fxg_protocol::config::process_env;
        let global = fxg_protocol::config::GlobalConfig::load(&env)
            .context("failed to load ~/.flexagent/config.toml")?;

        let mut config = fxg_node::daemon::DaemonConfig::from_env(&env);
        if let Some(node_id) = global.node.node_id.clone() {
            config.node_id = node_id;
        }
        if let Some(name) = global.node.name.clone() {
            config.node_name = name;
        }
        if let Some(listen) = self.listen.or_else(|| global.node.listen_addr.clone()) {
            config.listen_addr = listen;
        }
        config.allow_remote_pty = self.allow_remote_pty || global.node.allow_remote_pty;

        fxg_node::daemon::NodeDaemon::run(config)
            .await
            .context("fxg daemon failed")
    }
}

// ----------------------------------------------------------------------
// fxg ps / fxg session list
// ----------------------------------------------------------------------

/// セッション一覧の表示オプション (`fxg ps` と `fxg session list` で共通)。
#[derive(Debug, Args)]
pub struct SessionListArgs {
    /// 停止済みセッションも含めて表示する
    #[usage(short = 'a', long)]
    all: bool,
    /// 論理プロジェクトIDで絞り込む
    #[usage(long)]
    project: Option<String>,
    /// ノードIDで絞り込む
    #[usage(long)]
    node: Option<String>,
    /// JSON 形式で出力する
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for SessionListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let sessions = client.list_sessions(self.all).await?;
        let sessions: Vec<_> = sessions
            .into_iter()
            .filter(|session| {
                self.project
                    .as_deref()
                    .is_none_or(|project| session.project_id == project)
            })
            .filter(|session| {
                self.node
                    .as_deref()
                    .is_none_or(|node| session.node_id == node)
            })
            .collect();

        if self.json {
            println!("{}", serde_json::to_string_pretty(&sessions)?);
            return Ok(());
        }

        if sessions.is_empty() {
            println!("(セッションなし)");
            return Ok(());
        }

        let rows: Vec<Vec<String>> = sessions
            .iter()
            .map(|session| {
                vec![
                    short_id(&session.session_id),
                    truncate(&session.node_id, 16),
                    truncate(&session.agent_id, 16),
                    session.status.to_string(),
                    format_unix_ms_utc(session.updated_at),
                    truncate(&session.title, 32),
                ]
            })
            .collect();
        print_table(
            &["SESSION", "NODE", "AGENT", "STATUS", "UPDATED", "TITLE"],
            &rows,
        );
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg session <command>
// ----------------------------------------------------------------------

/// `fxg session <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct SessionArgs {
    #[usage(subcommand)]
    command: SessionCommands,
}

/// セッション管理サブコマンド。
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum SessionCommands {
    /// セッション一覧を表示する
    List(SessionListArgs),
}

// ----------------------------------------------------------------------
// fxg project <command>
// ----------------------------------------------------------------------

/// `fxg project <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct ProjectArgs {
    #[usage(subcommand)]
    command: ProjectCommands,
}

/// 論理プロジェクト管理サブコマンド。
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum ProjectCommands {
    /// カレントディレクトリの論理プロジェクト情報を表示する
    Info(ProjectInfoArgs),
    /// 登録済み論理プロジェクト一覧を表示する
    List(ProjectListArgs),
    /// カレントディレクトリを指定の論理プロジェクトIDへ紐付ける
    Link(ProjectLinkArgs),
    /// ディレクトリ配下のGitリポジトリを一括スキャンして登録する
    Scan(ProjectScanArgs),
}

/// `fxg project info`
#[derive(Debug, Args)]
pub struct ProjectInfoArgs {
    /// JSON 形式で出力する
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for ProjectInfoArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let project = client.project_info(&cwd).await?;

        if self.json {
            println!("{}", serde_json::to_string_pretty(&project)?);
            return Ok(());
        }

        println!("project_id:      {}", project.project_id);
        println!("name:            {}", project.name);
        println!(
            "canonical_git_url: {}",
            project.canonical_git_url.as_deref().unwrap_or("-")
        );
        println!(
            "git_root:        {}",
            project.git_root.as_deref().unwrap_or("-")
        );
        println!(
            "subpath:         {}",
            project.relative_subpath.as_deref().unwrap_or("-")
        );
        println!("local_path:      {}", project.local_path);
        println!("source:          {}", project.source);
        Ok(())
    }
}

/// `fxg project list`
#[derive(Debug, Args)]
pub struct ProjectListArgs {
    /// JSON 形式で出力する
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for ProjectListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let projects = client.list_projects().await?;

        if self.json {
            println!("{}", serde_json::to_string_pretty(&projects)?);
            return Ok(());
        }

        if projects.is_empty() {
            println!(
                "(プロジェクトなし。`fxg project scan` または `fxg project link` で登録できます)"
            );
            return Ok(());
        }

        for project in &projects {
            println!("{}  ({})", project.project_id, project.name);
            for binding in &project.bindings {
                let branch = binding.git_branch.as_deref().unwrap_or("-");
                let kind = if binding.is_worktree {
                    "worktree"
                } else {
                    "main"
                };
                println!(
                    "  - {} [{}] {} ({})",
                    binding.local_path, branch, kind, binding.node_id
                );
            }
        }
        Ok(())
    }
}

/// `fxg project link <project-id>`
#[derive(Debug, Args)]
pub struct ProjectLinkArgs {
    /// 紐付け先の論理プロジェクトID (例: github.com/nazo6/flexagent)
    project_id: String,
}

impl usage::RunAsync for ProjectLinkArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let message = client.link_project(&cwd, &self.project_id).await?;
        println!(
            "{}",
            message.unwrap_or_else(|| format!("linked to {}", self.project_id))
        );
        Ok(())
    }
}

/// `fxg project scan [dir]`
#[derive(Debug, Args)]
pub struct ProjectScanArgs {
    /// スキャン対象ディレクトリ (省略時は config.toml の project_scan_dirs)
    dir: Option<String>,
    /// JSON 形式で出力する
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for ProjectScanArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let (projects, scanned_dirs) = client.scan_projects(self.dir).await?;

        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "scanned_dirs": scanned_dirs,
                    "projects": projects,
                }))?
            );
            return Ok(());
        }

        println!("scanned: {}", scanned_dirs.join(", "));
        for project in &projects {
            println!(
                "  {}  ({} bindings)",
                project.project_id,
                project.bindings.len()
            );
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg worktree <command>
// ----------------------------------------------------------------------

/// `fxg worktree <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct WorktreeArgs {
    #[usage(subcommand)]
    command: WorktreeCommands,
}

/// Git Worktree 管理サブコマンド。
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum WorktreeCommands {
    /// Worktree 一覧を表示する
    List(WorktreeListArgs),
    /// 新規 Worktree を作成する
    Add(WorktreeAddArgs),
    /// Worktree を削除する
    Remove(WorktreeRemoveArgs),
    /// 削除済み Worktree の管理情報をクリーンアップする
    Prune(WorktreePruneArgs),
}

/// `fxg worktree list`
#[derive(Debug, Args)]
pub struct WorktreeListArgs {
    /// 論理プロジェクトIDで絞り込む
    #[usage(long)]
    project: Option<String>,
    /// JSON 形式で出力する
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for WorktreeListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let worktrees = client
            .list_worktrees(Some(cwd.to_string_lossy().into_owned()), self.project)
            .await?;

        if self.json {
            println!("{}", serde_json::to_string_pretty(&worktrees)?);
            return Ok(());
        }

        if worktrees.is_empty() {
            println!("(Worktree なし)");
            return Ok(());
        }

        let rows: Vec<Vec<String>> = worktrees
            .iter()
            .map(|worktree| {
                vec![
                    if worktree.is_main { "main" } else { "worktree" }.to_owned(),
                    worktree
                        .branch
                        .clone()
                        .unwrap_or_else(|| "(detached)".to_owned()),
                    worktree
                        .head_commit
                        .as_deref()
                        .map(|commit| commit.chars().take(8).collect())
                        .unwrap_or_else(|| "-".to_owned()),
                    worktree.path.clone(),
                ]
            })
            .collect();
        print_table(&["KIND", "BRANCH", "HEAD", "PATH"], &rows);
        Ok(())
    }
}

/// `fxg worktree add <branch>`
#[derive(Debug, Args)]
pub struct WorktreeAddArgs {
    /// 作成するブランチ名 (例: feat/auth)
    branch: String,
    /// 起点ブランチ (省略時は現在の HEAD)
    #[usage(long)]
    base: Option<String>,
    /// 配置先パスの明示指定 (省略時は worktree_dir_template から解決)
    #[usage(long)]
    path: Option<String>,
    /// 論理プロジェクトID (省略時はカレントディレクトリから解決)
    #[usage(long)]
    project: Option<String>,
}

impl usage::RunAsync for WorktreeAddArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let (path, branch, created, hook_logs) = client
            .add_worktree(&cwd, self.project, &self.branch, self.base, self.path)
            .await?;

        if created {
            println!("created worktree {branch} at {path}");
        } else {
            println!("reusing existing worktree {branch} at {path}");
        }
        for log in hook_logs {
            let status = if log.success { "ok" } else { "failed" };
            println!("[{status}] {}", log.command);
            if !log.output.is_empty() {
                for line in log.output.lines() {
                    println!("  {line}");
                }
            }
        }
        Ok(())
    }
}

/// `fxg worktree remove <target>`
#[derive(Debug, Args)]
pub struct WorktreeRemoveArgs {
    /// 削除対象のブランチ名またはパス
    target: String,
    /// 未コミット変更があっても強制削除する
    #[usage(short = 'f', long)]
    force: bool,
}

impl usage::RunAsync for WorktreeRemoveArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let message = client
            .remove_worktree(&cwd, &self.target, self.force)
            .await?;
        println!("{}", message.unwrap_or_else(|| "removed".to_owned()));
        Ok(())
    }
}

/// `fxg worktree prune`
#[derive(Debug, Args)]
pub struct WorktreePruneArgs {}

impl usage::RunAsync for WorktreePruneArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let message = client.prune_worktrees(&cwd).await?;
        println!("{}", message.unwrap_or_else(|| "pruned".to_owned()));
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg auth <command>
// ----------------------------------------------------------------------

/// `fxg auth <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct AuthArgs {
    #[usage(subcommand)]
    command: AuthCommands,
}

/// 認証トークン管理サブコマンド。
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum AuthCommands {
    /// クライアント認証トークン (auth_token) を表示する
    Token(AuthTokenArgs),
    /// クライアント認証トークンを再生成する
    RotateToken(AuthRotateArgs),
}

/// `fxg auth token`
#[derive(Debug, Args)]
pub struct AuthTokenArgs {}

impl usage::RunAsync for AuthTokenArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        println!("{}", client.auth_token(false).await?);
        Ok(())
    }
}

/// `fxg auth rotate-token`
#[derive(Debug, Args)]
pub struct AuthRotateArgs {}

impl usage::RunAsync for AuthRotateArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let token = client.auth_token(true).await?;
        println!("{token}");
        eprintln!("auth token を再生成しました (ブラウザ等の再接続時に再入力が必要です)");
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg kill-all
// ----------------------------------------------------------------------

/// `fxg kill-all` の引数。
#[derive(Debug, Args)]
pub struct KillAllArgs {
    /// 中央サーバーへ配信せずローカルノードのみ停止する
    /// (中央サーバーへの一括配信は Phase 4 で実装)
    #[usage(long)]
    local_only: bool,
}

impl usage::RunAsync for KillAllArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let (sessions, ptys) = client.kill_all().await?;
        if !self.local_only {
            // Phase 4 で中央サーバー経由の全ノード停止を追加する
            tracing::debug!("central server fan-out is implemented in phase 4");
        }
        println!(
            "killed {} session(s) and {} pty(s)",
            sessions.len(),
            ptys.len()
        );
        Ok(())
    }
}
