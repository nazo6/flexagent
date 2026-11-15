#![allow(
    clippy::disallowed_types,
    reason = "OpenCode v2 SSE/service.json payloads are parsed at this external boundary"
)]

use super::*;
use eventsource_stream::Eventsource;

pub(super) struct Server {
    pub(super) client: OpenCodeClient,
    pub(super) owned: Option<OwnedServer>,
}

pub(super) struct OwnedServer {
    pub(super) child: Mutex<Option<tokio::process::Child>>,
    pub(super) _guard: Option<fxg_pty::ProcessTreeGuard>,
}

impl Server {
    pub(super) fn is_owned(&self) -> bool {
        self.owned.is_some()
    }

    pub(super) async fn is_alive(&self) -> bool {
        let Some(owned) = &self.owned else {
            // A discovered or injected server has no child process to inspect.
            // Probe it so a stale service is re-discovered or replaced by a
            // private server instead of being retried forever.
            return tokio::time::timeout(SERVER_HEALTH_TIMEOUT, self.client.version())
                .await
                .is_ok_and(|result| result.is_ok());
        };
        let mut child = owned.child.lock().await;
        match child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    pub(super) async fn stop(&self) {
        let Some(owned) = &self.owned else {
            return;
        };
        let mut child = owned.child.lock().await;
        if let Some(child) = child.as_mut() {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                // SAFETY: `pid` is a valid process group leader created by `process_group(0)`.
                unsafe {
                    libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
                }
            }
            if let Err(err) = child.start_kill() {
                debug!("failed to stop opencode serve: {err}");
            }
            if let Err(err) = child.wait().await {
                debug!("failed to wait for opencode serve: {err}");
            }
        }
        *child = None;
    }
}

/// Native OpenCode v2 driver.
pub struct OpenCodeNativeDriver {
    pub(super) inner: Arc<DriverInner>,
}

impl Default for OpenCodeNativeDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenCodeNativeDriver {
    /// Creates a driver instance shared across all sessions of the node.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DriverInner {
                server: Mutex::new(None),
                injected: None,
                runtimes: std::sync::Mutex::new(HashMap::new()),
                children: std::sync::Mutex::new(HashMap::new()),
                sse_running: AtomicBool::new(false),
                active_ops: std::sync::atomic::AtomicUsize::new(0),
                sync: Mutex::new(()),
                stderr: std::sync::Mutex::new(Vec::new()),
            }),
        }
    }

    /// Creates a driver bound to an explicit server (tests / embedded clients).
    #[must_use]
    pub fn with_server(base_url: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(DriverInner {
                server: Mutex::new(None),
                injected: Some(OpenCodeClient::new(base_url, password)),
                runtimes: std::sync::Mutex::new(HashMap::new()),
                children: std::sync::Mutex::new(HashMap::new()),
                sse_running: AtomicBool::new(false),
                active_ops: std::sync::atomic::AtomicUsize::new(0),
                sync: Mutex::new(()),
                stderr: std::sync::Mutex::new(Vec::new()),
            }),
        }
    }

    /// Connects to an OpenCode service discovered on this machine.
    ///
    /// Used by the node management API. Does not start a server.
    pub async fn connect_management() -> anyhow::Result<OpenCodeClient> {
        if let Some(client) = env_client() {
            return Ok(client);
        }
        if let Some(client) = discover_service().await? {
            return Ok(client);
        }
        Err(anyhow!(
            "OpenCode is not running (start `opencode serve` or `opencode service start`)"
        ))
    }
}

pub(super) struct DriverInner {
    pub(super) server: Mutex<Option<Arc<Server>>>,
    /// Explicit server for tests / embedded clients (skips discovery).
    pub(super) injected: Option<OpenCodeClient>,
    pub(super) runtimes: std::sync::Mutex<HashMap<String, Arc<SessionRuntime>>>,
    /// child agent session ID -> (root agent session ID, depth, parent)
    pub(super) children: std::sync::Mutex<HashMap<String, ChildInfo>>,
    pub(super) sse_running: AtomicBool,
    /// Operations that currently need the server (session start / probe / list).
    /// `stop_if_idle` must not stop a server while one is in flight.
    pub(super) active_ops: std::sync::atomic::AtomicUsize,
    /// Serializes reconnect state syncs so overlapping `server.connected`
    /// frames do not duplicate backfill work.
    pub(super) sync: Mutex<()>,
    pub(super) stderr: std::sync::Mutex<Vec<String>>,
}

/// Keeps an operation counted for `DriverInner::active_ops`.
pub(super) struct ActiveOp<'a>(pub(super) &'a std::sync::atomic::AtomicUsize);

impl Drop for ActiveOp<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Debug)]
pub(super) struct ChildInfo {
    pub(super) root: String,
    pub(super) parent: Option<String>,
    pub(super) depth: u32,
    pub(super) title: Option<String>,
    pub(super) status: SubagentStatus,
}

impl DriverInner {
    pub(super) async fn ensure_server(
        self: &Arc<Self>,
        launch: &AgentLaunchSpec,
        cwd: &Path,
    ) -> anyhow::Result<Arc<Server>> {
        {
            let mut guard = self.server.lock().await;
            if let Some(server) = guard.as_ref() {
                if server.is_alive().await {
                    let server = Arc::clone(server);
                    drop(guard);
                    // An existing server may have been stored before its SSE
                    // pump started; make sure exactly one pump is running.
                    start_sse(self);
                    return Ok(server);
                }
                *guard = None;
            }
        }

        let server = if let Some(client) = self.injected.clone().or_else(env_client) {
            Server {
                client,
                owned: None,
            }
        } else if let Some(client) = discover_service().await? {
            Server {
                client,
                owned: None,
            }
        } else {
            spawn_server(self, launch, cwd).await?
        };
        verify_version(&server.client).await?;
        let server = Arc::new(server);
        {
            let mut guard = self.server.lock().await;
            if let Some(existing) = guard.as_ref() {
                let existing = Arc::clone(existing);
                drop(guard);
                start_sse(self);
                return Ok(existing);
            }
            *guard = Some(Arc::clone(&server));
        }
        start_sse(self);
        info!(base_url = %server.client.base_url, "OpenCode native driver connected");
        Ok(server)
    }

    pub(super) async fn server(&self) -> Option<Arc<Server>> {
        self.server.lock().await.clone()
    }

    pub(super) async fn invalidate_server(&self, failed: &Arc<Server>) {
        let mut guard = self.server.lock().await;
        if guard.as_ref().is_some_and(|s| Arc::ptr_eq(s, failed)) {
            *guard = None;
        }
    }

    pub(super) async fn stop_if_idle(&self) -> bool {
        if self.active_ops.load(Ordering::SeqCst) > 0 {
            return false;
        }
        let idle = self.runtimes.lock().expect("runtimes poisoned").is_empty();
        if !idle {
            return false;
        }
        let server = {
            let mut guard = self.server.lock().await;
            if guard.as_ref().is_some_and(|s| s.is_owned()) {
                guard.take()
            } else {
                None
            }
        };
        if let Some(server) = server {
            server.stop().await;
            return true;
        }
        false
    }

    pub(super) fn register(&self, runtime: Arc<SessionRuntime>) {
        self.runtimes
            .lock()
            .expect("runtimes poisoned")
            .insert(runtime.agent_session_id.clone(), runtime);
    }

    pub(super) fn unregister(&self, agent_session_id: &str) {
        self.runtimes
            .lock()
            .expect("runtimes poisoned")
            .remove(agent_session_id);
        self.children
            .lock()
            .expect("children poisoned")
            .retain(|_, child| child.root != agent_session_id);
    }

    pub(super) fn runtime_for(
        &self,
        agent_session_id: &str,
    ) -> Option<(Arc<SessionRuntime>, Option<String>)> {
        let runtimes = self.runtimes.lock().expect("runtimes poisoned");
        if let Some(runtime) = runtimes.get(agent_session_id) {
            return Some((Arc::clone(runtime), None));
        }
        let children = self.children.lock().expect("children poisoned");
        let child = children.get(agent_session_id)?;
        let runtime = runtimes.get(&child.root)?;
        Some((Arc::clone(runtime), Some(agent_session_id.to_owned())))
    }

    pub(super) fn runtimes(&self) -> Vec<Arc<SessionRuntime>> {
        self.runtimes
            .lock()
            .expect("runtimes poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub(super) fn children_for_root(&self, root: &str) -> Vec<String> {
        self.children
            .lock()
            .expect("children poisoned")
            .iter()
            .filter(|(_, child)| child.root == root)
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub(super) fn child_info(&self, child_session_id: &str) -> Option<ChildInfo> {
        self.children
            .lock()
            .expect("children poisoned")
            .get(child_session_id)
            .cloned()
    }

    /// Records a child lifecycle change without downgrading a terminal failure
    /// to `completed` when a later `session.idle` arrives.
    pub(super) fn update_child_status(&self, child_session_id: &str, status: SubagentStatus) {
        let mut children = self.children.lock().expect("children poisoned");
        let Some(child) = children.get_mut(child_session_id) else {
            return;
        };
        if status == SubagentStatus::Completed
            && matches!(
                child.status,
                SubagentStatus::Failed | SubagentStatus::Cancelled
            )
        {
            return;
        }
        child.status = status;
    }

    pub(super) fn note_stderr(&self, line: String) {
        let mut lines = self.stderr.lock().expect("stderr poisoned");
        lines.push(line);
        if lines.len() > MAX_STDERR_LINES {
            let drain = lines.len() - MAX_STDERR_LINES;
            lines.drain(0..drain);
        }
    }
}

pub(super) fn env_client() -> Option<OpenCodeClient> {
    let url = env_var("FXG_OPENCODE_URL")?;
    let password = env_var("FXG_OPENCODE_PASSWORD").unwrap_or_default();
    if url.trim().is_empty() {
        return None;
    }
    Some(OpenCodeClient::new(url, password))
}

pub(super) async fn discover_service() -> anyhow::Result<Option<OpenCodeClient>> {
    let Some(path) = service_json_path() else {
        return Ok(None);
    };
    let text = match tokio::fs::read_to_string(&path).await {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(err) => {
            // A partially written or stale service file must not prevent the
            // driver from starting its own private server.
            debug!(
                path = %path.display(),
                "ignoring invalid OpenCode service file: {err}"
            );
            return Ok(None);
        }
    };
    let Some(url) = value.get("url").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(password) = value.get("password").and_then(Value::as_str) else {
        return Ok(None);
    };
    let client = OpenCodeClient::new(url, password);
    match client.version().await {
        Ok(_) => Ok(Some(client)),
        Err(err) => {
            debug!(%url, "stale OpenCode service file: {err:#}");
            Ok(None)
        }
    }
}

pub(super) fn service_json_path() -> Option<PathBuf> {
    dirs::state_dir()
        .map(|dir| dir.join("opencode/service.json"))
        .or_else(|| {
            std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .map(|dir| dir.join("opencode/service.json"))
        })
        .or_else(|| dirs::home_dir().map(|dir| dir.join(".local/state/opencode/service.json")))
}

#[allow(
    clippy::manual_ok_err,
    reason = "Result::ok is disallowed by the workspace lint"
)]
pub(super) async fn verify_version(client: &OpenCodeClient) -> anyhow::Result<()> {
    let version = client.version().await?;
    let major = version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|part| match part.parse::<u64>() {
            Ok(value) => Some(value),
            Err(_) => None,
        });
    if major != Some(OPENCODE_MAJOR) {
        anyhow::bail!(
            "OpenCode v2 is required by the native driver (found {})",
            if version.is_empty() {
                "unknown"
            } else {
                &version
            }
        );
    }
    Ok(())
}

pub(super) async fn spawn_server(
    inner: &Arc<DriverInner>,
    launch: &AgentLaunchSpec,
    cwd: &Path,
) -> anyhow::Result<Server> {
    let program = launch
        .program
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("the OpenCode program is not installed"))?;
    let program = fxg_pty::resolve_command(&program.to_string_lossy(), cwd)?;
    let password = format!("{}{}", uuid_v7(), uuid_v7()).replace('-', "");
    let mut command = tokio::process::Command::new(&program);
    if launch.args.is_empty() {
        command.arg("serve");
    } else {
        command.args(&launch.args);
    }
    command
        .arg("--hostname")
        .arg("127.0.0.1")
        .arg("--port")
        .arg("0")
        .env("OPENCODE_PASSWORD", &password)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    for (key, value) in &launch.env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start opencode serve ({})", program.display()))?;
    let guard = match fxg_pty::ProcessTreeGuard::new() {
        Ok(guard) => Some(guard),
        Err(err) => {
            tracing::debug!("failed to create the OpenCode process tree guard: {err}");
            None
        }
    };
    #[cfg(windows)]
    if let (Some(guard), Some(handle)) = (guard.as_ref(), child.raw_handle())
        && let Err(err) = guard.attach_raw_handle(handle)
    {
        tracing::warn!("failed to bind opencode serve to a Windows Job Object: {err}");
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("opencode serve stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("opencode serve stderr is unavailable"))?;
    let (url_tx, mut url_rx) = mpsc::unbounded_channel::<String>();
    spawn_log_pump(inner, stdout, url_tx.clone());
    spawn_log_pump(inner, stderr, url_tx);

    let url = tokio::time::timeout(SERVER_READY_TIMEOUT, async {
        while let Some(line) = url_rx.recv().await {
            if let Some(url) = parse_server_url(&line) {
                return Some(url);
            }
        }
        None
    })
    .await
    .map_err(|err| anyhow!("opencode serve did not become ready in time: {err}"))?
    .ok_or_else(|| anyhow!("opencode serve exited before reporting its address"))?;

    let client = OpenCodeClient::new(&url, &password);
    let healthy = tokio::time::timeout(SERVER_READY_TIMEOUT, async {
        loop {
            match client.version().await {
                Ok(_) => return true,
                Err(_) => tokio::time::sleep(READY_POLL_INTERVAL).await,
            }
        }
    })
    .await
    .unwrap_or(false);
    if !healthy {
        anyhow::bail!("opencode serve did not become healthy at {url}");
    }

    Ok(Server {
        client,
        owned: Some(OwnedServer {
            child: Mutex::new(Some(child)),
            _guard: guard,
        }),
    })
}

pub(super) fn parse_server_url(line: &str) -> Option<String> {
    let marker = "server listening on ";
    let index = line.find(marker)?;
    let rest = line[index + marker.len()..].trim();
    rest.split_whitespace()
        .next()
        .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
        .map(str::to_owned)
}

pub(super) fn spawn_log_pump<R>(
    inner: &Arc<DriverInner>,
    reader: R,
    url_tx: mpsc::UnboundedSender<String>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    let inner = Arc::clone(inner);
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            inner.note_stderr(line.clone());
            if line.contains("server listening on ")
                && let Err(err) = url_tx.send(line.clone())
            {
                debug!("OpenCode URL channel closed: {err}");
            }
            debug!(target: "fxg_opencode", "opencode serve: {line}");
        }
    });
}

/// Starts the SSE pump if one is not already running.
fn start_sse(inner: &Arc<DriverInner>) {
    if !inner.sse_running.swap(true, Ordering::SeqCst) {
        let inner = Arc::clone(inner);
        tokio::spawn(async move {
            run_sse(inner).await;
        });
    }
}

/// Clears the pump flag and starts a new pump if a server was published after
/// the previous lookup (the `stop_if_idle` / start race).
async fn stop_sse_or_adopt_new_server(inner: &Arc<DriverInner>) {
    inner.sse_running.store(false, Ordering::SeqCst);
    if inner.server().await.is_some() {
        start_sse(inner);
    }
}

pub(super) async fn run_sse(inner: Arc<DriverInner>) {
    loop {
        let Some(server) = inner.server().await else {
            stop_sse_or_adopt_new_server(&inner).await;
            return;
        };
        if !server.is_alive().await {
            inner.invalidate_server(&server).await;
            stop_sse_or_adopt_new_server(&inner).await;
            return;
        }
        match stream_events(&inner, &server).await {
            Ok(()) => debug!("OpenCode SSE stream ended"),
            Err(err) => {
                debug!("OpenCode SSE stream failed: {err:#}");
                if !server.is_alive().await {
                    inner.invalidate_server(&server).await;
                    stop_sse_or_adopt_new_server(&inner).await;
                    return;
                }
            }
        }
        // The connection dropped: backfill every known session from its
        // durable log before reconnecting.
        backfill_all(&inner, &server).await;
        tokio::time::sleep(SSE_RECONNECT_DELAY).await;
    }
}

pub(super) async fn stream_events(
    inner: &Arc<DriverInner>,
    server: &Arc<Server>,
) -> anyhow::Result<()> {
    let request = server
        .client
        .auth(server.client.sse_http.get(server.client.url("/api/event")));
    let response = request.send().await?;
    if !response.status().is_success() {
        anyhow::bail!("OpenCode /api/event returned {}", response.status());
    }
    let mut stream = response.bytes_stream().eventsource();
    while let Some(event) = stream.next().await {
        let event = event?;
        // `eventsource-stream` reassembles frames across network chunks and
        // decodes UTF-8 only after a complete frame, so split multibyte
        // characters are preserved.
        if event.data.is_empty() || event.data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
            continue;
        };
        handle_raw_event(inner, &value).await;
    }
    Ok(())
}
