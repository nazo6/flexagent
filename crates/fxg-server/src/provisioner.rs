//! Provisioner management for temporary VM / sandbox nodes (on the central server host).
//!
//! Design: `docs/01-architecture-and-sync.md` §6.1–§6.4, `docs/05-cli-and-pwa-ui.md` §2.2.
//!
//! - Launches the command defined under `[provisioners.<name>]` in `config.toml` as a child
//!   process and uses its `stdin` / `stdout` as the JSON Lines transport for
//!   `NodeToServerMsg` / `ServerToNodeMsg` (a direct connection to `fxg daemon --stdio`)
//! - Injects `FXG_GIT_URL` / `FXG_GIT_BRANCH` / a short-lived `FXG_GIT_TOKEN` and the ephemeral
//!   node ID (`FXG_NODE_ID`) as environment variables at provisioner startup
//! - Streams `stderr` to the UI as `BootstrapLog` (ephemeral)
//! - After the handshake (NodeHello), registers the hub connection before sending
//!   `StartSession` to start the session (the node is "offline" during bootstrap)
//! - On idle timeout / when all sessions end, sends `DrainAndShutdown` and discards the child
//!   process (VM/container) after `DrainComplete`
//! - `WorkspaceBundleUpload` accumulates chunks until `DrainComplete`, saves them to
//!   `~/.flexagent/bundles/<session-id>.bundle` and records the path in
//!   `sessions.git_bundle_path`

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use fxg_protocol::client_api::ProvisionerTestResponse;
use fxg_protocol::common::{ForkHistoryItem, NodeLifecycleStatus, PromptInput, SessionStatus};
use fxg_protocol::config::{
    BUNDLES_DIR_NAME, DEFAULT_GIT_USERNAME, GitCredentialConfig, ProvisionerConfig,
};
use fxg_protocol::node_server::{NodeToServerMsg, ServerToNodeMsg};
use fxg_protocol::util::uuid_v7;
use fxg_protocol::{CommandId, NodeId};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::Command;
use tokio::sync::{oneshot, watch};

use crate::api::{ApiError, ClientEvent};
use crate::credentials::resolve_credential;
use crate::hub;
use crate::state::ServerState;

/// Upper bound for waiting on bootstrap completion (NodeHello).
const HELLO_TIMEOUT: Duration = Duration::from_secs(1800);
/// Upper bound for waiting on `DrainComplete`.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(60);
/// Tick interval for idle monitoring.
const MONITOR_INTERVAL: Duration = Duration::from_secs(5);
/// Upper bound for the `provisioners test` handshake wait.
const TEST_TIMEOUT: Duration = Duration::from_secs(300);
/// Number of bootstrap log lines kept by `provisioners test`.
const TEST_LOG_LINES: usize = 200;
/// Workspace path inside the temporary VM (must match the generated script).
pub(crate) const EPHEMERAL_WORKSPACE: &str = "/tmp/workspace";
/// fxg binary path inside the temporary VM.
const EPHEMERAL_BINARY: &str = "/tmp/fxg";

/// Runtime state of one temporary VM instance (keyed by node_id).
struct Instance {
    node_id: String,
    session_id: Option<String>,
    /// Child process termination signal (the pump task kills and drops the Job Object on it)
    terminate: watch::Sender<bool>,
    /// Time the last message was received (used for idle timeout decisions)
    last_activity: Arc<StdMutex<Instant>>,
}

/// Git bundle being accumulated (session_id → sequence of Base64 chunks).
#[derive(Default)]
struct BundleAccumulator {
    chunks: Vec<String>,
    branch: String,
    head_commit: String,
}

struct ProvisionerInner {
    fxg_home: PathBuf,
    configs: BTreeMap<String, ProvisionerConfig>,
    git_credentials: BTreeMap<String, GitCredentialConfig>,
    instances: StdMutex<HashMap<String, Instance>>,
    bundles: StdMutex<HashMap<String, BundleAccumulator>>,
    drain_waiters: StdMutex<HashMap<String, oneshot::Sender<()>>>,
}

/// Management of temporary VM provisioners (one instance is held by the central server).
#[derive(Clone)]
pub struct ProvisionerManager {
    inner: Arc<ProvisionerInner>,
}

impl std::fmt::Debug for ProvisionerManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let instances = self
            .inner
            .instances
            .lock()
            .map(|instances| instances.len())
            .unwrap_or(0);
        f.debug_struct("ProvisionerManager")
            .field("provisioners", &self.inner.configs.len())
            .field("instances", &instances)
            .finish()
    }
}

impl ProvisionerManager {
    /// Creates the manager from configuration.
    pub fn new(
        fxg_home: PathBuf,
        configs: BTreeMap<String, ProvisionerConfig>,
        git_credentials: BTreeMap<String, GitCredentialConfig>,
    ) -> Self {
        Self {
            inner: Arc::new(ProvisionerInner {
                fxg_home,
                configs,
                git_credentials,
                instances: StdMutex::new(HashMap::new()),
                bundles: StdMutex::new(HashMap::new()),
                drain_waiters: StdMutex::new(HashMap::new()),
            }),
        }
    }

    /// Configured provisioner definitions.
    pub fn configs(&self) -> &BTreeMap<String, ProvisionerConfig> {
        &self.inner.configs
    }

    /// Fetches a configuration (unknown names yield `NOT_FOUND`).
    pub fn config(&self, name: &str) -> Result<&ProvisionerConfig, ApiError> {
        self.inner
            .configs
            .get(name)
            .ok_or_else(|| ApiError::not_found(format!("provisioner not found: {name}")))
    }

    /// Git Credential Proxy configuration.
    pub fn git_credentials(&self) -> &BTreeMap<String, GitCredentialConfig> {
        &self.inner.git_credentials
    }

    /// `~/.flexagent` data directory.
    pub fn fxg_home(&self) -> &std::path::Path {
        &self.inner.fxg_home
    }

    /// Whether this is a running temporary node (an active instance exists).
    pub fn is_ephemeral(&self, node_id: &str) -> bool {
        self.inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .contains_key(node_id)
    }

    /// Session ID (`None` for unknown nodes).
    fn session_id(&self, node_id: &str) -> Option<String> {
        self.inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .get(node_id)
            .and_then(|instance| instance.session_id.clone())
    }

    fn activity(&self, node_id: &str) -> Option<Arc<StdMutex<Instant>>> {
        self.inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .get(node_id)
            .map(|instance| instance.last_activity.clone())
    }

    fn insert_instance(&self, instance: Instance) {
        self.inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .insert(instance.node_id.clone(), instance);
    }

    fn remove_instance(&self, node_id: &str) {
        self.inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .remove(node_id);
    }

    /// Asks the child process to terminate (kill and drop the Job Object).
    fn terminate(&self, node_id: &str) {
        if let Some(instance) = self
            .inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .get(node_id)
        {
            fxg_protocol::discard_with!(
                instance.terminate.send(true),
                "request termination of an ephemeral node instance"
            );
        }
    }

    fn register_drain_waiter(&self, node_id: &str) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .drain_waiters
            .lock()
            .expect("drain lock poisoned")
            .insert(node_id.to_owned(), tx);
        rx
    }

    /// A `DrainComplete` was received (notifies any waiter).
    fn take_drain_waiter(&self, node_id: &str) {
        if let Some(tx) = self
            .inner
            .drain_waiters
            .lock()
            .expect("drain lock poisoned")
            .remove(node_id)
        {
            fxg_protocol::discard_with!(tx.send(()), "notify a drain waiter that the node drained");
        }
    }

    fn push_bundle_chunk(&self, session_id: &str, branch: &str, head_commit: &str, chunk: &str) {
        let mut bundles = self.inner.bundles.lock().expect("bundles lock poisoned");
        let entry = bundles.entry(session_id.to_owned()).or_default();
        if !branch.is_empty() {
            entry.branch = branch.to_owned();
        }
        if !head_commit.is_empty() {
            entry.head_commit = head_commit.to_owned();
        }
        entry.chunks.push(chunk.to_owned());
    }

    fn take_bundle(&self, session_id: &str) -> Option<BundleAccumulator> {
        self.inner
            .bundles
            .lock()
            .expect("bundles lock poisoned")
            .remove(session_id)
    }
}

/// Session start request (provisioner launch + StartSession).
#[derive(Debug, Clone)]
pub struct SpawnSessionRequest {
    /// Server-assigned session ID
    pub session_id: String,
    /// Ephemeral node ID (server-assigned; injected as `FXG_NODE_ID`)
    pub node_id: String,
    /// Provisioner name
    pub provisioner: String,
    /// Logical project ID
    pub project_id: String,
    /// Git URL to clone from (`FXG_GIT_URL`)
    pub git_url: String,
    /// Branch to clone (`FXG_GIT_BRANCH`; set when `--worktree` is specified)
    pub git_branch: Option<String>,
    /// Agent ID to launch
    pub agent_id: String,
    /// Initial prompt
    pub initial_prompt: Option<PromptInput>,
    /// Initial mode
    pub mode: Option<String>,
    /// Extra arguments for the agent
    pub extra_args: Option<Vec<String>>,
    /// Fork history from another node (replay injection)
    pub fork_context: Option<Vec<ForkHistoryItem>>,
    /// Saved Git bundle (Base64)
    pub restore_bundle_b64: Option<String>,
}

/// Launches a provisioner for a session (sends StartSession after the handshake).
pub async fn spawn_session(
    state: &ServerState,
    request: SpawnSessionRequest,
) -> Result<(), ApiError> {
    let config = state.provisioners().config(&request.provisioner)?.clone();
    let node_id = request.node_id.clone();
    let session_id = request.session_id.clone();
    let idle_timeout_secs = config.idle_timeout_secs.unwrap_or(900);

    let mut env: Vec<(String, String)> = vec![
        ("FXG_NODE_ID".to_owned(), node_id.clone()),
        (
            "FXG_NODE_NAME".to_owned(),
            format!("{} (ephemeral)", request.provisioner),
        ),
        ("FXG_GIT_URL".to_owned(), request.git_url.clone()),
    ];
    if let Some(branch) = request.git_branch.as_deref().filter(|b| !b.is_empty()) {
        env.push(("FXG_GIT_BRANCH".to_owned(), branch.to_owned()));
    }
    match resolve_git_token(state, &request.git_url).await {
        Some(response) => {
            env.push((
                "FXG_GIT_USERNAME".to_owned(),
                if response.username.trim().is_empty() {
                    DEFAULT_GIT_USERNAME.to_owned()
                } else {
                    response.username
                },
            ));
            if let Some(token) = response.token {
                env.push(("FXG_GIT_TOKEN".to_owned(), token));
            }
        }
        None => {
            tracing::warn!(
                git_url = request.git_url,
                "no git credential configured for the project host; clone may fail"
            );
        }
    }

    let pending = PendingStart {
        session_id: session_id.clone(),
        project_id: request.project_id.clone(),
        agent_id: request.agent_id.clone(),
        initial_prompt: request.initial_prompt.clone(),
        mode: request.mode.clone(),
        extra_args: request.extra_args.clone(),
        fork_context: request.fork_context.clone(),
        restore_bundle_b64: request.restore_bundle_b64.clone(),
    };

    let spawn = SpawnRequest {
        node_id: node_id.clone(),
        provisioner: request.provisioner.clone(),
        config,
        env,
        mode: SpawnMode::Session(Box::new(pending.clone())),
    };
    let handles = spawn_child(state, spawn).await?;
    let hello = handles
        .hello
        .ok_or_else(|| ApiError::internal("ephemeral spawn returned no handshake channel"))?;

    fxg_protocol::spawn_guarded!(
        "ephemeral handshake",
        await_handshake_and_start(state.clone(), node_id.clone(), hello, pending)
    );
    fxg_protocol::spawn_guarded!(
        "ephemeral idle monitor",
        monitor_instance(
            state.clone(),
            node_id.clone(),
            Duration::from_secs(idle_timeout_secs),
        )
    );
    tracing::info!(
        node_id = node_id.as_str(),
        session_id,
        provisioner = request.provisioner,
        "ephemeral node provisioning started"
    );
    Ok(())
}

/// Pre-registers an ephemeral node (for the `sessions.node_id` FK and the `provisioner` display).
///
/// The `NodeHello` upsert sends `provisioner = NULL`, but `COALESCE` keeps the value registered
/// here.
pub(crate) async fn register_ephemeral_node(
    state: &ServerState,
    node_id: &str,
    provisioner: &str,
    idle_timeout_secs: u64,
) -> Result<(), ApiError> {
    let mut record = fxg_db::NodeRecord::new(
        node_id.to_owned(),
        format!("{provisioner} (ephemeral)"),
        "linux",
        "x86_64",
        env!("CARGO_PKG_VERSION"),
    );
    record.is_ephemeral = true;
    record.provisioner = Some(provisioner.to_owned());
    record.lifecycle_status = NodeLifecycleStatus::Provisioning;
    record.idle_timeout_secs = Some(idle_timeout_secs);
    record.is_online = false;
    state
        .db()
        .upsert_node(&record)
        .await
        .map_err(ApiError::from)
}

/// `fxg provisioners test <name>`: validates provisioner startup and the NodeHello handshake.
pub async fn provisioner_test(
    state: &ServerState,
    name: &str,
) -> Result<ProvisionerTestResponse, ApiError> {
    let config = state.provisioners().config(name)?.clone();
    let node_id = format!("fxg-test-{}", &uuid_v7()[..8]);

    let spawn = SpawnRequest {
        node_id: node_id.clone(),
        provisioner: name.to_owned(),
        config,
        env: vec![("FXG_NODE_ID".to_owned(), node_id.clone())],
        mode: SpawnMode::Test,
    };
    let handles = spawn_child(state, spawn).await?;
    let hello = handles
        .hello
        .ok_or_else(|| ApiError::internal("provisioner test returned no handshake channel"))?;
    let terminate = handles.terminate.clone();
    let test_log = handles.test_log.clone().unwrap_or_default();

    let outcome = tokio::time::timeout(TEST_TIMEOUT, hello).await;
    let log_lines: Vec<String> = test_log
        .lock()
        .expect("test log poisoned")
        .iter()
        .rev()
        .take(TEST_LOG_LINES)
        .rev()
        .cloned()
        .collect();

    // Always discard after the validation (drain is best effort)
    if let Err(err) = state
        .hub()
        .send(
            &NodeId::from(node_id.as_str()),
            ServerToNodeMsg::DrainAndShutdown {
                reason: "provisioner test completed".to_owned(),
                create_git_bundle: false,
            },
        )
        .await
    {
        tracing::warn!(
            node_id = node_id.as_str(),
            "failed to request drain after the provisioner test: {err:#}"
        );
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    fxg_protocol::discard_with!(
        terminate.send(true),
        "terminate the provisioner test instance"
    );
    state
        .hub()
        .detach_ephemeral(&NodeId::from(node_id.as_str()), None)
        .await;
    if let Err(err) = state
        .db()
        .set_node_lifecycle(node_id.as_str(), NodeLifecycleStatus::Terminated)
        .await
    {
        tracing::debug!(
            node_id = node_id.as_str(),
            "failed to mark test node terminated: {err}"
        );
    }

    match outcome {
        Ok(Ok(hello_node_id)) => Ok(ProvisionerTestResponse {
            name: name.to_owned(),
            ok: true,
            node_id: Some(hello_node_id.into()),
            log_lines,
            error: None,
        }),
        Ok(Err(_)) => Ok(ProvisionerTestResponse {
            name: name.to_owned(),
            ok: false,
            node_id: None,
            log_lines,
            error: Some("provisioner exited before the daemon handshake".to_owned()),
        }),
        Err(_) => Ok(ProvisionerTestResponse {
            name: name.to_owned(),
            ok: false,
            node_id: None,
            log_lines,
            error: Some(format!(
                "timed out waiting for NodeHello ({}s)",
                TEST_TIMEOUT.as_secs()
            )),
        }),
    }
}

/// Child process launch request.
struct SpawnRequest {
    node_id: String,
    provisioner: String,
    config: ProvisionerConfig,
    env: Vec<(String, String)>,
    mode: SpawnMode,
}

/// Launch mode (for a session / for a connectivity check).
enum SpawnMode {
    /// For a session: send `StartSession` after bootstrap
    Session(Box<PendingStart>),
    /// For a connectivity check: wait for NodeHello and then exit
    Test,
}

/// Information needed to send `StartSession` (used after the handshake).
#[derive(Clone)]
struct PendingStart {
    session_id: String,
    project_id: String,
    agent_id: String,
    initial_prompt: Option<PromptInput>,
    mode: Option<String>,
    extra_args: Option<Vec<String>>,
    fork_context: Option<Vec<ForkHistoryItem>>,
    restore_bundle_b64: Option<String>,
}

/// Handles obtained after launching the child process.
struct SpawnHandles {
    /// Notifies the node ID on NodeHello receipt (closes the channel on failure)
    hello: Option<oneshot::Receiver<String>>,
    /// Child process termination signal
    terminate: watch::Sender<bool>,
    /// Logs collected by `provisioners test`
    test_log: Option<Arc<StdMutex<Vec<String>>>>,
}

/// Launches the provisioner command and starts the stdin/stdout/stderr pumps.
async fn spawn_child(state: &ServerState, request: SpawnRequest) -> Result<SpawnHandles, ApiError> {
    let manager = state.provisioners().clone();
    let bootstrap = compose_bootstrap_script(
        env!("CARGO_PKG_VERSION"),
        matches!(request.mode, SpawnMode::Session(_)),
    );
    let instance_name = format!(
        "fxg-eph-{}",
        &request.node_id[..request.node_id.len().min(24)]
    );

    let mut command = Command::new(&request.config.command);
    for arg in &request.config.args {
        command.arg(
            arg.replace("{BOOTSTRAP_SCRIPT}", &bootstrap)
                .replace("{INSTANCE_NAME}", &instance_name)
                .replace("{SESSION_ID}", &request.node_id),
        );
    }
    for (key, value) in &request.config.env {
        command.env(key, value);
    }
    for (key, value) in &request.env {
        command.env(key, value);
    }
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|err| {
        ApiError::internal(format!(
            "failed to spawn provisioner {}: {err}",
            request.provisioner
        ))
    })?;

    // Windows: assign to a Job Object so the VM/container is reliably terminated on server exit
    let guard = match fxg_pty::ProcessTreeGuard::new() {
        Ok(guard) => {
            #[cfg(windows)]
            if let Some(handle) = child.raw_handle() {
                if let Err(err) = guard.attach_raw_handle(handle) {
                    tracing::warn!(
                        "failed to attach the provisioner to a job object; it may outlive the server: {err}"
                    );
                }
            }
            Some(guard)
        }
        Err(err) => {
            tracing::warn!("failed to create process tree guard: {err}");
            None
        }
    };

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| ApiError::internal("failed to capture provisioner stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::internal("failed to capture provisioner stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::internal("failed to capture provisioner stderr"))?;

    let (hello_tx, hello_rx) = oneshot::channel::<String>();
    let (terminate_tx, terminate_rx) = watch::channel(false);
    let last_activity = Arc::new(StdMutex::new(Instant::now()));
    let test_log: Option<Arc<StdMutex<Vec<String>>>> = match request.mode {
        SpawnMode::Test => Some(Arc::new(StdMutex::new(Vec::new()))),
        SpawnMode::Session(_) => None,
    };

    let node_id = request.node_id.clone();
    let session_id = match &request.mode {
        SpawnMode::Session(pending) => Some(pending.session_id.clone()),
        SpawnMode::Test => None,
    };
    let pump_state = state.clone();
    let pump_manager = manager.clone();
    let pump_node_id = node_id.clone();
    let pump_log = test_log.clone();
    let pump_activity = last_activity.clone();
    let pump_session_id = session_id.clone();

    // stdout pump: owns the child process lifetime and registers the hub connection on NodeHello
    fxg_protocol::spawn_guarded!("ephemeral stdio pump", async move {
        let _guard = guard;
        let mut terminate_rx = terminate_rx;
        let mut child = child;
        let mut stdin_writer = Some(BufWriter::new(stdin));
        let mut stdin_forwarder: Option<tokio::task::JoinHandle<()>> = None;
        let mut hello_tx = Some(hello_tx);
        let mut stdout_lines = BufReader::new(stdout).lines();
        let mut stderr_lines = BufReader::new(stderr).lines();
        let mut attached = false;

        loop {
            tokio::select! {
                _ = terminate_rx.changed() => {
                    if *terminate_rx.borrow() {
                        fxg_protocol::discard_with!(
                            child.start_kill(),
                            "request termination of the ephemeral node child"
                        );
                    }
                    break;
                }
                line = stderr_lines.next_line() => match line {
                    Ok(Some(line)) => {
                        // Stream bootstrap logs to the UI as ephemeral events
                        if let Some(log) = &pump_log
                            && let Ok(mut lines) = log.lock()
                        {
                            lines.push(line.clone());
                        }
                        match &pump_session_id {
                            Some(session_id) => {
                                fxg_protocol::discard_with!(
                                    pump_state.hub().client_events().send(ClientEvent::BootstrapLog {
                                        session_id: session_id.clone().into(),
                                        line,
                                    }),
                                    "stream a bootstrap log line to clients"
                                );
                            }
                            None => tracing::debug!(node_id = pump_node_id, "{line}"),
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        tracing::debug!(node_id = pump_node_id, "stderr read failed: {err}");
                    }
                },
                line = stdout_lines.next_line() => match line {
                    Ok(Some(line)) => {
                        if let Ok(mut activity) = pump_activity.lock() {
                            *activity = Instant::now();
                        }
                        if line.trim().is_empty() {
                            continue;
                        }
                        let message: NodeToServerMsg = match serde_json::from_str(&line) {
                            Ok(message) => message,
                            Err(err) => {
                                tracing::debug!(
                                    node_id = pump_node_id,
                                    "stdout line is not a node message (skipped): {err}"
                                );
                                continue;
                            }
                        };
                        if let NodeToServerMsg::NodeHello {
                            node_id: hello_node_id,
                            ..
                        } = &message
                            && !attached
                        {
                            attached = true;
                            // Register the hub connection and forward commands (StartSession etc.)
                            // to the child process's stdin
                            let (mut commands, _conn_id) =
                                pump_state.hub().attach_ephemeral(&NodeId::from(pump_node_id.as_str())).await;
                            if let Some(mut writer) = stdin_writer.take() {
                                stdin_forwarder = Some(tokio::spawn(async move {
                                    while let Some(message) = commands.recv().await {
                                        let mut payload = match serde_json::to_vec(&message) {
                                            Ok(payload) => payload,
                                            Err(err) => {
                                                tracing::warn!(
                                                    "failed to encode node message: {err}"
                                                );
                                                continue;
                                            }
                                        };
                                        payload.push(b'\n');
                                        if writer.write_all(&payload).await.is_err()
                                            || writer.flush().await.is_err()
                                        {
                                            break;
                                        }
                                    }
                                }));
                            }
                            if let Some(tx) = hello_tx.take() {
                                fxg_protocol::discard_with!(
                                    tx.send(hello_node_id.as_str().to_owned()),
                                    "report the ephemeral node id to a waiter that may be gone"
                                );
                            }
                        }
                        if matches!(&message, NodeToServerMsg::DrainComplete { .. }) {
                            pump_manager.take_drain_waiter(&pump_node_id);
                        }
                        hub::handle_stdio_message(&pump_state, &NodeId::from(pump_node_id.as_str()), message).await;
                    }
                    Ok(None) => break,
                    Err(err) => {
                        tracing::warn!(node_id = pump_node_id, "stdout read failed: {err}");
                        break;
                    }
                },
            }
        }

        // Wait for the child process to exit, then clean up (including killed processes)
        let status = child.wait().await;
        if let Some(forwarder) = stdin_forwarder {
            forwarder.abort();
        }
        // If it exited before the handshake, notify the receiver (close the channel)
        drop(hello_tx.take());
        pump_state
            .hub()
            .detach_ephemeral(
                &NodeId::from(pump_node_id.as_str()),
                match status {
                    Ok(status) => status.code(),
                    Err(err) => {
                        tracing::warn!(
                            node_id = pump_node_id,
                            "failed to wait for the ephemeral node: {err}"
                        );
                        None
                    }
                },
            )
            .await;
        pump_manager.remove_instance(&pump_node_id);
        tracing::info!(node_id = pump_node_id, "ephemeral node instance ended");
    });

    // Register the instance for idle monitoring (keeps the session info)
    manager.insert_instance(Instance {
        node_id,
        session_id,
        terminate: terminate_tx.clone(),
        last_activity,
    });

    Ok(SpawnHandles {
        hello: Some(hello_rx),
        terminate: terminate_tx,
        test_log,
    })
}

/// Waits for the handshake and sends `StartSession`.
async fn await_handshake_and_start(
    state: ServerState,
    node_id: String,
    hello: oneshot::Receiver<String>,
    pending: PendingStart,
) {
    let handshake = tokio::time::timeout(HELLO_TIMEOUT, hello).await;
    let hello_node_id = match handshake {
        Ok(Ok(hello_node_id)) => hello_node_id,
        Ok(Err(_)) => {
            fail_session(
                &state,
                &pending.session_id,
                "provisioner exited before the daemon handshake",
            )
            .await;
            return;
        }
        Err(_) => {
            fail_session(
                &state,
                &pending.session_id,
                "timed out waiting for the ephemeral node handshake",
            )
            .await;
            drain(&state, &node_id, "bootstrap timeout", false).await;
            return;
        }
    };
    if !hello_node_id.is_empty() && hello_node_id != node_id {
        tracing::warn!(
            node_id = node_id.as_str(),
            hello_node_id,
            "ephemeral node reported an unexpected node_id"
        );
    }

    let command_id = CommandId::from(uuid_v7());
    let message = ServerToNodeMsg::StartSession {
        command_id: command_id.clone(),
        session_id: pending.session_id.clone().into(),
        project_id: pending.project_id.clone().into(),
        local_path: EPHEMERAL_WORKSPACE.to_owned(),
        agent_id: pending.agent_id.clone(),
        initial_prompt: pending.initial_prompt.clone(),
        mode: pending.mode.clone(),
        extra_args: pending.extra_args.clone(),
        fork_context_messages: pending.fork_context.clone(),
        restore_git_bundle_b64: pending.restore_bundle_b64.clone(),
    };
    match state
        .hub()
        .command(&NodeId::from(node_id.as_str()), &command_id, message)
        .await
    {
        Ok(result) if result.success => {
            tracing::info!(
                node_id = node_id.as_str(),
                session_id = pending.session_id,
                "ephemeral session started"
            );
        }
        Ok(result) => {
            fail_session(
                &state,
                &pending.session_id,
                result
                    .error
                    .as_ref()
                    .map(|error| error.message.as_str())
                    .unwrap_or("start session failed"),
            )
            .await;
        }
        Err(err) => {
            fail_session(&state, &pending.session_id, &err.message).await;
        }
    }
}

/// Records a provisioning session as failed (best effort).
async fn fail_session(state: &ServerState, session_id: &str, reason: &str) {
    tracing::warn!(session_id, reason, "ephemeral session failed");
    if let Err(err) = state
        .db()
        .set_provisional_session_status(session_id, SessionStatus::Error)
        .await
    {
        tracing::debug!(session_id, "failed to record session error: {err}");
    }
}

/// Monitors idle timeout / end of all sessions for a temporary node.
async fn monitor_instance(state: ServerState, node_id: String, idle_timeout: Duration) {
    loop {
        tokio::time::sleep(MONITOR_INTERVAL).await;
        let manager = state.provisioners();
        let Some(activity) = manager.activity(&node_id) else {
            return;
        };
        let idle = match activity.lock() {
            Ok(value) => value.elapsed(),
            Err(err) => {
                tracing::error!(node_id = node_id.as_str(), "activity lock poisoned: {err}");
                continue;
            }
        };
        if idle >= idle_timeout {
            tracing::info!(
                node_id = node_id.as_str(),
                "ephemeral node idle timeout; draining"
            );
            drain(&state, &node_id, "idle timeout", true).await;
            return;
        }
        // If the session has ended, discard the node as idle
        if let Some(session_id) = manager.session_id(&node_id)
            && let Ok(Some(session)) = state.db().get_session(&session_id).await
            && matches!(
                session.status,
                SessionStatus::Stopped | SessionStatus::Error
            )
        {
            tracing::info!(
                node_id = node_id.as_str(),
                session_id,
                "ephemeral session ended; draining"
            );
            drain(&state, &node_id, "session ended", true).await;
            return;
        }
    }
}

/// Sends `DrainAndShutdown` and discards the node after `DrainComplete` (or a timeout).
///
/// Best effort: events not yet sent are lost if the node crashes before draining.
pub(crate) async fn drain(
    state: &ServerState,
    node_id: &str,
    reason: &str,
    create_git_bundle: bool,
) -> bool {
    if !state.provisioners().is_ephemeral(node_id) {
        return false;
    }
    let drain_rx = state.provisioners().register_drain_waiter(node_id);
    tracing::info!(node_id, reason, "draining ephemeral node");
    if let Err(err) = state
        .hub()
        .send(
            &NodeId::from(node_id),
            ServerToNodeMsg::DrainAndShutdown {
                reason: reason.to_owned(),
                create_git_bundle,
            },
        )
        .await
    {
        tracing::warn!(node_id, "failed to request drain: {err:#}");
    }
    let drained = tokio::time::timeout(DRAIN_TIMEOUT, drain_rx).await.is_ok();
    if !drained {
        tracing::warn!(node_id, "drain timed out; forcing termination");
    }
    state.provisioners().terminate(node_id);
    drained
}

/// Node IDs of all running temporary VMs (for shutdown handling).
pub fn running_node_ids(state: &ServerState) -> Vec<String> {
    match state.provisioners().inner.instances.lock() {
        Ok(instances) => instances.keys().cloned().collect(),
        Err(err) => {
            tracing::error!("instances lock poisoned: {err}");
            Vec::new()
        }
    }
}

/// Accumulates `WorkspaceBundleUpload` chunks.
pub(crate) fn on_bundle_upload(
    state: &ServerState,
    session_id: &str,
    branch: &str,
    head_commit: &str,
    chunk: &str,
) {
    state
        .provisioners()
        .push_bundle_chunk(session_id, branch, head_commit, chunk);
}

/// Handling when `DrainComplete` is received (save the bundle + notify drain waiters).
pub(crate) async fn on_drain_complete(state: &ServerState, node_id: &str) {
    state.provisioners().take_drain_waiter(node_id);
    if let Some(session_id) = state.provisioners().session_id(node_id) {
        save_bundle(state, &session_id).await;
    }
}

/// Concatenates and decodes the accumulated bundle chunks and saves them to a file.
async fn save_bundle(state: &ServerState, session_id: &str) {
    let Some(bundle) = state.provisioners().take_bundle(session_id) else {
        return;
    };
    let encoded: String = bundle.chunks.concat();
    let bytes = match base64::engine::general_purpose::STANDARD.decode(encoded.as_bytes()) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(session_id, "failed to decode workspace bundle: {err}");
            return;
        }
    };
    let dir = state.provisioners().fxg_home().join(BUNDLES_DIR_NAME);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!(session_id, "failed to create bundles dir: {err}");
        return;
    }
    let path = dir.join(format!("{session_id}.bundle"));
    if let Err(err) = std::fs::write(&path, &bytes) {
        tracing::warn!(session_id, "failed to write workspace bundle: {err}");
        return;
    }
    if let Err(err) = state
        .db()
        .set_git_bundle_path(session_id, Some(&path.to_string_lossy()))
        .await
    {
        tracing::warn!(session_id, "failed to record git_bundle_path: {err}");
        return;
    }
    tracing::info!(
        session_id,
        path = %path.display(),
        bytes = bytes.len(),
        head_commit = bundle.head_commit,
        branch = bundle.branch,
        "workspace bundle stored"
    );
}

/// Terminates all running temporary VMs (on server shutdown).
pub async fn terminate_all(state: &ServerState) {
    for node_id in running_node_ids(state) {
        if let Err(err) = state
            .hub()
            .send(
                &NodeId::from(node_id.as_str()),
                ServerToNodeMsg::DrainAndShutdown {
                    reason: "server shutting down".to_owned(),
                    create_git_bundle: false,
                },
            )
            .await
        {
            tracing::warn!(node_id = %node_id,
                "failed to request drain during shutdown; terminating instead: {err:#}"
            );
        }
        state.provisioners().terminate(&node_id);
    }
}

/// Resolves the short-lived token for the Git URL's host (`None` when not found).
pub(crate) async fn resolve_git_token(
    state: &ServerState,
    git_url: &str,
) -> Option<crate::credentials::CredentialResponse> {
    let host = url_host(git_url)?;
    let credentials = state.provisioners().git_credentials();
    let config = credentials.get(&host).or_else(|| credentials.get("*"))?;
    match resolve_credential(config).await {
        Ok(response) => Some(response),
        Err(err) => {
            tracing::warn!(host, "failed to resolve git credential: {err}");
            None
        }
    }
}

/// Extracts the host name from a URL (`https://github.com/...` / `git@github.com:...`).
pub(crate) fn url_host(url: &str) -> Option<String> {
    if let Some(rest) = url.split("://").nth(1) {
        let authority = rest.split('/').next()?;
        let host = authority
            .rsplit_once('@')
            .map(|(_, host)| host)
            .unwrap_or(authority);
        let host = host.split(':').next()?;
        return (!host.is_empty()).then(|| host.to_owned());
    }
    // SCP form (`git@github.com:owner/repo.git`)
    if let Some((_, rest)) = url.split_once('@')
        && let Some((host, _)) = rest.split_once(':')
        && !host.is_empty()
        && !host.contains('/')
    {
        return Some(host.to_owned());
    }
    None
}

/// Generates the temporary VM bootstrap script (`{BOOTSTRAP_SCRIPT}`).
///
/// Routes all output to `stderr` (`>&2`) and reserves `stdout` for `fxg daemon --stdio`
/// protocol traffic.
fn compose_bootstrap_script(version: &str, clone_workspace: bool) -> String {
    let clone_step = if clone_workspace {
        format!(
            r#"'{}' bootstrap-workspace --repo "$FXG_GIT_URL" ${{FXG_GIT_BRANCH:+--branch "$FXG_GIT_BRANCH"}} --dir {} >&2
if [ -f "$FXG_HOME/bootstrap.env" ]; then . "$FXG_HOME/bootstrap.env"; fi
exec '{}' daemon --stdio --ephemeral --workspace {}"#,
            EPHEMERAL_BINARY, EPHEMERAL_WORKSPACE, EPHEMERAL_BINARY, EPHEMERAL_WORKSPACE
        )
    } else {
        // Connectivity check: skip the clone and only verify the daemon handshake
        format!(
            "exec '{}' daemon --stdio --ephemeral --workspace /tmp",
            EPHEMERAL_BINARY
        )
    };

    format!(
        r#"set -eu
export FXG_HOME="${{FXG_HOME:-$HOME/.flexagent}}"
mkdir -p "$FXG_HOME"
if ! command -v git >/dev/null 2>&1 || ! command -v curl >/dev/null 2>&1; then
  if command -v apt-get >/dev/null 2>&1; then
    (apt-get update && apt-get install -y --no-install-recommends git curl ca-certificates) >&2 || true
  fi
fi
fxg_os=$(uname -s | tr '[:upper:]' '[:lower:]')
case "$fxg_os" in
  linux) fxg_os=linux ;;
  darwin) fxg_os=macos ;;
  *) fxg_os=linux ;;
esac
fxg_arch=$(uname -m)
case "$fxg_arch" in
  x86_64|amd64) fxg_arch=x86_64 ;;
  aarch64|arm64) fxg_arch=aarch64 ;;
  *) fxg_arch=x86_64 ;;
esac
FXG_BINARY_URL="${{FXG_BINARY_URL:-https://github.com/nazo6/flexagent/releases/download/v{version}/fxg-$fxg_os-$fxg_arch}}"
echo "[fxg-bootstrap] downloading fxg ($fxg_os-$fxg_arch) from $FXG_BINARY_URL" >&2
curl -fsSL "$FXG_BINARY_URL" -o '{binary}' >&2
chmod +x '{binary}'
{clone_step}"#,
        version = version,
        binary = EPHEMERAL_BINARY,
        clone_step = clone_step,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_script_separates_stdout_and_starts_daemon() {
        let script = compose_bootstrap_script("9.9.9", true);
        assert!(script.contains("bootstrap-workspace"));
        assert!(
            script
                .contains("exec '/tmp/fxg' daemon --stdio --ephemeral --workspace /tmp/workspace")
        );
        assert!(script.contains("v9.9.9"));
        assert!(script.contains(">&2"));
    }

    #[test]
    fn test_script_skips_workspace_clone() {
        let script = compose_bootstrap_script("9.9.9", false);
        assert!(!script.contains("bootstrap-workspace"));
        assert!(script.contains("daemon --stdio --ephemeral --workspace /tmp"));
    }

    #[test]
    fn url_host_extracts_from_https_and_scp_forms() {
        assert_eq!(
            url_host("https://github.com/nazo6/flexagent.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            url_host("https://user@gitlab.example.com:8443/x/y").as_deref(),
            Some("gitlab.example.com")
        );
        assert_eq!(
            url_host("git@github.com:nazo6/flexagent.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(url_host("github.com/nazo6/flexagent"), None);
    }
}
