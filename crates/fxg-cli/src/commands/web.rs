//! `fxg web` command.

use anyhow::{Context, Result, bail};
use usage::Args;

use super::node::local_web_url;
use crate::client::NodeClient;

/// `fxg web [node|hub]`
#[derive(Debug, Args)]
pub struct WebArgs {
    /// Target: node (default) or hub
    target: Option<String>,
}

impl usage::RunAsync for WebArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let global = fxg_protocol::config::GlobalConfig::load(&env)
            .context("failed to load ~/.flexagent/config.toml")?;

        let url = match self.target.as_deref() {
            None | Some("node") => {
                let mut client = NodeClient::connect().await?;
                let token = client.auth_token(false).await?;
                let listen = global
                    .node
                    .listen_addr
                    .clone()
                    .unwrap_or_else(|| "127.0.0.1:7860".to_owned());
                local_web_url(&listen, &token)
            }
            Some("hub") => {
                if let Some(hub_url) = global.node.hub_url.clone() {
                    crate::hub_api::hub_http_url(&hub_url)?
                } else {
                    let listen = global.hub.resolved_listen_addr();
                    let host = match listen.rsplit_once(':') {
                        Some((host, port)) if host.is_empty() || host == "0.0.0.0" => {
                            format!("127.0.0.1:{port}")
                        }
                        _ => listen.to_owned(),
                    };
                    format!("http://{host}")
                }
            }
            Some(other) => bail!("unknown target: {other} (node | hub)"),
        };

        println!("{url}");
        if let Err(err) = opener::open_browser(&url) {
            tracing::warn!("failed to open default browser: {err}");
        }
        Ok(())
    }
}
