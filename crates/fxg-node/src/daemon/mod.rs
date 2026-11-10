//! Node daemon (`fxg node start`).
//!
//! Design: `docs/01-architecture-and-sync.md` §1, §7; `docs/03-protocol-and-api.md` §3, §4.
//!
//! - **Local HTTP/WS server** (strictly bound to `127.0.0.1:7860`):
//!   serves the client API (REST + Client WS) ([`http`])
//! - **Local IPC server** (Windows named pipe / Unix domain socket):
//!   accepts commands from the `fxg` CLI ([`ipc`])
//! - **Authentication**: Bearer / Cookie authentication via `~/.flexagent/auth_token` ([`auth`])

mod api;
mod auth;
mod http;
mod ipc;
mod ops;
mod stdio;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use fxg_acp::registry::AcpRegistry;
use fxg_db::{Db, DbRole};
use fxg_hub::api::ClientEvent;
use fxg_protocol::config::fxg_home;
use fxg_protocol::settings::{AgentSettings, NodeSettings};
use fxg_protocol::util::now_ms;
use fxg_pty::PtySessionManager;
use tokio::sync::{broadcast, watch};

use crate::error::NodeError;
use crate::paths::{NodePaths, ipc_endpoint};
use crate::session::SessionEventBus;
use crate::session_manager::{DriverFactory, SessionManager, default_driver_factory};

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
    /// Local IPC endpoint (named pipe / UDS path)
    pub ipc_endpoint: String,
    /// Node Hub WebSocket URL of the hub (standalone when unset)
    pub hub_url: Option<String>,
    /// Per-node pairing token (reads `~/.flexagent/node_token` when omitted)
    pub node_token: Option<String>,
    /// Whether this is an ephemeral VM / sandbox node (`--ephemeral`; reflected in
    /// NodeHello and the `nodes` row)
    pub ephemeral: bool,
    /// Data directory (`~/.flexagent`)
    pub fxg_home: PathBuf,
    /// Effective logging settings (for the read-only settings catalog)
    pub log: fxg_protocol::config::LogConfig,
}

impl DaemonConfig {
    /// Builds the default configuration (`node_id` / `node_name` are values resolved by the caller).
    pub fn new(
        fxg_home: impl Into<PathBuf>,
        node_id: impl Into<String>,
        node_name: impl Into<String>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            node_name: node_name.into(),
            listen_addr: fxg_protocol::config::NodeBootstrapConfig::DEFAULT_LISTEN_ADDR.to_owned(),
            ipc_endpoint: ipc_endpoint(&fxg_protocol::config::process_env),
            hub_url: None,
            node_token: None,
            ephemeral: false,
            fxg_home: fxg_home.into(),
            log: fxg_protocol::config::LogConfig::default(),
        }
    }

    /// Builds the configuration by resolving the data directory and default IPC
    /// endpoint from the environment.
    pub fn from_env(env: fxg_protocol::config::EnvLookup<'_>) -> Self {
        let home = fxg_home(env);
        Self {
            node_id: String::new(),
            node_name: hostname(),
            listen_addr: fxg_protocol::config::NodeBootstrapConfig::DEFAULT_LISTEN_ADDR.to_owned(),
            ipc_endpoint: ipc_endpoint(env),
            hub_url: env(fxg_protocol::config::env_keys::HUB_URL),
            node_token: env(fxg_protocol::config::env_keys::NODE_TOKEN),
            ephemeral: false,
            fxg_home: home,
            log: fxg_protocol::config::LogConfig::default(),
        }
    }
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
    paths: NodePaths,
    config: DaemonConfig,
    /// Node-owned settings (`node_settings` in `node.db`)
    node_settings: RwLock<NodeSettings>,
    /// Agent settings (`agent_settings` in `node.db`)
    agent_settings: RwLock<AgentSettings>,
    token: RwLock<String>,
    started_at: i64,
    /// Normalized events delivered to Client WS (forwarded from the bus)
    client_events: broadcast::Sender<ClientEvent>,
    /// Whether the hub's outbox sync worker is connected
    hub_connected: AtomicBool,
    /// `NodeHello` resend trigger (on worktree changes / project registration)
    hello_refresh: tokio::sync::Notify,
    /// Git credential proxy (in-memory auth relay for ephemeral VMs)
    credentials: crate::credentials::CredentialBroker,
    /// Process-local delivery counters exposed by diagnostics
    diagnostics_counters: std::sync::Arc<fxg_hub::api::DiagnosticsCounters>,
    /// Shutdown sender used by the local IPC `Shutdown` request (set at startup)
    shutdown_tx: std::sync::OnceLock<watch::Sender<bool>>,
}

/// Shared daemon state (shared by HTTP handlers / IPC handlers / the CLI).
#[derive(Clone)]
pub struct DaemonState {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DaemonState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonState")
            .field("node_id", &self.inner.config.node_id)
            .field("listen_addr", &self.inner.config.listen_addr)
            .field("ipc_endpoint", &self.inner.config.ipc_endpoint)
            .finish()
    }
}

impl DaemonState {
    /// Registers the shutdown sender used by the local IPC `Shutdown` request.
    pub(crate) fn set_shutdown_sender(&self, sender: watch::Sender<bool>) {
        fxg_protocol::discard_with!(
            self.inner.shutdown_tx.set(sender),
            "register the node shutdown sender"
        );
    }

    /// Requests a graceful node shutdown (local IPC `Shutdown` / `fxg node stop`).
    pub(crate) fn request_shutdown(&self) {
        if let Some(sender) = self.inner.shutdown_tx.get() {
            fxg_protocol::discard_with!(sender.send(true), "request node shutdown from local IPC");
        }
    }

    /// Initializes the state (creates the data directory + loads the auth token +
    /// resolves the node ID).
    pub async fn new(config: DaemonConfig) -> Result<Self, NodeError> {
        Self::with_driver_factory(config, default_driver_factory()).await
    }

    /// Initializes the state with a replaced agent driver factory.
    ///
    /// The production path is [`Self::new`] (auto-selects the ACP / OpenCode2 driver).
    /// Tests inject a mock driver to verify without real processes.
    #[doc(hidden)]
    pub async fn with_driver_factory(
        mut config: DaemonConfig,
        driver_factory: DriverFactory,
    ) -> Result<Self, NodeError> {
        let paths = NodePaths::new(config.fxg_home.clone());
        paths.ensure_dirs()?;
        let db = Db::open(&paths.node_db_path(), DbRole::Node).await?;

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
        config.fxg_home = paths.fxg_home().to_path_buf();

        let token = auth::load_or_create(&paths.auth_token_path())?;
        let bus = SessionEventBus::new(db.clone()).with_node_id(config.node_id.clone());
        // Reconcile the state of sessions lost when the previous daemon exited
        reconcile_stale_sessions(&bus).await?;

        // Runtime settings are owned by `node.db`; `config.toml` only bootstraps the process.
        let node_settings = db.ensure_node_settings(&config.node_name).await?;
        config.node_name = node_settings.name.clone();
        let agent_settings = db.ensure_agent_settings().await?;
        let registry = AcpRegistry::new(
            paths.fxg_home(),
            &agent_settings,
            db.custom_agents().await?,
            db.enabled_package_agents().await?,
        );
        let sessions = SessionManager::with_factory(
            bus.clone(),
            paths.clone(),
            config.node_id.clone(),
            registry,
            driver_factory,
        );

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
                hub_connected: AtomicBool::new(false),
                hello_refresh: tokio::sync::Notify::new(),
                credentials: crate::credentials::CredentialBroker::new(),
                diagnostics_counters: std::sync::Arc::new(fxg_hub::api::DiagnosticsCounters::new()),
                shutdown_tx: std::sync::OnceLock::new(),
            }),
        };

        // Generate the GIT_ASKPASS helper injected into agent processes
        // (Git operations in ephemeral VMs authenticate through this script → local IPC →
        //  the hub's in-memory relay. Design: docs/01 §6.4)
        if let Err(err) = crate::credentials::write_askpass_script(state.inner.paths.fxg_home()) {
            tracing::warn!("failed to write git-askpass script: {err}");
        }

        // Forward session event bus events to the events delivered to Client WS
        // (bus Lagged is recovered by the Client WS loop replaying from the cursor)
        let mut bus_rx = state.inner.bus.subscribe();
        let client_tx = state.inner.client_events.clone();
        let counters = state.diagnostics_counters_arc();
        fxg_protocol::spawn_guarded!("local client event forwarder", async move {
            loop {
                match bus_rx.recv().await {
                    Ok(crate::session::SessionBroadcast::Persisted { event, cursor }) => {
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

    /// Path resolution for the data directory.
    pub fn paths(&self) -> &NodePaths {
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

    /// Reloads custom agents / package enablement from `node.db` into the registry.
    pub async fn refresh_agent_catalog(&self) -> Result<(), NodeError> {
        let settings = self.inner.db.ensure_agent_settings().await?;
        let custom = self.inner.db.custom_agents().await?;
        let enabled = self.inner.db.enabled_package_agents().await?;
        self.inner
            .sessions
            .registry()
            .apply_catalog(&settings, custom, enabled);
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

    /// Whether the hub's outbox sync worker is connected.
    pub fn hub_connected(&self) -> bool {
        self.inner.hub_connected.load(Ordering::Relaxed)
    }

    /// Updates the hub connection state (outbox sync worker only).
    pub fn set_hub_connected(&self, connected: bool) {
        self.inner.hub_connected.store(connected, Ordering::Relaxed);
    }

    /// Git credential proxy broker.
    pub fn credentials(&self) -> &crate::credentials::CredentialBroker {
        &self.inner.credentials
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

    /// The current client auth token.
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

    /// List of available agent IDs (installed from the ACP Registry + custom + externally installed).
    ///
    /// Resolved with a short timeout so that daemon startup is not blocked even
    /// where fetching/expanding the registry is slow (on failure, only custom definitions).
    pub async fn installed_agents(&self) -> Vec<String> {
        let registry = self.inner.sessions.registry();
        // Custom definitions do not depend on fetching the registry, so always include them
        let mut ids: Vec<String> = registry.custom_agents().into_keys().collect();
        let listed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let index = registry.index(false).await?;
            Ok::<_, anyhow::Error>(
                registry
                    .list(&index, false)
                    .into_iter()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>(),
            )
        })
        .await;
        match listed {
            Ok(Ok(agents)) => ids.extend(agents),
            Ok(Err(err)) => {
                tracing::warn!("failed to list installed agents (reporting partial list): {err}");
            }
            Err(_) => {
                tracing::warn!("listing installed agents timed out (reporting partial list)");
            }
        }
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
            SessionStatus::Bootstrapping,
            SessionStatus::Idle,
            SessionStatus::Running,
            SessionStatus::WaitingPermission,
            SessionStatus::WaitingInput,
        ]
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
    /// caller (IPC / HTTP handler) that holds the client information (IP / route).
    ///
    /// Returns the list of terminated session IDs and PTY IDs.
    pub async fn kill_all_local(
        &self,
        reason: &str,
    ) -> Result<(Vec<String>, Vec<String>), NodeError> {
        use fxg_protocol::common::SessionStatus;
        use fxg_protocol::events::UnifiedEventPayload;

        let killed_ptys = self.inner.pty.kill_all();

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

/// Starts the outbox sync worker (returns `None` when no hub URL is set).
///
/// The per-node token is resolved in the order `config.node_token` / `FXG_NODE_TOKEN` →
/// `~/.flexagent/node_token`.
fn start_sync_worker(
    state: &DaemonState,
    shutdown_tx: &watch::Sender<bool>,
) -> Result<Option<tokio::task::JoinHandle<()>>, NodeError> {
    let Some(url) = state.config().hub_url.clone() else {
        return Ok(None);
    };
    let token = match state.config().node_token.clone() {
        Some(token) if !token.trim().is_empty() => token,
        _ => match std::fs::read_to_string(state.paths().node_token_path()) {
            Ok(token) if !token.trim().is_empty() => token.trim().to_owned(),
            _ => {
                tracing::warn!("hub_url is set but node_token is missing; outbox sync disabled");
                return Ok(None);
            }
        },
    };
    let state = state.clone();
    let shutdown = shutdown_tx.subscribe();
    let task = tokio::spawn(async move {
        crate::sync::run(state, url, token, shutdown).await;
    });
    Ok(Some(task))
}

/// A started node daemon.
pub struct NodeDaemon {
    state: DaemonState,
    http_addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    http_task: tokio::task::JoinHandle<()>,
    ipc_task: tokio::task::JoinHandle<()>,
    sync_task: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for NodeDaemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeDaemon")
            .field("node_id", &self.state.node_id())
            .field("http_addr", &self.http_addr)
            .field("ipc_endpoint", &self.state.config().ipc_endpoint)
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
    /// starts listening on the local HTTP/WS and local IPC).
    pub async fn start(config: DaemonConfig) -> Result<Self, NodeError> {
        Self::start_with_driver_factory(config, default_driver_factory()).await
    }

    /// Starts the daemon with a replaced agent driver factory
    /// (for tests. The production path is [`Self::start`]).
    #[doc(hidden)]
    pub async fn start_with_driver_factory(
        config: DaemonConfig,
        driver_factory: DriverFactory,
    ) -> Result<Self, NodeError> {
        let state = DaemonState::with_driver_factory(config, driver_factory).await?;
        state.register_node().await?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        state.set_shutdown_sender(shutdown_tx.clone());

        // Local HTTP/WS server (strict loopback binding)
        let listen_addr = state.config().listen_addr.clone();
        let listener = tokio::net::TcpListener::bind(&listen_addr)
            .await
            .map_err(|err| NodeError::Hub(format!("failed to bind {listen_addr}: {err}")))?;
        let http_addr = listener
            .local_addr()
            .map_err(|err| NodeError::Hub(err.to_string()))?;

        let http_state = state.clone();
        let http_shutdown = shutdown_rx.clone();
        let http_task = tokio::spawn(async move {
            http::serve(http_state, listener, http_shutdown).await;
        });

        // Local IPC server (named pipe / UDS).
        // First check whether another daemon is already running on the same endpoint
        // (to return the failure to the caller of `start`).
        ipc::check_endpoint_available(&state.config().ipc_endpoint).await?;
        let ipc_state = state.clone();
        let ipc_endpoint = state.config().ipc_endpoint.clone();
        let ipc_task = tokio::spawn(async move {
            if let Err(err) = ipc::serve(ipc_state, ipc_endpoint, shutdown_rx).await {
                tracing::error!("local ipc server stopped: {err}");
            }
        });

        // Request shutdown on Ctrl-C
        let signal_tx = shutdown_tx.clone();
        fxg_protocol::spawn_guarded!("ctrl-c handler", async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!("received Ctrl-C, shutting down");
                fxg_protocol::discard_with!(
                    signal_tx.send(true),
                    "request node shutdown after Ctrl-C"
                );
            }
        });

        // Outbox sync worker (started only when a hub URL is configured)
        let sync_task = start_sync_worker(&state, &shutdown_tx)?;

        tracing::info!(
            node_id = state.config().node_id,
            http = %http_addr,
            ipc = %state.config().ipc_endpoint,
            "fxg node start started"
        );

        Ok(Self {
            state,
            http_addr,
            shutdown_tx,
            http_task,
            ipc_task,
            sync_task,
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

    /// Local IPC endpoint.
    pub fn ipc_endpoint(&self) -> &str {
        &self.state.config().ipc_endpoint
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

        if let Err(err) = self.http_task.await {
            tracing::error!(task = "http", "http task terminated unexpectedly: {err}");
        }
        if let Err(err) = self.ipc_task.await {
            tracing::error!(task = "ipc", "ipc task terminated unexpectedly: {err}");
        }
        if let Some(sync_task) = self.sync_task
            && let Err(err) = sync_task.await
        {
            tracing::error!(task = "sync", "sync task terminated unexpectedly: {err}");
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
    fn daemon_config_from_env_uses_fxg_home() {
        let env = env_map(&[("FXG_HOME", "/tmp/fxg-test-home")]);
        let config = DaemonConfig::from_env(&|key| env.get(key).cloned());
        assert_eq!(config.fxg_home, PathBuf::from("/tmp/fxg-test-home"));
        assert_eq!(
            config.listen_addr,
            fxg_protocol::config::NodeBootstrapConfig::DEFAULT_LISTEN_ADDR
        );
        assert!(!config.ipc_endpoint.is_empty());
    }

    async fn start_test_daemon() -> (NodeDaemon, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "test-node", "Test Node");
        // Tests use an ephemeral port and a temporary IPC path to avoid port conflicts
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let daemon = NodeDaemon::start(config).await.expect("start daemon");
        (daemon, dir)
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
        let db = Db::open_in_memory(DbRole::Node).await.expect("db");
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
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "persisted-node", "Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let daemon = NodeDaemon::start(config).await.expect("start");
        daemon.shutdown();
        daemon.wait().await;

        // Restarting without a node_id still uses the same ID
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "", "");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
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
    async fn kill_all_stops_agent_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        crate::testutil::write_mock_agent_config(dir.path()).await;
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "kill-node", "Kill Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
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
}
