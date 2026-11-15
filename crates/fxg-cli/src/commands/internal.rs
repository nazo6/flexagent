//! Internal commands invoked by Git, provisioners, or system integrations.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fxg_protocol::paths::{Paths, Role};
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
    /// Delete all local data for development (node and hub)
    Wipe(WipeArgs),
}

// ----------------------------------------------------------------------
// fxg internal wipe
// ----------------------------------------------------------------------

/// Arguments of `fxg internal wipe` (development helper).
///
/// Wipes the data, state, cache and log directories of both roles. Refuses to
/// run while a node or hub is writing to them, because the running process
/// keeps SQLite handles and appenders open. Lists the targets and asks for a
/// confirmation before deleting anything.
#[derive(Debug, Args)]
pub struct WipeArgs {
    /// Also delete node.toml / hub.toml
    #[usage(long)]
    config: bool,
}

impl usage::RunAsync for WipeArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let roles = [
            (Role::Node, Paths::resolve(&env, Role::Node)),
            (Role::Hub, Paths::resolve(&env, Role::Hub)),
        ];

        for (role, paths) in &roles {
            let addr = paths.addr_path();
            if addr.is_file() {
                bail!(
                    "{role} appears to be running ({}); stop it with `fxg {role} stop` first",
                    addr.display()
                );
            }
        }

        // On macOS the config, data and state roots are the same directory, so
        // the same path may appear twice. Deduplicate before removing.
        let mut targets = BTreeSet::new();
        for (_, paths) in &roles {
            targets.insert(paths.data_dir());
            targets.insert(paths.state_dir());
            targets.insert(paths.cache_dir());
            targets.insert(paths.log_dir());
            if self.config {
                targets.insert(paths.config_path());
            }
        }

        eprintln!("fxg internal wipe deletes:");
        for path in &targets {
            eprintln!("  {}", path.display());
        }
        if !confirm("Delete these paths?")? {
            eprintln!("aborted; nothing was deleted");
            return Ok(());
        }

        for path in &targets {
            remove_path(path)?;
        }
        Ok(())
    }
}

/// Asks a yes/no question on stderr and reads the answer from stdin.
///
/// A missing answer (EOF) counts as "no", so non-interactive callers never
/// delete data by accident.
fn confirm(question: &str) -> Result<bool> {
    use std::io::Write;

    eprint!("{question} [y/N] ");
    std::io::stderr()
        .flush()
        .context("failed to flush the confirmation prompt")?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("failed to read the confirmation")?;
    let answer = answer.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

/// Removes a file or directory tree, ignoring a path that is already absent.
fn remove_path(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    let result = if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.with_context(|| format!("failed to remove {}", path.display()))?;
    println!("removed {}", path.display());
    Ok(())
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
