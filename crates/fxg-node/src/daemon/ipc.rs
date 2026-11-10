//! Local IPC server between the CLI (`fxg`) and the daemon.
//!
//! - **Transport**: Windows uses a named pipe (`\\.\pipe\fxg-node-<username>`);
//!   Linux / macOS / WSL use a Unix domain socket (`$XDG_RUNTIME_DIR/fxg/node.sock`
//!   or `/tmp/fxg-<uid>/node.sock`)
//! - **Framing**: Length-prefixed JSON
//!   (4-byte LE length header + JSON payload)
//!
//! Design: `docs/03-protocol-and-api.md` §4.
//!
//! - Normal commands are processed as one request / one response.

use std::path::{Path, PathBuf};

use fxg_db::SessionFilter;
use fxg_protocol::CommandId;
use fxg_protocol::common::{ErrorCode, ErrorInfo};
use fxg_protocol::events::SessionEventEnvelope;
use fxg_protocol::ipc::{
    IPC_PROTOCOL_VERSION, IpcOutcome, IpcRequest, IpcRequestKind, IpcResultKind, IpcServerMessage,
    LocalRequest, LocalResult, SessionDetail,
};
use fxg_protocol::node_hub::NodeRpcOutcome;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

use super::DaemonState;
use super::ops::AuditSource;
use crate::error::NodeError;
use crate::ipc_framing::{read_frame, write_message};

/// Upper limit of the session list returned by `fxg session list` etc.
const SESSION_LIST_LIMIT: u32 = 200;

/// Checks whether the IPC endpoint is taken by another daemon.
///
/// If a connectable socket remains, it is treated as another running daemon and
/// results in an error. A leftover socket that cannot be connected to is removed.
#[cfg(unix)]
pub async fn check_endpoint_available(endpoint: &str) -> Result<(), NodeError> {
    let path = Path::new(endpoint);
    if !path.exists() {
        return Ok(());
    }
    if tokio::net::UnixStream::connect(path).await.is_ok() {
        return Err(NodeError::Hub(format!(
            "another daemon is already listening on {endpoint}"
        )));
    }
    std::fs::remove_file(path).map_err(|err| NodeError::io(path, err))?;
    Ok(())
}

/// Checks whether the IPC endpoint is taken by another daemon (Windows: named pipe).
#[cfg(windows)]
pub async fn check_endpoint_available(endpoint: &str) -> Result<(), NodeError> {
    use tokio::net::windows::named_pipe::ClientOptions;

    match ClientOptions::new().open(endpoint) {
        Ok(_) => Err(NodeError::Hub(format!(
            "another daemon is already listening on {endpoint}"
        ))),
        // ERROR_PIPE_BUSY (231): running, but all instances are handling connections
        Err(err) if err.raw_os_error() == Some(crate::error::PIPE_BUSY) => Err(NodeError::Hub(
            format!("another daemon is already listening on {endpoint}"),
        )),
        // ERROR_FILE_NOT_FOUND (2) means not started. Other errors are detected at bind time
        Err(_) => Ok(()),
    }
}

/// Starts the local IPC server and accepts connections until shutdown.
#[cfg(unix)]
pub async fn serve(
    state: DaemonState,
    endpoint: String,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), NodeError> {
    let path = PathBuf::from(&endpoint);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| NodeError::io(parent, err))?;
    }

    let listener = tokio::net::UnixListener::bind(&path)
        .map_err(|err| NodeError::Hub(format!("failed to bind {endpoint}: {err}")))?;
    tracing::info!(endpoint = %endpoint, "local ipc server listening (unix socket)");

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        let state = state.clone();
                        let connection_shutdown = shutdown.clone();
                        fxg_protocol::spawn_guarded!("ipc connection", async move {
                            if let Err(err) = handle_connection(state, stream, connection_shutdown).await {
                                tracing::debug!("ipc connection ended: {err}");
                            }
                        });
                    }
                    Err(err) => tracing::warn!("ipc accept failed: {err}"),
                }
            }
        }
    }

    fxg_protocol::discard_with!(std::fs::remove_file(&path), "remove the stale IPC socket");
    Ok(())
}

/// Starts the local IPC server (Windows: named pipe).
#[cfg(windows)]
pub async fn serve(
    state: DaemonState,
    endpoint: String,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), NodeError> {
    use tokio::net::windows::named_pipe::HubOptions;

    let create_instance = |first: bool| {
        let mut options = HubOptions::new();
        options.first_pipe_instance(first);
        options
            .create(&endpoint)
            .map_err(|err| NodeError::Hub(format!("failed to create named pipe {endpoint}: {err}")))
    };

    tracing::info!(endpoint = %endpoint, "local ipc server listening (named pipe)");

    // If the listening instance disappears between connections, clients receive
    // ERROR_PIPE_BUSY (231) (observed on CI: windows-latest).
    // Prepare the next instance first, then wait for a connection on the current one.
    let mut server = create_instance(false)?;
    loop {
        let next = create_instance(false)?;
        tokio::select! {
            _ = shutdown.changed() => break,
            connected = server.connect() => {
                match connected {
                    Ok(()) => {
                        let state = state.clone();
                        let connection_shutdown = shutdown.clone();
                        fxg_protocol::spawn_guarded!("ipc connection", async move {
                            if let Err(err) = handle_connection(state, server, connection_shutdown).await {
                                tracing::debug!("ipc connection ended: {err}");
                            }
                        });
                        server = next;
                    }
                    Err(err) => tracing::warn!("named pipe connect failed: {err}"),
                }
            }
        }
    }
    Ok(())
}

/// Handles a single connection (length-prefixed JSON request/response loop).
///
/// Closes the connection when a shutdown request is received.
pub(crate) async fn handle_connection<S>(
    state: DaemonState,
    stream: S,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    loop {
        let frame = tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            frame = read_frame(&mut reader) => frame?,
        };
        let Some(frame) = frame else {
            return Ok(()); // The client disconnected
        };
        let request: IpcRequest = match serde_json::from_slice(&frame) {
            Ok(request) => request,
            Err(err) => {
                let response = IpcServerMessage::Response {
                    request_id: CommandId::new("invalid"),
                    outcome: IpcOutcome::Err {
                        error: ErrorInfo::new(
                            ErrorCode::InvalidState,
                            format!("invalid ipc message: {err}"),
                        ),
                    },
                };
                write_message(&mut writer, &response).await?;
                continue;
            }
        };
        if request.protocol_version != IPC_PROTOCOL_VERSION {
            tracing::error!(
                protocol_version = request.protocol_version,
                required = IPC_PROTOCOL_VERSION,
                "local IPC protocol version mismatch; restart the daemon"
            );
            let response = IpcServerMessage::Response {
                request_id: request.request_id,
                outcome: IpcOutcome::Err {
                    error: ErrorInfo::new(
                        ErrorCode::VersionMismatch,
                        format!(
                            "local IPC protocol version {} is not supported (daemon expects {}); restart the daemon",
                            request.protocol_version, IPC_PROTOCOL_VERSION
                        ),
                    ),
                },
            };
            write_message(&mut writer, &response).await?;
            return Ok(());
        }

        let shutdown_requested = matches!(
            &request.request,
            IpcRequestKind::Local {
                request: LocalRequest::Shutdown { .. }
            }
        );
        let response = dispatch(&state, request).await;
        write_message(&mut writer, &response).await?;
        if shutdown_requested {
            state.request_shutdown();
            return Ok(());
        }
    }
}

/// Dispatch error (structured error code).
#[derive(Debug)]
struct DispatchError {
    code: ErrorCode,
    message: String,
}

impl DispatchError {
    fn new(_command_id: &CommandId, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn from_node_error(_command_id: &CommandId, err: NodeError) -> Self {
        Self {
            code: err.error_code(),
            message: err.to_string(),
        }
    }

    fn from_api_error(_command_id: &CommandId, err: fxg_hub::api::ApiError) -> Self {
        Self {
            code: err.code,
            message: err.message,
        }
    }
}

/// Processes a single request and returns the response message.
pub async fn dispatch(state: &DaemonState, request: IpcRequest) -> IpcServerMessage {
    let request_id = request.request_id.clone();
    let outcome = match request.request {
        IpcRequestKind::Node { request } => {
            match crate::rpc::execute(state, &request_id, request, &AuditSource::local()).await {
                NodeRpcOutcome::Ok { response } => IpcOutcome::Ok {
                    result: IpcResultKind::Node { response },
                },
                NodeRpcOutcome::Err { error } => IpcOutcome::Err { error },
            }
        }
        IpcRequestKind::Local { request } => {
            if request.command_id() != &request_id {
                IpcOutcome::Err {
                    error: ErrorInfo::new(
                        ErrorCode::InvalidState,
                        "local IPC request_id does not match command_id",
                    ),
                }
            } else {
                dispatch_local(state, request).await
            }
        }
    };
    IpcServerMessage::Response {
        request_id,
        outcome,
    }
}

/// Dispatches a CLI-only operation and wraps the result.
async fn dispatch_local(state: &DaemonState, request: LocalRequest) -> IpcOutcome {
    match handle_local(state, request).await {
        Ok((_command_id, result)) => IpcOutcome::Ok {
            result: IpcResultKind::Local { result },
        },
        Err(err) => IpcOutcome::Err {
            error: ErrorInfo::new(err.code, err.message),
        },
    }
}

async fn handle_local(
    state: &DaemonState,
    request: LocalRequest,
) -> Result<(CommandId, LocalResult), DispatchError> {
    match request {
        LocalRequest::Ping { command_id } => Ok((
            command_id,
            LocalResult::Ack {
                message: Some("pong".to_owned()),
            },
        )),

        LocalRequest::GetLocalStatus { command_id } => {
            let sessions = state
                .db()
                .list_sessions(&SessionFilter {
                    statuses: DaemonState::active_statuses(),
                    limit: Some(SESSION_LIST_LIMIT),
                    ..SessionFilter::default()
                })
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            let unsynced = state
                .db()
                .unsynced_event_count()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            let last_synced_at = state
                .db()
                .last_synced_at()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            Ok((
                command_id,
                LocalResult::LocalStatus {
                    status: fxg_protocol::ipc::LocalStatus {
                        sessions,
                        hub_connected: state.hub_connected(),
                        unsynced_event_count: unsynced,
                        last_synced_at,
                    },
                },
            ))
        }
        LocalRequest::Diagnostics { command_id } => {
            let diagnostics = state
                .diagnostics_response()
                .await
                .map_err(|err| DispatchError::from_api_error(&command_id, err))?;
            Ok((command_id, LocalResult::Diagnostics { diagnostics }))
        }
        LocalRequest::SupportBundle { command_id } => {
            let bundle = state
                .support_bundle_response()
                .await
                .map_err(|err| DispatchError::from_api_error(&command_id, err))?;
            Ok((
                command_id,
                LocalResult::SupportBundle {
                    bundle: Box::new(bundle),
                },
            ))
        }
        LocalRequest::Incidents {
            command_id,
            limit,
            include_resolved,
        } => {
            let incidents = state
                .db()
                .list_incidents(limit.clamp(1, 500), include_resolved)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            let (retention_days, max_entries) = fxg_hub::api::incident_retention();
            Ok((
                command_id,
                LocalResult::Incidents {
                    incidents: fxg_protocol::common::IncidentListResponse {
                        incidents,
                        retention_days,
                        max_entries,
                    },
                },
            ))
        }

        LocalRequest::WorktreeAdd {
            command_id,
            cwd,
            project_id,
            branch,
            base_branch,
        } => {
            let outcome = state
                .worktree_add(
                    Path::new(&cwd),
                    project_id.as_ref().map(fxg_protocol::ProjectId::as_str),
                    &branch,
                    base_branch,
                    &AuditSource::local(),
                )
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                LocalResult::WorktreeAdded {
                    path: outcome.path.to_string_lossy().into_owned(),
                    branch: outcome.branch,
                    created: outcome.created,
                },
            ))
        }

        LocalRequest::ListSessions {
            command_id,
            include_stopped,
            include_archived,
        } => {
            let filter = SessionFilter {
                statuses: if include_stopped {
                    Vec::new()
                } else {
                    DaemonState::active_statuses()
                },
                include_archived,
                limit: Some(SESSION_LIST_LIMIT),
                ..SessionFilter::default()
            };
            let sessions = state
                .db()
                .list_sessions(&filter)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            Ok((command_id, LocalResult::Sessions { sessions }))
        }

        LocalRequest::Shutdown { command_id } => {
            // The connection handler requests shutdown after writing this response.
            Ok((command_id, LocalResult::Ack { message: None }))
        }
        LocalRequest::AuthToken { command_id, rotate } => {
            let token = if rotate {
                state
                    .rotate_auth_token()
                    .map_err(|err| DispatchError::from_node_error(&command_id, err))?
            } else {
                state.auth_token()
            };
            Ok((command_id, LocalResult::AuthToken { token }))
        }

        LocalRequest::GitCredential { command_id, prompt } => {
            // Git credential proxy: resolve by relaying in memory to the hub
            // (without leaving the token on disk inside the VM. Design: docs/01 §6.4)
            let result = match state.credentials().resolve(&prompt).await {
                Ok(response) => LocalResult::GitCredential {
                    username: if response.username.is_empty() {
                        crate::credentials::default_username().to_owned()
                    } else {
                        response.username
                    },
                    token: response.token,
                    error: response.error,
                },
                Err(err) => LocalResult::GitCredential {
                    username: crate::credentials::default_username().to_owned(),
                    token: None,
                    error: Some(err.to_string()),
                },
            };
            Ok((command_id, result))
        }

        LocalRequest::EnsureSession {
            command_id,
            cwd,
            agent_id,
            extra_args,
            initial_mode,
        } => {
            // Refresh the project binding of the working directory
            // (updates default path resolution and recent-use records at session start)
            state
                .resolve_and_register_project(Path::new(&cwd))
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            let outcome = state
                .session_manager()
                .ensure_session(
                    &command_id,
                    Path::new(&cwd),
                    &agent_id,
                    &extra_args,
                    initial_mode.as_deref(),
                )
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            state
                .record_audit(
                    fxg_db::audit::actions::SESSION_START,
                    &AuditSource::local(),
                    Some(&outcome.session_id),
                    serde_json::json!({ "cwd": cwd, "agent_id": agent_id }),
                )
                .await;
            Ok((
                command_id,
                LocalResult::EnsureSession {
                    session_id: outcome.session_id.into(),
                },
            ))
        }

        LocalRequest::SessionImport {
            command_id,
            agent_id,
            agent_session_id,
            cwd,
            title,
        } => {
            state
                .resolve_and_register_project(Path::new(&cwd))
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            let session_id = state
                .session_manager()
                .import_session(
                    &command_id,
                    &agent_id,
                    &agent_session_id,
                    Path::new(&cwd),
                    title.as_deref(),
                )
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            state
                .record_audit(
                    fxg_db::audit::actions::SESSION_START,
                    &AuditSource::local(),
                    Some(&session_id),
                    serde_json::json!({
                        "cwd": cwd,
                        "agent_id": agent_id,
                        "imported_agent_session_id": agent_session_id,
                    }),
                )
                .await;
            Ok((
                command_id,
                LocalResult::SessionImported {
                    session_id: session_id.into(),
                },
            ))
        }

        LocalRequest::SendPrompt {
            command_id,
            session_id,
            input,
            client_source,
        } => {
            state
                .session_manager()
                .send_prompt(&command_id, session_id.as_str(), &input, client_source)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                LocalResult::CommandAccepted {
                    session_id: Some(session_id),
                },
            ))
        }

        LocalRequest::ControlSession {
            command_id,
            session_id,
            action,
        } => {
            state
                .session_manager()
                .control(&command_id, session_id.as_str(), &action)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                LocalResult::CommandAccepted {
                    session_id: Some(session_id),
                },
            ))
        }

        LocalRequest::SessionFork {
            command_id,
            session_id,
            cwd,
        } => {
            let outcome = state
                .session_manager()
                .fork(
                    &command_id,
                    session_id.as_str(),
                    cwd.as_deref().map(Path::new),
                )
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                LocalResult::SessionForked {
                    session_id: outcome.session_id.into(),
                },
            ))
        }

        LocalRequest::SessionShow {
            command_id,
            session_id,
            recent_events,
        } => {
            let session = state
                .db()
                .get_session(session_id.as_str())
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?
                .ok_or_else(|| {
                    DispatchError::new(
                        &command_id,
                        ErrorCode::NotFound,
                        format!("session not found: {session_id}"),
                    )
                })?;
            let (event_count, recent_events) =
                session_event_tail(state.db(), session_id.as_str(), recent_events)
                    .await
                    .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            let pending_permissions = state
                .db()
                .pending_permissions()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?
                .into_iter()
                .filter(|entry| entry.session_id == session_id)
                .collect();
            let pending_elicitations = state
                .db()
                .pending_elicitations()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?
                .into_iter()
                .filter(|entry| entry.session_id == session_id)
                .collect();
            Ok((
                command_id,
                LocalResult::SessionDetail(Box::new(SessionDetail {
                    session,
                    event_count,
                    recent_events,
                    pending_permissions,
                    pending_elicitations,
                })),
            ))
        }
    }
}

/// Retrieves the total session event count and the last `recent` entries (oldest first).
///
/// Advances to the end in batches while keeping only the most recent entries in a ring buffer.
async fn session_event_tail(
    db: &fxg_db::Db,
    session_id: &str,
    recent: u32,
) -> Result<(u64, Vec<SessionEventEnvelope>), NodeError> {
    const BATCH: u32 = 200;
    let mut cursor = 0u64;
    let mut tail: std::collections::VecDeque<SessionEventEnvelope> =
        std::collections::VecDeque::new();
    loop {
        let batch = db.session_events_after(session_id, cursor, BATCH).await?;
        let fetched = batch.events.len() as u32;
        if fetched == 0 {
            break;
        }
        cursor = batch.cursor;
        for event in batch.events {
            tail.push_back(event);
            if tail.len() > recent as usize {
                tail.pop_front();
            }
        }
        if fetched < BATCH {
            break;
        }
    }
    Ok((cursor, tail.into_iter().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonConfig, NodeDaemon};
    use crate::git;
    use crate::ipc_client::IpcClient;
    use fxg_protocol::node_hub::{NodeRpcRequest, NodeRpcResponse};
    use std::time::Duration;

    async fn start_daemon() -> (NodeDaemon, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "ipc-node", "IPC Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let daemon = NodeDaemon::start(config).await.expect("start");
        wait_for_endpoint(daemon.ipc_endpoint()).await;
        (daemon, dir)
    }

    /// Waits until the IPC server is ready to accept connections.
    async fn wait_for_endpoint(endpoint: &str) {
        for _ in 0..200 {
            if IpcClient::connect(endpoint).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("ipc endpoint did not become ready: {endpoint}");
    }

    /// Connects over IPC, sends one request, and receives the response.
    ///
    /// The client uses `IpcClient` (verifying Unix domain sockets / named pipes
    /// with the same test code).
    async fn roundtrip(endpoint: &str, request: LocalRequest) -> IpcServerMessage {
        let request = IpcRequest {
            protocol_version: IPC_PROTOCOL_VERSION,
            request_id: request.command_id().clone(),
            request: IpcRequestKind::Local { request },
        };
        let mut client = IpcClient::connect(endpoint).await.expect("connect");
        client.request(&request).await.expect("request")
    }

    async fn roundtrip_node(
        endpoint: &str,
        request_id: impl Into<CommandId>,
        request: NodeRpcRequest,
    ) -> Result<NodeRpcResponse, Box<ErrorInfo>> {
        let request = IpcRequest {
            protocol_version: IPC_PROTOCOL_VERSION,
            request_id: request_id.into(),
            request: IpcRequestKind::Node { request },
        };
        let mut client = IpcClient::connect(endpoint).await.expect("connect");
        match client.request(&request).await.expect("request") {
            IpcServerMessage::Response {
                outcome:
                    IpcOutcome::Ok {
                        result: IpcResultKind::Node { response },
                    },
                ..
            } => Ok(response),
            IpcServerMessage::Response {
                outcome: IpcOutcome::Err { error },
                ..
            } => Err(Box::new(error)),
            other => panic!("unexpected response: {other:?}"),
        }
    }

    fn local_result(response: IpcServerMessage) -> LocalResult {
        match response {
            IpcServerMessage::Response {
                outcome:
                    IpcOutcome::Ok {
                        result: IpcResultKind::Local { result },
                    },
                ..
            } => result,
            other => panic!("unexpected response: {other:?}"),
        }
    }

    fn local_error(response: IpcServerMessage) -> ErrorInfo {
        match response {
            IpcServerMessage::Response {
                outcome: IpcOutcome::Err { error },
                ..
            } => error,
            other => panic!("unexpected response: {other:?}"),
        }
    }

    /// Starts a daemon with a mock agent driver injected (for session operations).
    async fn start_session_daemon() -> (NodeDaemon, crate::testutil::MockAgent, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        crate::testutil::write_mock_agent_config(dir.path()).await;
        let mut config =
            DaemonConfig::new(dir.path().to_path_buf(), "session-node", "Session Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let mock = crate::testutil::MockAgent::default();
        let daemon = NodeDaemon::start_with_driver_factory(config, mock.factory())
            .await
            .expect("start");
        wait_for_endpoint(daemon.ipc_endpoint()).await;
        (daemon, mock, dir)
    }

    #[tokio::test]
    async fn ipc_project_link_via_socket() {
        let (daemon, dir) = start_daemon().await;
        let endpoint = daemon.ipc_endpoint().to_owned();

        // Create a Git repository before linking
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        git::run_git(&repo, &["init", "-q", "-b", "main"])
            .await
            .expect("git init");
        git::run_git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:nazo6/flexagent.git",
            ],
        )
        .await
        .expect("remote");

        // project link writes an explicit path link
        let response = roundtrip_node(
            &endpoint,
            "c2",
            NodeRpcRequest::ProjectLink {
                project_id: "internal-agent-tools".into(),
                local_path: repo.to_string_lossy().into_owned(),
            },
        )
        .await
        .expect("project link");
        assert!(matches!(response, NodeRpcResponse::ProjectLink { .. }));
        let link = daemon
            .state()
            .db()
            .project_key_for_path(
                &fxg_pty::canonicalize(&repo)
                    .expect("canonicalize")
                    .to_string_lossy(),
            )
            .await
            .expect("link lookup")
            .expect("link exists");
        assert_eq!(link, "internal-agent-tools");

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn ipc_worktree_add_and_sessions() {
        let (daemon, dir) = start_daemon().await;
        let endpoint = daemon.ipc_endpoint().to_owned();

        // Git repository + 1 commit
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        git::run_git(&repo, &["init", "-q", "-b", "main"])
            .await
            .expect("git init");
        git::run_git(&repo, &["config", "user.email", "t@example.com"])
            .await
            .expect("email");
        git::run_git(&repo, &["config", "user.name", "t"])
            .await
            .expect("name");
        std::fs::write(repo.join("README.md"), "hello\n").expect("write");
        git::run_git(&repo, &["add", "-A"]).await.expect("add");
        git::run_git(&repo, &["commit", "-q", "-m", "init"])
            .await
            .expect("commit");

        let response = roundtrip(
            &endpoint,
            LocalRequest::WorktreeAdd {
                command_id: "w1".into(),
                cwd: repo.to_string_lossy().into_owned(),
                project_id: None,
                branch: "feat/ipc".to_owned(),
                base_branch: None,
            },
        )
        .await;
        let worktree_path = match local_result(response) {
            LocalResult::WorktreeAdded { path, created, .. } => {
                assert!(created);
                path
            }
            other => panic!("unexpected response: {other:?}"),
        };
        assert!(Path::new(&worktree_path).join("README.md").is_file());

        // Session list (empty)
        let response = roundtrip(
            &endpoint,
            LocalRequest::ListSessions {
                command_id: "s1".into(),
                include_stopped: true,
                include_archived: true,
            },
        )
        .await;
        match local_result(response) {
            LocalResult::Sessions { sessions } => assert!(sessions.is_empty()),
            other => panic!("unexpected response: {other:?}"),
        }

        // Getting and rotating the auth token
        let response = roundtrip(
            &endpoint,
            LocalRequest::AuthToken {
                command_id: "a1".into(),
                rotate: false,
            },
        )
        .await;
        let token = match local_result(response) {
            LocalResult::AuthToken { token } => token,
            other => panic!("unexpected response: {other:?}"),
        };
        assert_eq!(token, daemon.state().auth_token());

        let response = roundtrip(
            &endpoint,
            LocalRequest::AuthToken {
                command_id: "a2".into(),
                rotate: true,
            },
        )
        .await;
        match local_result(response) {
            LocalResult::AuthToken { token: rotated } => assert_ne!(rotated, token),
            other => panic!("unexpected response: {other:?}"),
        }

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn second_daemon_cannot_bind_same_socket() {
        let (daemon, dir) = start_daemon().await;
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "other", "Other");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = daemon.ipc_endpoint().to_owned();
        let err = NodeDaemon::start(config).await.expect_err("must fail");
        assert!(matches!(err, NodeError::Hub(_)), "unexpected error: {err}");

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn ipc_session_ensure_attach_and_prompt() {
        use fxg_protocol::common::SessionControlAction;

        let (daemon, mock, dir) = start_session_daemon().await;
        let endpoint = daemon.ipc_endpoint().to_owned();
        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");

        // EnsureSession: start a session
        let response = roundtrip(
            &endpoint,
            LocalRequest::EnsureSession {
                command_id: "e1".into(),
                cwd: workdir.to_string_lossy().into_owned(),
                agent_id: "mock".to_owned(),
                extra_args: Vec::new(),
                initial_mode: None,
            },
        )
        .await;
        let session_id = match local_result(response) {
            LocalResult::EnsureSession { session_id } => session_id,
            other => panic!("unexpected response: {other:?}"),
        };

        // A project binding is registered when the session starts
        // (foundation for default path resolution and recent-use records)
        let canonical_workdir = fxg_pty::canonicalize(&workdir).expect("canonicalize");
        let projects = daemon
            .state()
            .db()
            .list_projects()
            .await
            .expect("list projects");
        assert!(
            projects.iter().any(|project| project
                .bindings
                .iter()
                .any(|binding| binding.local_path == canonical_workdir.to_string_lossy())),
            "EnsureSession must register a project binding: {projects:?}"
        );

        // Sending a duplicate command_id yields COMMAND_DUPLICATE (idempotency)
        let response = roundtrip(
            &endpoint,
            LocalRequest::EnsureSession {
                command_id: "e1".into(),
                cwd: workdir.to_string_lossy().into_owned(),
                agent_id: "mock".to_owned(),
                extra_args: Vec::new(),
                initial_mode: None,
            },
        )
        .await;
        assert_eq!(local_error(response).code, ErrorCode::CommandDuplicate);

        // SendPrompt: send a prompt
        let response = roundtrip(
            &endpoint,
            LocalRequest::SendPrompt {
                command_id: "p1".into(),
                session_id: session_id.clone(),
                input: fxg_protocol::common::PromptInput::text("hello"),
                client_source: fxg_protocol::common::ClientSource::Cli,
            },
        )
        .await;
        match local_result(response) {
            LocalResult::CommandAccepted {
                session_id: Some(sid),
            } => {
                assert_eq!(sid, session_id);
            }
            other => panic!("unexpected response: {other:?}"),
        }

        // The prompt has reached the mock driver
        for _ in 0..100 {
            if !mock.prompts().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(mock.prompts(), vec!["hello"]);

        // ControlSession (SetMode) is forwarded to the driver
        let response = roundtrip(
            &endpoint,
            LocalRequest::ControlSession {
                command_id: "m1".into(),
                session_id: session_id.clone(),
                action: SessionControlAction::SetMode {
                    mode_id: "plan".to_owned(),
                },
            },
        )
        .await;
        assert!(matches!(
            response,
            IpcServerMessage::Response { request_id, .. } if request_id.as_str() == "m1"
        ));
        assert_eq!(mock.modes(), vec!["plan".to_owned()]);

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn ipc_session_resume_replays_context_and_rejects_active() {
        use fxg_acp::DriverEvent;
        use fxg_protocol::common::SessionStatus;
        use fxg_protocol::events::UnifiedEventPayload;

        let (daemon, mock, dir) = start_session_daemon().await;
        let endpoint = daemon.ipc_endpoint().to_owned();
        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");

        // Leave one conversation turn via EnsureSession → SendPrompt
        let response = roundtrip(
            &endpoint,
            LocalRequest::EnsureSession {
                command_id: "e1".into(),
                cwd: workdir.to_string_lossy().into_owned(),
                agent_id: "mock".to_owned(),
                extra_args: Vec::new(),
                initial_mode: None,
            },
        )
        .await;
        let session_id = match local_result(response) {
            LocalResult::EnsureSession { session_id, .. } => session_id,
            other => panic!("unexpected response: {other:?}"),
        };
        roundtrip(
            &endpoint,
            LocalRequest::SendPrompt {
                command_id: "p1".into(),
                session_id: session_id.clone(),
                input: fxg_protocol::common::PromptInput::text("hello resume"),
                client_source: fxg_protocol::common::ClientSource::Cli,
            },
        )
        .await;

        // Stop it by closing the driver so it can be resumed
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        mock.close_events();
        for _ in 0..200 {
            let row = daemon
                .state()
                .db()
                .get_session(session_id.as_str())
                .await
                .expect("query");
            if row.is_some_and(|session| session.status == SessionStatus::Stopped) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Resume (the mock driver has no native restore support → continue via history replay)
        let response = roundtrip_node(
            &endpoint,
            "r1",
            NodeRpcRequest::ResumeSession {
                session_id: session_id.clone(),
                force_replay: false,
            },
        )
        .await
        .expect("resume");
        match response {
            NodeRpcResponse::Resume { context_restored } => {
                assert!(!context_restored, "mock driver cannot restore natively");
            }
            other => panic!("unexpected response: {other:?}"),
        }

        // The replay context is sent to the new agent session
        let mut injected = false;
        for _ in 0..200 {
            if mock
                .prompts()
                .iter()
                .any(|prompt| prompt.contains("hello resume"))
            {
                injected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(injected, "resume context is injected to the agent");

        // Re-running on an active (already resumed) session is INVALID_STATE
        let error = roundtrip_node(
            &endpoint,
            "r2",
            NodeRpcRequest::ResumeSession {
                session_id: session_id.clone(),
                force_replay: false,
            },
        )
        .await
        .expect_err("active session");
        assert_eq!(error.code, ErrorCode::InvalidState);

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }
}
