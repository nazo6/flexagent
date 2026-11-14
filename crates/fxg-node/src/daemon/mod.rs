//! Node daemon (`fxg node start`).
//!
//! Design: `docs/01-architecture-and-sync.md` §1, §7; `docs/03-protocol-and-api.md` §3, §4.
//!
//! - **Local HTTP/WS server** (strictly bound to `127.0.0.1:7860`):
//!   serves the client API (REST + Client WS) ([`http`])
//! - **Authentication**: Bearer / Cookie authentication via `<data>/node/auth_token` ([`auth`])

mod api;
mod auth;
mod http;
mod ops;
mod pairing;
mod stdio;

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use fxg_acp::agent::AgentCatalog;
use fxg_acp::registry::AcpRegistry;
use fxg_db::Db;
use fxg_hub::api::ClientEvent;
use fxg_hub::push::{PushService, RunningTurns};
use fxg_protocol::client_api::{HubLinkState, HubLinkStatus};
use fxg_protocol::paths::{Paths, Role};
use fxg_protocol::settings::{AgentSettings, NodeSettings};
use fxg_protocol::util::now_ms;
use fxg_pty::PtySessionManager;
use tokio::sync::{broadcast, watch};

use crate::error::NodeError;
use crate::session::SessionEventBus;
use crate::session_manager::{DriverFactory, SessionManager};

/// Saves a token file with owner-only permissions (`0600` where supported).
pub use auth::save as save_token_file;
pub use auth::{generate_token, token_matches};
pub use ops::{AuditSource, WorktreeAddOutcome};
pub use stdio::{StdioConfig, run_stdio};

/// The `fxg` version (this crate's Cargo package version = workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Broadcast buffer for events delivered to Client WS.
const CLIENT_EVENT_CAPACITY: usize = 2048;

/// Node daemon startup configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    /// Node ID (auto-resolved from the DB or hostname when omitted)
    pub node_id: String,
    /// Display name
    pub node_name: String,
    /// Bind address of the local HTTP/WS server (default `127.0.0.1:7860`)
    pub listen_addr: String,
    /// Whether `listen_addr` came from an explicit configuration (`--listen` /
    /// `[node] listen_addr` / `FXG_NODE_LISTEN_ADDR`).
    ///
    /// When false and the default port is already in use, the daemon falls back
    /// to an ephemeral loopback port.
    pub listen_addr_explicit: bool,
    /// Hosts allowed in addition to the loopback family (`[node] allowed_hosts`)
    pub allowed_hosts: Vec<String>,
    /// Origins allowed in addition to the same origin (`[node] allowed_origins`)
    pub allowed_origins: Vec<String>,
    /// Whether this is an ephemeral VM / sandbox node (`--ephemeral`; reflected in
    /// NodeHello and the `nodes` row)
    pub ephemeral: bool,
    /// Role-scoped config/data/state/cache/log paths
    pub paths: Paths,
    /// Effective logging settings (for the read-only settings catalog)
    pub log: fxg_protocol::config::LogConfig,
}

impl DaemonConfig {
    /// Builds the default configuration (`node_id` / `node_name` are values resolved by the caller).
    pub fn new(paths: Paths, node_id: impl Into<String>, node_name: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            node_name: node_name.into(),
            listen_addr: fxg_protocol::config::NodeConfig::DEFAULT_LISTEN_ADDR.to_owned(),
            listen_addr_explicit: false,
            allowed_hosts: Vec::new(),
            allowed_origins: Vec::new(),
            ephemeral: false,
            paths,
            log: fxg_protocol::config::LogConfig::default(),
        }
    }

    /// Builds the configuration by resolving the role paths from the environment.
    pub fn from_env(env: fxg_protocol::config::EnvLookup<'_>) -> Self {
        Self {
            node_id: String::new(),
            node_name: hostname(),
            listen_addr: fxg_protocol::config::NodeConfig::DEFAULT_LISTEN_ADDR.to_owned(),
            listen_addr_explicit: false,
            allowed_hosts: Vec::new(),
            allowed_origins: Vec::new(),
            ephemeral: false,
            paths: Paths::resolve(env, Role::Node),
            log: fxg_protocol::config::LogConfig::default(),
        }
    }
}

/// Returns the same host with an ephemeral port (falls back to loopback).
fn ephemeral_addr(listen_addr: &str) -> String {
    listen_addr
        .parse::<SocketAddr>()
        .map(|addr| {
            let mut addr = addr;
            addr.set_port(0);
            addr.to_string()
        })
        .unwrap_or_else(|_| "127.0.0.1:0".to_owned())
}

/// Reconciles the state of sessions lost along with the previous process on daemon restart.
///
/// Sessions still marked active (`provisioning` to `waiting_permission`) have no
/// agent process (it exited with the daemon), so they are recorded again as
/// `stopped` (so that ghost sessions do not linger in `fxg session list`).
async fn reconcile_stale_sessions(bus: &SessionEventBus) -> Result<(), NodeError> {
    use fxg_db::SessionFilter;
    use fxg_protocol::common::SessionStatus;
    use fxg_protocol::events::UnifiedEventPayload;

    let filter = SessionFilter {
        statuses: DaemonState::active_statuses(),
        // Align even archived sessions to stopped if they are active (leave no ghosts)
        include_archived: true,
        ..SessionFilter::default()
    };
    for session in bus.db().list_sessions(&filter).await? {
        tracing::info!(
            session_id = %session.session_id,
            "marking session as stopped (daemon restarted)"
        );
        bus.record(
            session.session_id.as_str(),
            UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Stopped,
                error_message: None,
            },
        )
        .await?;
    }
    Ok(())
}

/// Returns the hostname-based node display name.
pub fn hostname() -> String {
    for variable in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(variable)
            && !value.trim().is_empty()
        {
            return value;
        }
    }
    match std::fs::read_to_string("/etc/hostname") {
        Ok(value) => {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_owned();
            }
        }
        Err(err) => tracing::debug!("failed to read /etc/hostname: {err}"),
    }
    "fxg-node".to_owned()
}

/// Auto-assigns a node ID from a hostname-based slug plus a short ID.
///
/// If a row for this node already exists in the `nodes` table it is reused
/// (resolved inside [`DaemonState::new`]), so this function is only used on first startup.
pub fn default_node_id() -> String {
    let host = hostname();
    let slug: String = host
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').replace("--", "-");
    let slug = if slug.is_empty() {
        "node".to_owned()
    } else {
        slug
    };
    // The leading part of a UUID v7 is a timestamp, so it cannot be used as a short ID.
    // Generate from randomness.
    let short_id = format!("{:08x}", rand::random::<u32>());
    format!("{slug}-{short_id}")
}

struct Inner {
    db: Db,
    bus: SessionEventBus,
    pty: PtySessionManager,
    sessions: SessionManager,
    paths: Paths,
    config: DaemonConfig,
    /// Node-owned settings (`node_settings` in `node.db`)
    node_settings: RwLock<NodeSettings>,
    /// Agent settings (`agent_settings` in `node.db`)
    agent_settings: RwLock<AgentSettings>,
    token: RwLock<String>,
    started_at: i64,
    /// Normalized events delivered to Client WS (forwarded from the bus)
    client_events: broadcast::Sender<ClientEvent>,
    /// VAPID Web Push sender for standalone / direct-node subscriptions
    push: PushService,
    /// Tracks turns observed as `Running` for turn-finished Web Push notifications
    running_turns: RunningTurns,
    /// PTY attachments held by the current hub connection (`pty_id` -> clients).
    ///
    /// The hub cannot send a detach when its link drops, so the node resets these
    /// counts when the connection ends.
    hub_pty_attachments: std::sync::Mutex<std::collections::HashMap<String, usize>>,
    /// Node ⇔ hub link state (outbox sync worker; `watch` also feeds Client WS)
    hub_link: watch::Sender<HubLinkStatus>,
    /// Connection target of the outbox sync worker (`None` = standalone)
    hub_target: watch::Sender<Option<crate::sync::HubTarget>>,
    /// Whether the last `NodeHello` hit the session reporting limit
    sessions_truncated: std::sync::atomic::AtomicBool,
    /// `NodeHello` resend trigger (on worktree changes / project registration)
    hello_refresh: tokio::sync::Notify,

    /// Process-local delivery counters exposed by diagnostics
    diagnostics_counters: std::sync::Arc<fxg_hub::api::DiagnosticsCounters>,
    /// Shutdown sender used by `POST /api/v1/system/shutdown` (set at startup)
    shutdown_tx: std::sync::OnceLock<watch::Sender<bool>>,
}

/// Shared daemon state (shared by HTTP handlers / the CLI).
#[derive(Clone)]
pub struct DaemonState {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DaemonState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonState")
            .field("node_id", &self.inner.config.node_id)
            .field("listen_addr", &self.inner.config.listen_addr)
            .finish()
    }
}

impl DaemonState {
    /// Registers the shutdown sender used by the local shutdown endpoint.
    pub(crate) fn set_shutdown_sender(&self, sender: watch::Sender<bool>) {
        fxg_protocol::discard_with!(
            self.inner.shutdown_tx.set(sender),
            "register the node shutdown sender"
        );
    }

    /// Requests a graceful node shutdown (`POST /api/v1/system/shutdown` / `fxg node stop`).
    pub(crate) fn request_shutdown(&self) {
        if let Some(sender) = self.inner.shutdown_tx.get() {
            fxg_protocol::discard_with!(sender.send(true), "request node shutdown from the API");
        }
    }

    /// Initializes the state (creates the data directory + loads the auth token +
    /// resolves the node ID).
    pub async fn new(config: DaemonConfig) -> Result<Self, NodeError> {
        Self::build(config, None).await
    }

    /// Initializes the state with a replaced agent driver factory.
    ///
    /// The production path is [`Self::new`]. Tests inject a mock driver to verify
    /// without real processes.
    #[doc(hidden)]
    pub async fn with_driver_factory(
        config: DaemonConfig,
        driver_factory: DriverFactory,
    ) -> Result<Self, NodeError> {
        Self::build(config, Some(driver_factory)).await
    }

    async fn build(
        mut config: DaemonConfig,
        driver_factory: Option<DriverFactory>,
    ) -> Result<Self, NodeError> {
        let paths = config.paths.clone();
        paths.ensure_dirs()?;
        let db = Db::open(&paths.db_path(), Role::Node).await?;

        // Node ID: resolve in the order configured value → existing DB row → new auto-assigned ID
        if config.node_id.trim().is_empty() {
            config.node_id = match db.list_nodes().await?.first() {
                Some(existing) => existing.node_id.as_str().to_owned(),
                None => default_node_id(),
            };
        }
        if config.node_name.trim().is_empty() {
            config.node_name = hostname();
        }

        let token = auth::load_or_create(&paths.auth_token_path())?;
        let bus = SessionEventBus::new(db.clone()).with_node_id(config.node_id.clone());
        // Reconcile the state of sessions lost when the previous daemon exited
        reconcile_stale_sessions(&bus).await?;

        // Runtime settings are owned by `node.db`; `node.toml` only bootstraps the process.
        let node_settings = db.ensure_node_settings(&config.node_name).await?;
        config.node_name = node_settings.name.clone();
        let push = PushService::load_or_create(&paths)
            .map_err(|err| NodeError::InvalidState(format!("web push init failed: {err}")))?;
        push.set_subject(node_settings.vapid_subject.as_deref());
        let running_turns = RunningTurns::default();
        let agent_settings = db.ensure_agent_settings().await?;
        let catalog = AgentCatalog::new(AcpRegistry::new(
            paths.clone(),
            &agent_settings,
            db.custom_agents().await?,
        ));
        let sessions = match driver_factory {
            Some(factory) => {
                SessionManager::with_factory(bus.clone(), config.node_id.clone(), catalog, factory)
            }
            None => SessionManager::new(bus.clone(), config.node_id.clone(), catalog),
        };

        // Pairing is owned by `node.db`; the supervisor reports the real state
        // (`Connecting`, or `Failed` when the token is missing).
        let pairing = db.ensure_node_pairing().await?;
        let initial_hub_link = match pairing.hub_url.as_deref() {
            Some(url) => HubLinkStatus::new(
                HubLinkState::Connecting,
                Some(fxg_protocol::config::hub_http_origin(url)),
                None,
            ),
            None => HubLinkStatus::standalone(),
        };

        let (client_events, _) = broadcast::channel(CLIENT_EVENT_CAPACITY);
        let state = Self {
            inner: Arc::new(Inner {
                db,
                bus,
                pty: PtySessionManager::new(),
                sessions,
                paths,
                config,
                node_settings: RwLock::new(node_settings),
                agent_settings: RwLock::new(agent_settings),
                token: RwLock::new(token),
                started_at: now_ms(),
                client_events,
                push,
                running_turns,
                hub_pty_attachments: std::sync::Mutex::new(std::collections::HashMap::new()),
                hub_link: watch::channel(initial_hub_link).0,
                hub_target: watch::channel(None).0,
                sessions_truncated: std::sync::atomic::AtomicBool::new(false),
                hello_refresh: tokio::sync::Notify::new(),
                diagnostics_counters: std::sync::Arc::new(fxg_hub::api::DiagnosticsCounters::new()),
                shutdown_tx: std::sync::OnceLock::new(),
            }),
        };

        // Forward session event bus events to the events delivered to Client WS
        // (bus Lagged is recovered by the Client WS loop replaying from the cursor)
        let mut bus_rx = state.inner.bus.subscribe();
        let client_tx = state.inner.client_events.clone();
        let counters = state.diagnostics_counters_arc();
        let push = state.inner.push.clone();
        let push_db = state.inner.db.clone();
        let running_turns = state.inner.running_turns.clone();
        fxg_protocol::spawn_guarded!("local client event forwarder", async move {
            loop {
                match bus_rx.recv().await {
                    Ok(crate::session::SessionBroadcast::Persisted { event, cursor }) => {
                        fxg_hub::push::dispatch_notifications(
                            &push,
                            &push_db,
                            &running_turns,
                            std::slice::from_ref(&event),
                            false,
                            Some(counters.clone()),
                        );
                        fxg_protocol::discard_with!(
                            client_tx.send(ClientEvent::Persisted {
                                event: Box::new(event),
                                cursor,
                            }),
                            "broadcast an event to local clients"
                        );
                    }
                    Ok(crate::session::SessionBroadcast::StreamDelta { session_id, delta }) => {
                        fxg_protocol::discard_with!(
                            client_tx.send(ClientEvent::StreamDelta { session_id, delta }),
                            "broadcast an event to local clients"
                        );
                    }
                    // Ephemeral events (keystrokes etc.) are delivered over the PTY WS
                    Ok(crate::session::SessionBroadcast::Ephemeral(_)) => {}
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        counters.add(fxg_hub::api::CounterKind::DroppedEvents, skipped);
                        counters.incr(fxg_hub::api::CounterKind::LaggedBroadcasts);
                        tracing::debug!(skipped, "client event forwarder lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(state)
    }

    /// The local node DB.
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// Session event bus.
    pub fn bus(&self) -> &SessionEventBus {
        &self.inner.bus
    }

    /// Local PTY manager.
    pub fn pty(&self) -> &PtySessionManager {
        &self.inner.pty
    }

    /// Agent session manager.
    pub fn session_manager(&self) -> &SessionManager {
        &self.inner.sessions
    }

    /// Role-scoped path resolution.
    pub fn paths(&self) -> &Paths {
        &self.inner.paths
    }

    /// Process-local delivery counters exposed by diagnostics.
    pub fn diagnostics_counters(&self) -> &fxg_hub::api::DiagnosticsCounters {
        &self.inner.diagnostics_counters
    }

    /// Shared counter handle for spawned tasks.
    pub fn diagnostics_counters_arc(&self) -> std::sync::Arc<fxg_hub::api::DiagnosticsCounters> {
        std::sync::Arc::clone(&self.inner.diagnostics_counters)
    }

    /// Startup configuration.
    pub fn config(&self) -> &DaemonConfig {
        &self.inner.config
    }

    /// The fxg version.
    pub fn version(&self) -> &'static str {
        VERSION
    }

    /// Current node-owned settings snapshot.
    pub fn node_settings(&self) -> NodeSettings {
        self.inner
            .node_settings
            .read()
            .expect("node settings lock poisoned")
            .clone()
    }

    /// Current agent settings snapshot.
    pub fn agent_settings(&self) -> AgentSettings {
        self.inner
            .agent_settings
            .read()
            .expect("agent settings lock poisoned")
            .clone()
    }

    /// Updates node settings under an optimistic lock and reports the new revision.
    pub async fn update_node_settings(
        &self,
        request: &fxg_protocol::settings::UpdateNodeSettingsRequest,
    ) -> Result<NodeSettings, NodeError> {
        let settings = self.inner.db.update_node_settings(request).await?;
        *self
            .inner
            .node_settings
            .write()
            .expect("node settings lock poisoned") = settings.clone();
        self.inner
            .push
            .set_subject(settings.vapid_subject.as_deref());
        self.bump_state_revision().await?;
        Ok(settings)
    }

    /// Updates agent settings under an optimistic lock, refreshes the registry catalog
    /// and reports the new revision.
    pub async fn update_agent_settings(
        &self,
        request: &fxg_protocol::settings::UpdateAgentSettingsRequest,
    ) -> Result<AgentSettings, NodeError> {
        let settings = self.inner.db.update_agent_settings(request).await?;
        *self
            .inner
            .agent_settings
            .write()
            .expect("agent settings lock poisoned") = settings.clone();
        self.refresh_agent_catalog().await?;
        self.bump_state_revision().await?;
        Ok(settings)
    }

    /// Reloads custom agents from `node.db` into the registry.
    pub async fn refresh_agent_catalog(&self) -> Result<(), NodeError> {
        let settings = self.inner.db.ensure_agent_settings().await?;
        let custom = self.inner.db.custom_agents().await?;
        self.inner
            .sessions
            .catalog()
            .apply_catalog(&settings, custom);
        *self
            .inner
            .agent_settings
            .write()
            .expect("agent settings lock poisoned") = settings;
        Ok(())
    }

    /// Advances `nodes.state_revision` and asks the sync worker to re-send `NodeHello`.
    pub async fn bump_state_revision(&self) -> Result<(), NodeError> {
        self.inner
            .db
            .bump_node_state_revision(self.node_id())
            .await?;
        self.trigger_node_hello();
        Ok(())
    }

    /// Current `nodes.state_revision` of this node.
    pub async fn state_revision(&self) -> Result<u64, NodeError> {
        Ok(self
            .inner
            .db
            .node_state_revision(self.node_id())
            .await?
            .unwrap_or(0))
    }

    /// Milliseconds elapsed since startup.
    pub fn uptime_ms(&self) -> i64 {
        now_ms().saturating_sub(self.inner.started_at)
    }

    /// VAPID Web Push sender.
    pub(crate) fn push(&self) -> &PushService {
        &self.inner.push
    }

    /// Current node ⇔ hub link state.
    pub fn hub_link(&self) -> HubLinkStatus {
        self.inner.hub_link.borrow().clone()
    }

    /// Subscribes to node ⇔ hub link state changes (Client WS notifications).
    pub fn hub_link_updates(&self) -> watch::Receiver<HubLinkStatus> {
        self.inner.hub_link.subscribe()
    }

    /// Replaces the node ⇔ hub link state (outbox sync worker only).
    pub fn set_hub_link(&self, status: HubLinkStatus) {
        self.inner.hub_link.send_replace(status);
    }

    /// Current outbox sync target (`None` while standalone).
    pub fn hub_target(&self) -> Option<crate::sync::HubTarget> {
        self.inner.hub_target.borrow().clone()
    }

    /// Subscribes to pairing changes (the supervisor reconnects on every change).
    pub fn hub_target_updates(&self) -> watch::Receiver<Option<crate::sync::HubTarget>> {
        self.inner.hub_target.subscribe()
    }

    /// Replaces the outbox sync target (pairing only).
    pub(crate) fn set_hub_target(&self, target: Option<crate::sync::HubTarget>) {
        self.inner.hub_target.send_replace(target);
    }

    /// Whether the last `NodeHello` hit the session reporting limit.
    pub fn sessions_truncated(&self) -> bool {
        self.inner
            .sessions_truncated
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Records whether the last `NodeHello` hit the session reporting limit.
    pub(crate) fn set_sessions_truncated(&self, truncated: bool) {
        self.inner
            .sessions_truncated
            .store(truncated, std::sync::atomic::Ordering::Relaxed);
    }

    /// Requests a resend of `NodeHello` (reports project bindings on worktree changes etc.).
    pub fn trigger_node_hello(&self) {
        self.inner.hello_refresh.notify_one();
    }

    /// Receives notifications of the `NodeHello` resend trigger (sync worker only).
    pub(crate) fn hello_refresh(&self) -> &tokio::sync::Notify {
        &self.inner.hello_refresh
    }

    /// The node ID.
    pub fn node_id(&self) -> &str {
        &self.inner.config.node_id
    }

    /// The current Web access token.
    pub fn auth_token(&self) -> String {
        self.inner
            .token
            .read()
            .expect("auth token lock poisoned")
            .clone()
    }

    /// Regenerates the auth token and updates the file (`0600`) and the in-memory value.
    pub fn rotate_auth_token(&self) -> Result<String, NodeError> {
        let token = auth::generate_token();
        auth::save(&self.inner.paths.auth_token_path(), &token)?;
        *self.inner.token.write().expect("auth token lock poisoned") = token.clone();
        tracing::info!("auth token rotated");
        Ok(token)
    }

    /// Registers this node in the `nodes` table (upsert with online state).
    pub async fn register_node(&self) -> Result<(), NodeError> {
        let revision = self
            .inner
            .db
            .node_state_revision(self.node_id())
            .await?
            .unwrap_or(0);
        self.register_node_with_revision(revision).await
    }

    /// Registers this node with an explicit `state_revision`.
    pub async fn register_node_with_revision(&self, state_revision: u64) -> Result<(), NodeError> {
        let record = fxg_db::NodeRecord {
            node_id: self.inner.config.node_id.clone(),
            name: self.node_settings().name,
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            version: VERSION.to_owned(),
            installed_agents: self.installed_agents().await,
            is_ephemeral: self.inner.config.ephemeral,
            provisioner: None,
            lifecycle_status: fxg_protocol::common::NodeLifecycleStatus::Ready,
            idle_timeout_secs: None,
            is_online: true,
            state_revision,
        };
        self.inner.db.upsert_node(&record).await?;
        Ok(())
    }

    /// List of available agent IDs (built-in CLIs on PATH + custom definitions).
    pub async fn installed_agents(&self) -> Vec<String> {
        let catalog = self.inner.sessions.catalog();
        let mut ids: Vec<String> = catalog.list().into_iter().map(|entry| entry.id).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Records the node as offline (on exit).
    pub async fn mark_offline(&self) -> Result<(), NodeError> {
        self.inner
            .db
            .set_node_online(&self.inner.config.node_id, false)
            .await?;
        Ok(())
    }

    /// List of session statuses considered locally active (the default filter for `fxg session list`).
    pub fn active_statuses() -> Vec<fxg_protocol::common::SessionStatus> {
        use fxg_protocol::common::SessionStatus;
        vec![
            SessionStatus::Provisioning,
            SessionStatus::Idle,
            SessionStatus::Running,
            SessionStatus::WaitingPermission,
            SessionStatus::WaitingInput,
        ]
    }

    /// Records that the current hub connection attached a client to a PTY.
    pub(crate) fn track_hub_pty_attach(&self, pty_id: &str) {
        let mut attachments = self
            .inner
            .hub_pty_attachments
            .lock()
            .expect("hub pty attachments poisoned");
        *attachments.entry(pty_id.to_owned()).or_default() += 1;
    }

    /// Records that a hub client detached from a PTY.
    pub(crate) fn track_hub_pty_detach(&self, pty_id: &str) {
        let mut attachments = self
            .inner
            .hub_pty_attachments
            .lock()
            .expect("hub pty attachments poisoned");
        let Some(count) = attachments.get_mut(pty_id) else {
            return;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            attachments.remove(pty_id);
        }
    }

    /// Forgets the attachments of a PTY that no longer exists.
    pub(crate) fn forget_hub_pty(&self, pty_id: &str) {
        self.inner
            .hub_pty_attachments
            .lock()
            .expect("hub pty attachments poisoned")
            .remove(pty_id);
    }

    /// Detaches every client held by the (now closed) hub connection.
    pub(crate) fn detach_hub_ptys(&self) {
        let attachments = std::mem::take(
            &mut *self
                .inner
                .hub_pty_attachments
                .lock()
                .expect("hub pty attachments poisoned"),
        );
        for (pty_id, count) in attachments {
            for _ in 0..count {
                self.inner.pty.detach(&pty_id);
            }
        }
    }

    /// Forcefully terminates all sessions and PTYs on the local node (emergency kill switch).
    ///
    /// - PTYs: [`PtySessionManager::kill_all`] (on Windows, terminates the entire
    ///   process tree via Job Object)
    /// - Agent sessions: [`SessionManager::shutdown_all`]
    ///   (terminates the entire agent process tree via the driver)
    /// - Sessions that were running: emits a `StatusChanged(Stopped)` event
    ///
    /// Recording to the audit log (`audit_logs`) is the responsibility of the
    /// caller (HTTP handler) that holds the client information (IP / route).
    ///
    /// Returns the list of terminated session IDs and PTY IDs.
    pub async fn kill_all_local(
        &self,
        reason: &str,
    ) -> Result<(Vec<String>, Vec<String>), NodeError> {
        use fxg_protocol::common::SessionStatus;
        use fxg_protocol::events::UnifiedEventPayload;

        let killed_ptys = self.inner.pty.kill_all();
        self.inner
            .hub_pty_attachments
            .lock()
            .expect("hub pty attachments poisoned")
            .clear();

        // Stop agent processes (including their process trees).
        let stopped_agents = self.inner.sessions.shutdown_all().await;
        if !stopped_agents.is_empty() {
            tracing::info!(count = stopped_agents.len(), "stopped local agent sessions");
        }

        let sessions = self
            .inner
            .db
            .list_sessions(&fxg_db::SessionFilter {
                statuses: Self::active_statuses(),
                // kill-all also stops archived active sessions
                include_archived: true,
                ..fxg_db::SessionFilter::default()
            })
            .await?;

        let mut killed_sessions = Vec::new();
        for session in sessions {
            match self
                .inner
                .bus
                .record(
                    session.session_id.as_str(),
                    UnifiedEventPayload::StatusChanged {
                        status: SessionStatus::Stopped,
                        error_message: Some(reason.to_owned()),
                    },
                )
                .await
            {
                Ok(_) => killed_sessions.push(session.session_id.to_string()),
                Err(err) => {
                    tracing::warn!("failed to stop session {}: {err}", session.session_id);
                }
            }
        }

        Ok((killed_sessions, killed_ptys))
    }
}

/// Starts the outbox sync supervisor (idle while the node is unpaired).
///
/// The pairing (hub address + token) is owned by `node.db` / the token file and
/// is applied when it changes, so pairing never needs a restart.
fn start_hub_link(
    state: &DaemonState,
    shutdown_tx: &watch::Sender<bool>,
) -> tokio::task::JoinHandle<()> {
    let state = state.clone();
    let shutdown = shutdown_tx.subscribe();
    tokio::spawn(async move {
        crate::sync::supervise(state, shutdown).await;
    })
}

/// A started node daemon.
pub struct NodeDaemon {
    state: DaemonState,
    http_addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    http_task: tokio::task::JoinHandle<()>,
    sync_task: Option<tokio::task::JoinHandle<()>>,
    addr_file: std::path::PathBuf,
}

impl std::fmt::Debug for NodeDaemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeDaemon")
            .field("node_id", &self.state.node_id())
            .field("http_addr", &self.http_addr)
            .finish()
    }
}

/// Shutdown request handle for the daemon.
///
/// Used to request shutdown from another thread / context such as the system
/// tray (obtained via [`NodeDaemon::shutdown_handle`]).
#[derive(Clone, Debug)]
pub struct ShutdownHandle(watch::Sender<bool>);

impl ShutdownHandle {
    /// Requests shutdown (actual stop is awaited via [`NodeDaemon::wait`]).
    pub fn shutdown(&self) {
        fxg_protocol::discard_with!(self.0.send(true), "request node shutdown");
    }
}

impl NodeDaemon {
    /// Starts the daemon (creates the data directory, registers the node,
    /// starts listening on the local HTTP/WS).
    pub async fn start(config: DaemonConfig) -> Result<Self, NodeError> {
        let state = DaemonState::new(config).await?;
        Self::start_state(state).await
    }

    /// Starts the daemon with a replaced agent driver factory
    /// (for tests. The production path is [`Self::start`]).
    #[doc(hidden)]
    pub async fn start_with_driver_factory(
        config: DaemonConfig,
        driver_factory: DriverFactory,
    ) -> Result<Self, NodeError> {
        let state = DaemonState::with_driver_factory(config, driver_factory).await?;
        Self::start_state(state).await
    }

    async fn start_state(state: DaemonState) -> Result<Self, NodeError> {
        state.register_node().await?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        state.set_shutdown_sender(shutdown_tx.clone());

        // Local HTTP/WS server (strict loopback binding). When the default address
        // is already in use and the user did not configure one explicitly, fall
        // back to an ephemeral loopback port (D8).
        let listen_addr = state.config().listen_addr.clone();
        let listener = match tokio::net::TcpListener::bind(&listen_addr).await {
            Ok(listener) => listener,
            Err(err)
                if err.kind() == std::io::ErrorKind::AddrInUse
                    && !state.config().listen_addr_explicit =>
            {
                let fallback = ephemeral_addr(&listen_addr);
                tracing::warn!(
                    requested = %listen_addr,
                    fallback = %fallback,
                    "listen address is in use; falling back to an ephemeral port"
                );
                tokio::net::TcpListener::bind(&fallback)
                    .await
                    .map_err(|err| NodeError::Hub(format!("failed to bind {fallback}: {err}")))?
            }
            Err(err) => {
                return Err(NodeError::Hub(format!(
                    "failed to bind {listen_addr}: {err}"
                )));
            }
        };
        let http_addr = listener
            .local_addr()
            .map_err(|err| NodeError::Hub(err.to_string()))?;

        // Publish the bound address so `fxg web` / `session new` can open the
        // Web UI even when the node was started with a `--listen` override.
        let addr_file = state.paths().addr_path();
        if let Err(err) = std::fs::write(&addr_file, http_addr.to_string()) {
            tracing::warn!(path = %addr_file.display(), "failed to write the node address file: {err}");
        }

        let http_state = state.clone();
        let http_shutdown = shutdown_rx.clone();
        let http_task = tokio::spawn(async move {
            http::serve(http_state, listener, http_shutdown).await;
        });

        // Request shutdown on Ctrl-C
        fxg_hub::spawn_ctrl_c_handler(shutdown_tx.clone());

        // Outbox sync worker (idle while standalone; reconnects when the pairing changes)
        if let Err(err) = state.refresh_hub_target().await {
            tracing::warn!("failed to load the hub pairing; sync stays disabled: {err}");
        }
        let sync_task = Some(start_hub_link(&state, &shutdown_tx));

        tracing::info!(
            node_id = state.config().node_id,
            http = %http_addr,
            "fxg node start started"
        );

        Ok(Self {
            state,
            http_addr,
            shutdown_tx,
            http_task,
            sync_task,
            addr_file,
        })
    }

    /// Shared state.
    pub fn state(&self) -> &DaemonState {
        &self.state
    }

    /// Actual bound address of the local HTTP/WS server.
    pub fn http_addr(&self) -> SocketAddr {
        self.http_addr
    }

    /// Returns a handle for requesting shutdown from another thread / the tray.
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle(self.shutdown_tx.clone())
    }

    /// Requests shutdown (actual stop is awaited via [`Self::wait`]).
    pub fn shutdown(&self) {
        self.shutdown_handle().shutdown();
    }

    /// Waits for a shutdown request, performs cleanup (PTY stop, offline recording), and exits.
    pub async fn wait(self) {
        let mut receiver = self.shutdown_tx.subscribe();
        // A `watch` receiver starts with the value at creation time marked as "seen",
        // so check the current value to see whether `shutdown()` already ran before waiting.
        if !*receiver.borrow_and_update()
            && let Err(err) = receiver.changed().await
        {
            tracing::debug!("shutdown sender dropped while waiting: {err}");
        }
        fxg_protocol::discard_with!(
            self.shutdown_tx.send(true),
            "request node shutdown while waiting"
        );

        let stopped = self.state.session_manager().shutdown_all().await;
        if !stopped.is_empty() {
            tracing::info!(count = stopped.len(), "stopped agent sessions on shutdown");
        }
        let killed = self.state.pty().kill_all();
        if !killed.is_empty() {
            tracing::info!(count = killed.len(), "killed local ptys on shutdown");
        }
        if let Err(err) = self.state.mark_offline().await {
            tracing::warn!("failed to mark node offline: {err}");
        }
        // Do not delete an address published by a replacement daemon (e.g.
        // `fxg node restart` starts the new process before the old one exits).
        let owns_addr_file = std::fs::read_to_string(&self.addr_file)
            .map(|addr| addr.trim() == self.http_addr.to_string())
            .unwrap_or(false);
        if owns_addr_file {
            fxg_protocol::discard_with!(
                std::fs::remove_file(&self.addr_file),
                "remove the node address file"
            );
        }

        let task_timeout = std::time::Duration::from_secs(3);
        fxg_hub::await_shutdown_task("http", self.http_task, task_timeout).await;
        if let Some(sync_task) = self.sync_task {
            fxg_hub::await_shutdown_task("sync", sync_task, task_timeout).await;
        }
        tracing::info!("fxg node start stopped");
    }

    /// Starts the daemon and runs until shutdown (for the CLI entry point).
    pub async fn run(config: DaemonConfig) -> Result<(), NodeError> {
        let daemon = Self::start(config).await?;
        daemon.wait().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::CommandId;
    use std::collections::HashMap;
    use std::time::Duration;

    fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn default_node_id_is_slugged_and_unique() {
        let a = default_node_id();
        let b = default_node_id();
        assert_ne!(a, b, "the short ID differs every time");
        assert!(!a.contains(' '));
        assert!(
            a.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        );
    }

    #[test]
    fn daemon_config_from_env_resolves_role_paths() {
        let env = env_map(&[("FXG_HOME", "/tmp/fxg-test-home")]);
        let config = DaemonConfig::from_env(&|key| env.get(key).cloned());
        assert_eq!(
            config.paths.db_path(),
            std::path::PathBuf::from("/tmp/fxg-test-home/data/node/node.db")
        );
        assert_eq!(
            config.listen_addr,
            fxg_protocol::config::NodeConfig::DEFAULT_LISTEN_ADDR
        );
    }

    async fn start_test_daemon() -> (NodeDaemon, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(
            Paths::portable(dir.path(), Role::Node),
            "test-node",
            "Test Node",
        );
        // Tests use an ephemeral port to avoid port conflicts
        config.listen_addr = "127.0.0.1:0".to_owned();
        let daemon = NodeDaemon::start(config).await.expect("start daemon");
        (daemon, dir)
    }

    #[tokio::test]
    async fn default_address_falls_back_to_an_ephemeral_port() {
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("occupy a port");
        let occupied_port = occupied.local_addr().expect("addr").port();

        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(
            Paths::portable(dir.path(), Role::Node),
            "fallback-node",
            "Fallback",
        );
        config.listen_addr = format!("127.0.0.1:{occupied_port}");
        config.listen_addr_explicit = false;

        let daemon = NodeDaemon::start(config)
            .await
            .expect("start with fallback");
        assert_ne!(daemon.http_addr().port(), occupied_port);

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn explicit_address_fails_when_in_use() {
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("occupy a port");
        let occupied_port = occupied.local_addr().expect("addr").port();

        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(
            Paths::portable(dir.path(), Role::Node),
            "explicit-node",
            "Explicit",
        );
        config.listen_addr = format!("127.0.0.1:{occupied_port}");
        config.listen_addr_explicit = true;

        let err = NodeDaemon::start(config).await.expect_err("must fail");
        assert!(matches!(err, NodeError::Hub(_)), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn pairing_is_stored_and_applied_without_restart() {
        use fxg_protocol::settings::UpdatePairingRequest;

        let (daemon, dir) = start_test_daemon().await;
        let state = daemon.state();
        let source = AuditSource::local();
        let revision = state.pairing_settings().await.expect("pairing").revision;
        let request = UpdatePairingRequest {
            hub_url: "http://127.0.0.1:1".to_owned(),
            token: Some("test-pairing-token".to_owned()),
            expected_revision: revision,
        };

        let snapshot = state.update_pairing(&request, &source).await.expect("pair");
        assert_eq!(snapshot.hub_url.as_deref(), Some("http://127.0.0.1:1"));
        assert!(snapshot.token_configured);
        assert_eq!(snapshot.revision, revision + 1);
        // The sync supervisor receives the new target immediately (no restart).
        assert!(state.hub_target().is_some());
        assert!(
            Paths::portable(dir.path(), Role::Node)
                .node_token_path()
                .is_file()
        );

        // A stale revision is rejected and does not overwrite the token.
        let err = state
            .update_pairing(
                &UpdatePairingRequest {
                    hub_url: "http://127.0.0.1:2".to_owned(),
                    token: Some("other-token".to_owned()),
                    expected_revision: revision,
                },
                &source,
            )
            .await
            .expect_err("stale revision must be rejected");
        assert!(matches!(
            err,
            NodeError::Db(fxg_db::DbError::RevisionMismatch)
        ));
        let token =
            std::fs::read_to_string(Paths::portable(dir.path(), Role::Node).node_token_path())
                .expect("token file");
        assert_eq!(token, "test-pairing-token");

        let cleared = state
            .clear_pairing(snapshot.revision, &source)
            .await
            .expect("clear");
        assert!(cleared.hub_url.is_none());
        assert!(!cleared.token_configured);
        assert!(state.hub_target().is_none());
        assert!(
            !Paths::portable(dir.path(), Role::Node)
                .node_token_path()
                .exists()
        );

        daemon.shutdown();
        fxg_protocol::discard_with!(
            tokio::time::timeout(Duration::from_secs(5), daemon.wait()).await,
            "wait for the daemon to stop"
        );
    }

    #[tokio::test]
    async fn daemon_starts_and_registers_itself() {
        let (daemon, _dir) = start_test_daemon().await;
        assert_ne!(daemon.http_addr().port(), 0, "ephemeral port is bound");

        let nodes = daemon.state().db().list_nodes().await.expect("nodes");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id.as_str(), "test-node");
        assert!(nodes[0].is_online);
        assert_eq!(nodes[0].os, std::env::consts::OS);

        // The auth token has been generated
        let token = daemon.state().auth_token();
        assert_eq!(token.len(), 64);
        assert!(daemon.state().paths().auth_token_path().is_file());

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("daemon stops within timeout");
    }

    #[tokio::test]
    async fn reconcile_marks_stale_sessions_stopped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open_in_memory(Role::Node).await.expect("db");
        db.upsert_node(&fxg_db::NodeRecord::new(
            "test-node",
            "Test Node",
            "windows",
            "x86_64",
            "0.1.0",
        ))
        .await
        .expect("node");
        let bus = SessionEventBus::new(db.clone());
        // Reproduce a session left in `idle` after the previous daemon was killed
        bus.create_session(
            "stale-session",
            fxg_protocol::events::UnifiedEventPayload::SessionCreated {
                node_id: "test-node".into(),
                project_id: "github.com/nazo6/flexagent".into(),
                project_name: "flexagent".to_owned(),
                local_path: dir.path().to_string_lossy().into_owned(),
                git_branch: None,
                is_worktree: false,
                agent_id: "mock".to_owned(),
                parent_session_id: None,
                title: "stale".to_owned(),
            },
        )
        .await
        .expect("create session");

        reconcile_stale_sessions(&bus).await.expect("reconcile");

        let session = db
            .get_session("stale-session")
            .await
            .expect("query")
            .expect("session row");
        assert_eq!(session.status, fxg_protocol::common::SessionStatus::Stopped);
    }

    #[tokio::test]
    async fn daemon_reuses_node_id_from_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(
            Paths::portable(dir.path(), Role::Node),
            "persisted-node",
            "Node",
        );
        config.listen_addr = "127.0.0.1:0".to_owned();
        let daemon = NodeDaemon::start(config).await.expect("start");
        daemon.shutdown();
        daemon.wait().await;

        // Restarting without a node_id still uses the same ID
        let mut config = DaemonConfig::new(Paths::portable(dir.path(), Role::Node), "", "");
        config.listen_addr = "127.0.0.1:0".to_owned();
        let daemon = NodeDaemon::start(config).await.expect("restart");
        assert_eq!(daemon.state().node_id(), "persisted-node");
        assert_eq!(daemon.state().config().node_name, "Node");
        daemon.shutdown();
        daemon.wait().await;
    }

    #[tokio::test]
    async fn rotate_auth_token_persists_new_value() {
        let (daemon, _dir) = start_test_daemon().await;
        let before = daemon.state().auth_token();
        let after = daemon.state().rotate_auth_token().expect("rotate");
        assert_ne!(before, after);
        assert_eq!(daemon.state().auth_token(), after);

        let on_disk = std::fs::read_to_string(daemon.state().paths().auth_token_path())
            .expect("read token file");
        assert_eq!(on_disk.trim(), after);

        daemon.shutdown();
        daemon.wait().await;
    }

    #[tokio::test]
    async fn terminal_paths_must_be_registered_workspaces() {
        let (daemon, dir) = start_test_daemon().await;
        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");
        let workspace = workdir.to_string_lossy().into_owned();

        assert!(
            daemon
                .state()
                .validate_terminal_path(&workspace)
                .await
                .is_err(),
            "an unregistered directory must be rejected"
        );

        daemon
            .state()
            .resolve_and_register_project(&workdir)
            .await
            .expect("register project");
        assert!(
            daemon
                .state()
                .validate_terminal_path(&workspace)
                .await
                .is_ok()
        );

        // A subdirectory is not a workspace of its own.
        let subdir = workdir.join("sub");
        std::fs::create_dir_all(&subdir).expect("mkdir subdir");
        assert!(
            daemon
                .state()
                .validate_terminal_path(&subdir.to_string_lossy())
                .await
                .is_err(),
            "subdirectories must not be opened as their own workspace"
        );

        daemon.shutdown();
        daemon.wait().await;
    }

    #[tokio::test]
    async fn opening_the_same_workspace_reuses_the_terminal() {
        use fxg_hub::api::ClientApiBackend as _;

        let (daemon, dir) = start_test_daemon().await;
        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");
        daemon
            .state()
            .resolve_and_register_project(&workdir)
            .await
            .expect("register project");
        let path = workdir.to_string_lossy().into_owned();
        let params = fxg_hub::api::PtyOpenParams {
            pty_id: "pty-a".to_owned(),
            node_id: daemon.state().node_id().into(),
            path: path.clone(),
            session_id: None,
            terminal_id: Some("tab-1".to_owned()),
            cols: 80,
            rows: 24,
            new: false,
            remote: false,
        };

        let first = daemon.state().pty_open(params.clone()).await.expect("open");
        assert!(!first.attached);
        let second = daemon
            .state()
            .pty_open(fxg_hub::api::PtyOpenParams {
                pty_id: "pty-b".to_owned(),
                ..params
            })
            .await
            .expect("reopen");
        assert!(second.attached);
        assert_eq!(first.pty_id, second.pty_id);
        assert_eq!(daemon.state().pty().list().len(), 1);

        let terminals = daemon.state().pty().list();
        // The node canonicalizes the workspace path (e.g. `/var` → `/private/var`
        // on macOS, 8.3 short names on Windows).
        let canonical = fxg_pty::canonicalize(&workdir).expect("canonical workspace");
        assert_eq!(terminals[0].path, canonical);
        assert_eq!(terminals[0].attached, 2);

        daemon.state().pty_detach(&first.pty_id).await;
        assert_eq!(daemon.state().pty().list()[0].attached, 1);
        daemon.state().pty_detach(&first.pty_id).await;
        assert_eq!(daemon.state().pty().list()[0].attached, 0);

        daemon.shutdown();
        daemon.wait().await;
    }

    #[tokio::test]
    async fn different_terminal_ids_open_separate_shells() {
        use fxg_hub::api::ClientApiBackend as _;

        let (daemon, dir) = start_test_daemon().await;
        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");
        daemon
            .state()
            .resolve_and_register_project(&workdir)
            .await
            .expect("register project");
        let params = fxg_hub::api::PtyOpenParams {
            pty_id: "pty-a".to_owned(),
            node_id: daemon.state().node_id().into(),
            path: workdir.to_string_lossy().into_owned(),
            session_id: None,
            terminal_id: Some("tab-1".to_owned()),
            cols: 80,
            rows: 24,
            new: false,
            remote: false,
        };

        let first = daemon
            .state()
            .pty_open(params.clone())
            .await
            .expect("open first");
        let second = daemon
            .state()
            .pty_open(fxg_hub::api::PtyOpenParams {
                pty_id: "pty-b".to_owned(),
                terminal_id: Some("tab-2".to_owned()),
                ..params.clone()
            })
            .await
            .expect("open second");
        assert!(!first.attached);
        assert!(!second.attached);
        assert_ne!(first.pty_id, second.pty_id);
        assert_eq!(daemon.state().pty().list().len(), 2);

        let reopened = daemon
            .state()
            .pty_open(fxg_hub::api::PtyOpenParams {
                pty_id: "pty-c".to_owned(),
                ..params
            })
            .await
            .expect("reopen first");
        assert!(reopened.attached);
        assert_eq!(reopened.pty_id, first.pty_id);

        daemon.shutdown();
        daemon.wait().await;
    }

    #[tokio::test]
    async fn pty_ws_replays_retained_output_after_reattach() {
        use base64::Engine as _;
        use futures_util::{SinkExt, StreamExt};
        use fxg_protocol::client_api::{PtyClientMessage, PtyServerMessage};
        use std::time::Duration;
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        async fn next_message(
            socket: &mut tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        ) -> PtyServerMessage {
            loop {
                let message = tokio::time::timeout(Duration::from_secs(15), socket.next())
                    .await
                    .expect("pty ws timed out")
                    .expect("pty ws closed")
                    .expect("pty ws error");
                if let WsMessage::Text(text) = message {
                    return serde_json::from_str(&text).expect("valid pty message");
                }
            }
        }

        let (daemon, dir) = start_test_daemon().await;
        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");
        daemon
            .state()
            .resolve_and_register_project(&workdir)
            .await
            .expect("register project");
        let open = serde_json::to_string(&PtyClientMessage::Open {
            node_id: daemon.state().node_id().into(),
            path: workdir.to_string_lossy().into_owned(),
            session_id: None,
            terminal_id: Some("tab-1".to_owned()),
            cols: 80,
            rows: 24,
            new: false,
        })
        .expect("open message");
        let addr = daemon.http_addr();
        let url = format!("ws://{addr}/api/v1/pty/ws");
        let token = daemon.state().auth_token();

        // First connection: spawn the workspace shell.
        let mut request = url.as_str().into_client_request().expect("ws request");
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("auth header"),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("connect ws");
        socket
            .send(WsMessage::Text(open.clone().into()))
            .await
            .expect("send open");
        let pty_id = match next_message(&mut socket).await {
            PtyServerMessage::Opened {
                pty_id, attached, ..
            } => {
                assert!(!attached);
                pty_id
            }
            other => panic!("expected opened, got {other:?}"),
        };

        let input = serde_json::to_string(&PtyClientMessage::Input {
            data_b64: base64::engine::general_purpose::STANDARD.encode(b"printf 'marker-42\n'\n"),
        })
        .expect("input message");
        socket
            .send(WsMessage::Text(input.into()))
            .await
            .expect("send input");

        // Drain until the shell produced the marker.
        let mut saw_marker = false;
        for _ in 0..40 {
            if let PtyServerMessage::Output { data_b64, .. } = next_message(&mut socket).await {
                let data = base64::engine::general_purpose::STANDARD
                    .decode(data_b64.as_bytes())
                    .expect("base64");
                if String::from_utf8_lossy(&data).contains("marker-42") {
                    saw_marker = true;
                    break;
                }
            }
        }
        assert!(saw_marker, "the shell output was not received");
        drop(socket);

        // The socket close only detaches the client.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while daemon
            .state()
            .pty()
            .list()
            .first()
            .is_some_and(|info| info.attached > 0)
            && std::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Second connection: the same workspace reuses the shell and replays the
        // retained output before any new input.
        let mut request = url.as_str().into_client_request().expect("ws request");
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("auth header"),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("reconnect ws");
        socket
            .send(WsMessage::Text(open.into()))
            .await
            .expect("send open");
        let opened = next_message(&mut socket).await;
        match &opened {
            PtyServerMessage::Opened {
                pty_id: reused,
                attached,
                ..
            } => {
                assert!(attached);
                assert_eq!(reused, &pty_id);
            }
            other => panic!("expected opened, got {other:?}"),
        }

        let mut replayed = String::new();
        for _ in 0..40 {
            if let PtyServerMessage::Output { data_b64, .. } = next_message(&mut socket).await {
                let data = base64::engine::general_purpose::STANDARD
                    .decode(data_b64.as_bytes())
                    .expect("base64");
                replayed.push_str(&String::from_utf8_lossy(&data));
                if replayed.contains("marker-42") {
                    break;
                }
            }
        }
        assert!(
            replayed.contains("marker-42"),
            "retained output was not replayed: {replayed:?}"
        );

        drop(socket);
        daemon.shutdown();
        daemon.wait().await;
    }

    #[tokio::test]
    async fn kill_all_stops_agent_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        crate::testutil::write_mock_agent_config(&Paths::portable(dir.path(), Role::Node)).await;
        let mut config = DaemonConfig::new(
            Paths::portable(dir.path(), Role::Node),
            "kill-node",
            "Kill Node",
        );
        config.listen_addr = "127.0.0.1:0".to_owned();
        let mock = crate::testutil::MockAgent::default();
        let daemon = NodeDaemon::start_with_driver_factory(config, mock.factory())
            .await
            .expect("start");

        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");
        let outcome = daemon
            .state()
            .session_manager()
            .ensure_session(&CommandId::from("c1"), &workdir, "mock", &[], None)
            .await
            .expect("ensure session");
        assert_eq!(daemon.state().session_manager().list_active().len(), 1);

        // The kill switch stops the agent session and records Stopped
        let (killed_sessions, killed_ptys) = daemon
            .state()
            .kill_all_local("kill switch test")
            .await
            .expect("kill all");
        assert_eq!(killed_sessions, vec![outcome.session_id.clone()]);
        assert!(killed_ptys.is_empty());
        assert!(daemon.state().session_manager().list_active().is_empty());

        let sessions = daemon
            .state()
            .db()
            .list_sessions(&fxg_db::SessionFilter::default())
            .await
            .expect("list sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].status,
            fxg_protocol::common::SessionStatus::Stopped
        );

        daemon.shutdown();
        daemon.wait().await;
    }

    #[tokio::test]
    async fn workspace_file_api_stays_inside_registered_workspace() {
        use crate::NodeError;
        use crate::workspace;
        use fxg_protocol::client_api::{WorkspaceFileContent, WorkspaceGitFileStatus};
        use fxg_protocol::common::DiffScope;

        let (daemon, dir) = start_test_daemon().await;
        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");
        let path = fxg_pty::canonicalize(&workdir).expect("canonical workspace");
        let path_str = path.to_string_lossy().into_owned();
        daemon
            .state()
            .resolve_and_register_project(&workdir)
            .await
            .expect("register project");

        crate::git::run_git(&path, &["init"])
            .await
            .expect("git init");
        crate::git::run_git(&path, &["config", "user.email", "test@example.com"])
            .await
            .expect("git email");
        crate::git::run_git(&path, &["config", "user.name", "Test"])
            .await
            .expect("git name");
        std::fs::write(path.join("hello.txt"), "hello\n").expect("write hello");
        crate::git::run_git(&path, &["add", "hello.txt"])
            .await
            .expect("git add");
        crate::git::run_git(&path, &["commit", "-m", "initial"])
            .await
            .expect("git commit");
        std::fs::write(path.join("hello.txt"), "hello\nworld\n").expect("modify hello");
        std::fs::write(path.join("new.txt"), "new\n").expect("write new");

        let listing = workspace::list_workspace(daemon.state(), &path_str)
            .await
            .expect("list");
        assert_eq!(listing.root_path, path_str);
        assert!(
            listing
                .entries
                .iter()
                .any(|entry| entry.name == "hello.txt")
        );
        assert!(listing.entries.iter().any(|entry| entry.name == "new.txt"));
        assert!(!listing.entries.iter().any(|entry| entry.name == ".git"));

        let hello = path.join("hello.txt").to_string_lossy().into_owned();
        let view = workspace::read_workspace_file(daemon.state(), &hello)
            .await
            .expect("read");
        match view.content {
            WorkspaceFileContent::Text { text, .. } => assert_eq!(text, "hello\nworld\n"),
            _ => panic!("expected text file"),
        }

        let status = workspace::workspace_git_status(daemon.state(), &path_str)
            .await
            .expect("status");
        assert!(status.is_repository);
        assert!(status.entries.iter().any(|entry| {
            entry.path == "hello.txt" && entry.status == WorkspaceGitFileStatus::Modified
        }));
        assert!(status.entries.iter().any(|entry| {
            entry.path == "new.txt" && entry.status == WorkspaceGitFileStatus::Untracked
        }));

        let diff = workspace::workspace_git_diff(
            daemon.state(),
            &path_str,
            DiffScope::Uncommitted,
            None,
            Some(&hello),
        )
        .await
        .expect("diff");
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].path, "hello.txt");

        let new_file = path.join("new.txt").to_string_lossy().into_owned();
        let untracked = workspace::workspace_git_diff(
            daemon.state(),
            &path_str,
            DiffScope::Uncommitted,
            None,
            Some(&new_file),
        )
        .await
        .expect("untracked diff");
        assert_eq!(untracked.files.len(), 1);
        assert_eq!(untracked.files[0].new_text.as_deref(), Some("new\n"));

        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "no").expect("write outside");
        let error = workspace::list_workspace(daemon.state(), &outside.to_string_lossy())
            .await
            .expect_err("outside path must be rejected");
        assert!(matches!(error, NodeError::Forbidden(_)));

        let sub = path.join("sub");
        std::fs::create_dir_all(&sub).expect("mkdir sub");
        std::fs::write(sub.join("nested.txt"), "nested").expect("write nested");
        let sub_listing = workspace::list_workspace(daemon.state(), &sub.to_string_lossy())
            .await
            .expect("subdirectory listing");
        assert!(
            sub_listing
                .entries
                .iter()
                .any(|entry| entry.name == "nested.txt")
        );
        assert_eq!(sub_listing.root_path, path_str);

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let link = path.join("escape.txt");
            symlink(&outside, &link).expect("symlink");
            let error = workspace::read_workspace_file(daemon.state(), &link.to_string_lossy())
                .await
                .expect_err("symlink escape must be rejected");
            assert!(matches!(error, NodeError::Forbidden(_)));
        }

        daemon.shutdown();
        daemon.wait().await;
    }
}
