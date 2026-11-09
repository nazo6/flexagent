//! CLI subcommands (argument definitions and execution).
//!
//! The `usage` derives are the single source of truth; the KDL spec, shell
//! completions, manpage and Markdown reference are generated from
//! `__usage_spec__` (design: `docs/05-cli-and-pwa-ui.md` §1).
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use usage::Args;

use crate::client::DaemonClient;

use super::*;

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
    /// Standard I/O pipe (JSON Lines) mode (ephemeral VM; launched by a provisioner)
    #[usage(long)]
    stdio: bool,
    /// Ephemeral VM mode (flushes events and stops sessions on drain)
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
        if let Some(listen) = self
            .listen
            .clone()
            .or_else(|| global.node.listen_addr.clone())
        {
            config.listen_addr = listen;
        }
        // Outbox sync (central server connection; standalone / local only when unset)
        config.central_server_url = self
            .server_url
            .clone()
            .or_else(|| global.node.central_server_url.clone());
        config.node_token = env(fxg_protocol::config::env_keys::NODE_TOKEN);
        config.log = self.effective_log_config(global);
        config
    }

    /// Effective logging settings after CLI > env > config.toml precedence.
    fn effective_log_config(
        &self,
        global: &fxg_protocol::config::GlobalConfig,
    ) -> fxg_protocol::config::LogConfig {
        let mut log = global.log.clone();
        if let Some(level) = self.log_level.clone() {
            log.level = Some(level);
        }
        if self.no_log_file {
            log.file = Some("-".to_owned());
        } else if let Some(file) = self.log_file.clone() {
            log.file = Some(file);
        }
        if let Some(format) = self.log_format.clone() {
            log.format = Some(format);
        }
        log.no_color = self.no_color || global.log.no_color;
        log
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
            // In stdio mode the server owns the pipe, so WS sync settings are unused
            config.central_server_url = None;
            config.node_token = None;
            config.log = self.effective_log_config(&global);

            let stdio = fxg_node::daemon::StdioConfig {
                daemon: config,
                workspace: self.workspace.map(PathBuf::from),
                ephemeral: self.ephemeral,
            };
            return fxg_node::daemon::run_stdio(stdio)
                .await
                .context("fxg daemon --stdio failed");
        }

        let tray_configured = match node_db(&env).await {
            Ok(db) => match db.node_settings().await {
                Ok(settings) => settings.and_then(|settings| settings.tray),
                Err(err) => {
                    tracing::debug!("failed to read node settings for the tray: {err}");
                    None
                }
            },
            Err(err) => {
                tracing::debug!("failed to open node.db for the tray setting: {err}");
                None
            }
        };
        let tray_enabled = self.tray_requested(tray_configured);
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
    /// Effective logging settings after CLI > config.toml precedence.
    fn effective_log_config(
        &self,
        global: &fxg_protocol::config::GlobalConfig,
    ) -> fxg_protocol::config::LogConfig {
        let mut log = global.log.clone();
        if let Some(level) = self.log_level.clone() {
            log.level = Some(level);
        }
        if self.no_log_file {
            log.file = Some("-".to_owned());
        } else if let Some(file) = self.log_file.clone() {
            log.file = Some(file);
        }
        if let Some(format) = self.log_format.clone() {
            log.format = Some(format);
        }
        log.no_color = self.no_color || global.log.no_color;
        log
    }

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
            log: self.effective_log_config(&global),
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

// ----------------------------------------------------------------------
// fxg logs / fxg doctor
// ----------------------------------------------------------------------

/// Arguments of `fxg logs`.
#[derive(Debug, Args)]
pub struct LogsArgs {
    /// Only show logs related to this session (uses agent stderr diagnostics)
    #[usage(long)]
    session: Option<String>,
    /// Follow appended daemon log lines
    #[usage(short = 'f', long)]
    follow: bool,
    /// Number of lines to show
    #[usage(long, default = "100")]
    lines: usize,
}

impl usage::RunAsync for LogsArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let home = fxg_protocol::config::fxg_home(&env);
        if let Some(session_id) = self.session.as_deref() {
            let mut client = DaemonClient::connect().await?;
            let detail = client.session_show(session_id, 1).await?;
            let agent_id = detail.session.agent_id;
            let diagnostics = client.diagnostics().await?;
            let Some(agent) = diagnostics
                .agents
                .iter()
                .find(|agent| agent.agent_id == agent_id)
            else {
                println!("(no agent stderr captured for {agent_id})");
                return Ok(());
            };
            let start = agent.stderr_tail.len().saturating_sub(self.lines);
            for line in &agent.stderr_tail[start..] {
                println!("{line}");
            }
            return Ok(());
        }

        let path = home.join("logs").join("daemon.log");
        if !path.exists() {
            println!("(daemon log not found: {})", path.display());
            return Ok(());
        }
        let mut offset;
        if !self.follow {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(self.lines);
            for line in &lines[start..] {
                println!("{line}");
            }
            return Ok(());
        }
        // Follow mode: print the initial tail, then poll for appended bytes.
        let mut file = std::fs::File::open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        let initial = std::fs::read_to_string(&path)?;
        let lines: Vec<&str> = initial.lines().collect();
        let start = lines.len().saturating_sub(self.lines);
        for line in &lines[start..] {
            println!("{line}");
        }
        offset = match file.metadata() {
            Ok(metadata) => metadata.len(),
            Err(err) => {
                tracing::warn!(path = %path.display(), "failed to read log metadata: {err}");
                0
            }
        };
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let len = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(offset);
            if len < offset {
                offset = 0;
            }
            if len > offset {
                use std::io::{Read, Seek, SeekFrom};
                file.seek(SeekFrom::Start(offset))?;
                let mut appended = String::new();
                file.read_to_string(&mut appended)?;
                print!("{appended}");
                use std::io::Write;
                std::io::stdout().flush()?;
                offset = len;
            }
        }
    }
}

/// Arguments of `fxg doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for DoctorArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let home = fxg_protocol::config::fxg_home(&env);
        let mut checks: Vec<(String, bool, String)> = Vec::new();

        let config_ok = match fxg_protocol::config::GlobalConfig::load(&env) {
            Ok(_) => (true, "config.toml is readable".to_owned()),
            Err(err) => (false, format!("config.toml: {err}")),
        };
        checks.push(("config".to_owned(), config_ok.0, config_ok.1));

        checks.push((
            "home".to_owned(),
            true,
            if home.is_dir() {
                home.display().to_string()
            } else {
                format!("{} (created on first daemon start)", home.display())
            },
        ));

        let token_path = home.join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME);
        checks.push((
            "auth_token".to_owned(),
            true,
            if token_path.is_file() {
                token_path.display().to_string()
            } else {
                format!("{} (created on first daemon start)", token_path.display())
            },
        ));

        let db_path = home.join("node.db");
        let db_ok = if db_path.is_file() {
            match fxg_db::Db::open(&db_path, fxg_db::DbRole::Node).await {
                Ok(_) => (true, "node.db opened".to_owned()),
                Err(err) => (false, format!("node.db: {err}")),
            }
        } else {
            (true, "not initialized yet".to_owned())
        };
        checks.push(("database".to_owned(), db_ok.0, db_ok.1));

        let log_path = home.join("logs").join("daemon.log");
        checks.push((
            "logs".to_owned(),
            true,
            if log_path.is_file() {
                log_path.display().to_string()
            } else {
                format!("{} (not created yet)", log_path.display())
            },
        ));

        let git_ok = std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        checks.push((
            "git".to_owned(),
            git_ok,
            if git_ok {
                "git is available".to_owned()
            } else {
                "git was not found on PATH".to_owned()
            },
        ));

        if self.json {
            let value: Vec<_> = checks
                .iter()
                .map(|(name, ok, detail)| {
                    serde_json::json!({ "check": name, "ok": ok, "detail": detail })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            for (name, ok, detail) in &checks {
                println!("{} {name}: {detail}", if *ok { "OK  " } else { "FAIL" });
            }
        }
        if checks.iter().any(|(_, ok, _)| !ok) {
            bail!("doctor found failing checks");
        }
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
            if !no_open && let Err(err) = opener::open_browser(&url) {
                tracing::warn!("failed to open the Web UI in a browser: {err}");
            }
        }
        Err(_) => {
            println!("{session_id}");
        }
    }
    Ok(())
}
