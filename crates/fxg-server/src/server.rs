//! `fxg server`: 中央サーバーの起動エントリポイント。
//!
//! ルーター構成:
//! - Client REST / Client WS / PTY WS (`fxg-server::api` の共通ルーター。
//!   ローカルノードと同一パス)
//! - Node Hub (`/api/v1/node/ws`。ノード個別トークン認証)

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

/// 起動済みの中央サーバー。
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
    /// サーバーを起動する (`server.db` を開き、HTTP/WS のリッスンを開始)。
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

        // Client API (ノードと同一の共通ルーター)
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

        // Ctrl-C でシャットダウンを要求する
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

    /// 共有状態。
    pub fn state(&self) -> &ServerState {
        &self.state
    }

    /// 実際にバインドされたアドレス。
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// シャットダウンを要求する。
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// シャットダウン要求を送り、サーバータスクの終了を待つ。
    pub async fn wait(self) {
        let mut receiver = self.shutdown_tx.subscribe();
        // `watch` の受信者は生成時点の値を「既読」として開始するため、
        // 先に `shutdown()` 済みかどうかを現在値で確認してから待機する
        // (未要求なら Ctrl-C 等による要求まで待つ)。
        if !*receiver.borrow_and_update() {
            let _ = receiver.changed().await;
        }
        let _ = self.shutdown_tx.send(true);
        let _ = self.task.await;
    }

    /// サーバーを起動し、シャットダウンまで稼働する (CLI エントリポイント用)。
    pub async fn run(options: ServerOptions) -> Result<(), ServerError> {
        let server = Self::start(options).await?;
        server.wait().await;
        Ok(())
    }
}
