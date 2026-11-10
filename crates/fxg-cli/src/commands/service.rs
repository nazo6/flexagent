//! `fxg service` command-line arguments.

use anyhow::{Context, Result, bail};
use usage::Args;

use crate::service::{ServiceAction, ServiceTarget};

/// `fxg service <action> [node|hub]`
#[derive(Debug, Args)]
pub struct ServiceArgs {
    /// Action: install | uninstall
    action: String,
    /// Target: node (default) or hub
    target: Option<String>,
}

impl usage::RunAsync for ServiceArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let action = match self.action.as_str() {
            "install" => ServiceAction::Install,
            "uninstall" => ServiceAction::Uninstall,
            other => bail!("unknown action: {other} (install | uninstall)"),
        };
        let target = match self.target.as_deref() {
            None | Some("node") => ServiceTarget::Node,
            Some("hub") => ServiceTarget::Hub,
            Some(other) => bail!("unknown target: {other} (node | hub)"),
        };
        crate::service::run(action, target)
            .await
            .context("service command failed")
    }
}
