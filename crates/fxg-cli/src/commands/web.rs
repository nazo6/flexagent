//! `fxg web` command.

use anyhow::{Result, bail};
use nu_ansi_term::Color;
use usage::Args;

use crate::client::NodeClient;
use crate::output::{Banner, KvList};

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

        let (target_label, url) = match self.target.as_deref() {
            None | Some("node") => {
                let mut client = NodeClient::connect().await?;
                let token = client.auth_token(false).await?;
                let base = crate::client::local_node_base(&env)?;
                ("local node", format!("{base}/?token={token}"))
            }
            Some("hub") => {
                let base = crate::hub_api::resolve_hub_base(&env, self.hub.as_deref()).await?;
                let paths =
                    fxg_protocol::paths::Paths::resolve(&env, fxg_protocol::paths::Role::Hub);
                let url = if self.hub.is_none()
                    && paths.addr_path().is_file()
                    && let Ok(token) = std::fs::read_to_string(paths.auth_token_path())
                    && !token.trim().is_empty()
                {
                    format!("{base}/?token={}", token.trim())
                } else {
                    base
                };
                ("hub", url)
            }
            Some(other) => bail!("unknown target: {other} (node | hub)"),
        };

        println!(
            "{}",
            Banner::new(
                "FlexAgent Web UI",
                Color::Cyan.bold(),
                format!(" ({target_label})"),
                KvList::new().indent(2).field("URL:", url.as_str()),
            )
            .render(false)
        );
        match opener::open_browser(&url) {
            Ok(()) => println!("  Opened it in your default browser."),
            Err(err) => {
                tracing::debug!("failed to open the default browser: {err}");
                println!("  Could not open your default browser; open the URL above manually.");
            }
        }
        Ok(())
    }
}
