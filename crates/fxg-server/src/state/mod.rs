//! Shared state and Client API backend implementation for the central server (`fxg server`).
//!
//! Design: `docs/01-architecture-and-sync.md` §2.2 and §3.1, `docs/03-protocol-and-api.md` §3.
//!
//! The central server holds only a **projection** of session state (`server.db`); write
//! authority belongs to the executing node. Client operations are relayed to the target node
//! through [`NodeHub`] and responses are returned correlated by `command_id`.
//!
//! - this module: `ServerState` core (construction, DB / hub / push accessors, audit)
//! - `settings`: server settings, secrets and project profiles
//! - `sessions`: session lifecycle operations and their resolution helpers
//! - `api_impl`: `ClientApiBackend` implementation (relay, PTY, server-only operations)

use std::path::PathBuf;
use std::sync::Arc;

use fxg_db::{Db, DbRole};
use fxg_protocol::node_server::{NodeRpcRequest, NodeRpcResponse};
use tokio::sync::broadcast;

use crate::api::{ApiError, ClientApiBackend, ClientEvent, ClientInfo, auth};
use crate::error::ServerError;
use crate::hub::NodeHub;
use crate::provisioner::ProvisionerManager;
use crate::push::PushService;

use fxg_protocol::NodeId;

/// Broadcast capacity for Client WS / Node Hub delivery.
const CLIENT_EVENT_CAPACITY: usize = 2048;

/// Startup options for the central server.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Data directory (`~/.flexagent`)
    pub fxg_home: PathBuf,
    /// HTTP/WS bind address (default `0.0.0.0:8080`)
    pub listen_addr: String,
    /// Hosts allowed in addition to `localhost` / `127.0.0.1`
    pub allowed_hosts: Vec<String>,
    /// Origins allowed in addition to the same origin
    pub allowed_origins: Vec<String>,
    /// Effective logging settings (for the read-only settings catalog)
    pub log: fxg_protocol::config::LogConfig,
}

/// Shared state for the central server.
#[derive(Clone)]
pub struct ServerState {
    inner: Arc<ServerInner>,
}

struct ServerInner {
    db: Db,
    /// Client auth token (`RwLock` so it can be swapped on rotation).
    auth_token: std::sync::RwLock<String>,
    hub: NodeHub,
    options: ServerOptions,
    client_events: broadcast::Sender<ClientEvent>,
    push: PushService,
    provisioners: ProvisionerManager,
}

impl std::fmt::Debug for ServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerState")
            .field("listen_addr", &self.inner.options.listen_addr)
            .finish()
    }
}

impl ServerState {
    /// Opens `server.db`, loads the client auth token and initializes the state.
    pub async fn new(options: ServerOptions) -> Result<Self, ServerError> {
        std::fs::create_dir_all(&options.fxg_home)?;
        let db = Db::open(&fxg_db::hub_db_path(&options.fxg_home), DbRole::Hub).await?;
        let auth_token = auth::load_or_create_token(
            &options
                .fxg_home
                .join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME),
        )?;
        let (client_events, _) = broadcast::channel(CLIENT_EVENT_CAPACITY);
        let hub = NodeHub::new(db.clone(), client_events.clone());
        let push = PushService::load_or_create(&options.fxg_home)
            .map_err(|err| ServerError::Server(format!("web push init failed: {err}")))?;
        let server_settings = db.ensure_server_settings().await?;
        push.set_subject(server_settings.vapid_subject.as_deref());
        let provisioners = ProvisionerManager::new(
            options.fxg_home.clone(),
            db.provisioners().await?,
            server_settings.git_credentials.clone(),
        );
        Ok(Self {
            inner: Arc::new(ServerInner {
                db,
                auth_token: std::sync::RwLock::new(auth_token),
                hub,
                options,
                client_events,
                push,
                provisioners,
            }),
        })
    }

    /// Temporary VM provisioner management.
    pub fn provisioners(&self) -> &ProvisionerManager {
        &self.inner.provisioners
    }

    /// `server.db`.
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// Node connection registry.
    pub fn hub(&self) -> &NodeHub {
        &self.inner.hub
    }

    /// Executes a generic node RPC with a fresh correlation ID.
    async fn node_rpc(
        &self,
        node_id: &NodeId,
        request: NodeRpcRequest,
    ) -> Result<NodeRpcResponse, ApiError> {
        self.execute_node_rpc(node_id, request, None).await
    }

    /// VAPID Web Push delivery service.
    pub fn push(&self) -> &PushService {
        &self.inner.push
    }

    /// Startup options.
    pub fn options(&self) -> &ServerOptions {
        &self.inner.options
    }

    /// Client auth token.
    pub fn auth_token(&self) -> String {
        self.inner
            .auth_token
            .read()
            .expect("auth token poisoned")
            .clone()
    }

    /// Regenerates the client auth token and saves it to disk.
    ///
    /// The old token (Bearer / `fxg_session` cookie) is invalidated immediately.
    pub fn rotate_auth_token(&self) -> std::io::Result<String> {
        let token = auth::generate_token();
        auth::save_token(
            &self
                .inner
                .options
                .fxg_home
                .join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME),
            &token,
        )?;
        *self.inner.auth_token.write().expect("auth token poisoned") = token.clone();
        Ok(token)
    }

    /// Records an audit log entry in `server.db.audit_logs` (failures only warn).
    pub async fn record_audit(
        &self,
        action: fxg_protocol::common::AuditAction,
        client: &ClientInfo,
        session_id: Option<&str>,
        details: impl Into<fxg_protocol::common::RawJson>,
    ) {
        let audit =
            fxg_db::AuditLogRecord::new(action, client.ip.clone(), client.auth_subject.clone())
                .with_session(session_id)
                .with_user_agent(client.user_agent.as_deref())
                .with_details(details);
        self.inner.db.record_audit(&audit).await;
    }
}

mod api_impl;
mod sessions;
mod settings;
