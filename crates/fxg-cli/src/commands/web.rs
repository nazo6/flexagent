//! `fxg web` command.

use anyhow::{Result, bail};
use usage::Args;

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

        let url = match self.target.as_deref() {
            None | Some("node") => {
                let mut client = NodeClient::connect().await?;
                let token = client.auth_token(false).await?;
                let base = super::node::local_node_base(&env)?;
                format!("{base}/?token={token}")
            }
            Some("hub") => crate::hub_api::local_hub_base(&env)?,
            Some(other) => bail!("unknown target: {other} (node | hub)"),
        };

        println!("{url}");
        if let Err(err) = opener::open_browser(&url) {
            tracing::warn!("failed to open default browser: {err}");
        }
        Ok(())
    }
}
