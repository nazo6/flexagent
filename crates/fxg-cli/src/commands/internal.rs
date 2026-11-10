//! Internal commands invoked by Git, provisioners, or system integrations.

use std::path::PathBuf;

use anyhow::{Context, Result};
use fxg_protocol::CommandId;
use fxg_protocol::ipc::{
    IPC_PROTOCOL_VERSION, IpcOutcome, IpcRequest, IpcRequestKind, IpcResultKind, IpcServerMessage,
    LocalRequest, LocalResult,
};
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
    /// GIT_ASKPASS helper for the Git credential proxy
    GitAskpass(GitAskpassArgs),
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
    /// Log file destination (default: ~/.flexagent/logs/node.log)
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
        let global = fxg_protocol::config::GlobalConfig::load(&env)
            .context("failed to load ~/.flexagent/config.toml")?;

        let mut config = fxg_node::daemon::DaemonConfig::from_env(&env);
        if let Some(node_id) = global.node.node_id.clone() {
            config.node_id = node_id;
        }
        // In stdio mode the hub owns the pipe, so WS sync settings are unused.
        config.hub_url = None;
        config.node_token = None;
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
    global: &fxg_protocol::config::GlobalConfig,
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

// ----------------------------------------------------------------------
// fxg internal git-askpass (GIT_ASKPASS helper)
// ----------------------------------------------------------------------

/// Arguments of `fxg internal git-askpass` (an internal command Git invokes as
/// `GIT_ASKPASS`).
#[derive(Debug, Args)]
pub struct GitAskpassArgs {
    /// Prompt string passed by Git (`Username for '...'` / `Password for '...'`)
    prompt: String,
}

impl usage::RunAsync for GitAskpassArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let (username, token, error) = git_credential_via_ipc(&self.prompt).await?;
        if let Some(error) = error {
            anyhow::bail!("{error}");
        }
        let value = if fxg_node::credentials::prompt_wants_username(&self.prompt) {
            username
        } else {
            token.context("no credential token is available for this prompt")?
        };
        println!("{value}");
        Ok(())
    }
}

/// Temporary IPC bridge for `fxg internal git-askpass`.
///
/// The askpass command is removed together with the credential proxy in the
/// architecture simplification; until then it still talks to the daemon over
/// local IPC while all normal CLI commands use HTTP.
async fn git_credential_via_ipc(prompt: &str) -> Result<(String, Option<String>, Option<String>)> {
    let command_id = CommandId::from(format!(
        "cli-askpass-{}-{}",
        std::process::id(),
        fxg_protocol::util::uuid_v7()
    ));
    let request = IpcRequest {
        protocol_version: IPC_PROTOCOL_VERSION,
        request_id: command_id.clone(),
        request: IpcRequestKind::Local {
            request: LocalRequest::GitCredential {
                command_id,
                prompt: prompt.to_owned(),
            },
        },
    };
    let endpoint = fxg_node::ipc_endpoint(&fxg_protocol::config::process_env);
    let mut client = fxg_node::IpcClient::connect(&endpoint)
        .await
        .with_context(|| format!("failed to connect to the node at {endpoint}"))?;
    match client.request(&request).await? {
        IpcServerMessage::Response {
            outcome:
                IpcOutcome::Ok {
                    result:
                        IpcResultKind::Local {
                            result:
                                LocalResult::GitCredential {
                                    username,
                                    token,
                                    error,
                                },
                        },
                },
            ..
        } => Ok((username, token, error)),
        IpcServerMessage::Response {
            outcome: IpcOutcome::Err { error },
            ..
        } => anyhow::bail!("{}: {}", error.code, error.message),
        other => anyhow::bail!("unexpected ipc response: {other:?}"),
    }
}
