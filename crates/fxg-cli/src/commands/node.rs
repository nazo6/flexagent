//! `fxg node` commands: local node process lifecycle, pairing and Web access tokens.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use usage::{Args, Subcommands};

use crate::client::NodeClient;
use crate::service::ServiceTarget;

use super::*;

/// `fxg node <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct NodeArgs {
    #[usage(subcommand)]
    pub(crate) command: NodeCommands,
}

/// Local node process, pairing and Web access token commands.
#[derive(Debug, Subcommands)]
#[usage(run_async)]
pub enum NodeCommands {
    /// Start the local node in the foreground (or with --detach)
    Start(NodeStartArgs),
    /// Stop the local node (delegates to the OS service when installed)
    Stop(NodeStopArgs),
    /// Restart the local node
    Restart(NodeRestartArgs),
    /// Show the local node status
    Status(NodeStatusArgs),
    /// Pair this node with a hub (writes node_token and [node] hub_url)
    Pair(NodePairArgs),
    /// Remove the hub pairing (deletes node_token and clears [node] hub_url)
    Unpair(NodeUnpairArgs),
    /// Web access token for browsers and CLI clients
    AccessToken(NodeAccessTokenArgs),
}

// ----------------------------------------------------------------------
// fxg node start
// ----------------------------------------------------------------------

/// Arguments of `fxg node start`.
#[derive(Debug, Args)]
pub struct NodeStartArgs {
    /// Run in the background (detached)
    #[usage(long)]
    detach: bool,
    /// Bind address of the local HTTP/WS server (default: 127.0.0.1:7860)
    #[usage(long)]
    listen: Option<String>,
    /// Hub address (`http://host:port`; `ws://.../api/v1/node/ws` is also accepted)
    #[usage(long)]
    hub_url: Option<String>,
    /// Reside in the task tray (status / auto-start / Web UI / quit)
    #[usage(long)]
    tray: bool,
    /// Disable the tray residency enabled by `[node] tray`
    #[usage(long)]
    no_tray: bool,
    /// Log level (trace | debug | info | warn | error)
    #[usage(long)]
    pub(crate) log_level: Option<String>,
    /// Log file destination (default: ~/.flexagent/logs/node.log)
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

impl NodeStartArgs {
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
            kind: crate::logging::CommandLogKind::Node,
            is_stdio: false,
        }
    }

    /// Build the node config from the loaded global config.
    pub(crate) fn node_config_from(
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
            config.listen_addr_explicit = true;
        }
        // Outbox sync (hub connection; standalone / local only when unset)
        config.hub_url = self.hub_url.clone().or_else(|| global.node.hub_url.clone());
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

    /// Whether tray residency is requested.
    pub(crate) fn tray_requested(&self, configured: Option<bool>) -> bool {
        crate::tray::resolve_enabled(self.tray, self.no_tray, configured)
    }
}

impl usage::RunAsync for NodeStartArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let global = fxg_protocol::config::GlobalConfig::load(&env)
            .context("failed to load ~/.flexagent/config.toml")?;

        if NodeClient::connect_existing().await.is_ok() {
            println!("node is already running");
            return Ok(());
        }

        if crate::service::is_installed(ServiceTarget::Node) {
            crate::service::start(ServiceTarget::Node)?;
            println!("node started (service)");
            return Ok(());
        }

        if self.detach {
            let mut args: Vec<String> = vec!["node".to_owned(), "start".to_owned()];
            if let Some(listen) = &self.listen {
                args.push("--listen".to_owned());
                args.push(listen.clone());
            }
            if let Some(hub_url) = &self.hub_url {
                args.push("--hub-url".to_owned());
                args.push(hub_url.clone());
            }
            if self.tray {
                args.push("--tray".to_owned());
            }
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let log = crate::spawn::spawn_background(&refs, ServiceTarget::Node.log_name())?;
            wait_for_node().await?;
            println!("node started (detached); log: {}", log.display());
            return Ok(());
        }

        let tray_configured = load_tray_setting(&env).await;
        let tray_enabled = self.tray_requested(tray_configured);
        let config = self.node_config_from(&global);
        let node = fxg_node::daemon::NodeDaemon::start(config)
            .await
            .context("fxg node start failed")?;

        let log_file = crate::logging::resolve_log_file_path(
            &self.logging_options(),
            &global.log,
            node.state().paths().fxg_home(),
        );
        let use_color = crate::logging::should_use_color(self.no_color, global.log.no_color);
        let auth_token = if cfg!(debug_assertions) {
            Some(node.state().auth_token())
        } else {
            None
        };
        crate::logging::print_node_banner(
            node.state().node_id(),
            &node.http_addr().to_string(),
            log_file.as_deref(),
            node.state().config().hub_url.as_deref(),
            auth_token.as_deref(),
            use_color,
        );

        // Tray residency (on macOS it is handled on the main thread by the early
        // branch in main.rs)
        #[cfg(not(target_os = "macos"))]
        let tray = if tray_enabled {
            start_tray(&node)
        } else {
            None
        };
        #[cfg(target_os = "macos")]
        let _ = tray_enabled;

        node.wait().await;
        #[cfg(not(target_os = "macos"))]
        if let Some(tray) = tray {
            tray.close();
        }
        Ok(())
    }
}

/// Waits until the local node accepts HTTP connections (used after `--detach`).
async fn wait_for_node() -> Result<NodeClient> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match NodeClient::connect_existing().await {
            Ok(client) => return Ok(client),
            Err(err) if Instant::now() < deadline => {
                tracing::trace!("node not ready yet: {err}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

// ----------------------------------------------------------------------
// fxg node stop / restart
// ----------------------------------------------------------------------

/// Arguments of `fxg node stop`.
#[derive(Debug, Args)]
pub struct NodeStopArgs {}

impl usage::RunAsync for NodeStopArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        if crate::service::is_installed(ServiceTarget::Node) {
            crate::service::stop(ServiceTarget::Node)?;
            println!("node stopped (service)");
            return Ok(());
        }
        match NodeClient::connect_existing().await {
            Ok(mut client) => {
                client.shutdown().await?;
                println!("node stopped");
            }
            Err(_) => println!("node is not running"),
        }
        Ok(())
    }
}

/// Arguments of `fxg node restart`.
#[derive(Debug, Args)]
pub struct NodeRestartArgs {}

impl usage::RunAsync for NodeRestartArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        if crate::service::is_installed(ServiceTarget::Node) {
            crate::service::restart(ServiceTarget::Node)?;
            println!("node restarted (service)");
            return Ok(());
        }
        if let Ok(mut client) = NodeClient::connect_existing().await {
            client.shutdown().await?;
        }
        crate::spawn::spawn_background(&["node", "start"], ServiceTarget::Node.log_name())?;
        wait_for_node().await?;
        println!("node restarted");
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg node status
// ----------------------------------------------------------------------

/// Arguments of `fxg node status`.
#[derive(Debug, Args)]
pub struct NodeStatusArgs {
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for NodeStatusArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let service_installed = crate::service::is_installed(ServiceTarget::Node);
        let running = match NodeClient::connect_existing().await {
            Ok(mut client) => Some(client.local_status().await?),
            Err(_) => None,
        };

        if self.json {
            let value = serde_json::json!({
                "service_installed": service_installed,
                "running": running.is_some(),
                "status": running,
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }

        println!(
            "service:   {}",
            if service_installed {
                "installed"
            } else {
                "not installed"
            }
        );
        println!(
            "running:   {}",
            if running.is_some() { "yes" } else { "no" }
        );
        if let Some(status) = running {
            println!(
                "hub:       {}",
                if status.hub_connected {
                    "connected"
                } else {
                    "standalone"
                }
            );
            println!("outbox:    {}", status.unsynced_event_count);
            println!("sessions:  {}", status.sessions.len());
            if let Some(last) = status.last_synced_at {
                println!("synced:    {}", crate::client::format_unix_ms_utc(last));
            }
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg node pair / unpair
// ----------------------------------------------------------------------

/// `fxg node pair --hub <url> --token <token>`
#[derive(Debug, Args)]
pub struct NodePairArgs {
    /// Hub address (`http://host:port`; a full ws:// URL is also accepted)
    #[usage(long)]
    hub: String,
    /// Pairing token issued on the hub (`fxg hub pairing-token issue`)
    #[usage(long)]
    token: String,
}

impl usage::RunAsync for NodePairArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let token = self.token.trim();
        if token.is_empty() {
            bail!("the pairing token must not be empty");
        }
        let hub_url = fxg_protocol::config::node_ws_url(&self.hub)?;
        let env = fxg_protocol::config::process_env;
        let home = fxg_protocol::config::fxg_home(&env);
        std::fs::create_dir_all(&home)
            .with_context(|| format!("failed to create {}", home.display()))?;

        let token_path = home.join(fxg_protocol::config::NODE_TOKEN_FILE_NAME);
        fxg_node::daemon::save_token_file(&token_path, token)
            .with_context(|| format!("failed to write {}", token_path.display()))?;
        let config_path = home.join(fxg_protocol::config::CONFIG_FILE_NAME);
        set_hub_url(&config_path, Some(&hub_url))?;

        println!(
            "paired with {}",
            fxg_protocol::config::hub_http_origin(&hub_url)
        );
        println!("node pairing token: {}", token_path.display());
        println!("restart the node to apply: fxg node restart");
        Ok(())
    }
}

/// `fxg node unpair`
#[derive(Debug, Args)]
pub struct NodeUnpairArgs {}

impl usage::RunAsync for NodeUnpairArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let home = fxg_protocol::config::fxg_home(&env);
        let token_path = home.join(fxg_protocol::config::NODE_TOKEN_FILE_NAME);
        match std::fs::remove_file(&token_path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to remove {}", token_path.display()));
            }
        }
        let config_path = home.join(fxg_protocol::config::CONFIG_FILE_NAME);
        set_hub_url(&config_path, None)?;
        println!("unpaired; the node runs standalone");
        println!("restart the node to apply: fxg node restart");
        Ok(())
    }
}

/// Sets or removes `[node] hub_url` in `config.toml` (other keys and comments are kept).
fn set_hub_url(path: &std::path::Path, hub_url: Option<&str>) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let mut document: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if !document.contains_key("node") {
        document.insert("node", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let node = document
        .get_mut("node")
        .and_then(toml_edit::Item::as_table_mut)
        .with_context(|| format!("[node] in {} is not a table", path.display()))?;
    match hub_url {
        Some(url) => {
            node.insert("hub_url", toml_edit::value(url));
        }
        None => {
            node.remove("hub_url");
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, document.to_string())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

// ----------------------------------------------------------------------
// fxg node access-token
// ----------------------------------------------------------------------

/// `fxg node access-token <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct NodeAccessTokenArgs {
    #[usage(subcommand)]
    command: NodeAccessTokenCommands,
}

/// Web access token operations (for browsers and CLI clients).
#[derive(Debug, Subcommands)]
#[usage(run_async)]
pub enum NodeAccessTokenCommands {
    /// Show the Web access token
    Show(NodeAccessTokenShowArgs),
    /// Regenerate the Web access token
    Rotate(NodeAccessTokenRotateArgs),
}

/// `fxg node access-token show`
#[derive(Debug, Args)]
pub struct NodeAccessTokenShowArgs {}

impl usage::RunAsync for NodeAccessTokenShowArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        println!("{}", client.auth_token(false).await?);
        Ok(())
    }
}

/// `fxg node access-token rotate`
#[derive(Debug, Args)]
pub struct NodeAccessTokenRotateArgs {}

impl usage::RunAsync for NodeAccessTokenRotateArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        let token = client.auth_token(true).await?;
        println!("{token}");
        eprintln!(
            "regenerated the Web access token (browsers and other clients must enter it again)"
        );
        Ok(())
    }
}

// ----------------------------------------------------------------------
// Tray helpers
// ----------------------------------------------------------------------

/// Reads the `tray` setting from `node.db` (returns `None` if unset or on error).
pub(crate) async fn load_tray_setting(env: fxg_protocol::config::EnvLookup<'_>) -> Option<bool> {
    match node_db(env).await {
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
    }
}

/// Start the tray residency (failures only warn; the node keeps running).
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
// Web UI URL helpers
// ----------------------------------------------------------------------

/// Build the tokenized URL of the local Web UI (shared by `fxg web` and the
/// tray).
pub(crate) fn local_web_url(listen_addr: &str, token: &str) -> String {
    format!("http://{listen_addr}/?token={token}")
}

/// Build the tokenized Web UI URL for a specific session.
pub(crate) fn local_session_web_url(session_id: &str) -> Result<String> {
    let env = fxg_protocol::config::process_env;
    let token = fxg_hub::api::auth::load_or_create_token(
        &fxg_protocol::config::fxg_home(&env).join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME),
    )
    .context("failed to load ~/.flexagent/auth_token")?;
    let base = crate::client::local_node_base(&env)?;
    Ok(format!("{base}/sessions/{session_id}?token={token}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_hub_url_keeps_other_settings_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# keep me\n[log]\nlevel = \"debug\"\n\n[node]\nnode_id = \"n1\"\n",
        )
        .unwrap();

        set_hub_url(&path, Some("ws://hub.test:8080/api/v1/node/ws")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let document: toml_edit::DocumentMut = text.parse().unwrap();
        assert_eq!(
            document["node"]["hub_url"].as_str(),
            Some("ws://hub.test:8080/api/v1/node/ws")
        );
        assert_eq!(document["node"]["node_id"].as_str(), Some("n1"));
        assert_eq!(document["log"]["level"].as_str(), Some("debug"));
        assert!(text.contains("# keep me"));

        set_hub_url(&path, None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let document: toml_edit::DocumentMut = text.parse().unwrap();
        assert!(document["node"].get("hub_url").is_none());
        assert_eq!(document["node"]["node_id"].as_str(), Some("n1"));
    }
}
