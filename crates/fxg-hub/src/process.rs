//! `fxg hub start`: startup entry point for the hub.
//!
//! Router layout:
//! - Client REST / Client WS / PTY WS (the shared router from `fxg-server::api`, using the
//!   same paths as the local node)
//! - Node Hub (`/api/v1/node/ws`, authenticated with per-node tokens)

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
        let signal_tx = shutdown_tx.clone();
        fxg_protocol::spawn_guarded!("ctrl-c handler", async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!("received Ctrl-C, shutting down");
                fxg_protocol::discard_with!(signal_tx.send(true), "request shutdown after Ctrl-C");
            }
        });

        tracing::info!(
            addr = %bound,
            fxg_home = %server_state.options().fxg_home.display(),
            "fxg hub start started"
        );

        Ok(Self {
            state,
            listen_addr: bound,
            shutdown_tx,
            task,
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

        if let Err(err) = self.task.await {
            tracing::error!(
                task = "server",
                "server task terminated unexpectedly: {err}"
            );
        }
        tracing::info!("fxg hub start stopped");
    }

    /// Starts the hub and runs until shutdown (for the CLI entry point).
    pub async fn run(options: HubOptions) -> Result<(), HubError> {
        let server = Self::start(options).await?;
        server.wait().await;
        Ok(())
    }
}
