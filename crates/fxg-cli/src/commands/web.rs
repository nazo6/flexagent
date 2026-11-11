//! `fxg web` command.

use anyhow::{Result, bail};
use usage::Args;

use crate::client::NodeClient;

/// `fxg web [node|hub]`
#[derive(Debug, Args)]
pub struct WebArgs {
    /// Target: node (default) or hub
    target: Option<String>,
    /// Hub address to open (`http://host:port`; defaults to `hub.addr`, the
    /// node's pairing, then `[hub] listen_addr`)
    #[usage(long)]
    hub: Option<String>,
}

impl usage::RunAsync for WebArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;

        let url = match self.target.as_deref() {
            None | Some("node") => {
                let mut client = NodeClient::connect().await?;
                let token = client.auth_token(false).await?;
                let base = crate::client::local_node_base(&env)?;
                format!("{base}/?token={token}")
            }
            Some("hub") => crate::hub_api::resolve_hub_base(&env, self.hub.as_deref()).await?,
            Some(other) => bail!("unknown target: {other} (node | hub)"),
        };

        println!("{url}");
        if let Err(err) = opener::open_browser(&url) {
            tracing::warn!("failed to open default browser: {err}");
        }
        Ok(())
    }
}
