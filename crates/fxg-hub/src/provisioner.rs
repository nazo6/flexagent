//! Provisioner management for temporary VM / sandbox nodes (on the hub host).
//!
//! Design: `docs/01-architecture-and-sync.md` §6.1–§6.4, `docs/05-cli-and-pwa-ui.md` §2.2.
//!
//! - Launches the command defined under `[provisioners.<name>]` in `config.toml` as a child
//!   process and uses its `stdin` / `stdout` as the JSON Lines transport for
//!   `NodeToHubMsg` / `HubToNodeMsg` (a direct connection to `fxg internal stdio`)
//! - Injects `FXG_GIT_URL` / `FXG_GIT_BRANCH` / a short-lived `FXG_GIT_TOKEN` and the ephemeral
//!   node ID (`FXG_NODE_ID`) as environment variables at provisioner startup
//! - Streams `stderr` to the UI as `ProvisioningLog` (ephemeral)
//! - After the handshake (NodeHello), registers the hub connection before sending
//!   `StartSession` to start the session (the node is "offline" during bootstrap)
//! - On idle timeout / when all sessions end, sends `DrainAndShutdown` and discards the child
//!   process (VM/container) after `DrainComplete`

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant};

use fxg_protocol::client_api::ProvisionerTestResponse;
use fxg_protocol::common::{NodeLifecycleStatus, NodeProjectReport, PromptInput, SessionStatus};
use fxg_protocol::node_hub::{HubToNodeMsg, NodeToHubMsg};
use fxg_protocol::settings::ProvisionerDefinition;
use fxg_protocol::util::uuid_v7;
use fxg_protocol::{CommandId, NodeId};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::Command;
use tokio::sync::{oneshot, watch};

use crate::api::{ApiError, ClientEvent};
use crate::hub;
use crate::state::HubState;

/// Upper bound for waiting on bootstrap completion (NodeHello).
const HELLO_TIMEOUT: Duration = Duration::from_secs(1800);
/// Upper bound for waiting on `DrainComplete`.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(60);
/// Tick interval for idle monitoring.
const MONITOR_INTERVAL: Duration = Duration::from_secs(5);
/// Upper bound for the `provisioners test` handshake wait.
const TEST_TIMEOUT: Duration = Duration::from_secs(300);
/// Number of provisioning log lines kept by `provisioners test`.
const TEST_LOG_LINES: usize = 200;

/// Runtime state of one temporary VM instance (keyed by node_id).
struct Instance {
    node_id: String,
    session_id: Option<String>,
    /// Child process termination signal (the pump task kills and drops the Job Object on it)
    terminate: watch::Sender<bool>,
    /// Time the last message was received (used for idle timeout decisions)
    last_activity: Arc<StdMutex<Instant>>,
}

struct ProvisionerInner {
    fxg_home: PathBuf,
    configs: RwLock<BTreeMap<String, ProvisionerDefinition>>,
    instances: StdMutex<HashMap<String, Instance>>,
    drain_waiters: StdMutex<HashMap<String, oneshot::Sender<()>>>,
}

/// Management of temporary VM provisioners (one instance is held by the hub).
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
        let provisioners = self
            .inner
            .configs
            .read()
            .map(|configs| configs.len())
            .unwrap_or(0);
        f.debug_struct("ProvisionerManager")
            .field("provisioners", &provisioners)
            .field("instances", &instances)
            .finish()
    }
}

impl ProvisionerManager {
    /// Creates the manager from configuration.
    pub fn new(fxg_home: PathBuf, configs: BTreeMap<String, ProvisionerDefinition>) -> Self {
        Self {
            inner: Arc::new(ProvisionerInner {
                fxg_home,
                configs: RwLock::new(configs),
                instances: StdMutex::new(HashMap::new()),
                drain_waiters: StdMutex::new(HashMap::new()),
            }),
        }
    }

    /// Configured provisioner definitions (snapshot).
    pub fn configs(&self) -> BTreeMap<String, ProvisionerDefinition> {
        self.inner
            .configs
            .read()
            .expect("provisioner configs lock poisoned")
            .clone()
    }

    /// Fetches a configuration (unknown names yield `NOT_FOUND`).
    pub fn config(&self, name: &str) -> Result<ProvisionerDefinition, ApiError> {
        self.inner
            .configs
            .read()
            .expect("provisioner configs lock poisoned")
            .get(name)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("provisioner not found: {name}")))
    }

    /// Replaces the provisioner set after a settings update.
    pub fn apply_settings(&self, configs: BTreeMap<String, ProvisionerDefinition>) {
        *self
            .inner
            .configs
            .write()
            .expect("provisioner configs lock poisoned") = configs;
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
}

/// Session start request (provisioner launch + StartSession).
#[derive(Debug, Clone)]
pub struct SpawnSessionRequest {
    /// Hub-assigned session ID
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
    /// Initial session configuration
    pub initial_config: Vec<fxg_protocol::common::InitialConfig>,
    /// Initial mode
    pub mode: Option<String>,
    /// Extra arguments for the agent
    pub extra_args: Option<Vec<String>>,
}

/// Launches a provisioner for a session (sends StartSession after the handshake).
pub async fn spawn_session(state: &HubState, request: SpawnSessionRequest) -> Result<(), ApiError> {
    let config = state.provisioners().config(&request.provisioner)?;
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

    let pending = PendingStart {
        session_id: session_id.clone(),
        project_id: request.project_id.clone(),
        agent_id: request.agent_id.clone(),
        initial_prompt: request.initial_prompt.clone(),
        initial_config: request.initial_config.clone(),
        mode: request.mode.clone(),
        extra_args: request.extra_args.clone(),
    };

    let spawn = SpawnRequest {
        node_id: node_id.clone(),
        session_id: Some(session_id.clone()),
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
    state: &HubState,
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

/// Provisioner connectivity check: validates provisioner startup and the NodeHello handshake.
pub async fn provisioner_test(
    state: &HubState,
    name: &str,
) -> Result<ProvisionerTestResponse, ApiError> {
    let config = state.provisioners().config(name)?;
    let node_id = format!("fxg-test-{}", &uuid_v7()[..8]);

    let spawn = SpawnRequest {
        node_id: node_id.clone(),
        session_id: None,
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
            HubToNodeMsg::DrainAndShutdown {
                reason: "provisioner test completed".to_owned(),
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
        Ok(Ok(handshake)) => Ok(ProvisionerTestResponse {
            name: name.to_owned(),
            ok: true,
            node_id: Some(handshake.node_id.into()),
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
    session_id: Option<String>,
    provisioner: String,
    config: ProvisionerDefinition,
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
    initial_config: Vec<fxg_protocol::common::InitialConfig>,
    mode: Option<String>,
    extra_args: Option<Vec<String>>,
}

/// Handshake information reported by an ephemeral node.
#[derive(Clone, Debug)]
struct EphemeralHandshake {
    /// Node ID reported by `NodeHello`
    node_id: String,
    /// Projects / workspace bindings reported by `NodeHello`
    projects: Vec<NodeProjectReport>,
}

/// Replaces the provisioner argument placeholders.
fn substitute_placeholders(
    arg: &str,
    instance_name: &str,
    session_id: &str,
    node_id: &str,
) -> String {
    arg.replace("{INSTANCE_NAME}", instance_name)
        .replace("{SESSION_ID}", session_id)
        .replace("{NODE_ID}", node_id)
}

/// Handles obtained after launching the child process.
struct SpawnHandles {
    /// Notifies the handshake information on NodeHello receipt (closes the
    /// channel on failure)
    hello: Option<oneshot::Receiver<EphemeralHandshake>>,
    /// Child process termination signal
    terminate: watch::Sender<bool>,
    /// Logs collected by `provisioners test`
    test_log: Option<Arc<StdMutex<Vec<String>>>>,
}

/// Launches the provisioner command and starts the stdin/stdout/stderr pumps.
async fn spawn_child(state: &HubState, request: SpawnRequest) -> Result<SpawnHandles, ApiError> {
    let manager = state.provisioners().clone();
    let instance_name = format!(
        "fxg-eph-{}",
        &request.node_id[..request.node_id.len().min(24)]
    );

    let mut command = Command::new(&request.config.command);
    let session_id = request.session_id.as_deref().unwrap_or(&request.node_id);
    for arg in &request.config.args {
        command.arg(substitute_placeholders(
            arg,
            &instance_name,
            session_id,
            &request.node_id,
        ));
    }
    for (key, value) in &request.config.env {
        command.env(key, value);
    }
    for (key, secret_id) in &request.config.env_secrets {
        let value =
            fxg_db::secrets::read_secret(manager.fxg_home(), fxg_db::secrets::ROLE_HUB, secret_id)
                .map_err(|err| {
                    ApiError::internal(format!("failed to read secret {secret_id}: {err}"))
                })?
                .ok_or_else(|| {
                    ApiError::bad_request(format!(
                        "provisioner {} secret {secret_id} is not configured",
                        request.provisioner
                    ))
                })?;
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
                        "failed to attach the provisioner to a job object; it may outlive the hub: {err}"
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

    let (hello_tx, hello_rx) = oneshot::channel::<EphemeralHandshake>();
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
                                    pump_state.hub().client_events().send(ClientEvent::ProvisioningLog {
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
                        let message: NodeToHubMsg = match serde_json::from_str(&line) {
                            Ok(message) => message,
                            Err(err) => {
                                tracing::debug!(
                                    node_id = pump_node_id,
                                    "stdout line is not a node message (skipped): {err}"
                                );
                                continue;
                            }
                        };
                        if let NodeToHubMsg::NodeHello {
                            node_id: hello_node_id,
                            projects,
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
                                    tx.send(EphemeralHandshake {
                                        node_id: hello_node_id.as_str().to_owned(),
                                        projects: projects.clone(),
                                    }),
                                    "report the ephemeral handshake to a waiter that may be gone"
                                );
                            }
                        }
                        if matches!(&message, NodeToHubMsg::DrainComplete { .. }) {
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

/// Selects the workspace path reported by the ephemeral node's `NodeHello`.
fn workspace_path(project_id: &str, projects: &[NodeProjectReport]) -> Option<String> {
    fn first_binding(project: &NodeProjectReport) -> Option<String> {
        project
            .bindings
            .iter()
            .find(|binding| binding.path_exists)
            .or_else(|| project.bindings.first())
            .map(|binding| binding.local_path.clone())
    }

    projects
        .iter()
        .find(|project| project.project_id.as_str() == project_id)
        .and_then(first_binding)
        .or_else(|| projects.iter().find_map(first_binding))
}

/// Waits for the handshake and sends `StartSession`.
async fn await_handshake_and_start(
    state: HubState,
    node_id: String,
    hello: oneshot::Receiver<EphemeralHandshake>,
    pending: PendingStart,
) {
    let handshake = tokio::time::timeout(HELLO_TIMEOUT, hello).await;
    let EphemeralHandshake {
        node_id: hello_node_id,
        projects,
    } = match handshake {
        Ok(Ok(handshake)) => handshake,
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
            drain(&state, &node_id, "provisioning timeout").await;
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

    let Some(local_path) = workspace_path(&pending.project_id, &projects) else {
        fail_session(
            &state,
            &pending.session_id,
            "ephemeral node reported no workspace; the provisioner must start `fxg internal stdio --workspace <dir>`",
        )
        .await;
        drain(&state, &node_id, "missing workspace").await;
        return;
    };

    let command_id = CommandId::from(uuid_v7());
    let message = HubToNodeMsg::StartSession {
        command_id: command_id.clone(),
        session_id: pending.session_id.clone().into(),
        project_id: pending.project_id.clone().into(),
        local_path,
        agent_id: pending.agent_id.clone(),
        initial_prompt: pending.initial_prompt.clone(),
        initial_config: pending.initial_config.clone(),
        mode: pending.mode.clone(),
        extra_args: pending.extra_args.clone(),
        fork_from_session_id: None,
        fork_up_to_node_seq: None,
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
async fn fail_session(state: &HubState, session_id: &str, reason: &str) {
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
async fn monitor_instance(state: HubState, node_id: String, idle_timeout: Duration) {
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
            drain(&state, &node_id, "idle timeout").await;
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
            drain(&state, &node_id, "session ended").await;
            return;
        }
    }
}

/// Sends `DrainAndShutdown` and discards the node after `DrainComplete` (or a timeout).
///
/// Best effort: events not yet sent are lost if the node crashes before draining.
pub(crate) async fn drain(state: &HubState, node_id: &str, reason: &str) -> bool {
    if !state.provisioners().is_ephemeral(node_id) {
        return false;
    }
    let drain_rx = state.provisioners().register_drain_waiter(node_id);
    tracing::info!(node_id, reason, "draining ephemeral node");
    if let Err(err) = state
        .hub()
        .send(
            &NodeId::from(node_id),
            HubToNodeMsg::DrainAndShutdown {
                reason: reason.to_owned(),
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
pub fn running_node_ids(state: &HubState) -> Vec<String> {
    match state.provisioners().inner.instances.lock() {
        Ok(instances) => instances.keys().cloned().collect(),
        Err(err) => {
            tracing::error!("instances lock poisoned: {err}");
            Vec::new()
        }
    }
}

/// Handling when `DrainComplete` is received (notifies drain waiters).
pub(crate) fn on_drain_complete(state: &HubState, node_id: &str) {
    state.provisioners().take_drain_waiter(node_id);
}

/// Terminates all running temporary VMs (on server shutdown).
pub async fn terminate_all(state: &HubState) {
    for node_id in running_node_ids(state) {
        if let Err(err) = state
            .hub()
            .send(
                &NodeId::from(node_id.as_str()),
                HubToNodeMsg::DrainAndShutdown {
                    reason: "server shutting down".to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_provisioner_placeholders() {
        assert_eq!(
            substitute_placeholders(
                "run --name {INSTANCE_NAME} --label {SESSION_ID} --node {NODE_ID}",
                "fxg-eph-1",
                "session-1",
                "node-1",
            ),
            "run --name fxg-eph-1 --label session-1 --node node-1"
        );
    }

    #[test]
    fn workspace_path_prefers_the_pending_project() {
        let binding = |path: &str| fxg_protocol::common::ProjectNodeBinding {
            local_path: path.to_owned(),
            is_worktree: false,
            path_exists: true,
            git_branch: None,
            last_used_at: 0,
        };
        let project = |id: &str, path: &str| NodeProjectReport {
            project_id: id.into(),
            name: id.to_owned(),
            canonical_git_url: None,
            identity: fxg_protocol::settings::ProjectIdentity::git_remote(),
            bindings: vec![binding(path)],
        };
        let projects = vec![project("other", "/other"), project("wanted", "/wanted")];
        assert_eq!(
            workspace_path("wanted", &projects).as_deref(),
            Some("/wanted")
        );
        assert_eq!(
            workspace_path("missing", &projects).as_deref(),
            Some("/other")
        );
    }
}
