//! `fxg node web` / `fxg hub web`: open the Web UI in the default browser.

use anyhow::Result;
use usage::Args;

use crate::client::NodeClient;
use crate::output::{Banner, KvList, Role, print_block, theme};

/// `fxg node web`
#[derive(Debug, Args)]
pub struct NodeWebArgs {}

impl usage::RunAsync for NodeWebArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let mut client = NodeClient::connect().await?;
        let token = client.auth_token(false).await?;
        let base = crate::client::local_node_base(&env)?;
        open_web_ui("local node", &format!("{base}/?token={token}"));
        Ok(())
    }
}

/// `fxg hub web`
#[derive(Debug, Args)]
pub struct HubWebArgs {}

impl usage::RunAsync for HubWebArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let base = crate::hub_api::resolve_hub_base(&env, None).await?;
        let paths = fxg_protocol::paths::Paths::resolve(&env, fxg_protocol::paths::Role::Hub);
        // Only attach the local token when the hub runs on this host: a remote
        // hub (resolved through the node's pairing) has a different token.
        let url = if paths.addr_path().is_file()
            && let Ok(token) = std::fs::read_to_string(paths.auth_token_path())
            && !token.trim().is_empty()
        {
            format!("{base}/?token={}", token.trim())
        } else {
            base
        };
        open_web_ui("hub", &url);
        Ok(())
    }
}

/// Print the URL, then open it in the default browser.
fn open_web_ui(target_label: &str, url: &str) {
    print_block(
        Banner::new(
            "FlexAgent Web UI",
            Role::Accent,
            format!(" ({target_label})"),
            KvList::new().indent(2).field("URL:", url),
        )
        .render(theme())
        .trim_end(),
    );
    match opener::open_browser(url) {
        Ok(()) => println!("  Opened it in your default browser."),
        Err(err) => {
            tracing::debug!("failed to open the default browser: {err}");
            println!("  Could not open your default browser; open the URL above manually.");
        }
    }
}
