//! `fxg hub` commands: hub process lifecycle and node pairing tokens.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use usage::{Args, Subcommands};

use crate::hub_api::HubClient;
use crate::service::ServiceTarget;

/// `fxg hub <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct HubArgs {
    #[usage(subcommand)]
    pub(crate) command: HubCommands,
}

/// Hub process and node pairing token commands.
#[derive(Debug, Subcommands)]
#[usage(run_async)]
pub enum HubCommands {
    /// Start the hub in the foreground (or with --detach)
    Start(HubStartArgs),
    /// Stop the hub (delegates to the OS service when installed)
    Stop(HubStopArgs),
    /// Restart the hub
    Restart(HubRestartArgs),
    /// Show the hub status
    Status(HubStatusArgs),
    /// Node pairing tokens (one per node)
    PairingToken(HubPairingTokenArgs),
}

// ----------------------------------------------------------------------
// fxg hub start
// ----------------------------------------------------------------------

/// Arguments of `fxg hub start`.
#[derive(Debug, Args)]
pub struct HubStartArgs {
    /// Run in the background (detached)
    #[usage(long)]
    detach: bool,
    /// HTTP/WS bind address (default: 0.0.0.0:8080)
    #[usage(long)]
    listen: Option<String>,
    /// Port override (takes precedence over --listen)
    #[usage(long)]
    port: Option<u16>,
    /// Log level (trace | debug | info | warn | error)
    #[usage(long)]
    pub(crate) log_level: Option<String>,
    /// Log file destination (default: <logs>/hub/hub.log)
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

impl HubStartArgs {
    /// Effective logging settings after CLI > hub.toml precedence.
    fn effective_log_config(
        &self,
        config_file: &fxg_protocol::config::HubConfig,
    ) -> fxg_protocol::config::LogConfig {
        let mut log = config_file.log.clone();
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
        log.no_color = self.no_color || config_file.log.no_color;
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
            kind: crate::logging::CommandLogKind::Hub,
            is_stdio: false,
        }
    }

    fn resolved_listen_addr(&self, config_file: &fxg_protocol::config::HubConfig) -> String {
        let mut listen_addr = self
            .listen
            .clone()
            .unwrap_or_else(|| config_file.resolved_listen_addr().to_owned());
        if let Some(port) = self.port {
            let host = listen_addr
                .rsplit_once(':')
                .map(|(host, _)| host.to_owned())
                .unwrap_or_else(|| "0.0.0.0".to_owned());
            listen_addr = format!("{host}:{port}");
        }
        listen_addr
    }
}

impl usage::RunAsync for HubStartArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let config_file =
            fxg_protocol::config::HubConfig::load(&env).context("failed to load hub.toml")?;

        if let Ok(client) = HubClient::local(&fxg_protocol::config::process_env, None).await
            && client.system_info().await.is_ok()
        {
            println!("hub is already running");
            return Ok(());
        }

        if crate::service::is_installed(ServiceTarget::Hub) {
            crate::service::start(ServiceTarget::Hub)?;
            println!("hub started (service)");
            return Ok(());
        }

        if self.detach {
            let mut args: Vec<String> = vec!["hub".to_owned(), "start".to_owned()];
            if let Some(listen) = &self.listen {
                args.push("--listen".to_owned());
                args.push(listen.clone());
            }
            if let Some(port) = self.port {
                args.push("--port".to_owned());
                args.push(port.to_string());
            }
            let listen_addr = self.resolved_listen_addr(&config_file);
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let log = crate::spawn::spawn_background(
                &refs,
                fxg_protocol::paths::Role::Hub,
                ServiceTarget::Hub.log_name(),
            )?;
            let address = wait_for_hub(&listen_addr).await?;
            println!("hub started (detached)");
            println!("  Address:  http://{address}");
            println!("  Log:      {}", log.display());
            return Ok(());
        }

        let listen_addr = self.resolved_listen_addr(&config_file);
        let paths = fxg_protocol::paths::Paths::resolve(&env, fxg_protocol::paths::Role::Hub);
        let options = fxg_hub::HubOptions {
            paths: paths.clone(),
            listen_addr,
            allowed_hosts: config_file.allowed_hosts.clone(),
            allowed_origins: config_file.allowed_origins.clone(),
            log: self.effective_log_config(&config_file),
        };
        let hub = fxg_hub::Hub::start(options)
            .await
            .context("fxg hub start failed")?;

        let log_file = crate::logging::resolve_log_file_path(
            &self.logging_options(),
            &config_file.log,
            &paths,
        );
        let use_color = crate::logging::should_use_color(self.no_color, config_file.log.no_color);
        let auth_token = if cfg!(debug_assertions) {
            Some(hub.state().auth_token())
        } else {
            None
        };
        crate::logging::print_hub_banner(
            &hub.listen_addr().to_string(),
            &paths,
            log_file.as_deref(),
            auth_token.as_deref(),
            use_color,
        );

        hub.wait().await;
        Ok(())
    }
}

/// Waits until the hub accepts TCP connections (used after `--detach`).
///
/// Returns the address the hub published in `hub.addr` (the actual bound
/// address) or the requested address when the file is unavailable.
async fn wait_for_hub(listen_addr: &str) -> Result<String> {
    let addr = crate::hub_api::normalize_listen(listen_addr);
    let socket: std::net::SocketAddr = addr
        .parse()
        .with_context(|| format!("invalid listen address: {addr}"))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if std::net::TcpStream::connect_timeout(&socket, Duration::from_millis(500)).is_ok() {
            return Ok(published_hub_addr(&addr));
        }
        if Instant::now() >= deadline {
            anyhow::bail!("hub did not become ready on {addr}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Reads the bound hub address from `hub.addr`, falling back to `fallback`.
fn published_hub_addr(fallback: &str) -> String {
    let env = fxg_protocol::config::process_env;
    let addr_path =
        fxg_protocol::paths::Paths::resolve(&env, fxg_protocol::paths::Role::Hub).addr_path();
    match std::fs::read_to_string(addr_path) {
        Ok(raw) => {
            let addr = raw.trim();
            if addr.is_empty() {
                fallback.to_owned()
            } else {
                crate::hub_api::normalize_listen(addr)
            }
        }
        Err(err) => {
            tracing::debug!("failed to read hub.addr: {err}");
            fallback.to_owned()
        }
    }
}

// ----------------------------------------------------------------------
// fxg hub stop / restart
// ----------------------------------------------------------------------

/// Arguments of `fxg hub stop`.
#[derive(Debug, Args)]
pub struct HubStopArgs {}

impl usage::RunAsync for HubStopArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        if crate::service::is_installed(ServiceTarget::Hub) {
            crate::service::stop(ServiceTarget::Hub)?;
            println!("hub stopped (service)");
            return Ok(());
        }
        let client = HubClient::local(&fxg_protocol::config::process_env, None).await?;
        match client.shutdown().await {
            Ok(()) => println!("hub stopped"),
            Err(_) => println!("hub is not running"),
        }
        Ok(())
    }
}

/// Arguments of `fxg hub restart`.
#[derive(Debug, Args)]
pub struct HubRestartArgs {}

impl usage::RunAsync for HubRestartArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        if crate::service::is_installed(ServiceTarget::Hub) {
            crate::service::restart(ServiceTarget::Hub)?;
            println!("hub restarted (service)");
            return Ok(());
        }
        if let Ok(client) = HubClient::local(&fxg_protocol::config::process_env, None).await {
            fxg_protocol::discard_with!(client.shutdown().await, "stop the hub before restarting");
        }
        let env = fxg_protocol::config::process_env;
        let config_file =
            fxg_protocol::config::HubConfig::load(&env).context("failed to load hub.toml")?;
        let listen_addr = config_file.resolved_listen_addr().to_owned();
        crate::spawn::spawn_background(
            &["hub", "start"],
            fxg_protocol::paths::Role::Hub,
            ServiceTarget::Hub.log_name(),
        )?;
        let address = wait_for_hub(&listen_addr).await?;
        println!("hub restarted");
        println!("  Address:  http://{address}");
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg hub status
// ----------------------------------------------------------------------

/// Arguments of `fxg hub status`.
#[derive(Debug, Args)]
pub struct HubStatusArgs {
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for HubStatusArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let service_installed = crate::service::is_installed(ServiceTarget::Hub);
        let client = HubClient::local(&fxg_protocol::config::process_env, None).await?;
        let address = client.base().to_owned();
        let info = match client.system_info().await {
            Ok(info) => Some(info),
            Err(err) => {
                tracing::debug!("hub system info unavailable: {err}");
                None
            }
        };
        let nodes = match client.nodes().await {
            Ok(nodes) => Some(nodes),
            Err(err) => {
                tracing::debug!("hub nodes unavailable: {err}");
                None
            }
        };

        if self.json {
            let value = serde_json::json!({
                "service_installed": service_installed,
                "running": info.is_some(),
                "address": address,
                "info": info,
                "nodes": nodes.map(|nodes| nodes.nodes),
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
        println!("running:   {}", if info.is_some() { "yes" } else { "no" });
        println!("address:   {address}");
        if let Some(info) = info {
            println!("version:   {}", info.version);
            println!(
                "nodes:     {}",
                nodes.map(|nodes| nodes.nodes.len()).unwrap_or(0)
            );
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg hub pairing-token
// ----------------------------------------------------------------------

/// `fxg hub pairing-token <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct HubPairingTokenArgs {
    #[usage(subcommand)]
    command: HubPairingTokenCommands,
}

/// Node pairing token operations (run on the hub host).
#[derive(Debug, Subcommands)]
#[usage(run_async)]
pub enum HubPairingTokenCommands {
    /// Issue and print a node pairing token
    Issue(HubPairingTokenIssueArgs),
    /// Revoke a node pairing token
    Revoke(HubPairingTokenRevokeArgs),
    /// List issued node pairing tokens
    List(HubPairingTokenListArgs),
}

/// `fxg hub pairing-token issue <node-id>`
#[derive(Debug, Args)]
pub struct HubPairingTokenIssueArgs {
    /// Target node ID (the `fxg node` node_id)
    node_id: String,
}

impl usage::RunAsync for HubPairingTokenIssueArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let db = open_hub_db().await?;
        // Register unknown nodes with placeholder values
        // (overwritten with real values by `NodeHello` when the node connects)
        let nodes = db.list_nodes().await?;
        if !nodes
            .iter()
            .any(|node| node.node_id.as_str() == self.node_id)
        {
            db.upsert_node(&fxg_db::NodeRecord::pending(self.node_id.clone()))
                .await?;
        }
        let token = fxg_hub::api::auth::generate_token();
        db.set_node_token_hash(&self.node_id, Some(&fxg_hub::api::auth::hash_token(&token)))
            .await?;
        // The plaintext token is shown only once at issuance (the hub stores
        // only the hash)
        println!("{token}");
        eprintln!(
            "issued a pairing token; on that node run: fxg node pair --hub <hub-address> --token <token>"
        );
        Ok(())
    }
}

/// `fxg hub pairing-token revoke <node-id>`
#[derive(Debug, Args)]
pub struct HubPairingTokenRevokeArgs {
    /// Target node ID
    node_id: String,
}

impl usage::RunAsync for HubPairingTokenRevokeArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let db = open_hub_db().await?;
        db.set_node_token_hash(&self.node_id, None).await?;
        println!("revoked the pairing token for {}", self.node_id);
        Ok(())
    }
}

/// `fxg hub pairing-token list`
#[derive(Debug, Args)]
pub struct HubPairingTokenListArgs {}

impl usage::RunAsync for HubPairingTokenListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let db = open_hub_db().await?;
        let tokens = db.list_node_tokens().await?;
        if tokens.is_empty() {
            println!("(no pairing tokens issued)");
            return Ok(());
        }
        for (node_id, hash) in tokens {
            let prefix: String = hash.chars().take(12).collect();
            println!("{node_id}\t{prefix}…");
        }
        Ok(())
    }
}

/// Open the hub DB (`<data>/hub/hub.db`).
async fn open_hub_db() -> Result<fxg_db::Db> {
    let env = fxg_protocol::config::process_env;
    let paths = fxg_protocol::paths::Paths::resolve(&env, fxg_protocol::paths::Role::Hub);
    let db = fxg_db::Db::open(&paths.db_path(), fxg_protocol::paths::Role::Hub).await?;
    Ok(db)
}
