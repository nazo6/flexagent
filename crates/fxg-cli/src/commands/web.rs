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
        // The hub process on this host only; a paired remote hub is not opened
        // implicitly and has a different access token.
        let base = crate::hub_api::local_hub_base(&env)?;
        let paths = fxg_protocol::paths::Paths::resolve(&env, fxg_protocol::paths::Role::Hub);
        let url = match fxg_api::auth::load_token(&paths.auth_token_path()) {
            Ok(token) => format!("{base}/?token={token}"),
            Err(err) => {
                tracing::debug!("failed to load the hub's auth_token: {err}");
                base
            }
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
