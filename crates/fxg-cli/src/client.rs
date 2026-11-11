//! Local node HTTP / WebSocket client and CLI output helpers.
//!
//! The `fxg` CLI drives the local daemon through its loopback HTTP API
//! (`fxg node start`). The address is published in `~/.flexagent/node.addr`;
//! a stale file is discarded by probing `GET /api/v1/meta` and auto-starting the
//! node when allowed (`FXG_NO_AUTO_NODE=1` disables auto-start).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use fxg_protocol::CommandId;
use fxg_protocol::client_api::{
    ApiErrorResponse, ConnectionRole, ControlSessionRequest, CreateSessionRequest,
    CreateWorktreeRequest, DiagnosticsResponse, ProjectPreview, ProjectPreviewRequest,
    RotateAuthTokenResponse, SendPromptRequest, SessionArchiveRequest, SessionArchiveResponse,
    SessionDetail, SessionForkSpec, SessionImportRequest, SessionImportResponse,
    SessionListResponse, SupportBundle, SystemInfoResponse, WorktreeInfo, WorktreesResponse,
};
use fxg_protocol::common::{
    ClientSource, CommandResult, IncidentListResponse, PromptInput, SessionControlAction,
    SessionStatus, SessionSummary,
};
use fxg_protocol::config::{
    AUTH_TOKEN_FILE_NAME, EnvLookup, GlobalConfig, env_keys, fxg_home, process_env,
};
use fxg_protocol::{NodeId, ProjectId};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// How long to wait for a freshly spawned daemon to accept HTTP connections.
const NODE_START_TIMEOUT: Duration = Duration::from_secs(15);
/// A startup lock older than this is considered stale (crashed starter).
const NODE_START_LOCK_STALE: Duration = Duration::from_secs(30);
/// Number of sessions fetched by list / status commands.
const SESSION_LIST_LIMIT: u32 = 200;

/// Runtime status of the local node shown by `fxg node status`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalStatus {
    /// Sessions running locally or recently active
    pub sessions: Vec<SessionSummary>,
    /// Node ⇔ hub link state
    pub hub_link: fxg_protocol::client_api::HubLinkStatus,
    /// Number of outbox events not yet sent
    pub unsynced_event_count: u64,
    /// Last synchronization time (Unix epoch ms)
    pub last_synced_at: Option<i64>,
}

/// HTTP client for the local node (`fxg node start`).
pub struct NodeClient {
    base: String,
    token: String,
    http: reqwest::Client,
    node_id: String,
}

/// Whether automatic daemon startup is enabled (`FXG_NO_AUTO_NODE=1` disables it).
fn auto_node_enabled() -> bool {
    !matches!(
        process_env(env_keys::NO_AUTO_NODE)
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Base URL of the local node (`node.addr`, then `[node] listen_addr`, then
/// the default loopback address).
pub(crate) fn local_node_base(env: EnvLookup<'_>) -> Result<String> {
    let home = fxg_home(env);
    if let Ok(addr) = std::fs::read_to_string(home.join("node.addr")) {
        let addr = addr.trim();
        if !addr.is_empty() {
            return Ok(format!("http://{addr}"));
        }
    }
    let global = GlobalConfig::load(env).context("failed to load ~/.flexagent/config.toml")?;
    let listen = global
        .node
        .listen_addr
        .clone()
        .unwrap_or_else(|| "127.0.0.1:7860".to_owned());
    Ok(format!("http://{listen}"))
}

fn token_path() -> PathBuf {
    fxg_home(&process_env).join(AUTH_TOKEN_FILE_NAME)
}

/// Starts `fxg node start` in the background once.
///
/// Returns the node log path so the caller can include it in an error message.
fn ensure_node_started() -> Result<PathBuf> {
    let home = fxg_home(&process_env);
    let log_path = home.join("logs").join("node.log");
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let lock_path = home.join("node.start.lock");
    for attempt in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut lock) => {
                fxg_protocol::discard_with!(
                    writeln!(lock, "{}", std::process::id()),
                    "best-effort pid note in the node startup lock"
                );
                let spawn_result = crate::spawn::spawn_background(&["node", "start"], "node.log");
                remove_start_lock(&lock_path);
                spawn_result?;
                return Ok(log_path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another CLI is already starting the node; wait for it.
                if attempt == 0 && lock_is_stale(&lock_path) {
                    remove_start_lock(&lock_path);
                    continue;
                }
                return Ok(log_path);
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to create {}", lock_path.display()));
            }
        }
    }
    Ok(log_path)
}

/// Whether a startup lock is old enough to be considered abandoned.
fn lock_is_stale(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        tracing::debug!(path = %path.display(), "startup lock has no modification time");
        return false;
    };
    match modified.elapsed() {
        Ok(age) => age > NODE_START_LOCK_STALE,
        Err(err) => {
            tracing::debug!(path = %path.display(), "startup lock mtime is in the future: {err}");
            false
        }
    }
}

/// Removes the startup lock (best effort).
fn remove_start_lock(path: &Path) {
    fxg_protocol::discard_with!(std::fs::remove_file(path), "remove the node startup lock");
}

fn probe_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(700))
        .build()
        .context("failed to build the node probe client")
}

/// Probes the authenticated system info to distinguish a live node for this
/// FXG home from a stale `node.addr` or an unrelated service on the same port.
async fn node_ready(base: &str) -> bool {
    let Ok(client) = probe_client() else {
        return false;
    };
    let Ok(token) = fxg_hub::api::auth::load_or_create_token(&token_path()) else {
        return false;
    };
    let Ok(response) = client
        .get(format!("{base}/api/v1/system/info"))
        .bearer_auth(token)
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(info) = response.json::<SystemInfoResponse>().await else {
        return false;
    };
    info.role == ConnectionRole::LocalNode
}

/// Waits for a node to answer `/api/v1/meta`, re-reading `node.addr` while the
/// daemon binds its port.
async fn wait_for_node_ready(timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(base) = local_node_base(&process_env)
            && node_ready(&base).await
        {
            return Some(base);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

impl NodeClient {
    /// Connect to the local node, starting `fxg node start` if needed.
    pub async fn connect() -> Result<Self> {
        let base = local_node_base(&process_env)?;
        if let Ok(client) = Self::from_base(base.clone()).await {
            return Ok(client);
        }
        if !auto_node_enabled() {
            bail!("cannot connect to the node ({base}); start `fxg node start` first");
        }
        let log_path = ensure_node_started()?;
        let Some(base) = wait_for_node_ready(NODE_START_TIMEOUT).await else {
            bail!(
                "cannot connect to the node after auto-start; see {} (set FXG_NO_AUTO_NODE=1 to disable auto-start)",
                log_path.display()
            );
        };
        Self::from_base(base).await
    }

    /// Connect to a running node without auto-starting it.
    pub async fn connect_existing() -> Result<Self> {
        let base = local_node_base(&process_env)?;
        Self::from_base(base.clone())
            .await
            .with_context(|| format!("node is not running ({base}); start `fxg node start` first"))
    }

    async fn from_base(base: String) -> Result<Self> {
        let token = fxg_hub::api::auth::load_or_create_token(&token_path())
            .context("failed to load ~/.flexagent/auth_token")?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("failed to build the node HTTP client")?;
        let mut client = Self {
            base: base.trim_end_matches('/').to_owned(),
            token,
            http,
            node_id: String::new(),
        };
        let info: SystemInfoResponse = client.get("/api/v1/system/info").await?;
        if info.role != ConnectionRole::LocalNode {
            bail!("the endpoint at {} is not a local node", client.base);
        }
        client.node_id = info
            .node_id
            .ok_or_else(|| anyhow!("the local node did not report its node ID"))?
            .as_str()
            .to_owned();
        Ok(client)
    }

    /// Opens the Client event WebSocket (`/api/v1/client/ws`).
    pub async fn ws_connect(
        &self,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

        let ws_url = if let Some(rest) = self.base.strip_prefix("http://") {
            format!("ws://{rest}/api/v1/client/ws")
        } else if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}/api/v1/client/ws")
        } else {
            bail!("unsupported node base URL: {}", self.base);
        };
        let mut request = ws_url
            .into_client_request()
            .context("failed to build the Client WS request")?;
        request.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {}", self.token)
                .parse()
                .context("failed to build the Client WS authorization header")?,
        );
        let (socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .context("failed to connect to the Client WS")?;
        Ok(socket)
    }

    fn command_id(&mut self) -> CommandId {
        // A monotonic per-process ID is enough: command IDs are only for
        // duplicate rejection within the daemon.
        CommandId::from(format!(
            "cli-{}-{}",
            std::process::id(),
            fxg_protocol::util::uuid_v7()
        ))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Builds a URL with a percent-encoded logical project ID.
    fn project_url(&self, project_id: &str, suffix: &str) -> Result<String> {
        let mut url = reqwest::Url::parse(&format!("{}/", self.base))
            .context("failed to parse the node base URL")?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|()| anyhow!("the node base URL cannot be a base"))?;
            segments
                .push("api")
                .push("v1")
                .push("projects")
                .push(project_id);
            for part in suffix.trim_matches('/').split('/') {
                if !part.is_empty() {
                    segments.push(part);
                }
            }
        }
        Ok(url.to_string())
    }

    async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read the node response body")?;
        if !status.is_success() {
            if let Ok(api) = serde_json::from_str::<ApiErrorResponse>(&body) {
                bail!("{}: {}", api.error.code, api.error.message);
            }
            bail!("HTTP {status}: {}", body.trim());
        }
        serde_json::from_str(&body)
            .with_context(|| format!("failed to decode the node response: {body}"))
    }

    async fn check_success(response: reqwest::Response) -> Result<()> {
        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read the node response body")?;
        if status.is_success() {
            return Ok(());
        }
        if let Ok(api) = serde_json::from_str::<ApiErrorResponse>(&body) {
            bail!("{}: {}", api.error.code, api.error.message);
        }
        bail!("HTTP {status}: {}", body.trim());
    }

    async fn get_url<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("failed to reach the node at {url}"))?;
        Self::decode(response).await
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.get_url(&self.url(path)).await
    }

    async fn post_url<B: Serialize, T: DeserializeOwned>(&self, url: &str, body: &B) -> Result<T> {
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("failed to reach the node at {url}"))?;
        Self::decode(response).await
    }

    async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        self.post_url(&self.url(path), body).await
    }

    async fn put<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let url = self.url(path);
        let response = self
            .http
            .put(&url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("failed to reach the node at {url}"))?;
        Self::decode(response).await
    }

    async fn delete_with_body<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = self.url(path);
        let response = self
            .http
            .delete(&url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("failed to reach the node at {url}"))?;
        Self::decode(response).await
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let url = self.url(path);
        let response = self
            .http
            .delete(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("failed to reach the node at {url}"))?;
        Self::check_success(response).await
    }

    async fn inspect_project(&self, cwd: &Path) -> Result<ProjectPreview> {
        let path = format!("/api/v1/nodes/{}/projects/inspect", self.node_id);
        self.post(
            &path,
            &ProjectPreviewRequest {
                local_path: path_to_string(cwd),
            },
        )
        .await
    }

    /// Create or reuse a worktree through the REST API (`fxg session new -w` /
    /// `fxg session fork -w`).
    pub async fn add_worktree(
        &mut self,
        cwd: &Path,
        project_id: Option<String>,
        branch: &str,
        base_branch: Option<String>,
    ) -> Result<(String, String, bool)> {
        let project_id = match project_id {
            Some(project_id) => ProjectId::from(project_id),
            None => self.inspect_project(cwd).await?.project_id,
        };
        let url = self.project_url(project_id.as_str(), "worktrees")?;
        let before: WorktreesResponse = self.get_url(&url).await?;
        let was_present = before
            .worktrees
            .iter()
            .any(|worktree| worktree.branch.as_deref() == Some(branch));
        let worktree: WorktreeInfo = self
            .post_url(
                &url,
                &CreateWorktreeRequest {
                    node_id: NodeId::from(self.node_id.clone()),
                    branch: branch.to_owned(),
                    base_branch,
                },
            )
            .await?;
        Ok((
            worktree.path,
            worktree.branch.unwrap_or_else(|| branch.to_owned()),
            !was_present,
        ))
    }

    /// `fxg session list`
    pub async fn list_sessions(
        &mut self,
        include_stopped: bool,
        include_archived: bool,
    ) -> Result<Vec<SessionSummary>> {
        let response: SessionListResponse = self
            .get(&format!(
                "/api/v1/sessions?include_archived={include_archived}&limit={SESSION_LIST_LIMIT}"
            ))
            .await?;
        let mut sessions = response.sessions;
        if !include_stopped {
            sessions.retain(|session| is_active_status(session.status));
        }
        Ok(sessions)
    }

    /// `fxg session archive` / `unarchive` — archive or restore a session.
    pub async fn archive_session(
        &mut self,
        session_id: &str,
        archived: bool,
    ) -> Result<Option<i64>> {
        let response: SessionArchiveResponse = self
            .post(
                &format!("/api/v1/sessions/{session_id}/archive"),
                &SessionArchiveRequest { archived },
            )
            .await?;
        Ok(response.archived_at)
    }

    /// `fxg session delete` — delete a session (purges event bodies; irreversible).
    pub async fn delete_session(&mut self, session_id: &str) -> Result<()> {
        self.delete(&format!("/api/v1/sessions/{session_id}")).await
    }

    /// `fxg node stop` (graceful local node shutdown).
    pub async fn shutdown(&mut self) -> Result<()> {
        let _: fxg_protocol::client_api::ShutdownResponse = self
            .post("/api/v1/system/shutdown", &serde_json::json!({}))
            .await?;
        Ok(())
    }

    /// `fxg node status`
    pub async fn local_status(&mut self) -> Result<LocalStatus> {
        let info: SystemInfoResponse = self.get("/api/v1/system/info").await?;
        let sessions: SessionListResponse = self
            .get(&format!("/api/v1/sessions?limit={SESSION_LIST_LIMIT}"))
            .await?;
        Ok(LocalStatus {
            sessions: sessions
                .sessions
                .into_iter()
                .filter(|session| is_active_status(session.status))
                .collect(),
            hub_link: info
                .hub_link
                .unwrap_or_else(fxg_protocol::client_api::HubLinkStatus::standalone),
            unsynced_event_count: info.unsynced_event_count.unwrap_or(0),
            last_synced_at: info.last_synced_at,
        })
    }

    /// `GET /api/v1/nodes/{node_id}/settings` (node settings + hub pairing).
    pub async fn node_settings(&mut self) -> Result<fxg_protocol::settings::NodeSettingsSnapshot> {
        let path = format!("/api/v1/nodes/{}/settings", self.node_id);
        self.get(&path).await
    }

    /// `PUT /api/v1/nodes/{node_id}/settings/pairing` (applied immediately).
    pub async fn update_pairing(
        &mut self,
        request: &fxg_protocol::settings::UpdatePairingRequest,
    ) -> Result<fxg_protocol::settings::NodeSettingsSnapshot> {
        let path = format!("/api/v1/nodes/{}/settings/pairing", self.node_id);
        self.put(&path, request).await
    }

    /// `DELETE /api/v1/nodes/{node_id}/settings/pairing`.
    pub async fn clear_pairing(
        &mut self,
        expected_revision: u64,
    ) -> Result<fxg_protocol::settings::NodeSettingsSnapshot> {
        let path = format!("/api/v1/nodes/{}/settings/pairing", self.node_id);
        self.delete_with_body(
            &path,
            &serde_json::json!({ "expected_revision": expected_revision }),
        )
        .await
    }

    /// `fxg node access-token show` / `fxg node access-token rotate`
    pub async fn auth_token(&mut self, rotate: bool) -> Result<String> {
        if !rotate {
            return fxg_hub::api::auth::load_or_create_token(&token_path())
                .context("failed to load ~/.flexagent/auth_token");
        }
        let response: RotateAuthTokenResponse = self
            .post("/api/v1/auth/rotate-token", &serde_json::json!({}))
            .await?;
        self.token = response.token.clone();
        Ok(response.token)
    }

    /// Start a new session for `fxg session new <agent>`.
    pub async fn ensure_session(
        &mut self,
        cwd: &Path,
        agent_id: &str,
        extra_args: &[String],
        initial_mode: Option<&str>,
    ) -> Result<String> {
        let project_id = self.inspect_project(cwd).await?.project_id;
        let request = CreateSessionRequest {
            command_id: self.command_id(),
            project_id,
            node_id: Some(NodeId::from(self.node_id.clone())),
            provisioner: None,
            local_path: Some(path_to_string(cwd)),
            worktree: None,
            agent_id: agent_id.to_owned(),
            initial_prompt: None,
            initial_config: Vec::new(),
            mode: initial_mode.map(str::to_owned),
            extra_args: (!extra_args.is_empty()).then(|| extra_args.to_vec()),
            fork: None,
        };
        let response = self
            .post::<_, fxg_protocol::client_api::CreateSessionResponse>(
                "/api/v1/sessions",
                &request,
            )
            .await?;
        Ok(response.session_id.to_string())
    }

    /// `fxg session import <agent-session-id>`.
    pub async fn import_session(
        &mut self,
        agent_id: &str,
        agent_session_id: &str,
        cwd: &str,
        title: Option<&str>,
    ) -> Result<String> {
        let command_id = self.command_id();
        let response: SessionImportResponse = self
            .post(
                "/api/v1/sessions/import",
                &SessionImportRequest {
                    command_id,
                    node_id: None,
                    agent_id: agent_id.to_owned(),
                    agent_session_id: agent_session_id.to_owned(),
                    cwd: cwd.to_owned(),
                    title: title.map(str::to_owned),
                },
            )
            .await?;
        Ok(response.session_id.to_string())
    }

    /// `fxg doctor` / `fxg logs --session` (local diagnostics).
    pub async fn diagnostics(&mut self) -> Result<DiagnosticsResponse> {
        self.get("/api/v1/diagnostics").await
    }

    /// `fxg support bundle` (redacted support bundle).
    pub async fn support_bundle(&mut self) -> Result<SupportBundle> {
        self.get("/api/v1/diagnostics/bundle").await
    }

    /// `fxg support incidents` (persisted incidents).
    pub async fn incidents(
        &mut self,
        limit: u32,
        include_resolved: bool,
    ) -> Result<IncidentListResponse> {
        self.get(&format!(
            "/api/v1/diagnostics/incidents?limit={limit}&include_resolved={include_resolved}"
        ))
        .await
    }

    /// `fxg session prompt` (headless send).
    pub async fn send_prompt(
        &mut self,
        session_id: &str,
        input: &PromptInput,
        client_source: ClientSource,
    ) -> Result<()> {
        let command_id = self.command_id();
        let result: CommandResult = self
            .post(
                &format!("/api/v1/sessions/{session_id}/prompt"),
                &SendPromptRequest {
                    command_id,
                    input: input.clone(),
                    client_source,
                },
            )
            .await?;
        check_command_result(result)
    }

    /// Mode switch / config change / cancel / kill (`fxg session stop|kill`).
    pub async fn control_session(
        &mut self,
        session_id: &str,
        action: SessionControlAction,
    ) -> Result<()> {
        let command_id = self.command_id();
        let result: CommandResult = self
            .post(
                &format!("/api/v1/sessions/{session_id}/control"),
                &ControlSessionRequest { command_id, action },
            )
            .await?;
        check_command_result(result)
    }

    /// `fxg session show`
    pub async fn session_show(
        &mut self,
        session_id: &str,
        recent_events: u32,
    ) -> Result<SessionDetail> {
        self.get(&format!(
            "/api/v1/sessions/{session_id}?recent_events={recent_events}"
        ))
        .await
    }

    /// `fxg session fork`
    pub async fn fork_session(&mut self, session_id: &str, cwd: Option<&Path>) -> Result<String> {
        let source: SessionDetail = self
            .get(&format!("/api/v1/sessions/{session_id}?recent_events=0"))
            .await?;
        let request = CreateSessionRequest {
            command_id: self.command_id(),
            project_id: source.session.project_id,
            node_id: Some(NodeId::from(self.node_id.clone())),
            provisioner: None,
            local_path: Some(
                cwd.map(path_to_string)
                    .unwrap_or_else(|| source.session.local_path.clone()),
            ),
            worktree: None,
            agent_id: source.session.agent_id,
            initial_prompt: None,
            initial_config: Vec::new(),
            mode: None,
            extra_args: None,
            fork: Some(SessionForkSpec {
                from_session_id: session_id.into(),
            }),
        };
        let response = self
            .post::<_, fxg_protocol::client_api::CreateSessionResponse>(
                "/api/v1/sessions",
                &request,
            )
            .await?;
        Ok(response.session_id.to_string())
    }

    /// `fxg session resume` — resume a stopped session.
    ///
    /// Returns whether native context was restored. When `context_restored =
    /// false` the session continues by replaying its history.
    pub async fn resume_session(&mut self, session_id: &str, force_replay: bool) -> Result<bool> {
        let response: fxg_protocol::client_api::ResumeSessionResponse = self
            .post(
                &format!("/api/v1/sessions/{session_id}/resume"),
                &fxg_protocol::client_api::ResumeSessionRequest { force_replay },
            )
            .await?;
        Ok(response.context_restored)
    }
}

fn is_active_status(status: SessionStatus) -> bool {
    matches!(
        status,
        SessionStatus::Provisioning
            | SessionStatus::Idle
            | SessionStatus::Running
            | SessionStatus::WaitingPermission
            | SessionStatus::WaitingInput
    )
}

fn check_command_result(result: CommandResult) -> Result<()> {
    if result.success {
        return Ok(());
    }
    let message = result
        .error
        .map(|error| format!("{}: {}", error.code, error.message))
        .unwrap_or_else(|| "the command failed".to_owned());
    bail!("{message}")
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Print a simple table made of a header and rows to stdout.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| header.chars().count())
        .collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
    }

    let print_row = |cells: Vec<String>| {
        let line = cells
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let width = widths.get(index).copied().unwrap_or(0);
                let padding = width.saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(padding))
            })
            .collect::<Vec<_>>()
            .join("  ");
        println!("{}", line.trim_end());
    };

    print_row(headers.iter().map(|header| (*header).to_owned()).collect());
    for row in rows {
        print_row(row.clone());
    }
}

/// Format Unix epoch milliseconds as `YYYY-MM-DD HH:MM:SSZ` (UTC).
///
/// Minimal implementation for CLI listings (avoids adding a date-time crate
/// such as chrono).
pub fn format_unix_ms_utc(ms: i64) -> String {
    let total_seconds = ms.div_euclid(1000);
    let seconds_of_day = total_seconds.rem_euclid(86_400);
    let days = total_seconds.div_euclid(86_400);

    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z")
}

/// Compute (year, month, day) from days since 1970-01-01 (Howard Hinnant's
/// civil_from_days).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

/// Truncate a string to a maximum number of characters, marking the cut with
/// an ellipsis.
pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Render a session ID in short form (`0195f0ab-1234-...` → `0195f0ab`).
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_millis_as_utc() {
        assert_eq!(format_unix_ms_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(
            format_unix_ms_utc(1_700_000_000_000),
            "2023-11-14 22:13:20Z"
        );
        assert_eq!(
            format_unix_ms_utc(1_752_000_000_000),
            "2025-07-08 18:40:00Z"
        );
        // Negative epochs (before 1970) must not break either.
        assert_eq!(format_unix_ms_utc(-86_400_000), "1969-12-31 00:00:00Z");
    }

    #[test]
    fn truncates_long_text_on_char_boundary() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
        assert_eq!(truncate("日本語テキスト", 4), "日本語…"); // allow-japanese: multibyte truncation
    }

    #[test]
    fn shortens_session_ids() {
        assert_eq!(short_id("0195f0ab-1234-7abc-8def-0123456789ab"), "0195f0ab");
        assert_eq!(short_id("abc"), "abc");
    }

    #[test]
    fn command_result_success_is_accepted() {
        let result = CommandResult::success("c1", "s1");
        assert!(check_command_result(result).is_ok());
    }
}
