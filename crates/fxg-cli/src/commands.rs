//! サブコマンドの引数定義と実行処理。
//!
//! CLI の宣言 (`usage` の derive) が単一のソースであり、`__usage_spec__` から
//! KDL spec・シェル補完・manpage・Markdown リファレンスを生成できる
//! (設計: `docs/05-cli-and-pwa-ui.md` §1)。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fxg_acp::registry::AcpRegistry;
use fxg_protocol::common::{PermissionOption, SessionControlAction, SessionStatus};
use fxg_protocol::events::UnifiedEventPayload;
use fxg_protocol::ipc::{AttachMode, IpcResult, IpcServerMessage};
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
    /// 中央サーバーの Node Hub WebSocket URL (Outbox 同期。未指定時はスタンドアロン)
    #[usage(long)]
    server_url: Option<String>,
    /// リモート (中央サーバー経由) からの Web PTY 起動を許可する
    #[usage(long)]
    allow_remote_pty: bool,
    /// 標準入出力パイプ (JSON Lines) モード (一時VM。プロビジョナーが起動する)
    #[usage(long)]
    stdio: bool,
    /// 一時VMモード (Drain 時の Git バンドル退避を有効化)
    #[usage(long)]
    ephemeral: bool,
    /// `--stdio` 時の初期対象ディレクトリ (クローン済みワークスペース)
    #[usage(long)]
    workspace: Option<String>,
    /// タスクトレイに常駐する (状態表示 / 自動起動 / Web UI / 終了)
    #[usage(long)]
    tray: bool,
    /// `[node] tray` で有効化されたトレイ常駐を無効にする
    #[usage(long)]
    no_tray: bool,
}

impl DaemonArgs {
    /// 読み込み済みのグローバル設定からデーモン設定を組み立てる。
    pub(crate) fn daemon_config_from(
        &self,
        global: &fxg_protocol::config::GlobalConfig,
    ) -> fxg_node::daemon::DaemonConfig {
        let env = fxg_protocol::config::process_env;
        let mut config = fxg_node::daemon::DaemonConfig::from_env(&env);
        if let Some(node_id) = global.node.node_id.clone() {
            config.node_id = node_id;
        }
        if let Some(name) = global.node.name.clone() {
            config.node_name = name;
        }
        if let Some(listen) = self
            .listen
            .clone()
            .or_else(|| global.node.listen_addr.clone())
        {
            config.listen_addr = listen;
        }
        config.allow_remote_pty = self.allow_remote_pty || global.node.allow_remote_pty;
        // Outbox 同期 (中央サーバー接続。未指定時はスタンドアロン・ローカルのみ)
        config.central_server_url = self
            .server_url
            .clone()
            .or_else(|| global.node.central_server_url.clone());
        config.node_token = global.node.node_token.clone();
        config
    }

    /// トレイ常駐が要求されているか (`--stdio` / `--ephemeral` では常に無効)。
    pub(crate) fn tray_requested(&self, configured: Option<bool>) -> bool {
        if self.stdio || self.ephemeral || self.workspace.is_some() {
            return false;
        }
        crate::tray::resolve_enabled(self.tray, self.no_tray, configured)
    }
}

impl usage::RunAsync for DaemonArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let global = fxg_protocol::config::GlobalConfig::load(&env)
            .context("failed to load ~/.flexagent/config.toml")?;

        if self.stdio || self.ephemeral || self.workspace.is_some() {
            if !self.stdio {
                bail!("--ephemeral / --workspace は --stdio と併用してください");
            }
            let mut config = fxg_node::daemon::DaemonConfig::from_env(&env);
            if let Some(node_id) = global.node.node_id.clone() {
                config.node_id = node_id;
            }
            if let Some(name) = global.node.name.clone() {
                config.node_name = name;
            }
            config.allow_remote_pty = self.allow_remote_pty || global.node.allow_remote_pty;
            // stdio ではサーバーがパイプを所有するため、WS 同期設定は使用しない
            config.central_server_url = None;
            config.node_token = None;

            let stdio = fxg_node::daemon::StdioConfig {
                daemon: config,
                workspace: self.workspace.map(PathBuf::from),
                ephemeral: self.ephemeral,
            };
            return fxg_node::daemon::run_stdio(stdio)
                .await
                .context("fxg daemon --stdio failed");
        }

        let tray_enabled = self.tray_requested(global.node.tray);
        let config = self.daemon_config_from(&global);
        let daemon = fxg_node::daemon::NodeDaemon::start(config)
            .await
            .context("fxg daemon failed")?;

        // トレイ常駐 (macOS は main.rs の早期分岐でメインスレッドにて処理される)
        #[cfg(not(target_os = "macos"))]
        let tray = if tray_enabled {
            start_tray(&daemon)
        } else {
            None
        };
        #[cfg(target_os = "macos")]
        let _ = tray_enabled;

        daemon.wait().await;
        #[cfg(not(target_os = "macos"))]
        if let Some(tray) = tray {
            tray.close();
        }
        Ok(())
    }
}

/// トレイ常駐を開始する (失敗は警告のみでデーモンは継続する)。
#[cfg(not(target_os = "macos"))]
fn start_tray(daemon: &fxg_node::daemon::NodeDaemon) -> Option<crate::tray::TrayHandle> {
    let ctx = crate::tray::TrayContext {
        state: daemon.state().clone(),
        shutdown: daemon.shutdown_handle(),
    };
    match crate::tray::spawn(&tokio::runtime::Handle::current(), ctx) {
        Ok(tray) => {
            tracing::info!("tray icon started");
            Some(tray)
        }
        Err(err) => {
            tracing::warn!("failed to start the tray (continuing without it): {err:#}");
            None
        }
    }
}

// ----------------------------------------------------------------------
// fxg server
// ----------------------------------------------------------------------

/// `fxg server` の引数。
#[derive(Debug, Args)]
pub struct ServerArgs {
    /// HTTP/WS バインド先 (既定: 0.0.0.0:8080)
    #[usage(long)]
    listen: Option<String>,
    /// ポート番号上書き (`--listen` より優先)
    #[usage(long)]
    port: Option<u16>,
}

impl usage::RunAsync for ServerArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let global = fxg_protocol::config::GlobalConfig::load(&env)
            .context("failed to load ~/.flexagent/config.toml")?;

        let mut listen_addr = self
            .listen
            .clone()
            .unwrap_or_else(|| global.server.resolved_listen_addr().to_owned());
        if let Some(port) = self.port {
            let host = listen_addr
                .rsplit_once(':')
                .map(|(host, _)| host.to_owned())
                .unwrap_or_else(|| "0.0.0.0".to_owned());
            listen_addr = format!("{host}:{port}");
        }

        let options = fxg_server::ServerOptions {
            fxg_home: fxg_protocol::config::fxg_home(&env),
            listen_addr,
            allowed_hosts: global.server.allowed_hosts.clone(),
            allowed_origins: global.server.allowed_origins.clone(),
            provisioners: global.provisioners.clone(),
            git_credentials: global.server.git_credentials.clone(),
        };
        fxg_server::Server::run(options)
            .await
            .context("fxg server failed")
    }
}

// ----------------------------------------------------------------------
// fxg service
// ----------------------------------------------------------------------

/// `fxg service <action>` の引数。
#[derive(Debug, Args)]
pub struct ServiceArgs {
    /// 実行する操作: install | uninstall | start | stop | restart | status
    action: String,
    /// `daemon` ではなく中央サーバー (`server`) を対象にする
    #[usage(long)]
    server: bool,
}

impl usage::RunAsync for ServiceArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let action = crate::service::ServiceAction::parse(&self.action).with_context(|| {
            format!(
                "unknown action: {} (install | uninstall | start | stop | restart | status)",
                self.action
            )
        })?;
        crate::service::run(action, self.server).await
    }
}

// ----------------------------------------------------------------------
// fxg web
// ----------------------------------------------------------------------

/// `fxg web` の引数。
#[derive(Debug, Args)]
pub struct WebArgs {
    /// ローカルノードではなく中央サーバー URL を開く
    #[usage(long)]
    server: bool,
}

impl usage::RunAsync for WebArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let global = fxg_protocol::config::GlobalConfig::load(&env)
            .context("failed to load ~/.flexagent/config.toml")?;

        let url = if self.server {
            // 中央サーバーはローカルに認証トークンを持たないため、URL のみ開いて
            // 初回トークン入力ダイアログに委ねる (docs/05 §1.1)
            let server_url = global
                .node
                .central_server_url
                .clone()
                .context("node.central_server_url が config.toml に設定されていません")?;
            crate::server_api::central_http_url(&server_url)?
        } else {
            let token = fxg_server::api::auth::load_or_create_token(
                &fxg_protocol::config::fxg_home(&env)
                    .join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME),
            )
            .context("failed to load ~/.flexagent/auth_token")?;
            let listen = global
                .node
                .listen_addr
                .clone()
                .unwrap_or_else(|| "127.0.0.1:7860".to_owned());
            local_web_url(&listen, &token)
        };

        println!("{url}");
        opener::open_browser(&url).context("failed to open the default browser")?;
        Ok(())
    }
}

/// ローカル Web UI のトークン付き URL を組み立てる (`fxg web` / トレイで共用)。
pub(crate) fn local_web_url(listen_addr: &str, token: &str) -> String {
    format!("http://{listen_addr}/?token={token}")
}

// ----------------------------------------------------------------------
// fxg run / fxg attach
// ----------------------------------------------------------------------

/// `fxg run <agent>` の引数。
#[derive(Debug, Args)]
pub struct RunArgs {
    /// エージェントID (`opencode` / `antigravity` / `claude` 等)
    agent: String,
    /// 初期プロンプトを送信してからアタッチする
    #[usage(short = 'p', long)]
    prompt: Option<String>,
    /// TUI をアタッチせずバックグラウンド起動し、セッションIDを出力する
    #[usage(short = 'd', long)]
    detach: bool,
    /// 初期モード (`code` / `plan` 等)
    #[usage(long)]
    mode: Option<String>,
    /// Worktree を作成/再利用して起動する
    #[usage(short = 'w', long)]
    worktree: Option<String>,
    /// Worktree 新規作成時のベースブランチ
    #[usage(long)]
    base: Option<String>,
    /// 一時VMプロビジョナーで起動する (中央サーバーホスト上で起動。中央サーバー必須)
    #[usage(long)]
    provisioner: Option<String>,
    /// `opencode2` を標準ACPモード (`opencode2 acp`) で起動する
    #[usage(long)]
    acp: bool,
    /// エージェントプロセスへのパススルー引数
    #[usage(value_name = "EXTRA_ARGS", double_dash = "required")]
    extra_args: Vec<String>,
}

impl usage::RunAsync for RunArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;

        // 一時VMプロビジョナー: 中央サーバー API 経由で起動する
        // (プロビジョナーはサーバーホスト上で子プロセスとして起動されるため)
        if let Some(provisioner) = &self.provisioner {
            return run_with_provisioner(&self, provisioner, &cwd, &env).await;
        }

        let mut client = DaemonClient::connect().await?;
        let cwd = match &self.worktree {
            Some(branch) => create_worktree(&mut client, &cwd, branch, self.base.clone()).await?,
            None => cwd,
        };

        let (session_id, _) = client
            .ensure_session(
                &cwd,
                &self.agent,
                &self.extra_args,
                self.mode.as_deref(),
                self.acp,
            )
            .await?;

        if let Some(prompt) = &self.prompt {
            client.send_prompt(&session_id, prompt, "cli").await?;
        }

        if self.detach {
            println!("{session_id}");
            return Ok(());
        }
        crate::tui::attach(&mut client, &session_id).await
    }
}

/// `fxg run <agent> --provisioner <name>` (中央サーバー API 経由)。
async fn run_with_provisioner(
    args: &RunArgs,
    provisioner: &str,
    cwd: &Path,
    env: fxg_protocol::config::EnvLookup<'_>,
) -> Result<()> {
    let server = crate::server_api::ServerClient::from_config(env)?;

    // カレントディレクトリの論理プロジェクトを解決する (Git remote が正データ)
    let project = fxg_node::resolve_project(cwd, "cli")
        .await
        .context("failed to resolve logical project (Git リポジトリ内で実行してください)")?;

    // `-w/--worktree`: 一時VMでは新規 Worktree ではなくクローン先ブランチとして使う
    let worktree = match (&args.worktree, &args.base) {
        (Some(branch), base) => Some(fxg_protocol::client_api::WorktreeSpec {
            branch: branch.clone(),
            base_branch: base.clone(),
            new_path: None,
        }),
        (None, _) => None,
    };

    let request = fxg_protocol::client_api::CreateSessionRequest {
        command_id: format!("cli-{}", fxg_protocol::util::uuid_v7()),
        project_id: project.project_id.clone(),
        node_id: None,
        provisioner: Some(provisioner.to_owned()),
        local_path: None,
        worktree,
        agent_id: args.agent.clone(),
        initial_prompt: args.prompt.clone(),
        fork: None,
    };
    let response = server.create_session(&request).await?;
    eprintln!(
        "provisioning session {} via '{provisioner}' (bootstrap ログは Web UI / `fxg provisioners test` で確認できます)",
        response.session_id
    );
    println!("{}", response.session_id);
    Ok(())
}

// ----------------------------------------------------------------------
// fxg provisioners list / test
// ----------------------------------------------------------------------

/// `fxg provisioners <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct ProvisionersArgs {
    #[usage(subcommand)]
    command: ProvisionersCommands,
}

/// プロビジョナー管理サブコマンド。
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum ProvisionersCommands {
    /// 設定ファイルに定義された一時VMプロビジョナー一覧を表示する
    List(ProvisionersListArgs),
    /// プロビジョナーの起動・`fxg daemon --stdio` ハンドシェイク疎通を検証する
    Test(ProvisionersTestArgs),
}

/// `fxg provisioners list`
#[derive(Debug, Args)]
pub struct ProvisionersListArgs {
    /// JSON 形式で出力する
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for ProvisionersListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let global = fxg_protocol::config::GlobalConfig::load(&env)
            .context("failed to load ~/.flexagent/config.toml")?;
        let provisioners: Vec<_> = global
            .provisioners
            .iter()
            .map(|(name, config)| {
                serde_json::json!({
                    "name": name,
                    "description": config.description,
                    "command": config.command,
                    "args": config.args,
                    "idle_timeout_secs": config.idle_timeout_secs,
                })
            })
            .collect();
        if self.json {
            println!("{}", serde_json::to_string_pretty(&provisioners)?);
            return Ok(());
        }
        if provisioners.is_empty() {
            println!(
                "(プロビジョナー未定義。~/.flexagent/config.toml の [provisioners.<name>] で定義します)"
            );
            return Ok(());
        }
        let rows: Vec<Vec<String>> = provisioners
            .iter()
            .map(|entry| {
                vec![
                    entry["name"].as_str().unwrap_or_default().to_owned(),
                    entry["description"].as_str().unwrap_or_default().to_owned(),
                    entry["command"].as_str().unwrap_or_default().to_owned(),
                    entry["idle_timeout_secs"]
                        .as_u64()
                        .map(|secs| format!("{secs}s"))
                        .unwrap_or_else(|| "-".to_owned()),
                ]
            })
            .collect();
        print_table(&["NAME", "DESCRIPTION", "COMMAND", "IDLE"], &rows);
        Ok(())
    }
}

/// `fxg provisioners test <name>`
#[derive(Debug, Args)]
pub struct ProvisionersTestArgs {
    /// 検証するプロビジョナー名
    name: String,
}

impl usage::RunAsync for ProvisionersTestArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let server = crate::server_api::ServerClient::from_config(&env)?;
        eprintln!(
            "testing provisioner '{}' via {} (bootstrap の完了まで数分かかる場合があります)",
            self.name,
            server.base_url()
        );
        let response = server.provisioner_test(&self.name).await?;
        for line in &response.log_lines {
            eprintln!("  {line}");
        }
        if response.ok {
            println!(
                "ok: {} (node {})",
                response.name,
                response.node_id.as_deref().unwrap_or("-")
            );
            return Ok(());
        }
        bail!(
            "provisioner '{}' failed: {}",
            response.name,
            response.error.as_deref().unwrap_or("unknown error")
        );
    }
}

// ----------------------------------------------------------------------
// fxg bootstrap-workspace
// ----------------------------------------------------------------------

/// `fxg bootstrap-workspace` の引数 (一時VM内でプロビジョナーが実行する)。
#[derive(Debug, Args)]
pub struct BootstrapWorkspaceArgs {
    /// クローン元 Git URL
    #[usage(long)]
    repo: String,
    /// 対象ブランチ (省略時はリモートの既定ブランチ)
    #[usage(long)]
    branch: Option<String>,
    /// クローン先ディレクトリ (既定: /tmp/workspace)
    #[usage(long)]
    dir: Option<String>,
}

impl usage::RunAsync for BootstrapWorkspaceArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let dir = self
            .dir
            .clone()
            .unwrap_or_else(|| "/tmp/workspace".to_owned());
        let options = fxg_node::bootstrap::BootstrapOptions {
            repo: self.repo.clone(),
            branch: self.branch.clone(),
            dir: PathBuf::from(&dir),
            fxg_home: fxg_protocol::config::fxg_home(&env),
            install_toolchain: true,
            extra_tools: Vec::new(),
        };
        let outcome = fxg_node::bootstrap::bootstrap_workspace(options)
            .await
            .context("fxg bootstrap-workspace failed")?;
        // stdout はプロトコル専用のため、結果も stderr に出力する
        eprintln!(
            "[fxg-bootstrap] ready: {} (tools: {})",
            outcome.dir.display(),
            outcome.tools.join(", ")
        );
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg git-askpass (GIT_ASKPASS ヘルパー)
// ----------------------------------------------------------------------

/// `fxg git-askpass` の引数 (`GIT_ASKPASS` として Git が呼び出す内部用コマンド)。
#[derive(Debug, Args)]
pub struct GitAskpassArgs {
    /// Git が渡すプロンプト文字列 (`Username for '...'` / `Password for '...'`)
    prompt: String,
}

impl usage::RunAsync for GitAskpassArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let (username, token, error) = client.git_credential(&self.prompt).await?;
        if let Some(error) = error {
            bail!("{error}");
        }
        let value = if fxg_node::credentials::prompt_wants_username(&self.prompt) {
            username
        } else {
            token.unwrap_or_default()
        };
        println!("{value}");
        Ok(())
    }
}

/// `fxg attach [session-id]` の引数。
#[derive(Debug, Args)]
pub struct AttachArgs {
    /// セッションID (省略時はカレントディレクトリの直近アクティブセッション)
    session_id: Option<String>,
}

impl usage::RunAsync for AttachArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let session_id = match self.session_id {
            Some(session_id) => session_id,
            None => latest_session_for_cwd(&mut client).await?,
        };
        crate::tui::attach(&mut client, &session_id).await
    }
}

/// Worktree を作成/再利用し、そのパスを返す (`fxg run -w` / `fxg session fork -w`)。
async fn create_worktree(
    client: &mut DaemonClient,
    cwd: &Path,
    branch: &str,
    base_branch: Option<String>,
) -> Result<PathBuf> {
    let (path, branch, created, hook_logs) = client
        .add_worktree(cwd, None, branch, base_branch, None)
        .await?;
    if created {
        eprintln!("created worktree {branch} at {path}");
    } else {
        eprintln!("reusing existing worktree {branch} at {path}");
    }
    for log in hook_logs {
        let status = if log.success { "ok" } else { "failed" };
        eprintln!("[{status}] {}", log.command);
        for line in log.output.lines() {
            eprintln!("  {line}");
        }
    }
    Ok(PathBuf::from(path))
}

/// カレントディレクトリの直近アクティブセッションを解決する
/// (`fxg attach` の ID 省略時)。
async fn latest_session_for_cwd(client: &mut DaemonClient) -> Result<String> {
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    let sessions = client.list_sessions(false).await?;
    sessions
        .into_iter()
        .filter(|session| session.status != SessionStatus::Stopped)
        .find(|session| same_directory(Path::new(&session.local_path), &cwd))
        .map(|session| session.session_id)
        .with_context(|| {
            format!(
                "{} の直近アクティブセッションが見つかりません (`fxg ps` で確認してください)",
                cwd.display()
            )
        })
}

/// 2 つのディレクトリが同一かを判定する (表記ゆれは canonicalize で吸収)。
fn same_directory(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fxg_pty::canonicalize(a), fxg_pty::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
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
    /// セッション詳細・状態・イベント統計を表示する
    Show(SessionShowArgs),
    /// 既存セッションへ非対話 (ヘッドレス) でプロンプトを送信する
    Prompt(SessionPromptArgs),
    /// 実行中ターンのキャンセル、またはセッションの正常停止
    Stop(SessionStopArgs),
    /// セッションの子プロセスツリーを強制終了する
    Kill(SessionKillArgs),
    /// 指定ターン時点へファイルと会話を巻き戻す
    Revert(SessionRevertArgs),
    /// 指定時点から会話を分岐して新規セッションを作成する
    Fork(SessionForkArgs),
}

/// `fxg session show <id>`
#[derive(Debug, Args)]
pub struct SessionShowArgs {
    /// セッションID
    session_id: String,
    /// 表示する直近イベント数
    #[usage(long, default = "20")]
    events: u32,
    /// JSON 形式で出力する
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for SessionShowArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let detail = client.session_show(&self.session_id, self.events).await?;
        if self.json {
            let value = serde_json::json!({
                "session": detail.session,
                "event_count": detail.event_count,
                "recent_events": detail.recent_events,
                "pending_permissions": detail.pending_permissions,
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }

        let session = &detail.session;
        let fields = [
            ("SESSION", session.session_id.clone()),
            ("TITLE", session.title.clone()),
            ("PROJECT", session.project_id.clone()),
            ("NODE", session.node_id.clone()),
            ("AGENT", session.agent_id.clone()),
            ("STATUS", session.status.to_string()),
            (
                "MODE",
                session
                    .current_mode
                    .clone()
                    .unwrap_or_else(|| "-".to_owned()),
            ),
            ("PATH", session.local_path.clone()),
            (
                "BRANCH",
                session.git_branch.clone().unwrap_or_else(|| "-".to_owned()),
            ),
            (
                "WORKTREE",
                if session.is_worktree { "yes" } else { "no" }.to_owned(),
            ),
            ("EVENTS", detail.event_count.to_string()),
            ("CREATED", format_unix_ms_utc(session.created_at)),
            ("UPDATED", format_unix_ms_utc(session.updated_at)),
        ];
        for (label, value) in fields {
            println!("{label:<9}{value}");
        }

        if !detail.pending_permissions.is_empty() {
            println!("\n承認待ち:");
            let rows: Vec<Vec<String>> = detail
                .pending_permissions
                .iter()
                .map(|entry| {
                    vec![
                        truncate(&entry.request_id, 24),
                        truncate(&entry.tool_name, 20),
                        truncate(&entry.summary, 48),
                    ]
                })
                .collect();
            print_table(&["REQUEST", "TOOL", "SUMMARY"], &rows);
        }

        if !detail.recent_events.is_empty() {
            println!("\n直近イベント:");
            for event in &detail.recent_events {
                println!(
                    "  {:>5}  {}  {}",
                    event.node_seq,
                    format_unix_ms_utc(event.created_at),
                    describe_event(&event.payload)
                );
            }
        }
        Ok(())
    }
}

/// イベントを CLI 表示用の 1 行へ要約する。
fn describe_event(payload: &UnifiedEventPayload) -> String {
    match payload {
        UnifiedEventPayload::SessionCreated {
            title, agent_id, ..
        } => format!("session_created agent={agent_id} title={title}"),
        UnifiedEventPayload::SessionTitleChanged { title } => {
            format!("title_changed {}", truncate(title, 40))
        }
        UnifiedEventPayload::SessionAgentBound { agent_session_id } => {
            format!("agent_bound {agent_session_id}")
        }
        UnifiedEventPayload::UserMessage {
            text,
            client_source,
            ..
        } => format!("user_message ({client_source}) {}", truncate(text, 60)),
        UnifiedEventPayload::AgentMessage { text, .. } => {
            format!("agent_message {}", truncate(text, 60))
        }
        UnifiedEventPayload::AgentThought { text, .. } => {
            format!("agent_thought {}", truncate(text, 60))
        }
        UnifiedEventPayload::ToolCall { title, status, .. } => {
            format!("tool_call [{status}] {}", truncate(title, 60))
        }
        UnifiedEventPayload::PlanUpdate { entries } => {
            format!("plan_update entries={}", entries.len())
        }
        UnifiedEventPayload::PermissionRequest {
            tool_name, summary, ..
        } => format!("permission_request [{tool_name}] {}", truncate(summary, 60)),
        UnifiedEventPayload::PermissionResolved {
            request_id,
            selected_option_id,
            ..
        } => format!("permission_resolved {request_id} → {selected_option_id}"),
        UnifiedEventPayload::SessionReverted {
            target_node_seq,
            restored_files,
            removed_files,
            ..
        } => format!(
            "session_reverted seq={target_node_seq} restored={restored_files} removed={removed_files}"
        ),
        UnifiedEventPayload::TerminalOutput {
            terminal_id,
            command,
            ..
        } => format!("terminal_output {terminal_id} {}", truncate(command, 40)),
        UnifiedEventPayload::TerminalInput { terminal_id, .. } => {
            format!("terminal_input {terminal_id}")
        }
        UnifiedEventPayload::CapabilitiesUpdated { .. } => "capabilities_updated".to_owned(),
        UnifiedEventPayload::StatusChanged {
            status,
            error_message,
        } => match error_message {
            Some(message) => format!("status_changed {status} ({message})"),
            None => format!("status_changed {status}"),
        },
        UnifiedEventPayload::BootstrapLog { line } => {
            format!("bootstrap {}", truncate(line, 60))
        }
    }
}

/// `fxg session prompt <id> <text>`
#[derive(Debug, Args)]
pub struct SessionPromptArgs {
    /// セッションID
    session_id: String,
    /// 送信するプロンプト本文
    text: String,
    /// ターン完了まで待機し、エージェントの出力を表示する
    #[usage(long)]
    wait: bool,
}

impl usage::RunAsync for SessionPromptArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        client
            .send_prompt(&self.session_id, &self.text, "cli")
            .await?;
        if !self.wait {
            println!("accepted ({})", short_id(&self.session_id));
            return Ok(());
        }
        wait_for_turn(&mut client, &self.session_id, &self.text).await
    }
}

/// ターン完了 (`idle`) までイベントを購読して出力する
/// (`fxg session prompt --wait`)。
async fn wait_for_turn(client: &mut DaemonClient, session_id: &str, prompt: &str) -> Result<()> {
    let attach_mode = client.attach_session(session_id, None).await?;
    if !matches!(attach_mode, AttachMode::AcpTui) {
        bail!("--wait は内蔵TUI (AcpTui) モードのセッションのみ対応しています");
    }

    let mut seen_prompt = false;
    loop {
        let Some(message) = client.next_message().await? else {
            break;
        };
        match message {
            IpcServerMessage::EventBatch { events, .. } => {
                for event in events {
                    if print_and_check_done(&event.payload, prompt, &mut seen_prompt) {
                        return Ok(());
                    }
                }
            }
            IpcServerMessage::LiveStreamDelta { .. } => {}
            IpcServerMessage::Result { result, .. } => match result {
                IpcResult::Ack { .. } | IpcResult::CommandAccepted { .. } => {}
                other => tracing::debug!("ignoring result while waiting: {other:?}"),
            },
            IpcServerMessage::Error { code, message, .. } => {
                bail!("{code}: {message}");
            }
        }
    }
    Ok(())
}

/// イベントを出力し、ターン完了なら `true` を返す。
fn print_and_check_done(
    payload: &UnifiedEventPayload,
    prompt: &str,
    seen_prompt: &mut bool,
) -> bool {
    match payload {
        UnifiedEventPayload::UserMessage { text, .. } if text == prompt => {
            *seen_prompt = true;
        }
        UnifiedEventPayload::AgentMessage { text, .. } if !text.is_empty() => {
            println!("{text}");
        }
        UnifiedEventPayload::AgentThought { text, .. } if !text.is_empty() => {
            println!("Think ▸ {text}");
        }
        UnifiedEventPayload::ToolCall { title, status, .. } => {
            println!("· tool [{status}] {title}");
        }
        UnifiedEventPayload::PermissionRequest {
            tool_name, summary, ..
        } => {
            println!("! 承認待ち [{tool_name}] {summary} (`fxg inbox list` で確認) ");
        }
        UnifiedEventPayload::StatusChanged {
            status,
            error_message,
        } => {
            if let Some(message) = error_message {
                println!("! error: {message}");
            }
            if *seen_prompt {
                return matches!(
                    status,
                    SessionStatus::Idle | SessionStatus::Error | SessionStatus::Stopped
                );
            }
        }
        _ => {}
    }
    false
}

/// `fxg session stop <id>`
#[derive(Debug, Args)]
pub struct SessionStopArgs {
    /// セッションID
    session_id: String,
    /// セッションは維持し、実行中のターンのみ中断する
    #[usage(long)]
    turn_only: bool,
}

impl usage::RunAsync for SessionStopArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        if self.turn_only {
            client
                .control_session(&self.session_id, SessionControlAction::Cancel)
                .await?;
            println!("cancelled current turn ({})", short_id(&self.session_id));
        } else {
            client
                .control_session(&self.session_id, SessionControlAction::Kill)
                .await?;
            println!("stopped session ({})", short_id(&self.session_id));
        }
        Ok(())
    }
}

/// `fxg session kill <id>`
#[derive(Debug, Args)]
pub struct SessionKillArgs {
    /// セッションID
    session_id: String,
}

impl usage::RunAsync for SessionKillArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        client
            .control_session(&self.session_id, SessionControlAction::Kill)
            .await?;
        println!(
            "killed session process tree ({})",
            short_id(&self.session_id)
        );
        Ok(())
    }
}

/// `fxg session revert <id> --to-seq <NODE_SEQ>`
#[derive(Debug, Args)]
pub struct SessionRevertArgs {
    /// セッションID
    session_id: String,
    /// 巻き戻し先のイベント連番 (対象ターンの `UserMessage.node_seq`)
    #[usage(long)]
    to_seq: u64,
}

impl usage::RunAsync for SessionRevertArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let outcome = client
            .revert_session(&self.session_id, Some(self.to_seq))
            .await?;
        println!(
            "reverted to node_seq={} (restored={}, removed={})",
            outcome.target_node_seq, outcome.restored_files, outcome.removed_files
        );
        println!("  tree:   {}", outcome.restored_tree_hash);
        if let Some(backup) = &outcome.backup_tree_hash {
            println!("  backup: {backup}");
        }
        Ok(())
    }
}

/// `fxg session fork <id>`
#[derive(Debug, Args)]
pub struct SessionForkArgs {
    /// 分岐元のセッションID
    session_id: String,
    /// 分岐元の連番 (省略時は最新)
    #[usage(long)]
    from_seq: Option<u64>,
    /// 分岐後のエージェントID (省略時は同一エージェント)
    #[usage(long)]
    agent: Option<String>,
    /// 新規 Worktree へ分岐する
    #[usage(short = 'w', long)]
    worktree: Option<String>,
    /// Worktree 新規作成時のベースブランチ
    #[usage(long)]
    base: Option<String>,
    /// 一時VMプロビジョナーで分岐する (中央サーバー必須。退避済み Git バンドルを復元)
    #[usage(long)]
    provisioner: Option<String>,
}

impl usage::RunAsync for SessionForkArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        // 一時VM上での分岐: 中央サーバー API 経由で Context Fork を要求する
        if let Some(provisioner) = &self.provisioner {
            let env = fxg_protocol::config::process_env;
            let server = crate::server_api::ServerClient::from_config(&env)?;
            let sessions = server.sessions().await?;
            let source = sessions
                .iter()
                .find(|session| session.session_id == self.session_id)
                .with_context(|| format!("セッションが見つかりません: {}", self.session_id))?;
            let request = fxg_protocol::client_api::CreateSessionRequest {
                command_id: format!("cli-{}", fxg_protocol::util::uuid_v7()),
                project_id: source.project_id.clone(),
                node_id: None,
                provisioner: Some(provisioner.clone()),
                local_path: None,
                worktree: None,
                agent_id: self
                    .agent
                    .clone()
                    .unwrap_or_else(|| source.agent_id.clone()),
                initial_prompt: None,
                fork: Some(fxg_protocol::client_api::SessionForkSpec {
                    from_session_id: self.session_id.clone(),
                    from_node_seq: self.from_seq,
                    // 中央サーバーが退避済みバンドル (git_bundle_path) を自動で読み込む
                    restore_git_bundle_b64: None,
                }),
            };
            let response = server.create_session(&request).await?;
            println!("{}", response.session_id);
            return Ok(());
        }

        let mut client = DaemonClient::connect().await?;
        let cwd = match &self.worktree {
            Some(branch) => {
                let dir = std::env::current_dir().context("failed to resolve current directory")?;
                Some(create_worktree(&mut client, &dir, branch, self.base.clone()).await?)
            }
            None => None,
        };

        let (session_id, _) = client
            .fork_session(&self.session_id, self.from_seq, self.agent, cwd.as_deref())
            .await?;
        println!("forked: {session_id}");
        crate::tui::attach(&mut client, &session_id).await
    }
}

// ----------------------------------------------------------------------
// fxg inbox <command>
// ----------------------------------------------------------------------

/// `fxg inbox <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct InboxArgs {
    #[usage(subcommand)]
    command: InboxCommands,
}

/// 承認待ちリクエスト (グローバル Inbox) の操作。
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum InboxCommands {
    /// 現在承認待ちのリクエスト一覧を表示する
    List(InboxListArgs),
    /// 承認リクエストを許可する
    Approve(InboxApproveArgs),
    /// 承認リクエストを却下する
    Reject(InboxRejectArgs),
}

/// `fxg inbox list`
#[derive(Debug, Args)]
pub struct InboxListArgs {
    /// JSON 形式で出力する
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for InboxListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let requests = client.inbox_list().await?;
        if self.json {
            println!("{}", serde_json::to_string_pretty(&requests)?);
            return Ok(());
        }
        if requests.is_empty() {
            println!("(承認待ちリクエストなし)");
            return Ok(());
        }
        let rows: Vec<Vec<String>> = requests
            .iter()
            .map(|entry| {
                vec![
                    truncate(&entry.request_id, 24),
                    short_id(&entry.session_id),
                    truncate(&entry.tool_name, 20),
                    truncate(&entry.summary, 40),
                    format_unix_ms_utc(entry.created_at),
                ]
            })
            .collect();
        print_table(&["REQUEST", "SESSION", "TOOL", "SUMMARY", "CREATED"], &rows);
        Ok(())
    }
}

/// `fxg inbox approve <req-id>`
#[derive(Debug, Args)]
pub struct InboxApproveArgs {
    /// 承認リクエストID
    request_id: String,
    /// 同一ツール操作をセッション中常時許可する (`allow_always`)
    #[usage(long)]
    always: bool,
}

impl usage::RunAsync for InboxApproveArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let entry = find_permission(&mut client, &self.request_id).await?;
        let preferred = if self.always {
            "allow_always"
        } else {
            "allow_once"
        };
        let option_id = option_by_kind(&entry.options, preferred)
            // 該当種別が無いドライバ向けフォールバック (却下系以外の先頭)
            .or_else(|| {
                entry
                    .options
                    .iter()
                    .find(|option| !PermissionOption::is_reject_kind(&option.kind))
                    .map(|option| option.option_id.clone())
            })
            .with_context(|| format!("許可オプションがありません: {}", self.request_id))?;
        client
            .respond_permission(&entry.session_id, &entry.request_id, &option_id)
            .await?;
        println!("approved {} ({option_id})", short_id(&entry.session_id));
        Ok(())
    }
}

/// `fxg inbox reject <req-id>`
#[derive(Debug, Args)]
pub struct InboxRejectArgs {
    /// 承認リクエストID
    request_id: String,
}

impl usage::RunAsync for InboxRejectArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let entry = find_permission(&mut client, &self.request_id).await?;
        let option_id = option_by_kind(&entry.options, "reject")
            .with_context(|| format!("却下オプションがありません: {}", self.request_id))?;
        client
            .respond_permission(&entry.session_id, &entry.request_id, &option_id)
            .await?;
        println!("rejected {} ({option_id})", short_id(&entry.session_id));
        Ok(())
    }
}

/// 承認待ちリクエストを `request_id` で引く。
async fn find_permission(
    client: &mut DaemonClient,
    request_id: &str,
) -> Result<fxg_protocol::common::PermissionRequestEntry> {
    client
        .inbox_list()
        .await?
        .into_iter()
        .find(|entry| entry.request_id == request_id)
        .with_context(|| format!("承認待ちリクエストが見つかりません: {request_id}"))
}

/// 種別の先頭一致で `option_id` を選ぶ。
fn option_by_kind(options: &[PermissionOption], kind: &str) -> Option<String> {
    options
        .iter()
        .find(|option| option.kind.starts_with(kind))
        .map(|option| option.option_id.clone())
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
// fxg agents <command>
// ----------------------------------------------------------------------

/// `fxg agents <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct AgentsArgs {
    #[usage(subcommand)]
    command: AgentsCommands,
}

/// ACP Registry エージェントの管理。
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum AgentsCommands {
    /// 利用可能なエージェント一覧とインストール状態を表示する
    List(AgentsListArgs),
    /// ACP Registry から指定エージェントを事前ダウンロード・展開する
    Install(AgentsInstallArgs),
    /// レジストリインデックスおよび導入済みエージェントを更新する
    Update(AgentsUpdateArgs),
    /// キャッシュ済みのエージェントバイナリを削除する
    Remove(AgentsRemoveArgs),
}

/// `~/.flexagent/config.toml` から ACP Registry を構成する。
fn acp_registry() -> Result<AcpRegistry> {
    let env = fxg_protocol::config::process_env;
    let global = fxg_protocol::config::GlobalConfig::load(&env)
        .context("failed to load ~/.flexagent/config.toml")?;
    let paths = fxg_node::NodePaths::from_env(&env);
    Ok(AcpRegistry::new(
        paths.fxg_home().to_path_buf(),
        &global.agents,
    ))
}

/// `fxg agents list`
#[derive(Debug, Args)]
pub struct AgentsListArgs {
    /// ACP Registry 上の未導入エージェントも表示する
    #[usage(long)]
    all: bool,
    /// JSON 形式で出力する
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for AgentsListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let registry = acp_registry()?;
        let index = registry
            .index(false)
            .await
            .context("failed to load ACP registry")?;
        let entries = registry.list(&index, self.all);
        if self.json {
            let value: Vec<serde_json::Value> = entries
                .iter()
                .map(|entry| {
                    serde_json::json!({
                        "id": entry.id,
                        "name": entry.name,
                        "version": entry.version,
                        "description": entry.description,
                        "installed": entry.installed,
                        "distributions": entry.distributions,
                        "custom": entry.custom,
                        "builtin": entry.builtin,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }
        if entries.is_empty() {
            println!("(導入済みエージェントなし。`fxg agents list --all` でレジストリ全体を表示)");
            return Ok(());
        }
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|entry| {
                vec![
                    entry.id.clone(),
                    truncate(&entry.name, 24),
                    entry.version.clone(),
                    if entry.installed { "yes" } else { "-" }.to_owned(),
                    entry.distributions.join(","),
                ]
            })
            .collect();
        print_table(&["ID", "NAME", "VERSION", "INSTALLED", "DIST"], &rows);
        Ok(())
    }
}

/// `fxg agents install <id>`
#[derive(Debug, Args)]
pub struct AgentsInstallArgs {
    /// エージェントID (エイリアス可)
    id: String,
    /// 導入するバージョン (レジストリ提供バージョンのみ)
    #[usage(long)]
    version: Option<String>,
}

impl usage::RunAsync for AgentsInstallArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let registry = acp_registry()?;
        let id = registry.resolve_alias(&self.id);
        let index = registry
            .index(false)
            .await
            .context("failed to load ACP registry")?;
        let agent = index.find(&id).with_context(|| {
            format!("エージェントがレジストリにありません: {id} (`fxg agents list --all`)")
        })?;
        if let Some(version) = &self.version
            && version != &agent.version
        {
            bail!(
                "レジストリが提供するのは {} のみです (指定: {version})",
                agent.version
            );
        }
        if agent.distribution.binary.is_empty() {
            println!("{id} は npx/uvx 配布のため個別インストールは不要です (実行時に自動取得)");
            return Ok(());
        }
        let path = registry
            .install(&id, &index)
            .await
            .context("failed to install agent")?;
        println!("installed {id} {} → {}", agent.version, path.display());
        Ok(())
    }
}

/// `fxg agents update [id]`
#[derive(Debug, Args)]
pub struct AgentsUpdateArgs {
    /// 更新対象のエージェントID (省略時は導入済み全エージェント)
    id: Option<String>,
}

impl usage::RunAsync for AgentsUpdateArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let registry = acp_registry()?;
        // インデックスを強制再取得する
        let index = registry
            .index(true)
            .await
            .context("failed to fetch ACP registry")?;

        let ids: Vec<String> = match &self.id {
            Some(id) => vec![registry.resolve_alias(id)],
            None => index
                .agents
                .iter()
                .filter(|agent| registry.is_installed(agent))
                .map(|agent| agent.id.clone())
                .collect(),
        };
        if ids.is_empty() {
            println!("(導入済みエージェントなし)");
            return Ok(());
        }

        for id in ids {
            let Some(agent) = index.find(&id) else {
                eprintln!("{id}: レジストリから削除されています (スキップ)");
                continue;
            };
            if agent.distribution.binary.is_empty() {
                println!("{id}: npx/uvx 配布のため更新不要");
                continue;
            }
            if registry
                .installed_versions(&id)
                .iter()
                .any(|version| version == &agent.version)
            {
                println!("{id}: 最新 ({})", agent.version);
                continue;
            }
            let path = registry
                .install(&id, &index)
                .await
                .with_context(|| format!("failed to update agent: {id}"))?;
            println!("updated {id} → {} ({})", agent.version, path.display());
        }
        Ok(())
    }
}

/// `fxg agents remove <id>`
#[derive(Debug, Args)]
pub struct AgentsRemoveArgs {
    /// エージェントID (エイリアス可)
    id: String,
}

impl usage::RunAsync for AgentsRemoveArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let registry = acp_registry()?;
        let id = registry.resolve_alias(&self.id);
        let removed = registry
            .remove(&id)
            .with_context(|| format!("failed to remove agent: {id}"))?;
        if removed {
            println!("removed {id}");
        } else {
            println!("{id} は導入されていません");
        }
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
    /// ノード個別トークン (Node ⇔ Server ペアリング) を管理する
    #[usage(name = "node-token")]
    NodeToken(NodeTokenArgs),
}

/// `fxg auth node-token` のサブコマンド。
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct NodeTokenArgs {
    #[usage(subcommand)]
    command: NodeTokenCommands,
}

/// ノード個別トークン操作。
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum NodeTokenCommands {
    /// ノード個別トークンを発行して表示する (中央サーバー上で実行)
    Issue(NodeTokenIssueArgs),
    /// ノード個別トークンを失効させる
    Revoke(NodeTokenRevokeArgs),
    /// 発行済みノードトークン一覧を表示する
    List(NodeTokenListArgs),
}

/// `fxg auth node-token issue <node-id>`
#[derive(Debug, Args)]
pub struct NodeTokenIssueArgs {
    /// 対象ノードID (fxg daemon の node_id)
    node_id: String,
}

impl usage::RunAsync for NodeTokenIssueArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let db = open_hub_db().await?;
        // 未登録のノードはプレースホルダで登録する
        // (ノード接続時の `NodeHello` で実情報に上書きされる)
        let nodes = db.list_nodes().await?;
        if !nodes.iter().any(|node| node.node_id == self.node_id) {
            db.upsert_node(&fxg_db::NodeRecord::new(
                self.node_id.clone(),
                self.node_id.clone(),
                "unknown",
                "unknown",
                "unknown",
            ))
            .await?;
        }
        let token = fxg_server::api::auth::generate_token();
        db.set_node_token_hash(
            &self.node_id,
            Some(&fxg_server::api::auth::hash_token(&token)),
        )
        .await?;
        // 平文トークンは発行時に一度だけ表示する (サーバーにはハッシュのみ保存)
        println!("{token}");
        eprintln!(
            "node_token を発行しました。ノード側の ~/.flexagent/node_token に保存してください。"
        );
        Ok(())
    }
}

/// `fxg auth node-token revoke <node-id>`
#[derive(Debug, Args)]
pub struct NodeTokenRevokeArgs {
    /// 対象ノードID
    node_id: String,
}

impl usage::RunAsync for NodeTokenRevokeArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let db = open_hub_db().await?;
        db.set_node_token_hash(&self.node_id, None).await?;
        println!("revoked node token for {}", self.node_id);
        Ok(())
    }
}

/// `fxg auth node-token list`
#[derive(Debug, Args)]
pub struct NodeTokenListArgs {}

impl usage::RunAsync for NodeTokenListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let db = open_hub_db().await?;
        let tokens = db.list_node_tokens().await?;
        if tokens.is_empty() {
            println!("(no node tokens issued)");
            return Ok(());
        }
        for (node_id, hash) in tokens {
            let prefix: String = hash.chars().take(12).collect();
            println!("{node_id}\t{prefix}…");
        }
        Ok(())
    }
}

/// 中央サーバーの DB (`~/.flexagent/server.db`) を開く。
async fn open_hub_db() -> Result<fxg_db::Db> {
    let env = fxg_protocol::config::process_env;
    let home = fxg_protocol::config::fxg_home(&env);
    let db = fxg_db::Db::open(&fxg_db::hub_db_path(&home), fxg_db::DbRole::Hub).await?;
    Ok(db)
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
    /// (全ノード一括停止は中央サーバーの `POST /api/v1/system/kill-switch`
    ///  から配信される)
    #[usage(long)]
    local_only: bool,
}

impl usage::RunAsync for KillAllArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let (sessions, ptys) = client.kill_all().await?;
        if !self.local_only {
            tracing::debug!(
                "all-node fan-out is issued by the central server via POST /api/v1/system/kill-switch"
            );
        }
        println!(
            "killed {} session(s) and {} pty(s)",
            sessions.len(),
            ptys.len()
        );
        Ok(())
    }
}
