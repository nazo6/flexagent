//! `fxg hub start`: startup entry point for the hub.
//!
//! Router layout:
//! - Client REST / Client WS / PTY WS (the shared router from `fxg-server::api`, using the
//!   same paths as the local node)
//! - Node Hub (`/api/v1/node/ws`, authenticated with node pairing tokens)

use std::net::SocketAddr;

use axum::Router;
use axum::routing::get;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::api::{ClientApiOptions, client_router};
use crate::error::HubError;
use crate::hub;
use crate::state::{HubOptions, HubState};

/// A started hub.
pub struct Hub {
    state: HubState,
    listen_addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
    addr_file: std::path::PathBuf,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("listen_addr", &self.listen_addr)
            .finish()
    }
}

impl Hub {
    /// Starts the hub (opens `hub.db` and begins listening for HTTP/WS).
    pub async fn start(options: HubOptions) -> Result<Self, HubError> {
        let state = HubState::new(options).await?;
        let listen_addr = state.options().listen_addr.clone();
        let listener = TcpListener::bind(&listen_addr)
            .await
            .map_err(|err| HubError::Hub(format!("failed to bind {listen_addr}: {err}")))?;
        let bound = listener
            .local_addr()
            .map_err(|err| HubError::Hub(err.to_string()))?;

        // Publish the bound address so `fxg hub status` / `stop` / `web hub`
        // can reach a hub started with a CLI `--listen` / `--port` override.
        let addr_file = state.options().paths.addr_path();
        if let Err(err) = std::fs::write(&addr_file, bound.to_string()) {
            tracing::warn!(path = %addr_file.display(), "failed to write the hub address file: {err}");
        }

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        state.set_shutdown_sender(shutdown_tx.clone());

        // Client API (shared router, identical to the node)
        let client = client_router(
            state.clone(),
            ClientApiOptions {
                port: bound.port(),
                extra_allowed_hosts: state.options().allowed_hosts.clone(),
                extra_allowed_origins: state.options().allowed_origins.clone(),
            },
        );
        // Node Hub (`/api/v1/node/ws`)
        let node_hub = Router::new()
            .route("/api/v1/node/ws", get(hub::node_ws))
            .with_state(state.clone());
        let app = client.merge(node_hub);

        let server_state = state.clone();
        let task = tokio::spawn(async move {
            let server = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                if let Err(err) = shutdown_rx.changed().await {
                    tracing::debug!("shutdown sender dropped; shutting down: {err}");
                }
            });
            tracing::info!(addr = %bound, "fxg hub start listening");
            if let Err(err) = server.await {
                tracing::error!("fxg hub start stopped: {err}");
            }
        });

        // Request shutdown on Ctrl-C
        spawn_ctrl_c_handler(shutdown_tx.clone());

        tracing::info!(
            addr = %bound,
            data_dir = %server_state.options().paths.data_dir().display(),
            "fxg hub start started"
        );

        Ok(Self {
            state,
            listen_addr: bound,
            shutdown_tx,
            task,
            addr_file,
        })
    }

    /// Shared state.
    pub fn state(&self) -> &HubState {
        &self.state
    }

    /// The address actually bound.
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// Requests shutdown.
    pub fn shutdown(&self) {
        fxg_protocol::discard_with!(self.shutdown_tx.send(true), "request server shutdown");
    }

    /// Requests shutdown and waits for the hub task to finish.
    pub async fn wait(self) {
        let mut receiver = self.shutdown_tx.subscribe();
        // `watch` receivers start with the value at creation marked as "seen", so check the
        // current value first to see whether `shutdown()` already ran, then wait for a
        // request (from Ctrl-C or otherwise).
        if !*receiver.borrow_and_update()
            && let Err(err) = receiver.changed().await
        {
            tracing::debug!("shutdown sender dropped while waiting: {err}");
        }
        fxg_protocol::discard_with!(
            self.shutdown_tx.send(true),
            "request server shutdown while waiting"
        );

        // Drain / force-terminate running temporary VMs (best effort)
        let ephemeral_nodes = crate::provisioner::running_node_ids(&self.state);
        crate::provisioner::terminate_all(&self.state).await;
        self.state
            .hub()
            .detach_all_ephemeral(&ephemeral_nodes)
            .await;

        await_shutdown_task("server", self.task, std::time::Duration::from_secs(3)).await;
        fxg_protocol::discard_with!(
            std::fs::remove_file(&self.addr_file),
            "remove the hub address file"
        );
        tracing::info!("fxg hub start stopped");
    }

    /// Starts the hub and runs until shutdown (for the CLI entry point).
    pub async fn run(options: HubOptions) -> Result<(), HubError> {
        let server = Self::start(options).await?;
        server.wait().await;
        Ok(())
    }
}

/// Spawns a shutdown signal handler (`Ctrl-C` on all platforms, plus `SIGTERM` on Unix).
///
/// The first signal requests graceful shutdown; a subsequent signal while
/// already shutting down forces immediate process exit.
pub fn spawn_ctrl_c_handler(shutdown_tx: watch::Sender<bool>) {
    fxg_protocol::spawn_guarded!("shutdown signal handler", async move {
        #[cfg(unix)]
        let mut sigterm =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => Some(signal),
                Err(err) => {
                    tracing::warn!("failed to install SIGTERM handler: {err}");
                    None
                }
            };

        loop {
            let (signal_name, force_exit_code) = {
                #[cfg(unix)]
                {
                    tokio::select! {
                        res = tokio::signal::ctrl_c() => {
                            if res.is_err() {
                                break;
                            }
                            ("Ctrl-C", 130)
                        }
                        () = wait_sigterm(&mut sigterm) => ("SIGTERM", 143),
                    }
                }
                #[cfg(not(unix))]
                {
                    if tokio::signal::ctrl_c().await.is_err() {
                        break;
                    }
                    ("Ctrl-C", 130)
                }
            };

            if *shutdown_tx.borrow() {
                tracing::warn!(
                    signal = signal_name,
                    "received second shutdown signal; forcing exit"
                );
                std::process::exit(force_exit_code);
            }
            tracing::info!(
                signal = signal_name,
                "received {signal_name}, shutting down (send again to force exit)"
            );
            fxg_protocol::discard_with!(
                shutdown_tx.send(true),
                "request shutdown after shutdown signal"
            );
        }
    });
}

#[cfg(unix)]
async fn wait_sigterm(sigterm: &mut Option<tokio::signal::unix::Signal>) {
    if let Some(stream) = sigterm.as_mut()
        && stream.recv().await.is_some()
    {
        return;
    }
    *sigterm = None;
    std::future::pending::<()>().await;
}

/// Waits for a background task to exit during shutdown, aborting it on timeout.
pub async fn await_shutdown_task(
    task_name: &'static str,
    mut handle: JoinHandle<()>,
    timeout: std::time::Duration,
) {
    match tokio::time::timeout(timeout, &mut handle).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            tracing::error!(
                task = task_name,
                "{task_name} task terminated unexpectedly: {err}"
            );
        }
        Err(_) => {
            tracing::warn!(
                task = task_name,
                timeout = ?timeout,
                "timed out waiting for {task_name} task during shutdown; aborting"
            );
            handle.abort();
        }
    }
}
