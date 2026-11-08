//! Subcommand argument definitions and their execution logic.
//!
//! The CLI declaration (`usage` derives) is the single source of truth, and the
//! KDL spec, shell completions, manpage and Markdown reference are generated
//! from `__usage_spec__` (design: `docs/05-cli-and-pwa-ui.md` §1).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fxg_acp::registry::{AcpRegistry, AgentListEntry, LocalSource};
use fxg_protocol::common::{
    ElicitationAction, ElicitationRequestEntry, PermissionOption, SessionControlAction,
    SessionStatus,
};
use fxg_protocol::events::UnifiedEventPayload;
use usage::Args;

use crate::client::{DaemonClient, format_unix_ms_utc, print_table, short_id, truncate};

// ----------------------------------------------------------------------
// fxg daemon
// ----------------------------------------------------------------------

/// Arguments of `fxg daemon`.
#[derive(Debug, Args)]
pub struct DaemonArgs {
    /// Bind address of the local HTTP/WS server (default: 127.0.0.1:7860)
    #[usage(long)]
    listen: Option<String>,
    /// Node Hub WebSocket URL of the central server (Outbox sync; standalone when unset)
    #[usage(long)]
    server_url: Option<String>,
    /// Allow Web PTY launches from remote clients (through the central server)
    #[usage(long)]
    allow_remote_pty: bool,
    /// Standard I/O pipe (JSON Lines) mode (ephemeral VM; launched by a provisioner)
    #[usage(long)]
    stdio: bool,
    /// Ephemeral VM mode (enables Git bundle backup on drain)
    #[usage(long)]
    ephemeral: bool,
    /// Initial target directory in `--stdio` mode (the cloned workspace)
    #[usage(long)]
    workspace: Option<String>,
    /// Reside in the task tray (status / auto-start / Web UI / quit)
    #[usage(long)]
    tray: bool,
    /// Disable the tray residency enabled by `[node] tray`
    #[usage(long)]
    no_tray: bool,
    /// Log level (trace | debug | info | warn | error)
    #[usage(long)]
    pub(crate) log_level: Option<String>,
    /// Log file destination (default: ~/.flexagent/logs/daemon.log)
    #[usage(long)]
    pub(crate) log_file: Option<String>,
    /// Disable writing to the log file
    #[usage(long)]
    pub(crate) no_log_file: bool,
    /// Log output format (text | json)
    #[usage(long)]
    pub(crate) log_format: Option<String>,
    /// Disable ANSI color output
    #[usage(long)]
    pub(crate) no_color: bool,
}

impl DaemonArgs {
    /// Build the logging options.
    pub(crate) fn logging_options(&self) -> crate::logging::LoggingOptions {
        crate::logging::LoggingOptions {
            log_level: self.log_level.clone(),
            verbose: 0,
            quiet: false,
            no_color: self.no_color,
            log_file: if self.no_log_file {
                Some("-".to_owned())
            } else {
                self.log_file.clone()
            },
            log_format: self.log_format.clone(),
            kind: crate::logging::CommandLogKind::Daemon,
            is_stdio: self.stdio,
        }
    }
    /// Build the daemon config from the loaded global config.
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
        // Outbox sync (central server connection; standalone / local only when unset)
        config.central_server_url = self
            .server_url
            .clone()
            .or_else(|| global.node.central_server_url.clone());
        config.node_token = global.node.node_token.clone();
        config
    }

    /// Whether tray residency is requested (always disabled for `--stdio` /
    /// `--ephemeral`).
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
                bail!("--ephemeral / --workspace require --stdio");
            }
            let mut config = fxg_node::daemon::DaemonConfig::from_env(&env);
            if let Some(node_id) = global.node.node_id.clone() {
                config.node_id = node_id;
            }
            if let Some(name) = global.node.name.clone() {
                config.node_name = name;
            }
            config.allow_remote_pty = self.allow_remote_pty || global.node.allow_remote_pty;
            // In stdio mode the server owns the pipe, so WS sync settings are unused
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

        let log_file = crate::logging::resolve_log_file_path(
            &self.logging_options(),
            &global.log,
            daemon.state().paths().fxg_home(),
        );
        let use_color = crate::logging::should_use_color(self.no_color, global.log.no_color);
        let auth_token = if cfg!(debug_assertions) {
            Some(daemon.state().auth_token())
        } else {
            None
        };
        crate::logging::print_daemon_banner(
            daemon.state().node_id(),
            &daemon.http_addr().to_string(),
            daemon.ipc_endpoint(),
            log_file.as_deref(),
            daemon.state().config().central_server_url.as_deref(),
            auth_token.as_deref(),
            use_color,
        );

        // Tray residency (on macOS it is handled on the main thread by the early
        // branch in main.rs)
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

/// Start the tray residency (failures only warn; the daemon keeps running).
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

/// Arguments of `fxg server`.
#[derive(Debug, Args)]
pub struct ServerArgs {
    /// HTTP/WS bind address (default: 0.0.0.0:8080)
    #[usage(long)]
    listen: Option<String>,
    /// Port override (takes precedence over `--listen`)
    #[usage(long)]
    port: Option<u16>,
    /// Log level (trace | debug | info | warn | error)
    #[usage(long)]
    pub(crate) log_level: Option<String>,
    /// Log file destination (default: ~/.flexagent/logs/server.log)
    #[usage(long)]
    pub(crate) log_file: Option<String>,
    /// Disable writing to the log file
    #[usage(long)]
    pub(crate) no_log_file: bool,
    /// Log output format (text | json)
    #[usage(long)]
    pub(crate) log_format: Option<String>,
    /// Disable ANSI color output
    #[usage(long)]
    pub(crate) no_color: bool,
}

impl ServerArgs {
    /// Build the logging options.
    pub(crate) fn logging_options(&self) -> crate::logging::LoggingOptions {
        crate::logging::LoggingOptions {
            log_level: self.log_level.clone(),
            verbose: 0,
            quiet: false,
            no_color: self.no_color,
            log_file: if self.no_log_file {
                Some("-".to_owned())
            } else {
                self.log_file.clone()
            },
            log_format: self.log_format.clone(),
            kind: crate::logging::CommandLogKind::Server,
            is_stdio: false,
        }
    }
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
        let fxg_home = options.fxg_home.clone();
        let server = fxg_server::Server::start(options)
            .await
            .context("fxg server failed")?;

        let log_file =
            crate::logging::resolve_log_file_path(&self.logging_options(), &global.log, &fxg_home);
        let use_color = crate::logging::should_use_color(self.no_color, global.log.no_color);
        let auth_token = if cfg!(debug_assertions) {
            Some(server.state().auth_token())
        } else {
            None
        };
        crate::logging::print_server_banner(
            &server.listen_addr().to_string(),
            &fxg_home,
            log_file.as_deref(),
            auth_token.as_deref(),
            use_color,
        );

        server.wait().await;
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg service
// ----------------------------------------------------------------------

/// Arguments of `fxg service <action>`.
#[derive(Debug, Args)]
pub struct ServiceArgs {
    /// Operation to run: install | uninstall | start | stop | restart | status
    action: String,
    /// Target the central server (`server`) instead of the `daemon`
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

/// Arguments of `fxg web`.
#[derive(Debug, Args)]
pub struct WebArgs {
    /// Open the central server URL instead of the local node
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
            // The central server keeps no local auth token, so open the URL only
            // and leave it to the first-run token prompt (docs/05 §1.1)
            let server_url = global
                .node
                .central_server_url
                .clone()
                .context("node.central_server_url is not set in config.toml")?;
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

/// Build the tokenized URL of the local Web UI (shared by `fxg web` and the
/// tray).
pub(crate) fn local_web_url(listen_addr: &str, token: &str) -> String {
    format!("http://{listen_addr}/?token={token}")
}

/// Build the tokenized Web UI URL for a specific session.
pub(crate) fn local_session_web_url(session_id: &str) -> Result<String> {
    let env = fxg_protocol::config::process_env;
    let global = fxg_protocol::config::GlobalConfig::load(&env)
        .context("failed to load ~/.flexagent/config.toml")?;
    let token = fxg_server::api::auth::load_or_create_token(
        &fxg_protocol::config::fxg_home(&env).join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME),
    )
    .context("failed to load ~/.flexagent/auth_token")?;
    let listen = global
        .node
        .listen_addr
        .clone()
        .unwrap_or_else(|| "127.0.0.1:7860".to_owned());
    Ok(format!(
        "http://{listen}/sessions/{session_id}?token={token}"
    ))
}

/// Print the session ID and Web UI URL, and open it in a browser unless
/// disabled.
pub(crate) fn open_session_url(session_id: &str, no_open: bool) -> Result<()> {
    match local_session_web_url(session_id) {
        Ok(url) => {
            println!("Session: {session_id}");
            println!("Web UI:  {url}");
            if !no_open {
                let _ = opener::open_browser(&url);
            }
        }
        Err(_) => {
            println!("{session_id}");
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------
// fxg run
// ----------------------------------------------------------------------

/// Arguments of `fxg run <agent>`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Agent ID (`opencode` / `claude`, etc.)
    agent: String,
    /// Send an initial prompt
    #[usage(short = 'p', long)]
    prompt: Option<String>,
    /// Print only the session ID and do not open a browser
    #[usage(short = 'd', long)]
    detach: bool,
    /// Do not open the Web UI in the default browser after start
    #[usage(long)]
    no_open: bool,
    /// Initial mode (`code` / `plan`, etc.)
    #[usage(long)]
    mode: Option<String>,
    /// Create or reuse a worktree for the session
    #[usage(short = 'w', long)]
    worktree: Option<String>,
    /// Base branch used when creating a new worktree
    #[usage(long)]
    base: Option<String>,
    /// Start through an ephemeral VM provisioner (runs on the central server
    /// host; requires the central server)
    #[usage(long)]
    provisioner: Option<String>,
    /// Arguments passed through to the agent process
    #[usage(value_name = "EXTRA_ARGS", double_dash = "required")]
    extra_args: Vec<String>,
}

impl usage::RunAsync for RunArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;

        // Ephemeral VM provisioner: start through the central server API
        // (the provisioner runs as a child process on the server host)
        if let Some(provisioner) = &self.provisioner {
            return run_with_provisioner(&self, provisioner, &cwd, &env).await;
        }

        let mut client = DaemonClient::connect().await?;
        let cwd = match &self.worktree {
            Some(branch) => create_worktree(&mut client, &cwd, branch, self.base.clone()).await?,
            None => cwd,
        };

        let session_id = client
            .ensure_session(&cwd, &self.agent, &self.extra_args, self.mode.as_deref())
            .await?;

        if let Some(prompt) = &self.prompt {
            client.send_prompt(&session_id, prompt, "cli").await?;
        }

        if self.detach {
            println!("{session_id}");
            return Ok(());
        }
        open_session_url(&session_id, self.no_open)
    }
}

/// `fxg run <agent> --provisioner <name>` (through the central server API).
async fn run_with_provisioner(
    args: &RunArgs,
    provisioner: &str,
    cwd: &Path,
    env: fxg_protocol::config::EnvLookup<'_>,
) -> Result<()> {
    let server = crate::server_api::ServerClient::from_config(env)?;

    // Resolve the logical project of the current directory (the Git remote is
    // the source of truth)
    let project = fxg_node::resolve_project(cwd, "cli")
        .await
        .context("failed to resolve logical project (run inside a Git repository)")?;

    // `-w/--worktree`: in an ephemeral VM this selects the branch to clone
    // instead of creating a new worktree
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
        mode: args.mode.clone(),
        extra_args: if args.extra_args.is_empty() {
            None
        } else {
            Some(args.extra_args.clone())
        },
        fork: None,
    };
    let response = server.create_session(&request).await?;
    eprintln!(
        "provisioning session {} via '{provisioner}' (bootstrap logs are available in the Web UI / `fxg provisioners test`)",
        response.session_id
    );
    if args.detach {
        println!("{}", response.session_id);
    } else {
        open_session_url(&response.session_id, args.no_open)?;
    }
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

/// Provisioner management subcommands.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum ProvisionersCommands {
    /// List the ephemeral VM provisioners defined in the config file
    List(ProvisionersListArgs),
    /// Verify that a provisioner starts and completes the `fxg daemon --stdio` handshake
    Test(ProvisionersTestArgs),
}

/// `fxg provisioners list`
#[derive(Debug, Args)]
pub struct ProvisionersListArgs {
    /// Output as JSON
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
                "(no provisioners defined; add [provisioners.<name>] to ~/.flexagent/config.toml)"
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
    /// Name of the provisioner to verify
    name: String,
}

impl usage::RunAsync for ProvisionersTestArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let server = crate::server_api::ServerClient::from_config(&env)?;
        eprintln!(
            "testing provisioner '{}' via {} (bootstrap can take a few minutes)",
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

/// Arguments of `fxg bootstrap-workspace` (run by the provisioner inside an
/// ephemeral VM).
#[derive(Debug, Args)]
pub struct BootstrapWorkspaceArgs {
    /// Git URL to clone from
    #[usage(long)]
    repo: String,
    /// Target branch (defaults to the remote's default branch)
    #[usage(long)]
    branch: Option<String>,
    /// Destination directory (default: /tmp/workspace)
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
        // stdout is reserved for the protocol, so the result goes to stderr too
        eprintln!(
            "[fxg-bootstrap] ready: {} (tools: {})",
            outcome.dir.display(),
            outcome.tools.join(", ")
        );
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg git-askpass (GIT_ASKPASS helper)
// ----------------------------------------------------------------------

/// Arguments of `fxg git-askpass` (an internal command Git invokes as
/// `GIT_ASKPASS`).
#[derive(Debug, Args)]
pub struct GitAskpassArgs {
    /// Prompt string passed by Git (`Username for '...'` / `Password for '...'`)
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
            token.context("no credential token is available for this prompt")?
        };
        println!("{value}");
        Ok(())
    }
}

/// Create or reuse a worktree and return its path (`fxg run -w` /
/// `fxg session fork -w`).
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

// ----------------------------------------------------------------------
// fxg ps / fxg session list
// ----------------------------------------------------------------------

/// Display options for the session list (shared by `fxg ps` and
/// `fxg session list`).
#[derive(Debug, Args)]
pub struct SessionListArgs {
    /// Include stopped sessions
    #[usage(short = 'a', long)]
    all: bool,
    /// Include archived sessions
    #[usage(long)]
    archived: bool,
    /// Filter by logical project ID
    #[usage(long)]
    project: Option<String>,
    /// Filter by node ID
    #[usage(long)]
    node: Option<String>,
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for SessionListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let sessions = client.list_sessions(self.all, self.archived).await?;
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
            println!("(no sessions)");
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

/// Session management subcommands.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum SessionCommands {
    /// List sessions
    List(SessionListArgs),
    /// Show session details, state and event statistics
    Show(SessionShowArgs),
    /// Send a prompt to an existing session non-interactively (headless)
    Prompt(SessionPromptArgs),
    /// Cancel the running turn, or stop the session gracefully
    Stop(SessionStopArgs),
    /// Force-terminate the session's child process tree
    Kill(SessionKillArgs),
    /// Rewind files and conversation to the given turn
    Revert(SessionRevertArgs),
    /// Resume a stopped session while keeping its conversation context
    Resume(SessionResumeArgs),
    /// Create a new session by forking the conversation from the given point
    Fork(SessionForkArgs),
    /// Archive a session from the list (restorable)
    Archive(SessionArchiveArgs),
    /// Move an archived session back into the list
    Unarchive(SessionUnarchiveArgs),
    /// Delete a session (purges the conversation log; irreversible)
    Delete(SessionDeleteArgs),
}

/// `fxg session show <id>`
#[derive(Debug, Args)]
pub struct SessionShowArgs {
    /// Session ID
    session_id: String,
    /// Number of recent events to show
    #[usage(long, default = "20")]
    events: u32,
    /// Output as JSON
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
        let usage_display = session
            .usage
            .as_ref()
            .map(|usage| {
                let percent = if usage.context_size > 0 {
                    format!(
                        " ({:.1}%)",
                        usage.used_tokens as f64 / usage.context_size as f64 * 100.0
                    )
                } else {
                    String::new()
                };
                let tokens = format!(
                    "{}/{} tokens{percent}",
                    usage.used_tokens, usage.context_size
                );
                match &usage.cost {
                    Some(cost) => format!("{tokens}, cost {:.4} {}", cost.amount, cost.currency),
                    None => tokens,
                }
            })
            .unwrap_or_else(|| "-".to_owned());
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
            ("USAGE", usage_display),
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
            println!("\nPending approvals:");
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
            println!("\nRecent events:");
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

/// Summarize an event into a single line for CLI display.
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
        UnifiedEventPayload::ElicitationRequest { message, .. } => {
            format!("elicitation_request {}", truncate(message, 60))
        }
        UnifiedEventPayload::ElicitationResolved {
            elicitation_id,
            action,
            ..
        } => format!("elicitation_resolved {elicitation_id} → {action}"),
        UnifiedEventPayload::SessionReverted {
            target_node_seq,
            restored_files,
            removed_files,
            ..
        } => format!(
            "session_reverted seq={target_node_seq} restored={restored_files} removed={removed_files}"
        ),
        UnifiedEventPayload::SessionArchived { archived } => {
            format!("session_archived archived={archived}")
        }
        UnifiedEventPayload::SessionDeleted {} => "session_deleted".to_owned(),
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
        UnifiedEventPayload::UsageUpdated {
            used_tokens,
            context_size,
            cost,
        } => {
            let tokens = format!("usage {used_tokens}/{context_size}");
            match cost {
                Some(cost) => format!("{tokens} cost={:.4} {}", cost.amount, cost.currency),
                None => tokens,
            }
        }
        UnifiedEventPayload::TurnEnded { reason } => {
            format!("turn_ended {reason}")
        }
        UnifiedEventPayload::CompactionUpdated { status, .. } => {
            format!("compaction_updated {status}")
        }
    }
}

/// `fxg session prompt <id> <text>`
#[derive(Debug, Args)]
pub struct SessionPromptArgs {
    /// Session ID
    session_id: String,
    /// Prompt text to send
    text: String,
    /// Wait for the turn to finish and print the agent output
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

/// Poll events until the turn finishes (`idle`) and print them
/// (`fxg session prompt --wait`).
async fn wait_for_turn(client: &mut DaemonClient, session_id: &str, prompt: &str) -> Result<()> {
    let mut seen_prompt = false;
    let mut last_seq = 0u64;
    loop {
        let detail = client.session_show(session_id, 100).await?;
        for event in detail.recent_events {
            if event.node_seq > last_seq {
                last_seq = event.node_seq;
                if print_and_check_done(&event.payload, prompt, &mut seen_prompt) {
                    return Ok(());
                }
            }
        }
        if seen_prompt
            && matches!(
                detail.session.status,
                SessionStatus::Idle | SessionStatus::Error | SessionStatus::Stopped
            )
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Print an event and return `true` when the turn is done.
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
            println!("• tool [{status}] {title}");
        }
        UnifiedEventPayload::PermissionRequest {
            tool_name, summary, ..
        } => {
            println!("! pending approval [{tool_name}] {summary} (see `fxg inbox list`) ");
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
    /// Session ID
    session_id: String,
    /// Keep the session and only interrupt the running turn
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
    /// Session ID
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

/// `fxg session archive <id>`
#[derive(Debug, Args)]
pub struct SessionArchiveArgs {
    /// Session ID
    session_id: String,
}

impl usage::RunAsync for SessionArchiveArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let archived_at = client.archive_session(&self.session_id, true).await?;
        println!("archived session ({})", short_id(&self.session_id));
        if let Some(archived_at) = archived_at {
            println!("  archived_at: {}", format_unix_ms_utc(archived_at));
        }
        Ok(())
    }
}

/// `fxg session unarchive <id>`
#[derive(Debug, Args)]
pub struct SessionUnarchiveArgs {
    /// Session ID
    session_id: String,
}

impl usage::RunAsync for SessionUnarchiveArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        client.archive_session(&self.session_id, false).await?;
        println!("unarchived session ({})", short_id(&self.session_id));
        Ok(())
    }
}

/// `fxg session delete <id>`
#[derive(Debug, Args)]
pub struct SessionDeleteArgs {
    /// Session ID
    session_id: String,
}

impl usage::RunAsync for SessionDeleteArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        client.delete_session(&self.session_id).await?;
        println!(
            "deleted session ({}) — the conversation log is purged and cannot be restored",
            short_id(&self.session_id)
        );
        Ok(())
    }
}

/// `fxg session revert <id> --to-seq <NODE_SEQ>`
#[derive(Debug, Args)]
pub struct SessionRevertArgs {
    /// Session ID
    session_id: String,
    /// Event sequence number to rewind to (the target turn's
    /// `UserMessage.node_seq`)
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
    /// Session ID to fork from
    session_id: String,
    /// Sequence number to fork from (latest when omitted)
    #[usage(long)]
    from_seq: Option<u64>,
    /// Agent ID after the fork (same agent when omitted)
    #[usage(long)]
    agent: Option<String>,
    /// Fork into a new worktree
    #[usage(short = 'w', long)]
    worktree: Option<String>,
    /// Base branch used when creating a new worktree
    #[usage(long)]
    base: Option<String>,
    /// Fork through an ephemeral VM provisioner (requires the central server;
    /// restores the backed-up Git bundle)
    #[usage(long)]
    provisioner: Option<String>,
    /// Print only the session ID and do not open a browser
    #[usage(short = 'd', long)]
    detach: bool,
    /// Do not open the Web UI in the default browser after the fork
    #[usage(long)]
    no_open: bool,
}

impl usage::RunAsync for SessionForkArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        // Fork inside an ephemeral VM: request a context fork through the central
        // server API
        if let Some(provisioner) = &self.provisioner {
            let env = fxg_protocol::config::process_env;
            let server = crate::server_api::ServerClient::from_config(&env)?;
            let sessions = server.sessions().await?;
            let source = sessions
                .iter()
                .find(|session| session.session_id == self.session_id)
                .with_context(|| format!("session not found: {}", self.session_id))?;
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
                mode: None,
                extra_args: None,
                fork: Some(fxg_protocol::client_api::SessionForkSpec {
                    from_session_id: self.session_id.clone(),
                    from_node_seq: self.from_seq,
                    // The central server loads the backed-up bundle
                    // (git_bundle_path) automatically
                    restore_git_bundle_b64: None,
                }),
            };
            let response = server.create_session(&request).await?;
            if self.detach {
                println!("{}", response.session_id);
            } else {
                open_session_url(&response.session_id, self.no_open)?;
            }
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

        let session_id = client
            .fork_session(&self.session_id, self.from_seq, self.agent, cwd.as_deref())
            .await?;
        if self.detach {
            println!("{session_id}");
            return Ok(());
        }
        open_session_url(&session_id, self.no_open)
    }
}

/// `fxg session resume <id>`
#[derive(Debug, Args)]
pub struct SessionResumeArgs {
    /// Session ID to resume
    session_id: String,
    /// Print only the session ID and do not open a browser
    #[usage(short = 'd', long)]
    detach: bool,
    /// Do not open the Web UI in the default browser after resuming
    #[usage(long)]
    no_open: bool,
    /// Skip native restoration (session/resume, session/load, existing-session
    /// bind) and continue by replaying history
    #[usage(long)]
    replay: bool,
}

impl usage::RunAsync for SessionResumeArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let context_restored = client.resume_session(&self.session_id, self.replay).await?;
        if context_restored {
            eprintln!("resumed (agent context restored): {}", self.session_id);
        } else {
            eprintln!("resumed with replayed history: {}", self.session_id);
        }
        if self.detach {
            println!("{}", self.session_id);
            return Ok(());
        }
        open_session_url(&self.session_id, self.no_open)
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

/// Operations on pending approval requests (the global inbox).
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum InboxCommands {
    /// List the requests currently waiting for approval
    List(InboxListArgs),
    /// Approve an approval request
    Approve(InboxApproveArgs),
    /// Reject an approval request
    Reject(InboxRejectArgs),
    /// Answer a question (elicitation)
    Answer(InboxAnswerArgs),
}

/// `fxg inbox list`
#[derive(Debug, Args)]
pub struct InboxListArgs {
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for InboxListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let requests = client.inbox_list().await?;
        let elicitations = client.inbox_elicitations().await?;
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "requests": requests,
                    "elicitations": elicitations,
                }))?
            );
            return Ok(());
        }
        if requests.is_empty() {
            println!("(no pending approval requests)");
        } else {
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
        }
        if elicitations.is_empty() {
            println!("(no unanswered questions)");
        } else {
            let rows: Vec<Vec<String>> = elicitations
                .iter()
                .map(|entry| {
                    vec![
                        truncate(&entry.elicitation_id, 24),
                        short_id(&entry.session_id),
                        truncate(&entry.mode, 8),
                        truncate(&entry.message, 40),
                        format_unix_ms_utc(entry.created_at),
                    ]
                })
                .collect();
            print_table(
                &["QUESTION", "SESSION", "MODE", "MESSAGE", "CREATED"],
                &rows,
            );
        }
        Ok(())
    }
}

/// `fxg inbox approve <req-id>`
#[derive(Debug, Args)]
pub struct InboxApproveArgs {
    /// Approval request ID
    request_id: String,
    /// Allow the same tool action for the rest of the session (`allow_always`)
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
            // Fallback for drivers without that option kind (first non-reject
            // option)
            .or_else(|| {
                entry
                    .options
                    .iter()
                    .find(|option| !PermissionOption::is_reject_kind(&option.kind))
                    .map(|option| option.option_id.clone())
            })
            .with_context(|| format!("no allow option available: {}", self.request_id))?;
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
    /// Approval request ID
    request_id: String,
}

impl usage::RunAsync for InboxRejectArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let entry = find_permission(&mut client, &self.request_id).await?;
        let option_id = option_by_kind(&entry.options, "reject")
            .with_context(|| format!("no reject option available: {}", self.request_id))?;
        client
            .respond_permission(&entry.session_id, &entry.request_id, &option_id)
            .await?;
        println!("rejected {} ({option_id})", short_id(&entry.session_id));
        Ok(())
    }
}

/// Look up a pending approval request by `request_id`.
async fn find_permission(
    client: &mut DaemonClient,
    request_id: &str,
) -> Result<fxg_protocol::common::PermissionRequestEntry> {
    client
        .inbox_list()
        .await?
        .into_iter()
        .find(|entry| entry.request_id == request_id)
        .with_context(|| format!("pending approval request not found: {request_id}"))
}

/// `fxg inbox answer <elicitation-id>`
#[derive(Debug, Args)]
pub struct InboxAnswerArgs {
    /// elicitation ID (the QUESTION column of `fxg inbox list`)
    elicitation_id: String,
    /// accept: supply the answer as a JSON object (e.g. '{"strategy":"a"}')
    #[usage(long)]
    json: Option<String>,
    /// accept: add a `key=value` answer (repeatable; the value is parsed as
    /// JSON and falls back to a string)
    #[usage(long = "set")]
    set: Vec<String>,
    /// accept: answer with the given content (empty when nothing is specified)
    #[usage(long)]
    accept: bool,
    /// Explicitly decline
    #[usage(long)]
    decline: bool,
    /// Cancel without answering
    #[usage(long)]
    cancel: bool,
}

impl usage::RunAsync for InboxAnswerArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let entry = find_elicitation(&mut client, &self.elicitation_id).await?;
        let has_content = self.json.is_some() || !self.set.is_empty();
        let (action, content) = if self.decline {
            (ElicitationAction::Decline, serde_json::Value::Null)
        } else if self.cancel {
            (ElicitationAction::Cancel, serde_json::Value::Null)
        } else if self.accept || has_content {
            (
                ElicitationAction::Accept,
                build_elicitation_content(&self.json, &self.set)?,
            )
        } else {
            // No arguments: show the question and how to answer it
            println!("Question: {}", entry.message);
            if let Some(properties) = entry
                .requested_schema
                .get("properties")
                .and_then(|value| value.as_object())
            {
                println!("Answer fields:");
                for (key, property) in properties {
                    let type_name = property
                        .get("type")
                        .and_then(|value| value.as_str())
                        .unwrap_or("string");
                    println!("  {key} ({type_name})");
                }
            }
            println!(
                "Example: fxg inbox answer {} --accept --set key=value",
                self.elicitation_id
            );
            println!(
                "        fxg inbox answer {} --decline | --cancel",
                self.elicitation_id
            );
            return Ok(());
        };
        client
            .respond_elicitation(&entry.session_id, &entry.elicitation_id, action, content)
            .await?;
        println!("answered {} ({action})", short_id(&entry.session_id));
        Ok(())
    }
}

/// Look up an unanswered question by `elicitation_id`.
async fn find_elicitation(
    client: &mut DaemonClient,
    elicitation_id: &str,
) -> Result<ElicitationRequestEntry> {
    client
        .inbox_elicitations()
        .await?
        .into_iter()
        .find(|entry| entry.elicitation_id == elicitation_id)
        .with_context(|| format!("unanswered question not found: {elicitation_id}"))
}

/// Build the accept content from `--json` / `--set`.
fn build_elicitation_content(json: &Option<String>, set: &[String]) -> Result<serde_json::Value> {
    let mut content = match json {
        Some(raw) => serde_json::from_str::<serde_json::Value>(raw)
            .with_context(|| format!("invalid JSON for --json: {raw}"))?,
        None => serde_json::json!({}),
    };
    let Some(object) = content.as_object_mut() else {
        bail!("--json requires a JSON object");
    };
    for pair in set {
        let (key, value) = pair
            .split_once('=')
            .with_context(|| format!("--set requires key=value format: {pair}"))?;
        let value = serde_json::from_str::<serde_json::Value>(value)
            .unwrap_or_else(|_| serde_json::Value::String(value.to_owned()));
        object.insert(key.to_owned(), value);
    }
    Ok(content)
}

/// Select the first `option_id` whose kind matches.
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

/// Logical project management subcommands.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum ProjectCommands {
    /// Show the logical project info for the current directory
    Info(ProjectInfoArgs),
    /// List registered logical projects
    List(ProjectListArgs),
    /// Link the current directory to the given logical project ID
    Link(ProjectLinkArgs),
    /// Scan and register the Git repositories under a directory
    Scan(ProjectScanArgs),
}

/// `fxg project info`
#[derive(Debug, Args)]
pub struct ProjectInfoArgs {
    /// Output as JSON
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
    /// Output as JSON
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
            println!("(no projects; register one with `fxg project scan` or `fxg project link`)");
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
    /// Logical project ID to link to (e.g. github.com/nazo6/flexagent)
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
    /// Directory to scan (defaults to project_scan_dirs in config.toml)
    dir: Option<String>,
    /// Output as JSON
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

/// Git worktree management subcommands.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum WorktreeCommands {
    /// List worktrees
    List(WorktreeListArgs),
    /// Create a new worktree
    Add(WorktreeAddArgs),
    /// Remove a worktree
    Remove(WorktreeRemoveArgs),
    /// Clean up metadata of worktrees that were deleted
    Prune(WorktreePruneArgs),
}

/// `fxg worktree list`
#[derive(Debug, Args)]
pub struct WorktreeListArgs {
    /// Filter by logical project ID
    #[usage(long)]
    project: Option<String>,
    /// Output as JSON
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
            println!("(no worktrees)");
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
    /// Branch name to create (e.g. feat/auth)
    branch: String,
    /// Base branch (defaults to the current HEAD)
    #[usage(long)]
    base: Option<String>,
    /// Explicit destination path (defaults to a path resolved from worktree_dir_template)
    #[usage(long)]
    path: Option<String>,
    /// Logical project ID (defaults to resolving from the current directory)
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
    /// Branch name or path to remove
    target: String,
    /// Force removal even with uncommitted changes
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

/// ACP Registry agent management.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum AgentsCommands {
    /// List available agents and their installation state
    List(AgentsListArgs),
    /// Pre-download and unpack the given agent from the ACP Registry
    Install(AgentsInstallArgs),
    /// Update the registry index and installed agents
    Update(AgentsUpdateArgs),
    /// Delete cached agent binaries
    Remove(AgentsRemoveArgs),
}

/// Build an ACP Registry from `~/.flexagent/config.toml`.
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
    /// Also show agents that are not installed in the ACP Registry
    #[usage(long)]
    all: bool,
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

/// Agent launch-form labels (for display).
fn source_labels(entry: &AgentListEntry) -> Vec<&'static str> {
    let mut labels = Vec::new();
    if entry.custom.is_some() {
        labels.push("custom");
    }
    for source in &entry.sources {
        labels.push(match source {
            LocalSource::External(_) => "external",
            LocalSource::Npx => "npx",
            LocalSource::Uvx => "uvx",
        });
    }
    labels
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
                        "available": entry.is_available(),
                        "registry": entry.registry,
                        "package_available": entry.package_available,
                        "sources": source_labels(entry),
                        "custom": entry.custom.is_some(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }
        if entries.is_empty() {
            println!(
                "(no agents available; use `fxg agents list --all` to show the whole registry)"
            );
            return Ok(());
        }
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|entry| {
                vec![
                    entry.id.clone(),
                    truncate(&entry.name, 24),
                    entry.version.clone(),
                    if entry.is_available() { "yes" } else { "-" }.to_owned(),
                    source_labels(entry).join(","),
                ]
            })
            .collect();
        print_table(&["ID", "NAME", "VERSION", "AVAILABLE", "SOURCES"], &rows);
        Ok(())
    }
}

/// `fxg agents install <id>`
#[derive(Debug, Args)]
pub struct AgentsInstallArgs {
    /// Agent ID (aliases allowed)
    id: String,
    /// Version to install (only versions offered by the registry)
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
            format!("agent not found in the registry: {id} (`fxg agents list --all`)")
        })?;
        if agent.distribution.npx.is_some() || agent.distribution.uvx.is_some() {
            registry.set_package_enabled(&id, true)?;
            println!("enabled package agent {id} (npx/uvx)");
            return Ok(());
        }
        bail!(
            "fxg does not download binaries automatically. Install the binary from the tool's distributor and put it on PATH, or configure it as a custom agent (in `config.toml` or the Web UI)"
        );
    }
}

/// `fxg agents update`
#[derive(Debug, Args)]
pub struct AgentsUpdateArgs {}

impl usage::RunAsync for AgentsUpdateArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let registry = acp_registry()?;
        // Force a refetch of the index
        let index = registry
            .index(true)
            .await
            .context("failed to fetch ACP registry")?;
        println!("updated ACP registry index ({} agents)", index.agents.len());
        Ok(())
    }
}

/// `fxg agents remove <id>`
#[derive(Debug, Args)]
pub struct AgentsRemoveArgs {
    /// Agent ID (aliases allowed)
    id: String,
}

impl usage::RunAsync for AgentsRemoveArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let registry = acp_registry()?;
        let id = registry.resolve_alias(&self.id);
        let removed_custom = registry.remove_custom(&id)?;
        let disabled_package = if registry.enabled_package_agents().contains(&id) {
            registry.set_package_enabled(&id, false)?;
            true
        } else {
            false
        };
        if removed_custom || disabled_package {
            println!("removed/disabled {id}");
        } else {
            println!("{id} is not configured");
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

/// Auth token management subcommands.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum AuthCommands {
    /// Show the client auth token (auth_token)
    Token(AuthTokenArgs),
    /// Regenerate the client auth token
    RotateToken(AuthRotateArgs),
    /// Manage per-node tokens (Node ⇔ Server pairing)
    #[usage(name = "node-token")]
    NodeToken(NodeTokenArgs),
}

/// Subcommands of `fxg auth node-token`.
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct NodeTokenArgs {
    #[usage(subcommand)]
    command: NodeTokenCommands,
}

/// Per-node token operations.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum NodeTokenCommands {
    /// Issue and print a per-node token (run on the central server)
    Issue(NodeTokenIssueArgs),
    /// Revoke a per-node token
    Revoke(NodeTokenRevokeArgs),
    /// List issued node tokens
    List(NodeTokenListArgs),
}

/// `fxg auth node-token issue <node-id>`
#[derive(Debug, Args)]
pub struct NodeTokenIssueArgs {
    /// Target node ID (the `fxg daemon` node_id)
    node_id: String,
}

impl usage::RunAsync for NodeTokenIssueArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let db = open_hub_db().await?;
        // Register unknown nodes with placeholder values
        // (overwritten with real values by `NodeHello` when the node connects)
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
        // The plaintext token is shown only once at issuance (the server stores
        // only the hash)
        println!("{token}");
        eprintln!("issued a node_token; save it to ~/.flexagent/node_token on the node side.");
        Ok(())
    }
}

/// `fxg auth node-token revoke <node-id>`
#[derive(Debug, Args)]
pub struct NodeTokenRevokeArgs {
    /// Target node ID
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

/// Open the central server DB (`~/.flexagent/server.db`).
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
        eprintln!("regenerated the auth token (browsers and other clients must enter it again)");
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg kill-all
// ----------------------------------------------------------------------

/// Arguments of `fxg kill-all`.
#[derive(Debug, Args)]
pub struct KillAllArgs {
    /// Stop only the local node without fanning out through the central server
    /// (stopping every node at once is fanned out by the central server's
    /// `POST /api/v1/system/kill-switch`)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_elicitation_content_parses_json_and_set_pairs() {
        let content = build_elicitation_content(
            &Some("{\"strategy\":\"a\"}".to_owned()),
            &["count=3".to_owned(), "note=hello".to_owned()],
        )
        .expect("content");
        assert_eq!(content["strategy"], "a");
        assert_eq!(content["count"], 3);
        // Values that do not parse as JSON are treated as strings
        assert_eq!(content["note"], "hello");
    }

    #[test]
    fn build_elicitation_content_rejects_non_object_json() {
        let err = build_elicitation_content(&Some("[1,2]".to_owned()), &[]).expect_err("array");
        assert!(err.to_string().contains("JSON object"), "got: {err}");
    }

    #[test]
    fn build_elicitation_content_requires_key_value_pairs() {
        let err = build_elicitation_content(&None, &["invalid".to_owned()]).expect_err("pair");
        assert!(err.to_string().contains("key=value"), "got: {err}");
    }
}
