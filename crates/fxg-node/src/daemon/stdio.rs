//! `fxg daemon --stdio`: standard I/O transport for ephemeral VM and sandbox nodes.
//!
//! Design: `docs/01-architecture-and-sync.md` §6.1, `docs/03-protocol-and-api.md` §2.
//!
//! Speaks JSON Lines over **`stdin` (`ServerToNodeMsg`) / `stdout`
//! (`NodeToServerMsg`)** of the provisioner command that the central server
//! (`fxg server`) starts as a child process (fully independent of any network
//! or VPN). Bootstrap and `mise` / `uv` tool installation logs all go to
//! `stderr`, keeping `stdout` reserved for protocol traffic.
//!
//! - Uses the same [`SessionManager`](crate::session_manager::SessionManager) /
//!   event bus / outbox as persistent nodes, with 100% shared message types
//! - `--workspace <DIR>`: registers the cloned workspace as a project and pins
//!   `StartSession`'s `local_path` to this directory
//! - `--ephemeral`: on `DrainAndShutdown`, saves a Git bundle and exits
//! - Also starts a local IPC server (`fxg git-askpass` → Git credential proxy)

use std::path::PathBuf;
use std::time::Duration;

use fxg_protocol::node_server::NodeToServerMsg;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::{mpsc, watch};

use super::{DaemonConfig, DaemonState};
use crate::error::NodeError;

/// Startup configuration for `fxg daemon --stdio`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioConfig {
    /// Base configuration (node ID / data directory, etc.)
    pub daemon: DaemonConfig,
    /// `--workspace`: initial target directory (the cloned workspace)
    pub workspace: Option<PathBuf>,
    /// `--ephemeral`: ephemeral VM mode (enables Git bundle saving on drain)
    pub ephemeral: bool,
}

/// Capacity of the outgoing queue (including the PTY forwarder).
const OUT_QUEUE_CAPACITY: usize = 1024;

/// Upper bound on the wait time for flushing the outgoing queue after a drain.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(15);

/// Event produced by the dedicated stdin reader thread.
enum StdinEvent {
    /// A complete JSON line.
    Line(String),
    /// Normal end of input (parent closed the pipe).
    Eof,
    /// An I/O error while reading stdin (distinct from a clean EOF).
    Error(String),
}

/// Runs the daemon over the stdio transport (does not return until exit).
pub async fn run_stdio(config: StdioConfig) -> Result<(), NodeError> {
    let mut daemon_config = config.daemon.clone();
    daemon_config.ephemeral = config.ephemeral;
    let state = DaemonState::new(daemon_config).await?;
    state.register_node().await?;

    // Register `--workspace` as a logical project
    // (it is included in the NodeHello project report and used server-side to start sessions)
    if let Some(workspace) = config.workspace.as_deref() {
        if !workspace.is_dir() {
            return Err(NodeError::Server(format!(
                "--workspace directory does not exist: {}",
                workspace.display()
            )));
        }
        match state.resolve_and_register_project(workspace).await {
            Ok(resolved) => tracing::info!(
                project_id = %resolved.project_id,
                path = %workspace.display(),
                "workspace registered"
            ),
            Err(err) => {
                tracing::warn!(
                    path = %workspace.display(),
                    "failed to register workspace project: {err}"
                );
            }
        }
    }

    // Local IPC server (for the GIT_ASKPASS helper / local CLI)
    let (ipc_shutdown_tx, ipc_shutdown_rx) = watch::channel(false);
    let ipc_task = tokio::spawn({
        let state = state.clone();
        let endpoint = state.config().ipc_endpoint.clone();
        async move {
            if let Err(err) = super::ipc::serve(state, endpoint, ipc_shutdown_rx).await {
                tracing::error!("local ipc server stopped: {err}");
            }
        }
    });

    // stdout is reserved for protocol traffic (JSON Lines). Writes are serialized in a dedicated task.
    let (out_tx, mut out_rx) = mpsc::channel::<NodeToServerMsg>(OUT_QUEUE_CAPACITY);
    let writer = tokio::spawn(async move {
        let mut writer = BufWriter::new(tokio::io::stdout());
        while let Some(message) = out_rx.recv().await {
            let mut line = match serde_json::to_vec(&message) {
                Ok(line) => line,
                Err(err) => {
                    tracing::warn!("failed to serialize message for stdout: {err}");
                    continue;
                }
            };
            line.push(b'\n');
            if writer.write_all(&line).await.is_err() || writer.flush().await.is_err() {
                break;
            }
        }
        if let Err(err) = writer.flush().await {
            tracing::debug!("failed to flush stdout at the end of the transport: {err}");
        }
    });

    // Bind the Git credential proxy to this transport
    state.credentials().attach(out_tx.clone());

    // 1. Handshake + flush of unsent events
    let hello = crate::sync::build_hello(&state).await;
    if let Err(err) = out_tx.send(hello).await {
        tracing::warn!("failed to send NodeHello over stdio: {err}");
    }
    if let Err(err) = crate::sync::push_outbox(&state, &out_tx).await {
        tracing::warn!("failed to flush outbox at startup: {err}");
    }

    // 2. Bidirectional loop (stdin `ServerToNodeMsg` / event bus → stdout)
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let ctx = crate::sync::CommandContext {
        workspace: config.workspace.as_deref(),
        ephemeral: config.ephemeral,
        shutdown: Some(&shutdown_tx),
    };
    let mut bus_rx = state.bus().subscribe();

    // Read stdin on a dedicated OS thread. tokio's `stdin` keeps a blocking task
    // resident in the runtime, so dropping the runtime after a drain would prevent
    // the process from exiting (leaving read(0) incomplete).
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<StdinEvent>(64);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in std::io::BufRead::lines(stdin.lock()) {
            match line {
                Ok(line) => {
                    if stdin_tx.blocking_send(StdinEvent::Line(line)).is_err() {
                        return;
                    }
                }
                Err(err) => {
                    if stdin_tx
                        .blocking_send(StdinEvent::Error(err.to_string()))
                        .is_err()
                    {
                        tracing::debug!("stdin reader receiver is gone");
                    }
                    return;
                }
            }
        }
        // Signal a clean EOF (ignored if the receiver already stopped)
        if stdin_tx.blocking_send(StdinEvent::Eof).is_err() {
            tracing::debug!("stdin reader receiver is gone");
        }
    });

    tracing::info!(
        node_id = state.node_id(),
        ephemeral = config.ephemeral,
        "fxg daemon started (stdio transport)"
    );

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            event = stdin_rx.recv() => match event {
                Some(StdinEvent::Line(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    crate::sync::handle_server_message(&state, &line, &out_tx, &ctx).await;
                }
                Some(StdinEvent::Eof) => {
                    tracing::info!("stdin closed; shutting down");
                    break;
                }
                Some(StdinEvent::Error(err)) => {
                    tracing::error!("failed to read stdin: {err}; shutting down");
                    break;
                }
                None => {
                    tracing::error!("stdin reader channel closed unexpectedly; shutting down");
                    break;
                }
            },
            bus = bus_rx.recv() => match bus {
                Ok(crate::session::SessionBroadcast::Persisted { event, .. }) => {
                    fxg_protocol::discard_with!(
                        out_tx
                            .send(NodeToServerMsg::EventBatchPush { events: vec![event] })
                            .await,
                        "push a persisted event to the central server"
                    );
                }
                Ok(crate::session::SessionBroadcast::StreamDelta { session_id, delta }) => {
                    fxg_protocol::discard_with!(
                        out_tx
                            .send(NodeToServerMsg::LiveStreamDelta { session_id, delta })
                            .await,
                        "push a live stream delta to the central server"
                    );
                }
                // Ephemeral events (keystrokes etc.) are not synchronized
                Ok(crate::session::SessionBroadcast::Ephemeral(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "session bus lagged; flushing outbox");
                    if let Err(err) = crate::sync::push_outbox(&state, &out_tx).await {
                        tracing::warn!("failed to flush the outbox after the bus lagged: {err}");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
        }
    }

    // 3. Shutdown (best effort): stop agents / PTYs, flush unsent events,
    //    then close stdout
    if let Err(err) = state.kill_all_local("stdio transport ending").await {
        tracing::warn!("failed to stop sessions on shutdown: {err}");
    }
    if let Err(err) = crate::sync::push_outbox(&state, &out_tx).await {
        tracing::warn!("failed to flush outbox on shutdown: {err}");
    }
    state.credentials().detach();
    if !state.config().ephemeral {
        // Ephemeral VMs have their node row dropped server-side, so offline state is only
        // recorded for regular nodes
        if let Err(err) = state.mark_offline().await {
            tracing::warn!("failed to mark node offline: {err}");
        }
    }
    fxg_protocol::discard_with!(ipc_shutdown_tx.send(true), "request local IPC shutdown");
    drop(out_tx);
    if let Err(err) = tokio::time::timeout(FLUSH_TIMEOUT, writer).await {
        tracing::warn!("timed out waiting for stdout to flush: {err}");
    }
    if let Err(err) = ipc_task.await {
        tracing::warn!("local IPC task terminated unexpectedly: {err}");
    }
    tracing::info!("fxg daemon stopped (stdio transport)");
    Ok(())
}
