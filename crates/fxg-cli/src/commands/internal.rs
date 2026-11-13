//! Internal commands invoked by Git, provisioners, or system integrations.

use std::path::PathBuf;

use anyhow::{Context, Result};
use usage::{Args, Subcommands};

/// `fxg internal <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct InternalArgs {
    #[usage(subcommand)]
    pub(crate) command: InternalCommands,
}

/// Internal commands invoked by Git, provisioners, or system integrations.
#[derive(Debug, Subcommands)]
#[usage(run_async)]
pub enum InternalCommands {
    /// Standard I/O pipe (JSON Lines) transport for an ephemeral node
    Stdio(StdioArgs),
}

// ----------------------------------------------------------------------
// fxg internal stdio
// ----------------------------------------------------------------------

/// Arguments of `fxg internal stdio` (launched by a provisioner inside an
/// ephemeral VM).
#[derive(Debug, Args)]
pub struct StdioArgs {
    /// Ephemeral VM mode (flushes events and stops sessions on drain)
    #[usage(long)]
    ephemeral: bool,
    /// Initial target directory (the cloned workspace)
    #[usage(long)]
    workspace: Option<String>,
    /// Log level (trace | debug | info | warn | error)
    #[usage(long)]
    pub(crate) log_level: Option<String>,
    /// Log file destination (default: <logs>/node/node.log)
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

impl StdioArgs {
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
            is_stdio: true,
        }
    }
}

impl usage::RunAsync for StdioArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let global =
            fxg_protocol::config::NodeConfig::load(&env).context("failed to load node.toml")?;

        let mut config = fxg_node::daemon::DaemonConfig::from_env(&env);
        if let Some(node_id) = global.node_id.clone() {
            config.node_id = node_id;
        }
        config.log = effective_log_config(&global, &self);

        let stdio = fxg_node::daemon::StdioConfig {
            daemon: config,
            workspace: self.workspace.map(PathBuf::from),
            ephemeral: self.ephemeral,
        };
        fxg_node::daemon::run_stdio(stdio)
            .await
            .context("fxg internal stdio failed")
    }
}

fn effective_log_config(
    global: &fxg_protocol::config::NodeConfig,
    args: &StdioArgs,
) -> fxg_protocol::config::LogConfig {
    let mut log = global.log.clone();
    if let Some(level) = args.log_level.clone() {
        log.level = Some(level);
    }
    if args.no_log_file {
        log.file = Some("-".to_owned());
    } else if let Some(file) = args.log_file.clone() {
        log.file = Some(file);
    }
    if let Some(format) = args.log_format.clone() {
        log.format = Some(format);
    }
    log.no_color = args.no_color || global.log.no_color;
    log
}
