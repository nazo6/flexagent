//! Entry point of the `fxg` binary (CLI / TUI).
//!
//! CLI parsing uses **`usage-rs`** (the `usage` crate), not `clap`.
//! The `#[derive(Cli)]` / `Args` / `Subcommands` declarations are the single
//! source of truth, and the KDL spec, shell completions, manpage and Markdown
//! reference are generated from `__usage_spec__` (design:
//! `docs/05-cli-and-pwa-ui.md` §1).
//!
//! Phase 2 provides the basic subcommands (`daemon` / `ps` / `session list` /
//! `project` / `worktree` / `auth` / `kill-all`). Phase 3 added the agent
//! execution commands (`run` / `attach` / `session
//! show|prompt|stop|kill|revert|fork` / `inbox` / `agents`) and the built-in
//! TUI (`tui`).

extern crate usage_rs as usage;

use std::process::ExitCode;

use usage::{Cli, RunAsync, Subcommands};

mod client;
mod commands;
pub mod logging;
mod server_api;
mod service;
mod tray;

/// Root command of `fxg`.
#[derive(Debug, Cli)]
#[usage(run_async)]
#[usage(
    bin = "fxg",
    version = env!("CARGO_PKG_VERSION"),
    about = "FlexAgent: a local-first agent manager",
    arg_required_else_help
)]
struct Fxg {
    #[usage(subcommand)]
    command: Commands,
}

/// Top-level subcommands.
#[derive(Debug, Subcommands)]
#[usage(run_async)]
enum Commands {
    /// Start a new session
    Run(commands::RunArgs),
    /// List running and recent sessions (alias for `fxg session list`)
    Ps(commands::SessionListArgs),
    /// Session management (list / show / prompt / stop / kill / revert / fork)
    Session(commands::SessionArgs),
    /// Review and answer pending approval requests (list / approve / reject)
    Inbox(commands::InboxArgs),
    /// Logical project management (info / list / link / scan)
    Project(commands::ProjectArgs),
    /// Git worktree management (list / add / remove / prune)
    #[usage(alias = "wt")]
    Worktree(commands::WorktreeArgs),
    /// Agent management (list / install / update / remove)
    Agents(commands::AgentsArgs),
    /// Inspect ephemeral VM / sandbox provisioners (list / test)
    Provisioners(commands::ProvisionersArgs),
    /// Start the node daemon in the foreground
    Daemon(commands::DaemonArgs),
    /// Start the central server in the foreground (:8080)
    Server(commands::ServerArgs),
    /// Zero-touch initialization inside an ephemeral VM (git clone + automatic
    /// tool install; output goes to stderr)
    BootstrapWorkspace(commands::BootstrapWorkspaceArgs),
    /// GIT_ASKPASS helper for the Git credential proxy (internal use)
    GitAskpass(commands::GitAskpassArgs),
    /// Manage the background service that starts at OS login
    Service(commands::ServiceArgs),
    /// Open a tokenized URL in the default browser
    Web(commands::WebArgs),
    /// Auth token management (token / rotate-token / node-token)
    Auth(commands::AuthArgs),
    /// [Emergency stop] Force-terminate every local session, child process tree and PTY
    KillAll(commands::KillAllArgs),
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Fxg::parse();

    let env = fxg_protocol::config::process_env;
    let fxg_home = fxg_protocol::config::fxg_home(&env);
    let global = fxg_protocol::config::GlobalConfig::load(&env).unwrap_or_default();

    let logging_options = match &cli.command {
        Commands::Daemon(daemon_args) => daemon_args.logging_options(),
        Commands::Server(server_args) => server_args.logging_options(),
        _ => logging::LoggingOptions {
            kind: logging::CommandLogKind::Cli,
            ..Default::default()
        },
    };
    logging::init_logging(&logging_options, &global, &fxg_home);

    // On macOS the tray (NSStatusItem) must be created and driven on the main
    // thread's event loop, so `fxg daemon --tray` hands the main thread over to
    // the tao loop here (the daemon body moves to a worker thread).
    #[cfg(target_os = "macos")]
    if let Some(code) = tray::run_daemon_if_requested(&cli.command) {
        return code;
    }

    match cli.run_async().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("fxg: {err:#}");
            ExitCode::FAILURE
        }
    }
}
