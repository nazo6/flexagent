//! Entry point of the `fxg` binary (CLI / TUI).
//!
//! CLI parsing uses **`usage-rs`** (the `usage` crate), not `clap`.
//! The `#[derive(Cli)]` / `Args` / `Subcommands` declarations are the single
//! source of truth, and the KDL spec, shell completions, manpage and Markdown
//! reference are generated from `__usage_spec__` (design:
//! `docs/05-cli-and-pwa-ui.md` §1).
//!
//! The CLI exposes the core commands (`run` / `session` / `agents login|logout` /
//! `ps` / `logs` / `doctor` / `daemon` / `server` / `service` / `auth` /
//! `kill-all` / `web`) plus the hidden `internal` namespace. Management features
//! owned by the Web UI (approval inbox, projects, worktrees, the agent catalog)
//! have no CLI subcommand.

// Migration baseline: remove this crate-level allow as Phase 1 of
// `docs/changelog/2026-11-08-type-safety-plan.md` lands.
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "type-safety migration baseline; see docs/06-type-safety.md"
)]

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
    default_subcommand = "web",
    default_subcommand_flags,
    default_subcommand_on_empty
)]
struct Fxg {
    #[usage(subcommand)]
    command: Commands,
}

/// Top-level subcommands.
#[derive(Debug, Subcommands)]
#[usage(run_async)]
enum Commands {
    /// Open a tokenized URL in the default browser
    Web(commands::WebArgs),
    /// Start a new session
    Run(commands::RunArgs),
    /// List running and recent sessions (alias for `fxg session list`)
    Ps(commands::SessionListArgs),
    /// Session management (list / show / prompt / stop / kill / revert / fork)
    Session(commands::SessionArgs),
    /// Agent authentication (login / logout)
    Agents(commands::AgentsArgs),
    /// Show local logs (daemon log / per-agent stderr)
    Logs(commands::LogsArgs),
    /// Check the local configuration, database, logs and toolchain
    Doctor(commands::DoctorArgs),
    /// Export a redacted support bundle / list persisted incidents
    Diagnostics(commands::DiagnosticsArgs),
    /// Start the node daemon in the foreground
    Daemon(commands::DaemonArgs),
    /// Start the central server in the foreground (:8080)
    Server(commands::ServerArgs),
    /// Manage the background service that starts at OS login
    Service(commands::ServiceArgs),
    /// Auth token management (token / rotate-token / node-token)
    Auth(commands::AuthArgs),
    /// [Emergency stop] Force-terminate every local session, child process tree and PTY
    KillAll(commands::KillAllArgs),
    /// Internal commands invoked by Git, provisioners, or system integrations
    #[usage(hide)]
    Internal(commands::InternalArgs),
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Fxg::parse();

    let env = fxg_protocol::config::process_env;
    let fxg_home = fxg_protocol::config::fxg_home(&env);
    let global = match fxg_protocol::config::GlobalConfig::load(&env) {
        Ok(config) => config,
        Err(err) => {
            // Logging is not initialized yet, so report this on stderr. Commands
            // that need the configuration fail with the same error later.
            eprintln!("fxg: failed to load config (using defaults for logging): {err}");
            fxg_protocol::config::GlobalConfig::default()
        }
    };

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
    if let Some(code) = tray::run_daemon_if_requested(&cli.command).await {
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
