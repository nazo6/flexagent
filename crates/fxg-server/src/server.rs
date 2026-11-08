//! `fxg server`: startup entry point for the central server.
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
use crate::error::ServerError;
use crate::hub;
use crate::state::{ServerOptions, ServerState};

/// A started central server.
pub struct Server {
    state: ServerState,
    listen_addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("listen_addr", &self.listen_addr)
            .finish()
    }
}

impl Server {
    /// Starts the server (opens `server.db` and begins listening for HTTP/WS).
    pub async fn start(options: ServerOptions) -> Result<Self, ServerError> {
        let state = ServerState::new(options).await?;
        let listen_addr = state.options().listen_addr.clone();
        let listener = TcpListener::bind(&listen_addr)
            .await
            .map_err(|err| ServerError::Server(format!("failed to bind {listen_addr}: {err}")))?;
        let bound = listener
            .local_addr()
            .map_err(|err| ServerError::Server(err.to_string()))?;

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

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
                let _ = shutdown_rx.changed().await;
            });
            tracing::info!(addr = %bound, "fxg server listening");
            if let Err(err) = server.await {
                tracing::error!("fxg server stopped: {err}");
            }
        });

        // Request shutdown on Ctrl-C
        let signal_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!("received Ctrl-C, shutting down");
                let _ = signal_tx.send(true);
            }
        });

        tracing::info!(
            addr = %bound,
            fxg_home = %server_state.options().fxg_home.display(),
            "fxg server started"
        );

        Ok(Self {
            state,
            listen_addr: bound,
            shutdown_tx,
            task,
        })
    }

    /// Shared state.
    pub fn state(&self) -> &ServerState {
        &self.state
    }

    /// The address actually bound.
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// Requests shutdown.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Requests shutdown and waits for the server task to finish.
    pub async fn wait(self) {
        let mut receiver = self.shutdown_tx.subscribe();
        // `watch` receivers start with the value at creation marked as "seen", so check the
        // current value first to see whether `shutdown()` already ran, then wait for a
        // request (from Ctrl-C or otherwise).
        if !*receiver.borrow_and_update() {
            let _ = receiver.changed().await;
        }
        let _ = self.shutdown_tx.send(true);

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
        tracing::info!("fxg server stopped");
    }

    /// Starts the server and runs until shutdown (for the CLI entry point).
    pub async fn run(options: ServerOptions) -> Result<(), ServerError> {
        let server = Self::start(options).await?;
        server.wait().await;
        Ok(())
    }
}
