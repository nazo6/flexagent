//! Shared state and Client API backend implementation for the hub (`fxg hub`).
//!
//! Design: `docs/01-architecture-and-sync.md` §2.2 and §3.1, `docs/03-protocol-and-api.md` §3.
//!
//! The hub holds only a **projection** of session state (`hub.db`); write
//! authority belongs to the executing node. Client operations are relayed to the target node
//! through [`NodeHub`] and responses are returned correlated by `command_id`.
//!
//! - this module: `HubState` core (construction, DB / hub / push accessors, audit)
//! - `settings`: hub settings, secrets and project profiles
//! - `sessions`: session lifecycle operations and their resolution helpers
//! - `api_impl`: `ClientApiBackend` implementation (relay, PTY, server-only operations)

use std::path::PathBuf;
use std::sync::Arc;

use fxg_db::{Db, DbRole};
use fxg_protocol::node_hub::{NodeRpcRequest, NodeRpcResponse};
use tokio::sync::{broadcast, watch};

use crate::api::{ApiError, ClientApiBackend, ClientEvent, ClientInfo, auth};
use crate::error::HubError;
use crate::hub::NodeHub;
use crate::provisioner::ProvisionerManager;
use crate::push::PushService;

use fxg_protocol::NodeId;

/// Broadcast capacity for Client WS / Node Hub delivery.
const CLIENT_EVENT_CAPACITY: usize = 2048;

/// Startup options for the hub.
#[derive(Debug, Clone)]
pub struct HubOptions {
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

/// Shared state for the hub.
#[derive(Clone)]
pub struct HubState {
    inner: Arc<HubInner>,
}

struct HubInner {
    db: Db,
    /// Client auth token (`RwLock` so it can be swapped on rotation).
    auth_token: std::sync::RwLock<String>,
    hub: NodeHub,
    options: HubOptions,
    client_events: broadcast::Sender<ClientEvent>,
    push: PushService,
    provisioners: ProvisionerManager,
    diagnostics_counters: std::sync::Arc<crate::api::DiagnosticsCounters>,
    /// Shutdown sender used by `POST /api/v1/system/shutdown` (set at startup)
    shutdown_tx: std::sync::OnceLock<watch::Sender<bool>>,
}

impl std::fmt::Debug for HubState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubState")
            .field("listen_addr", &self.inner.options.listen_addr)
            .finish()
    }
}

impl HubState {
    /// Registers the shutdown sender used by `POST /api/v1/system/shutdown`.
    pub(crate) fn set_shutdown_sender(&self, sender: watch::Sender<bool>) {
        fxg_protocol::discard_with!(
            self.inner.shutdown_tx.set(sender),
            "register the hub shutdown sender"
        );
    }

    /// Requests a graceful hub shutdown.
    pub(crate) fn request_shutdown(&self) {
        if let Some(sender) = self.inner.shutdown_tx.get() {
            fxg_protocol::discard_with!(sender.send(true), "request hub shutdown from the API");
        }
    }

    /// Opens `hub.db`, loads the client auth token and initializes the state.
    pub async fn new(options: HubOptions) -> Result<Self, HubError> {
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
            .map_err(|err| HubError::Hub(format!("web push init failed: {err}")))?;
        let hub_settings = db.ensure_hub_settings().await?;
        push.set_subject(hub_settings.vapid_subject.as_deref());
        let provisioners = ProvisionerManager::new(
            options.fxg_home.clone(),
            db.provisioners().await?,
            hub_settings.git_credentials.clone(),
        );
        Ok(Self {
            inner: Arc::new(HubInner {
                db,
                auth_token: std::sync::RwLock::new(auth_token),
                hub,
                options,
                client_events,
                push,
                provisioners,
                diagnostics_counters: std::sync::Arc::new(crate::api::DiagnosticsCounters::new()),
                shutdown_tx: std::sync::OnceLock::new(),
            }),
        })
    }

    /// Temporary VM provisioner management.
    pub fn provisioners(&self) -> &ProvisionerManager {
        &self.inner.provisioners
    }

    /// `hub.db`.
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

    /// Process-local delivery counters exposed by diagnostics.
    pub fn diagnostics_counters(&self) -> &crate::api::DiagnosticsCounters {
        &self.inner.diagnostics_counters
    }

    /// Shared counter handle for spawned tasks.
    pub fn diagnostics_counters_arc(&self) -> std::sync::Arc<crate::api::DiagnosticsCounters> {
        std::sync::Arc::clone(&self.inner.diagnostics_counters)
    }

    /// Startup options.
    pub fn options(&self) -> &HubOptions {
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

    /// Records an audit log entry in `hub.db.audit_logs` (failures only warn).
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
