//! Shared state and Client API backend implementation for the central server (`fxg server`).
//!
//! Design: `docs/01-architecture-and-sync.md` §2.2 and §3.1, `docs/03-protocol-and-api.md` §3.
//!
//! The central server holds only a **projection** of session state (`server.db`); write
//! authority belongs to the executing node. Client operations are relayed to the target node
//! through [`NodeHub`] and responses are returned correlated by `command_id`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use fxg_db::{Db, DbRole, PushSubscriptionRecord};
use fxg_protocol::CommandId;
use fxg_protocol::client_api::{
    AgentOpResponse, AgentsResponse, ConnectionRole, CreateSessionRequest, CreateSessionResponse,
    CreateWorktreeRequest, DiagnosticsResponse, IssueNodeTokenRequest, IssueNodeTokenResponse,
    KillSwitchResponse, NodeTokenSummary, NodeTokensResponse, ProjectLinkRequest,
    ProjectLinkResponse, ProjectScanRequest, ProjectScanResponse, ProvisionerSummary,
    ProvisionersResponse, PruneWorktreesRequest, PushSubscribeRequest, PushSubscribeResponse,
    RemoveWorktreeRequest, RespondElicitationRequest, RespondElicitationResponse,
    RespondPermissionRequest, RespondPermissionResponse, ResumeSessionRequest,
    ResumeSessionResponse, RotateAuthTokenResponse, SessionArchiveRequest, SessionArchiveResponse,
    WorktreeInfo, WorktreesResponse,
};
use fxg_protocol::common::{
    AgentAction, CommandResult, DiffScope, ErrorCode, WorkspaceDiffResponse, WorktreeAction,
};
use fxg_protocol::node_server::{NodeRpcRequest, NodeRpcResponse, ServerToNodeMsg};
use fxg_protocol::util::uuid_v7;
use tokio::sync::broadcast;

use crate::api::{
    ApiError, ClientApiBackend, ClientCommand, ClientEvent, ClientInfo, PtyChannelError,
    PtyChannelEvent, PtySpawnParams, SystemExtras, auth,
};
use crate::error::ServerError;
use crate::hub::NodeHub;
use crate::provisioner::{self, ProvisionerManager, SpawnSessionRequest};
use crate::push::PushService;
use fxg_protocol::settings::{
    ConfigSource, ProjectProfile, SecretsResponse, ServerBootstrapView, ServerSettingsSnapshot,
    SourcedValue, UpdateProjectProfileRequest, UpdateServerSettingsRequest, UpsertSecretRequest,
    sourced_option, sourced_string,
};
use fxg_protocol::{NodeId, ProjectId, SecretId};

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

    /// Builds the typed settings snapshot exposed by `GET /api/v1/settings/server`.
    pub async fn server_settings_snapshot(&self) -> Result<ServerSettingsSnapshot, ServerError> {
        let settings = self.db().ensure_server_settings().await?;
        let config = match fxg_protocol::config::GlobalConfig::load_from_path(
            &self
                .inner
                .options
                .fxg_home
                .join(fxg_protocol::config::CONFIG_FILE_NAME),
        ) {
            Ok(config) => config,
            Err(err) => {
                tracing::warn!("failed to read config.toml for the settings view: {err}");
                fxg_protocol::config::GlobalConfig::default()
            }
        };
        let env = fxg_protocol::config::process_env;
        let fxg_home_source = if env(fxg_protocol::config::ENV_FXG_HOME).is_some() {
            ConfigSource::Env
        } else {
            ConfigSource::Default
        };
        let fxg_home = SourcedValue::new(
            self.inner.options.fxg_home.to_string_lossy().into_owned(),
            fxg_home_source.clone(),
            (fxg_home_source == ConfigSource::Env)
                .then(|| fxg_protocol::config::ENV_FXG_HOME.to_owned()),
        );
        let listen_addr = sourced_string(
            &env,
            self.inner.options.listen_addr.clone(),
            fxg_protocol::config::env_keys::SERVER_LISTEN_ADDR,
            config.server.listen_addr.clone(),
            "server.listen_addr",
            Some(fxg_protocol::config::ServerBootstrapConfig::DEFAULT_LISTEN_ADDR),
            Some("--listen"),
        );
        let allowed_hosts = self.inner.options.allowed_hosts.clone();
        let allowed_origins = self.inner.options.allowed_origins.clone();
        let log = &self.inner.options.log;
        let log_level = sourced_string(
            &env,
            log.level.clone().unwrap_or_else(|| "info".to_owned()),
            fxg_protocol::config::env_keys::LOG_LEVEL,
            config.log.level.clone(),
            "log.level",
            Some("info"),
            Some("--log-level"),
        );
        let log_file = sourced_option(
            &env,
            log.file.clone().filter(|value| value != "-"),
            fxg_protocol::config::env_keys::LOG_FILE,
            config.log.file.clone(),
            "log.file",
            Some("--log-file"),
        );
        let log_format = sourced_string(
            &env,
            log.format.clone().unwrap_or_else(|| "text".to_owned()),
            fxg_protocol::config::env_keys::LOG_FORMAT,
            config.log.format.clone(),
            "log.format",
            Some("text"),
            Some("--log-format"),
        );
        let log_no_color = SourcedValue::new(
            log.no_color,
            if log.no_color == config.log.no_color {
                ConfigSource::ConfigFile
            } else {
                ConfigSource::Cli
            },
            Some("--no-color"),
        );
        Ok(ServerSettingsSnapshot {
            bootstrap: ServerBootstrapView {
                fxg_home,
                listen_addr,
                allowed_hosts: SourcedValue::new(
                    allowed_hosts,
                    if config.server.allowed_hosts.is_empty() {
                        ConfigSource::Default
                    } else {
                        ConfigSource::ConfigFile
                    },
                    Some("server.allowed_hosts"),
                ),
                allowed_origins: SourcedValue::new(
                    allowed_origins,
                    if config.server.allowed_origins.is_empty() {
                        ConfigSource::Default
                    } else {
                        ConfigSource::ConfigFile
                    },
                    Some("server.allowed_origins"),
                ),
                log_level,
                log_file,
                log_format,
                log_no_color,
            },
            server: settings,
        })
    }

    /// Updates server settings and reloads provisioners / Git credentials / VAPID subject.
    pub async fn update_server_settings(
        &self,
        request: &UpdateServerSettingsRequest,
    ) -> Result<ServerSettingsSnapshot, ServerError> {
        let settings = self.db().update_server_settings(request).await?;
        self.provisioners().apply_settings(
            self.db().provisioners().await?,
            settings.git_credentials.clone(),
        );
        self.push().set_subject(settings.vapid_subject.as_deref());
        self.server_settings_snapshot().await
    }

    /// Lists server-owned secret metadata.
    pub async fn server_secrets(&self) -> Result<SecretsResponse, ServerError> {
        Ok(SecretsResponse {
            secrets: fxg_db::secrets::list_secrets(self.db(), &self.inner.options.fxg_home).await?,
        })
    }

    /// Stores a server-owned secret.
    pub async fn upsert_server_secret(
        &self,
        id: &SecretId,
        request: &UpsertSecretRequest,
    ) -> Result<SecretsResponse, ServerError> {
        Ok(SecretsResponse {
            secrets: fxg_db::secrets::upsert_secret(
                self.db(),
                &self.inner.options.fxg_home,
                id,
                request,
            )
            .await?,
        })
    }

    /// Deletes a server-owned secret.
    pub async fn delete_server_secret(
        &self,
        id: &SecretId,
    ) -> Result<SecretsResponse, ServerError> {
        Ok(SecretsResponse {
            secrets: fxg_db::secrets::delete_secret(self.db(), &self.inner.options.fxg_home, id)
                .await?,
        })
    }

    /// Returns the project bootstrap profile (default when absent).
    pub async fn project_profile(&self, project_id: &str) -> Result<ProjectProfile, ApiError> {
        let id = ProjectId::from(project_id);
        if let Some(profile) = self.db().project_profile(&id).await? {
            return Ok(profile);
        }
        let projects = self.db().list_projects().await?;
        if !projects.iter().any(|project| project.project_id == id) {
            return Err(ApiError::not_found(format!(
                "project not found: {project_id}"
            )));
        }
        Ok(ProjectProfile {
            project_id: id,
            bootstrap_setup: Vec::new(),
            bootstrap_env: Default::default(),
            revision: 0,
            updated_at: 0,
        })
    }

    /// Updates the project bootstrap profile.
    pub async fn update_project_profile(
        &self,
        project_id: &str,
        request: &UpdateProjectProfileRequest,
    ) -> Result<ProjectProfile, ApiError> {
        let id = ProjectId::from(project_id);
        let profile = self.db().upsert_project_profile(&id, request).await?;
        Ok(profile)
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

    /// Resolves the executing node for a session (`NODE_OFFLINE` is returned early here).
    async fn session_node(
        &self,
        session_id: &fxg_protocol::SessionId,
    ) -> Result<fxg_protocol::NodeId, ApiError> {
        let session = self
            .inner
            .db
            .get_session(session_id.as_str())
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found(format!("session not found: {session_id}")))?;
        if !self.inner.hub.is_online(&session.node_id).await {
            return Err(ApiError::node_offline(
                session.node_id.as_str(),
                format!("node is offline: {}", session.node_id),
            ));
        }
        Ok(session.node_id)
    }

    /// Picks one online node that can serve the ACP Registry catalog.
    ///
    /// Resolves the default working directory for a project on this node.
    ///
    /// Prefers the most recently used binding (including worktrees) and falls back to the
    /// latest session's working directory when there is no binding
    /// ([`fxg_db::queries::resolve_default_local_path`]).
    async fn project_local_path(
        &self,
        project_id: &fxg_protocol::ProjectId,
        node_id: &fxg_protocol::NodeId,
    ) -> Result<String, ApiError> {
        self.inner
            .db
            .resolve_default_local_path(project_id, node_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "no default directory for project {project_id} on node {node_id}; \
                     specify local_path or register the project (scan / link / worktree)"
                ))
            })
    }

    /// When `always = true`, returns the `option_id` promoted to an `allow_always` option.
    async fn resolve_permission_option(
        &self,
        session_id: &str,
        request_id: &str,
        request: &RespondPermissionRequest,
    ) -> Result<String, ApiError> {
        if !request.always {
            return Ok(request.selected_option_id.clone());
        }
        let pending = self
            .inner
            .db
            .pending_permissions()
            .await
            .map_err(ApiError::from)?;
        if let Some(entry) = pending
            .iter()
            .find(|entry| entry.session_id.as_str() == session_id && entry.request_id == request_id)
            && let Some(option) = entry.options.iter().find(|option| option.kind.is_always())
        {
            return Ok(option.option_id.clone());
        }
        Ok(request.selected_option_id.clone())
    }
}

/// Helper that builds a `CommandResult`, with `session_id` on success.
fn command_failure(command_id: CommandId, err: ApiError) -> CommandResult {
    CommandResult::failure(command_id, err.into_info())
}

impl ServerState {
    /// Fetches the projection row for a logical project.
    async fn project_record(
        &self,
        project_id: &fxg_protocol::ProjectId,
    ) -> Result<fxg_protocol::common::ProjectSummary, ApiError> {
        let projects = self
            .inner
            .db
            .list_projects()
            .await
            .map_err(ApiError::from)?;
        projects
            .into_iter()
            .find(|project| &project.project_id == project_id)
            .ok_or_else(|| ApiError::not_found(format!("project not found: {project_id}")))
    }
}

#[async_trait]
impl ClientApiBackend for ServerState {
    fn db(&self) -> &Db {
        &self.inner.db
    }

    fn auth_token(&self) -> String {
        ServerState::auth_token(self)
    }

    fn connection_role(&self) -> ConnectionRole {
        ConnectionRole::CentralServer
    }

    async fn diagnostics(&self) -> Result<DiagnosticsResponse, ApiError> {
        Ok(DiagnosticsResponse {
            node_id: "central-server".to_owned(),
            generated_at: fxg_protocol::util::now_ms(),
            agents: Vec::new(),
            daemon_log: None,
        })
    }

    async fn system_extras(&self) -> Result<SystemExtras, ApiError> {
        Ok(SystemExtras {
            vapid_public_key: Some(self.inner.push.public_key().to_owned()),
            ..SystemExtras::default()
        })
    }

    fn subscribe_client_events(&self) -> broadcast::Receiver<ClientEvent> {
        self.inner.client_events.subscribe()
    }

    async fn dispatch_command(&self, command: ClientCommand) -> CommandResult {
        let command_id = command.command_id().to_owned();
        let (node_id, message) = match &command {
            ClientCommand::SendPrompt {
                session_id,
                input,
                client_source,
                ..
            } => (
                self.session_node(session_id).await,
                ServerToNodeMsg::SendPrompt {
                    command_id: command_id.clone(),
                    session_id: session_id.clone(),
                    input: input.clone(),
                    client_source: client_source.clone(),
                },
            ),
            ClientCommand::RespondPermission {
                session_id,
                request_id,
                selected_option_id,
                resolved_by,
                ..
            } => (
                self.session_node(session_id).await,
                ServerToNodeMsg::RespondPermission {
                    command_id: command_id.clone(),
                    session_id: session_id.clone(),
                    request_id: request_id.clone(),
                    selected_option_id: selected_option_id.clone(),
                    resolved_by: resolved_by.clone(),
                },
            ),
            ClientCommand::RespondElicitation {
                session_id,
                elicitation_id,
                action,
                content,
                resolved_by,
                ..
            } => (
                self.session_node(session_id).await,
                ServerToNodeMsg::RespondElicitation {
                    command_id: command_id.clone(),
                    session_id: session_id.clone(),
                    elicitation_id: elicitation_id.clone(),
                    action: *action,
                    content: content.clone(),
                    resolved_by: resolved_by.clone(),
                },
            ),
            ClientCommand::ControlSession {
                session_id, action, ..
            } => (
                self.session_node(session_id).await,
                ServerToNodeMsg::ControlSession {
                    command_id: command_id.clone(),
                    session_id: session_id.clone(),
                    action: action.clone(),
                },
            ),
        };

        match node_id {
            Ok(node_id) => self.run_ws_command(&node_id, &command_id, message).await,
            // Return immediately without queueing when the node is offline or the session unknown
            Err(err) => command_failure(command_id, err),
        }
    }

    async fn create_session(
        &self,
        request: CreateSessionRequest,
        client: ClientInfo,
    ) -> Result<CreateSessionResponse, ApiError> {
        // ACP-native fork: it must run on the node that owns the source session.
        // Cross-node / cross-agent / ephemeral-VM forks are not supported.
        let fork_source = match &request.fork {
            Some(fork) => Some(
                self.inner
                    .db
                    .get_session(fork.from_session_id.as_str())
                    .await
                    .map_err(ApiError::from)?
                    .ok_or_else(|| {
                        ApiError::not_found(format!("session not found: {}", fork.from_session_id))
                    })?,
            ),
            None => None,
        };

        // Temporary VM provisioner: launch a child process on the server host
        if let Some(provisioner_name) = request.provisioner.clone() {
            if fork_source.is_some() {
                return Err(ApiError::bad_request(
                    "native forks cannot be combined with a provisioner",
                ));
            }
            if request.node_id.is_some() {
                return Err(ApiError::bad_request(
                    "node_id and provisioner are mutually exclusive",
                ));
            }
            let project = self.project_record(&request.project_id).await?;
            let git_url = project.canonical_git_url.clone().ok_or_else(|| {
                ApiError::bad_request(format!(
                    "project {} has no canonical git url; cannot clone into an ephemeral node",
                    request.project_id
                ))
            })?;

            let session_id = uuid_v7();
            let node_id = format!("eph-{}", &session_id[..18.min(session_id.len())]);
            let git_branch = request
                .worktree
                .as_ref()
                .map(|spec| spec.branch.clone())
                .filter(|branch| !branch.trim().is_empty());
            let agent_id = request.agent_id.clone();

            // Pre-register the ephemeral node (FK for sessions.node_id / UI display)
            let idle_timeout_secs = self
                .provisioners()
                .config(&provisioner_name)?
                .idle_timeout_secs
                .unwrap_or(900);
            provisioner::register_ephemeral_node(
                self,
                &node_id,
                &provisioner_name,
                idle_timeout_secs,
            )
            .await?;

            // Provisional projection row so the UI can show the session during bootstrap
            if let Err(err) = self
                .db()
                .upsert_provisional_session(&fxg_db::registration::ProvisionalSessionRecord {
                    session_id: session_id.clone(),
                    project_id: request.project_id.to_string(),
                    project_name: project.name.clone(),
                    node_id: node_id.clone(),
                    local_path: provisioner::EPHEMERAL_WORKSPACE.to_owned(),
                    git_branch: git_branch.clone(),
                    agent_id: agent_id.clone(),
                    title: format!("{agent_id} @ {}", project.name),
                })
                .await
            {
                return Err(ApiError::from(err));
            }

            provisioner::spawn_session(
                self,
                SpawnSessionRequest {
                    session_id: session_id.clone(),
                    node_id: node_id.clone(),
                    provisioner: provisioner_name.clone(),
                    project_id: request.project_id.to_string(),
                    git_url,
                    git_branch,
                    agent_id: agent_id.clone(),
                    initial_prompt: request.initial_prompt.clone(),
                    initial_config: request.initial_config.clone(),
                    mode: request.mode.clone(),
                    extra_args: request.extra_args.clone(),
                },
            )
            .await?;

            self.record_audit(
                fxg_db::audit::actions::SESSION_START,
                &client,
                Some(&session_id),
                serde_json::json!({
                    "node_id": node_id,
                    "agent_id": agent_id,
                    "provisioner": provisioner_name,
                    "project_id": request.project_id,
                }),
            )
            .await;
            return Ok(CreateSessionResponse {
                session_id: session_id.into(),
                command_id: request.command_id,
            });
        }

        let node_id = match fork_source.as_ref() {
            Some(source) => {
                if let Some(requested) = &request.node_id
                    && requested != &source.node_id
                {
                    return Err(ApiError::bad_request(format!(
                        "a native fork must run on the source node ({})",
                        source.node_id
                    )));
                }
                let is_ephemeral = self
                    .inner
                    .db
                    .list_nodes()
                    .await
                    .map_err(ApiError::from)?
                    .iter()
                    .any(|node| node.node_id == source.node_id && node.is_ephemeral);
                if is_ephemeral {
                    return Err(ApiError::bad_request(
                        "native forks from ephemeral VM sessions are not supported",
                    ));
                }
                source.node_id.clone()
            }
            None => request
                .node_id
                .clone()
                .ok_or_else(|| ApiError::bad_request("node_id or provisioner is required"))?,
        };
        if !self.inner.hub.is_online(&node_id).await {
            return Err(ApiError::node_offline(
                node_id.as_str(),
                format!("node is offline: {node_id}"),
            ));
        }

        // When creating a worktree at the same time, create it on the node and use the real path
        let local_path = if let Some(spec) = &request.worktree {
            let info = match self
                .node_rpc(
                    &node_id,
                    NodeRpcRequest::ManageWorktree {
                        project_id: request.project_id.clone(),
                        action: WorktreeAction::Add {
                            branch: spec.branch.clone(),
                            base_branch: spec.base_branch.clone(),
                        },
                    },
                )
                .await?
            {
                NodeRpcResponse::Worktree {
                    worktree: Some(info),
                } => info,
                _ => return Err(ApiError::internal("node returned no worktree info")),
            };
            info.path
        } else if let Some(path) = request.local_path.clone() {
            path
        } else if let Some(source) = fork_source.as_ref() {
            source.local_path.clone()
        } else {
            self.project_local_path(&request.project_id, &node_id)
                .await?
        };

        let session_id = uuid_v7();
        let agent_id = fork_source.as_ref().map_or_else(
            || request.agent_id.clone(),
            |source| source.agent_id.clone(),
        );
        let result = self
            .inner
            .hub
            .command(
                &node_id,
                &request.command_id,
                ServerToNodeMsg::StartSession {
                    command_id: request.command_id.clone(),
                    session_id: session_id.clone().into(),
                    project_id: request.project_id.clone(),
                    local_path: local_path.clone(),
                    agent_id: agent_id.clone(),
                    initial_prompt: request.initial_prompt.clone(),
                    initial_config: request.initial_config.clone(),
                    mode: request.mode.clone(),
                    extra_args: request.extra_args.clone(),
                    fork_from_session_id: fork_source
                        .as_ref()
                        .map(|source| source.session_id.clone()),
                },
            )
            .await?;
        if !result.success {
            return Err(ApiError::from_command(result));
        }

        self.record_audit(
            fxg_db::audit::actions::SESSION_START,
            &client,
            Some(&session_id),
            serde_json::json!({
                "node_id": node_id,
                "agent_id": agent_id,
                "local_path": local_path,
                "project_id": request.project_id,
            }),
        )
        .await;

        Ok(CreateSessionResponse {
            session_id: session_id.into(),
            command_id: result.command_id,
        })
    }

    async fn respond_permission(
        &self,
        session_id: &str,
        request_id: &str,
        request: RespondPermissionRequest,
        client: ClientInfo,
    ) -> Result<RespondPermissionResponse, ApiError> {
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        let selected = self
            .resolve_permission_option(session_id, request_id, &request)
            .await?;
        let command_id = CommandId::from(uuid_v7());
        let result = self
            .inner
            .hub
            .command(
                &node_id,
                &command_id,
                ServerToNodeMsg::RespondPermission {
                    command_id: command_id.clone(),
                    session_id: session_id.into(),
                    request_id: request_id.to_owned(),
                    selected_option_id: selected.clone(),
                    resolved_by: request.resolved_by.clone(),
                },
            )
            .await?;

        if result.success {
            self.record_audit(
                fxg_db::audit::actions::PERMISSION_RESOLVED,
                &client,
                Some(session_id),
                serde_json::json!({
                    "request_id": request_id,
                    "selected_option_id": selected,
                    "resolved_by": request.resolved_by,
                    "node_id": node_id,
                }),
            )
            .await;
            Ok(RespondPermissionResponse {
                already_resolved: false,
            })
        } else if result
            .error
            .as_ref()
            .is_some_and(|error| error.code == ErrorCode::AlreadyResolved)
        {
            // Idempotent resolution across all clients (later calls are normal transitions)
            Ok(RespondPermissionResponse {
                already_resolved: true,
            })
        } else {
            Err(ApiError::from_command(result))
        }
    }

    async fn respond_elicitation(
        &self,
        session_id: &str,
        elicitation_id: &str,
        request: RespondElicitationRequest,
        client: ClientInfo,
    ) -> Result<RespondElicitationResponse, ApiError> {
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        let command_id = CommandId::from(uuid_v7());
        let result = self
            .inner
            .hub
            .command(
                &node_id,
                &command_id,
                ServerToNodeMsg::RespondElicitation {
                    command_id: command_id.clone(),
                    session_id: session_id.into(),
                    elicitation_id: elicitation_id.to_owned(),
                    action: request.action,
                    content: request.content.clone(),
                    resolved_by: request.resolved_by.clone(),
                },
            )
            .await?;

        if result.success {
            self.record_audit(
                fxg_db::audit::actions::ELICITATION_RESOLVED,
                &client,
                Some(session_id),
                serde_json::json!({
                    "elicitation_id": elicitation_id,
                    "action": request.action.as_str(),
                    "resolved_by": request.resolved_by,
                    "node_id": node_id,
                }),
            )
            .await;
            Ok(RespondElicitationResponse {
                already_resolved: false,
            })
        } else if result
            .error
            .as_ref()
            .is_some_and(|error| error.code == ErrorCode::AlreadyResolved)
        {
            // Idempotent resolution across all clients (later calls are normal transitions)
            Ok(RespondElicitationResponse {
                already_resolved: true,
            })
        } else {
            Err(ApiError::from_command(result))
        }
    }

    async fn resume_session(
        &self,
        session_id: &str,
        request: ResumeSessionRequest,
        client: ClientInfo,
    ) -> Result<ResumeSessionResponse, ApiError> {
        let session = self
            .inner
            .db
            .get_session(session_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found(format!("session not found: {session_id}")))?;

        // Resuming a temporary VM (provisioner) session (VM restart + workspace
        // handover) is v2,
        // so it is rejected in v1 (resident nodes only).
        let is_ephemeral = self
            .inner
            .db
            .list_nodes()
            .await
            .map_err(ApiError::from)?
            .into_iter()
            .any(|node| node.node_id == session.node_id && node.is_ephemeral);
        if is_ephemeral {
            return Err(ApiError::from_code(
                ErrorCode::InvalidState,
                "resume for ephemeral sessions is not supported yet",
            ));
        }

        // Relay to the executing node (offline yields `NODE_OFFLINE`)
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        let context_restored = match self
            .node_rpc(
                &node_id,
                NodeRpcRequest::ResumeSession {
                    session_id: session_id.into(),
                    force_replay: request.force_replay,
                },
            )
            .await?
        {
            NodeRpcResponse::Resume { context_restored } => context_restored,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };
        self.record_audit(
            fxg_db::audit::actions::SESSION_RESUME,
            &client,
            Some(session_id),
            serde_json::json!({
                "node_id": node_id,
                "force_replay": request.force_replay,
                "context_restored": context_restored,
            }),
        )
        .await;
        Ok(ResumeSessionResponse {
            session_id: session_id.into(),
            context_restored,
        })
    }

    async fn archive_session(
        &self,
        session_id: &str,
        request: SessionArchiveRequest,
        client: ClientInfo,
    ) -> Result<SessionArchiveResponse, ApiError> {
        // Relay to the executing node (existence check / offline is returned by `session_node`)
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        let archived_at = match self
            .node_rpc(
                &node_id,
                NodeRpcRequest::ArchiveSession {
                    session_id: session_id.into(),
                    archived: request.archived,
                },
            )
            .await?
        {
            NodeRpcResponse::Archive { archived_at } => archived_at,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };
        self.record_audit(
            fxg_db::audit::actions::SESSION_ARCHIVE,
            &client,
            Some(session_id),
            serde_json::json!({
                "node_id": node_id,
                "archived": request.archived,
                "archived_at": archived_at,
            }),
        )
        .await;
        Ok(SessionArchiveResponse {
            session_id: session_id.into(),
            archived_at,
        })
    }

    async fn delete_session(&self, session_id: &str, client: ClientInfo) -> Result<(), ApiError> {
        // Relay to the executing node (existence check / offline is returned by `session_node`)
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        self.node_rpc(
            &node_id,
            NodeRpcRequest::DeleteSession {
                session_id: session_id.into(),
            },
        )
        .await?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_DELETE,
            &client,
            Some(session_id),
            serde_json::json!({ "node_id": node_id }),
        )
        .await;
        Ok(())
    }

    async fn prune_worktrees(
        &self,
        project_id: &str,
        request: PruneWorktreesRequest,
        client: ClientInfo,
    ) -> Result<(), ApiError> {
        if !self.inner.hub.is_online(&request.node_id).await {
            return Err(ApiError::node_offline(
                request.node_id.as_str(),
                format!("node is offline: {}", request.node_id),
            ));
        }
        self.node_rpc(
            &request.node_id,
            NodeRpcRequest::ManageWorktree {
                project_id: project_id.into(),
                action: WorktreeAction::Prune,
            },
        )
        .await?;
        self.record_audit(
            fxg_db::audit::actions::WORKTREE_PRUNE,
            &client,
            None,
            serde_json::json!({
                "node_id": request.node_id,
                "project_id": project_id,
            }),
        )
        .await;
        Ok(())
    }

    async fn scan_projects(
        &self,
        node_id: &str,
        request: ProjectScanRequest,
        client: ClientInfo,
    ) -> Result<ProjectScanResponse, ApiError> {
        if !self
            .inner
            .hub
            .is_online(&fxg_protocol::NodeId::from(node_id))
            .await
        {
            return Err(ApiError::node_offline(
                node_id,
                format!("node is offline: {node_id}"),
            ));
        }
        let response = match self
            .node_rpc(
                &fxg_protocol::NodeId::from(node_id),
                NodeRpcRequest::ProjectScan {
                    dir: request.dir.clone(),
                },
            )
            .await?
        {
            NodeRpcResponse::ProjectScan { response } => response,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };
        self.record_audit(
            fxg_db::audit::actions::PROJECT_LINK,
            &client,
            None,
            serde_json::json!({
                "action": "scan",
                "node_id": node_id,
                "dir": request.dir,
                "scanned_dirs": response.scanned_dirs,
            }),
        )
        .await;
        Ok(response)
    }

    async fn link_project(
        &self,
        node_id: &str,
        request: ProjectLinkRequest,
        client: ClientInfo,
    ) -> Result<ProjectLinkResponse, ApiError> {
        if !self
            .inner
            .hub
            .is_online(&fxg_protocol::NodeId::from(node_id))
            .await
        {
            return Err(ApiError::node_offline(
                node_id,
                format!("node is offline: {node_id}"),
            ));
        }
        let response = match self
            .node_rpc(
                &fxg_protocol::NodeId::from(node_id),
                NodeRpcRequest::ProjectLink {
                    project_id: request.project_id.clone(),
                    local_path: request.local_path.clone(),
                },
            )
            .await?
        {
            NodeRpcResponse::ProjectLink { response } => response,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };
        self.record_audit(
            fxg_db::audit::actions::PROJECT_LINK,
            &client,
            None,
            serde_json::json!({
                "action": "link",
                "node_id": node_id,
                "project_id": response.project_id,
                "local_path": response.local_path,
            }),
        )
        .await;
        Ok(response)
    }

    async fn list_agents(&self) -> Result<AgentsResponse, ApiError> {
        // The central server does not pick a representative node anymore.
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "specify a node: GET /api/v1/nodes/{node_id}/agents",
        ))
    }

    async fn list_agents_node(&self, node_id: &str) -> Result<AgentsResponse, ApiError> {
        let mut response = match self
            .node_rpc(
                &fxg_protocol::NodeId::from(node_id),
                NodeRpcRequest::ListAgents,
            )
            .await?
        {
            NodeRpcResponse::Agents { response } => response,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };

        // Merge availability across all nodes (installed_agents from NodeHello)
        let nodes = self.inner.db.list_nodes().await.map_err(ApiError::from)?;
        for agent in &mut response.agents {
            agent.installed_nodes = nodes
                .iter()
                .filter(|node| node.installed_agents.iter().any(|id| id == &agent.id))
                .map(|node| node.node_id.as_str().to_owned())
                .collect();
        }
        Ok(response)
    }

    async fn manage_agent(
        &self,
        node_id: &str,
        action: AgentAction,
        client: ClientInfo,
    ) -> Result<AgentOpResponse, ApiError> {
        if !self
            .inner
            .hub
            .is_online(&fxg_protocol::NodeId::from(node_id))
            .await
        {
            return Err(ApiError::node_offline(
                node_id,
                format!("node is offline: {node_id}"),
            ));
        }
        let message = match self
            .node_rpc(
                &fxg_protocol::NodeId::from(node_id),
                NodeRpcRequest::ManageAgent {
                    action: action.clone(),
                },
            )
            .await?
        {
            NodeRpcResponse::AgentOp { message } => message,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };
        self.record_audit(
            fxg_db::audit::actions::AGENT_MANAGE,
            &client,
            None,
            serde_json::json!({
                "node_id": node_id,
                "action": action,
                "message": &message,
            }),
        )
        .await;
        Ok(AgentOpResponse { message })
    }

    async fn rotate_auth_token(
        &self,
        client: ClientInfo,
    ) -> Result<RotateAuthTokenResponse, ApiError> {
        let token = ServerState::rotate_auth_token(self).map_err(ApiError::internal)?;
        self.record_audit(
            fxg_db::audit::actions::AUTH_TOKEN_ROTATE,
            &client,
            None,
            serde_json::json!({ "scope": "server" }),
        )
        .await;
        Ok(RotateAuthTokenResponse { token })
    }

    async fn node_tokens(&self) -> Result<NodeTokensResponse, ApiError> {
        let tokens = self
            .inner
            .db
            .list_node_tokens()
            .await
            .map_err(ApiError::from)?;
        Ok(NodeTokensResponse {
            tokens: tokens
                .into_iter()
                .map(|(node_id, hash)| NodeTokenSummary {
                    node_id: node_id.into(),
                    token_prefix: hash.chars().take(12).collect(),
                })
                .collect(),
        })
    }

    async fn issue_node_token(
        &self,
        request: IssueNodeTokenRequest,
        client: ClientInfo,
    ) -> Result<IssueNodeTokenResponse, ApiError> {
        let node_id = request.node_id.as_str().trim().to_owned();
        if node_id.is_empty() {
            return Err(ApiError::bad_request("node_id must not be empty"));
        }
        // Register an unregistered node as a placeholder
        // (overwritten with real info by `NodeHello` when the node connects)
        let nodes = self.inner.db.list_nodes().await.map_err(ApiError::from)?;
        if !nodes.iter().any(|node| node.node_id.as_str() == node_id) {
            self.inner
                .db
                .upsert_node(&fxg_db::NodeRecord::new(
                    node_id.clone(),
                    node_id.clone(),
                    "unknown",
                    "unknown",
                    "unknown",
                ))
                .await
                .map_err(ApiError::from)?;
        }
        let token = auth::generate_token();
        self.inner
            .db
            .set_node_token_hash(&node_id, Some(&auth::hash_token(&token)))
            .await
            .map_err(ApiError::from)?;
        self.record_audit(
            fxg_db::audit::actions::NODE_TOKEN_MANAGE,
            &client,
            None,
            serde_json::json!({ "action": "issue", "node_id": &node_id }),
        )
        .await;
        Ok(IssueNodeTokenResponse {
            node_id: node_id.into(),
            token,
        })
    }

    async fn revoke_node_token(&self, node_id: &str, client: ClientInfo) -> Result<(), ApiError> {
        self.inner
            .db
            .set_node_token_hash(node_id, None)
            .await
            .map_err(ApiError::from)?;
        self.record_audit(
            fxg_db::audit::actions::NODE_TOKEN_MANAGE,
            &client,
            None,
            serde_json::json!({ "action": "revoke", "node_id": node_id }),
        )
        .await;
        Ok(())
    }

    async fn list_worktrees(&self, project_id: &str) -> Result<WorktreesResponse, ApiError> {
        let projects = self
            .inner
            .db
            .list_projects()
            .await
            .map_err(ApiError::from)?;
        let project = projects
            .iter()
            .find(|project| project.project_id.as_str() == project_id)
            .ok_or_else(|| ApiError::not_found(format!("project not found: {project_id}")))?;
        let worktrees = project
            .bindings
            .iter()
            .map(|binding| WorktreeInfo {
                node_id: binding.node_id.clone(),
                path: binding.local_path.clone(),
                branch: binding.git_branch.clone(),
                // HEAD commits are not included in the node's real-time reports (NodeHello)
                head_commit: None,
                is_main: !binding.is_worktree,
            })
            .collect();
        Ok(WorktreesResponse { worktrees })
    }

    async fn create_worktree(
        &self,
        project_id: &str,
        request: CreateWorktreeRequest,
        _client: ClientInfo,
    ) -> Result<WorktreeInfo, ApiError> {
        if !self.inner.hub.is_online(&request.node_id).await {
            return Err(ApiError::node_offline(
                request.node_id.as_str(),
                format!("node is offline: {}", request.node_id),
            ));
        }
        let info = match self
            .node_rpc(
                &request.node_id,
                NodeRpcRequest::ManageWorktree {
                    project_id: project_id.into(),
                    action: WorktreeAction::Add {
                        branch: request.branch,
                        base_branch: request.base_branch,
                    },
                },
            )
            .await?
        {
            NodeRpcResponse::Worktree {
                worktree: Some(info),
            } => info,
            _ => return Err(ApiError::internal("node returned no worktree info")),
        };
        Ok(info)
    }

    async fn remove_worktree(
        &self,
        project_id: &str,
        request: RemoveWorktreeRequest,
        _client: ClientInfo,
    ) -> Result<(), ApiError> {
        if !self.inner.hub.is_online(&request.node_id).await {
            return Err(ApiError::node_offline(
                request.node_id.as_str(),
                format!("node is offline: {}", request.node_id),
            ));
        }
        self.node_rpc(
            &request.node_id,
            NodeRpcRequest::ManageWorktree {
                project_id: project_id.into(),
                action: WorktreeAction::Remove {
                    path: request.path,
                    force: request.force,
                },
            },
        )
        .await?;
        Ok(())
    }

    async fn workspace_diff(
        &self,
        session_id: &str,
        scope: DiffScope,
        base_branch: Option<String>,
    ) -> Result<WorkspaceDiffResponse, ApiError> {
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        match self
            .node_rpc(
                &node_id,
                NodeRpcRequest::GetGitDiff {
                    session_id: session_id.into(),
                    scope,
                    base_branch,
                },
            )
            .await?
        {
            NodeRpcResponse::GitDiff { diff } => Ok(diff),
            _ => Err(ApiError::internal("unexpected node RPC response")),
        }
    }

    async fn provisioners(&self) -> Result<ProvisionersResponse, ApiError> {
        let provisioners = self
            .provisioners()
            .configs()
            .into_iter()
            .map(|(name, config)| ProvisionerSummary {
                name,
                description: config.description,
                command: config.command,
                args: config.args,
                idle_timeout_secs: config.idle_timeout_secs,
                env: config.env,
            })
            .collect();
        Ok(ProvisionersResponse { provisioners })
    }

    async fn provisioner_test(
        &self,
        name: &str,
    ) -> Result<fxg_protocol::client_api::ProvisionerTestResponse, ApiError> {
        crate::provisioner::provisioner_test(self, name).await
    }

    async fn settings_server(&self) -> Result<ServerSettingsSnapshot, ApiError> {
        self.server_settings_snapshot()
            .await
            .map_err(ApiError::internal)
    }

    async fn update_server_settings(
        &self,
        request: UpdateServerSettingsRequest,
    ) -> Result<ServerSettingsSnapshot, ApiError> {
        self.update_server_settings(&request)
            .await
            .map_err(ApiError::internal)
    }

    async fn execute_node_rpc(
        &self,
        node_id: &NodeId,
        request: NodeRpcRequest,
        _client: Option<ClientInfo>,
    ) -> Result<NodeRpcResponse, ApiError> {
        let request_id = CommandId::from(uuid_v7());
        self.inner.hub.rpc(node_id, &request_id, request).await
    }

    async fn server_secrets(&self) -> Result<SecretsResponse, ApiError> {
        ServerState::server_secrets(self)
            .await
            .map_err(ApiError::internal)
    }

    async fn upsert_server_secret(
        &self,
        secret_id: &SecretId,
        request: UpsertSecretRequest,
    ) -> Result<SecretsResponse, ApiError> {
        ServerState::upsert_server_secret(self, secret_id, &request)
            .await
            .map_err(secret_error)
    }

    async fn delete_server_secret(
        &self,
        secret_id: &SecretId,
    ) -> Result<SecretsResponse, ApiError> {
        ServerState::delete_server_secret(self, secret_id)
            .await
            .map_err(secret_error)
    }

    async fn project_profile(&self, project_id: &str) -> Result<ProjectProfile, ApiError> {
        ServerState::project_profile(self, project_id).await
    }

    async fn update_project_profile(
        &self,
        project_id: &str,
        request: UpdateProjectProfileRequest,
    ) -> Result<ProjectProfile, ApiError> {
        ServerState::update_project_profile(self, project_id, &request).await
    }

    async fn kill_switch(
        &self,
        reason: &str,
        client: ClientInfo,
    ) -> Result<KillSwitchResponse, ApiError> {
        let notified = self
            .inner
            .hub
            .broadcast(ServerToNodeMsg::KillAllSessions {
                reason: reason.to_owned(),
            })
            .await;
        self.record_audit(
            fxg_db::audit::actions::KILL_SWITCH,
            &client,
            None,
            serde_json::json!({
                "reason": reason,
                "notified_nodes": notified,
            }),
        )
        .await;
        Ok(KillSwitchResponse {
            notified_nodes: notified.len() as u64,
        })
    }

    async fn push_subscribe(
        &self,
        request: PushSubscribeRequest,
        client: ClientInfo,
    ) -> Result<PushSubscribeResponse, ApiError> {
        if request.endpoint.trim().is_empty() {
            return Err(ApiError::bad_request("endpoint is required"));
        }
        let record = PushSubscriptionRecord {
            endpoint: request.endpoint.clone(),
            p256dh: request.p256dh.clone(),
            auth: request.auth.clone(),
            device_name: request.device_name.clone(),
        };
        fxg_db::push::upsert_push_subscription(self.inner.db.pool(), &record)
            .await
            .map_err(ApiError::from)?;
        tracing::info!(
            endpoint = %record.endpoint,
            device = record.device_name.as_deref().unwrap_or("unknown"),
            client_ip = %client.ip,
            "registered web push subscription"
        );
        Ok(PushSubscribeResponse { ok: true })
    }

    async fn pty_spawn(&self, params: PtySpawnParams) -> Result<(), PtyChannelError> {
        let session = self
            .inner
            .db
            .get_session(&params.session_id)
            .await
            .map_err(PtyChannelError::internal)?
            .ok_or_else(|| {
                PtyChannelError::not_found(format!("session not found: {}", params.session_id))
            })?;
        if !self.inner.hub.is_online(&session.node_id).await {
            return Err(PtyChannelError::offline(format!(
                "node is offline: {}",
                session.node_id
            )));
        }
        self.inner
            .hub
            .insert_pty(&params.pty_id, &session.node_id)
            .await;
        let result = self
            .inner
            .hub
            .send(
                &session.node_id,
                ServerToNodeMsg::PtySpawn {
                    pty_id: params.pty_id.clone(),
                    session_id: params.session_id.clone().into(),
                    cols: params.cols,
                    rows: params.rows,
                    shell_cmd: params.shell_cmd.clone(),
                },
            )
            .await;
        if let Err(err) = result {
            self.inner.hub.remove_pty(&params.pty_id).await;
            return Err(PtyChannelError::new(err.code.as_str(), err.message));
        }
        Ok(())
    }

    async fn pty_attach(&self, pty_id: &str) -> Result<(), PtyChannelError> {
        if self.inner.hub.pty_subscribe(pty_id).await.is_some() {
            Ok(())
        } else {
            Err(PtyChannelError::not_found(format!(
                "pty session not found: {pty_id}"
            )))
        }
    }

    async fn pty_subscribe(
        &self,
        pty_id: &str,
    ) -> Result<broadcast::Receiver<PtyChannelEvent>, PtyChannelError> {
        self.inner
            .hub
            .pty_subscribe(pty_id)
            .await
            .ok_or_else(|| PtyChannelError::not_found(format!("pty session not found: {pty_id}")))
    }

    async fn pty_write(&self, pty_id: &str, data: &[u8]) -> Result<(), PtyChannelError> {
        use base64::Engine as _;
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(data);
        self.inner
            .hub
            .pty_send(
                pty_id,
                ServerToNodeMsg::PtyInput {
                    pty_id: pty_id.to_owned(),
                    data_b64,
                },
            )
            .await
    }

    async fn pty_resize(&self, pty_id: &str, cols: u16, rows: u16) -> Result<(), PtyChannelError> {
        self.inner
            .hub
            .pty_send(
                pty_id,
                ServerToNodeMsg::PtyResize {
                    pty_id: pty_id.to_owned(),
                    cols,
                    rows,
                },
            )
            .await
    }

    async fn pty_kill(&self, pty_id: &str) -> Result<(), PtyChannelError> {
        self.inner
            .hub
            .pty_send(
                pty_id,
                ServerToNodeMsg::PtyKill {
                    pty_id: pty_id.to_owned(),
                },
            )
            .await
    }

    async fn pty_disconnect(&self, pty_id: &str) {
        fxg_protocol::discard_with!(
            self.inner
                .hub
                .pty_send(
                    pty_id,
                    ServerToNodeMsg::PtyKill {
                        pty_id: pty_id.to_owned(),
                    },
                )
                .await,
            "notify the node that a pty websocket disconnected"
        );
        self.inner.hub.remove_pty(pty_id).await;
    }
}

impl ServerState {
    /// Relays a WS command to a node and returns the `CommandResult`.
    async fn run_ws_command(
        &self,
        node_id: &fxg_protocol::NodeId,
        command_id: &CommandId,
        message: ServerToNodeMsg,
    ) -> CommandResult {
        match self.inner.hub.command(node_id, command_id, message).await {
            Ok(result) => result,
            Err(err) => command_failure(command_id.clone(), err),
        }
    }
}

fn secret_error(err: ServerError) -> ApiError {
    match err {
        ServerError::SecretInvalid(message) => {
            ApiError::from_code(ErrorCode::SecretInvalid, message)
        }
        ServerError::Db(fxg_db::DbError::SecretInUse { id, used_by }) => ApiError::from_code(
            ErrorCode::SecretInUse,
            format!("secret {} is in use: {}", id.as_str(), used_by.join(", ")),
        ),
        ServerError::Db(fxg_db::DbError::SecretInvalid(message)) => {
            ApiError::from_code(ErrorCode::SecretInvalid, message)
        }
        other => ApiError::internal(other),
    }
}
