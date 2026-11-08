//! Outbox sync worker (Node <-> Central Server).
//!
//! Design: `docs/01-architecture-and-sync.md` §2.2, `docs/03-protocol-and-api.md` §2.
//!
//! - **Outbound WebSocket**: connects to the central server's node hub
//!   (`/api/v1/node/ws`) with `Authorization: Bearer <NODE_TOKEN>` and reconnects with
//!   exponential backoff on disconnect
//! - **Store-and-Forward**: `NodeHello` (session sync state) -> the server sends
//!   `ResyncRequest` when needed -> events after `synced_up_to_node_seq` are sent in
//!   batches with `EventBatchPush`, and `EventBatchAck` updates the watermark
//! - **Real-time delivery**: completed events are pushed immediately with
//!   `EventBatchPush`, streaming deltas are delivered as `LiveStreamDelta` (persistence
//!   happens only when the turn completes)
//! - **Remote command execution**: executes `StartSession` / `SendPrompt` /
//!   `RespondPermission` / `ControlSession` / `ManageWorktree` / `GetGitDiff` /
//!   `Pty*` / `KillAllSessions` and returns results correlated by `command_id`

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use fxg_db::SessionFilter;
use fxg_protocol::CommandId;
use fxg_protocol::client_api::WorktreeInfo;
use fxg_protocol::common::WorktreeAction;
use fxg_protocol::node_server::{NodeToServerMsg, ServerToNodeMsg, SessionSyncState};
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::daemon::{AuditSource, DaemonState, VERSION};
use crate::error::NodeError;
use crate::session::SessionBroadcast;
use crate::session_manager::{ResumeParams, StartSessionParams};

/// Initial reconnect backoff.
const RECONNECT_MIN: Duration = Duration::from_secs(1);
/// Upper bound of the reconnect backoff.
const RECONNECT_MAX: Duration = Duration::from_secs(30);
/// Maximum number of events per `EventBatchPush` message.
const OUTBOX_BATCH_EVENTS: usize = 200;
/// Send queue capacity (including the PTY forwarder).
const SEND_QUEUE_CAPACITY: usize = 1024;
/// Maximum number of sessions reported in `NodeHello`.
const HELLO_SESSION_LIMIT: u32 = 1000;
/// Maximum Base64 chunk size per `WorkspaceBundleUpload` message (8 MiB).
pub const BUNDLE_CHUNK_B64_BYTES: usize = 8 * 1024 * 1024;

/// Processing context for commands from the server (absorbs transport differences).
///
/// A resident node (WS) uses the default values for all fields; an ephemeral VM
/// (`--stdio`) passes the workspace override and the process shutdown signal used on
/// drain.
#[derive(Default, Clone, Copy)]
pub(crate) struct CommandContext<'a> {
    /// Override directory of `--stdio --workspace`
    /// (pins `StartSession.local_path` to a fixed clone destination)
    pub workspace: Option<&'a Path>,
    /// Whether this is ephemeral VM mode (`DrainAndShutdown` backs up a Git bundle)
    pub ephemeral: bool,
    /// Signal that requests process shutdown when `DrainAndShutdown` is received
    pub shutdown: Option<&'a watch::Sender<bool>>,
}

/// Runs the Outbox sync worker until shutdown.
///
/// Started as a task by `NodeDaemon::start`.
pub async fn run(
    state: DaemonState,
    server_url: String,
    node_token: String,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut backoff = RECONNECT_MIN;
    loop {
        if *shutdown.borrow() {
            break;
        }

        match connect_once(&state, &server_url, &node_token, &mut shutdown).await {
            Ok(()) => {
                backoff = RECONNECT_MIN;
            }
            Err(err) => {
                tracing::warn!(url = %server_url, "central server connection failed: {err:#}");
            }
        }
        state.set_central_connected(false);
        if *shutdown.borrow() {
            break;
        }

        // Exponential backoff (interrupted immediately by a shutdown request)
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown.changed() => break,
        }
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
    state.credentials().detach();
    state.set_central_connected(false);
    tracing::info!("outbox sync worker stopped");
}

/// Performs one connection's worth of sync with the server (does not return until the
/// connection drops).
async fn connect_once(
    state: &DaemonState,
    server_url: &str,
    node_token: &str,
    shutdown: &mut watch::Receiver<bool>,
) -> anyhow::Result<()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut request = server_url.into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {node_token}").parse()?);
    let (socket, _response) = tokio_tungstenite::connect_async(request).await?;
    let (mut sender, mut receiver) = socket.split();

    state.set_central_connected(true);
    tracing::info!(url = %server_url, "connected to central server");

    // Send queue (shared path for the PTY forwarder / command responses)
    let (out_tx, mut out_rx) = mpsc::channel::<NodeToServerMsg>(SEND_QUEUE_CAPACITY);
    // Bind the Git credential proxy (the ephemeral VM's GIT_ASKPASS) to this connection
    state.credentials().attach(out_tx.clone());

    // Subscribe to the bus before flushing to avoid missing events
    // (even if events included in the flush are duplicated by live delivery, the
    //  server's INSERT OR IGNORE and the MAX update of the watermark ACK make it
    //  harmless)
    let mut bus_rx = state.bus().subscribe();

    // 1. Handshake
    send_msg(&mut sender, &build_hello(state).await).await?;

    // 2. Flush unsent events
    flush_outbox(state, &mut sender).await?;

    // 3. Bidirectional loop
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                return Ok(());
            }
            outgoing = out_rx.recv() => match outgoing {
                Some(message) => send_msg(&mut sender, &message).await?,
                None => return Ok(()),
            },
            _ = state.hello_refresh().notified() => {
                // Re-report project bindings after a worktree change and so on
                send_msg(&mut sender, &build_hello(state).await).await?;
            }
            bus = bus_rx.recv() => match bus {
                Ok(SessionBroadcast::Persisted { event, .. }) => {
                    send_msg(
                        &mut sender,
                        &NodeToServerMsg::EventBatchPush { events: vec![event] },
                    )
                    .await?;
                }
                Ok(SessionBroadcast::StreamDelta { session_id, delta }) => {
                    send_msg(
                        &mut sender,
                        &NodeToServerMsg::LiveStreamDelta { session_id, delta },
                    )
                    .await?;
                }
                // Ephemeral events (keystrokes and so on) are not synced
                Ok(SessionBroadcast::Ephemeral(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    // Recover missed events from the Outbox (watermark based)
                    tracing::warn!(skipped, "session bus lagged; flushing outbox");
                    flush_outbox(state, &mut sender).await?;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
            },
            incoming = receiver.next() => match incoming {
                Some(Ok(WsMessage::Text(text))) => {
                    handle_server_message(state, &text, &out_tx, &CommandContext::default()).await;
                }
                Some(Ok(WsMessage::Ping(payload))) => {
                    sender.send(WsMessage::Pong(payload)).await?;
                }
                Some(Ok(WsMessage::Close(_))) | None => {
                    return Ok(());
                }
                Some(Err(err)) => {
                    tracing::warn!(url = %server_url, "central server websocket error: {err}");
                    return Err(err.into());
                }
                _ => {}
            },
        }
    }
}

/// Builds `NodeHello` (node info, projects, and session sync state).
pub(crate) async fn build_hello(state: &DaemonState) -> NodeToServerMsg {
    let projects = match state.list_projects_with_path_state().await {
        Ok(projects) => projects
            .into_iter()
            .map(|project| fxg_protocol::common::NodeProjectReport {
                project_id: project.project_id.clone(),
                name: project.name,
                canonical_git_url: project.canonical_git_url,
                bindings: project
                    .bindings
                    .into_iter()
                    .filter(|binding| binding.node_id.as_str() == state.node_id())
                    .map(|binding| fxg_protocol::common::ProjectNodeBinding {
                        local_path: binding.local_path,
                        is_worktree: binding.is_worktree,
                        path_exists: binding.path_exists,
                        git_branch: binding.git_branch,
                        last_used_at: binding.last_used_at,
                    })
                    .collect(),
            })
            .collect(),
        Err(err) => {
            tracing::warn!("failed to list projects for NodeHello: {err}");
            Vec::new()
        }
    };

    let sessions = match state
        .db()
        .list_sessions(&SessionFilter {
            // Archived sessions are also synced (so the resync safety net covers them)
            include_archived: true,
            limit: Some(HELLO_SESSION_LIMIT),
            ..SessionFilter::default()
        })
        .await
    {
        Ok(sessions) => sessions
            .into_iter()
            .map(|session| SessionSyncState {
                session_id: session.session_id,
                last_node_seq: session.last_node_seq,
            })
            .collect(),
        Err(err) => {
            tracing::warn!("failed to list sessions for NodeHello: {err}");
            Vec::new()
        }
    };

    NodeToServerMsg::NodeHello {
        node_id: state.node_id().into(),
        name: state.config().node_name.clone(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        version: VERSION.to_owned(),
        is_ephemeral: state.config().ephemeral,
        installed_agents: state.installed_agents().await,
        projects,
        sessions,
    }
}

/// Turns unsent events (after `synced_up_to_node_seq`) into a sequence of outgoing
/// messages.
pub(crate) async fn outbox_messages(state: &DaemonState) -> anyhow::Result<Vec<NodeToServerMsg>> {
    let mut messages = Vec::new();
    for batch in state.db().extract_outbox().await? {
        for chunk in batch.events.chunks(OUTBOX_BATCH_EVENTS) {
            messages.push(NodeToServerMsg::EventBatchPush {
                events: chunk.to_vec(),
            });
        }
    }
    Ok(messages)
}

/// Pushes unsent events into the send queue (for the stdio transport).
pub(crate) async fn push_outbox(
    state: &DaemonState,
    out: &mpsc::Sender<NodeToServerMsg>,
) -> anyhow::Result<()> {
    for message in outbox_messages(state).await? {
        if out.send(message).await.is_err() {
            anyhow::bail!("outbox sink closed");
        }
    }
    Ok(())
}

/// Sends all unsent events (after `synced_up_to_node_seq`).
async fn flush_outbox(
    state: &DaemonState,
    sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
) -> anyhow::Result<()> {
    for message in outbox_messages(state).await? {
        send_msg(sender, &message).await?;
    }
    Ok(())
}

/// Sends a single message over the WS.
async fn send_msg<S>(sender: &mut S, message: &NodeToServerMsg) -> anyhow::Result<()>
where
    S: futures_util::Sink<WsMessage> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    let text = serde_json::to_string(message)?;
    sender.send(WsMessage::Text(text.into())).await?;
    Ok(())
}

/// Handles messages from the server.
pub(crate) async fn handle_server_message(
    state: &DaemonState,
    text: &str,
    out: &mpsc::Sender<NodeToServerMsg>,
    ctx: &CommandContext<'_>,
) {
    let message: ServerToNodeMsg = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(err) => {
            tracing::warn!("invalid server message: {err}");
            return;
        }
    };

    match message {
        ServerToNodeMsg::EventBatchAck {
            session_id,
            acked_up_to_node_seq,
        } => {
            if let Err(err) = state
                .db()
                .ack_watermark(session_id.as_str(), acked_up_to_node_seq)
                .await
            {
                tracing::warn!(
                    session_id = session_id.as_str(),
                    "failed to update sync watermark: {err}"
                );
            }
        }
        ServerToNodeMsg::ResyncRequest { sessions } => {
            for target in sessions {
                match state
                    .db()
                    .extract_outbox_after(target.session_id.as_str(), target.from_node_seq)
                    .await
                {
                    Ok(events) => {
                        for chunk in events.chunks(OUTBOX_BATCH_EVENTS) {
                            if out
                                .send(NodeToServerMsg::EventBatchPush {
                                    events: chunk.to_vec(),
                                })
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            session_id = target.session_id.as_str(),
                            "failed to load events for resync: {err}"
                        );
                    }
                }
            }
        }
        ServerToNodeMsg::StartSession {
            command_id,
            session_id,
            project_id: _,
            local_path,
            agent_id,
            initial_prompt,
            mode,
            extra_args,
            fork_context_messages,
            restore_git_bundle_b64,
        } => {
            // An ephemeral VM (`--stdio --workspace`) is pinned to the cloned directory
            // (the server does not know the path inside the VM, so the requested
            // local_path is not used)
            let local_path = ctx
                .workspace
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(&local_path));

            // Restore the backed-up Git bundle (handover to another node / resuming an
            // ephemeral VM)
            let restore_bundle_file = match restore_git_bundle_b64.as_deref() {
                Some(b64) => match BASE64.decode(b64.as_bytes()) {
                    Ok(bytes) => match crate::bundle::write_bundle_file(
                        &state.paths().bundles_dir(),
                        session_id.as_str(),
                        &bytes,
                    ) {
                        Ok(file) => Some(file),
                        Err(err) => {
                            send_command_result(out, command_id, Err(err)).await;
                            return;
                        }
                    },
                    Err(err) => {
                        send_command_result(
                            out,
                            command_id,
                            Err(NodeError::InvalidSession(format!(
                                "invalid git bundle payload: {err}"
                            ))),
                        )
                        .await;
                        return;
                    }
                },
                None => None,
            };

            // Bring the project binding of the working directory up to date
            // (an ephemeral VM already registered it at bootstrap, but re-register on
            //  every path to also refresh `last_used_at`)
            if let Err(err) = state.resolve_and_register_project(&local_path).await {
                send_command_result(out, command_id, Err(err)).await;
                return;
            }

            let result = state
                .session_manager()
                .start_session(StartSessionParams {
                    command_id: &command_id,
                    session_id: session_id.as_str(),
                    local_path: &local_path,
                    agent_id: &agent_id,
                    initial_prompt: initial_prompt.as_ref(),
                    mode: mode.as_deref(),
                    extra_args: extra_args.as_deref(),
                    fork_context: fork_context_messages.as_deref(),
                    restore_git_bundle: restore_bundle_file.as_deref(),
                })
                .await
                .map(|_| ());
            match result {
                Ok(()) => {
                    state
                        .record_audit(
                            fxg_db::audit::actions::SESSION_START,
                            &AuditSource::remote(state.node_id()),
                            Some(session_id.as_str()),
                            serde_json::json!({
                                "agent_id": agent_id,
                                "local_path": local_path,
                            }),
                        )
                        .await;
                    send_command_result(out, command_id, Ok(session_id)).await;
                }
                Err(err) => {
                    send_command_result(out, command_id, Err(err)).await;
                }
            }
        }
        ServerToNodeMsg::SendPrompt {
            command_id,
            session_id,
            input,
            client_source,
        } => {
            let result = state
                .session_manager()
                .send_prompt(&command_id, session_id.as_str(), &input, client_source)
                .await
                .map(|()| session_id);
            send_command_result(out, command_id, result).await;
        }
        ServerToNodeMsg::RespondPermission {
            command_id,
            session_id,
            request_id,
            selected_option_id,
            resolved_by,
        } => {
            let result = state
                .session_manager()
                .respond_permission(
                    &command_id,
                    session_id.as_str(),
                    &request_id,
                    &selected_option_id,
                    resolved_by.clone(),
                )
                .await
                .map(|()| session_id.clone());
            if result.is_ok() {
                state
                    .record_audit(
                        fxg_db::audit::actions::PERMISSION_RESOLVED,
                        &AuditSource::remote(state.node_id()),
                        Some(session_id.as_str()),
                        serde_json::json!({
                            "request_id": request_id,
                            "selected_option_id": selected_option_id,
                            "resolved_by": resolved_by,
                        }),
                    )
                    .await;
            }
            send_command_result(out, command_id, result).await;
        }
        ServerToNodeMsg::RespondElicitation {
            command_id,
            session_id,
            elicitation_id,
            action,
            content,
            resolved_by,
        } => {
            let result = state
                .session_manager()
                .respond_elicitation(
                    &command_id,
                    session_id.as_str(),
                    &elicitation_id,
                    action,
                    content.clone(),
                    resolved_by.clone(),
                )
                .await
                .map(|()| session_id.clone());
            if result.is_ok() {
                state
                    .record_audit(
                        fxg_db::audit::actions::ELICITATION_RESOLVED,
                        &AuditSource::remote(state.node_id()),
                        Some(session_id.as_str()),
                        serde_json::json!({
                            "elicitation_id": elicitation_id,
                            "action": action.as_str(),
                            "resolved_by": resolved_by,
                        }),
                    )
                    .await;
            }
            send_command_result(out, command_id, result).await;
        }
        ServerToNodeMsg::ControlSession {
            command_id,
            session_id,
            action,
        } => {
            let result = state
                .session_manager()
                .control(&command_id, session_id.as_str(), &action)
                .await
                .map(|()| session_id);
            send_command_result(out, command_id, result).await;
        }
        ServerToNodeMsg::RevertSession {
            command_id,
            session_id,
            target_node_seq,
            files_only,
        } => {
            let outcome = state
                .session_manager()
                .revert(
                    &command_id,
                    session_id.as_str(),
                    target_node_seq,
                    files_only,
                )
                .await;
            let message = match outcome {
                Ok(outcome) => NodeToServerMsg::RevertResult {
                    command_id,
                    success: true,
                    code: None,
                    outcome: Some(fxg_protocol::client_api::SessionRevertResponse {
                        target_node_seq: outcome.target_node_seq,
                        restored_tree_hash: outcome.restored_tree_hash,
                        backup_tree_hash: outcome.backup_tree_hash,
                        restored_files: outcome.restored_files as u64,
                        removed_files: outcome.removed_files as u64,
                        context_rebuilt: outcome.context_rebuilt,
                    }),
                    error: None,
                },
                Err(err) => NodeToServerMsg::RevertResult {
                    command_id,
                    success: false,
                    code: Some(err.error_code()),
                    outcome: None,
                    error: Some(err.to_string()),
                },
            };
            fxg_protocol::discard_with!(
                out.send(message).await,
                "send a message to a channel that may be closed"
            );
        }
        ServerToNodeMsg::ResumeSession {
            command_id,
            session_id,
            force_replay,
        } => {
            let outcome = state
                .session_manager()
                .resume(ResumeParams {
                    command_id: &command_id,
                    session_id: session_id.as_str(),
                    force_replay,
                })
                .await;
            let message = match outcome {
                Ok(outcome) => {
                    state
                        .record_audit(
                            fxg_db::audit::actions::SESSION_RESUME,
                            &AuditSource::remote(state.node_id()),
                            Some(session_id.as_str()),
                            serde_json::json!({ "force_replay": force_replay }),
                        )
                        .await;
                    NodeToServerMsg::ResumeResult {
                        command_id,
                        success: true,
                        code: None,
                        context_restored: Some(outcome.context_restored),
                        error: None,
                    }
                }
                Err(err) => NodeToServerMsg::ResumeResult {
                    command_id,
                    success: false,
                    code: Some(err.error_code()),
                    context_restored: None,
                    error: Some(err.to_string()),
                },
            };
            fxg_protocol::discard_with!(
                out.send(message).await,
                "send a message to a channel that may be closed"
            );
        }
        ServerToNodeMsg::ArchiveSession {
            command_id,
            session_id,
            archived,
        } => {
            let outcome = state
                .set_session_archived(
                    &command_id,
                    session_id.as_str(),
                    archived,
                    &AuditSource::remote(state.node_id()),
                )
                .await;
            let message = match outcome {
                Ok(archived_at) => NodeToServerMsg::ArchiveResult {
                    command_id,
                    success: true,
                    code: None,
                    archived_at,
                    error: None,
                },
                Err(err) => NodeToServerMsg::ArchiveResult {
                    command_id,
                    success: false,
                    code: Some(err.error_code()),
                    archived_at: None,
                    error: Some(err.to_string()),
                },
            };
            fxg_protocol::discard_with!(
                out.send(message).await,
                "send a message to a channel that may be closed"
            );
        }
        ServerToNodeMsg::DeleteSession {
            command_id,
            session_id,
        } => {
            let outcome = state
                .purge_session(
                    &command_id,
                    session_id.as_str(),
                    &AuditSource::remote(state.node_id()),
                )
                .await;
            let message = match outcome {
                Ok(()) => NodeToServerMsg::DeleteResult {
                    command_id,
                    success: true,
                    code: None,
                    error: None,
                },
                Err(err) => NodeToServerMsg::DeleteResult {
                    command_id,
                    success: false,
                    code: Some(err.error_code()),
                    error: Some(err.to_string()),
                },
            };
            fxg_protocol::discard_with!(
                out.send(message).await,
                "send a message to a channel that may be closed"
            );
        }
        ServerToNodeMsg::PtySpawn {
            pty_id,
            session_id,
            cols,
            rows,
            shell_cmd,
        } => {
            handle_remote_pty_spawn(
                state,
                out,
                pty_id,
                session_id.to_string(),
                cols,
                rows,
                shell_cmd,
            )
            .await;
        }
        ServerToNodeMsg::PtyInput { pty_id, data_b64 } => {
            match BASE64.decode(data_b64.as_bytes()) {
                Ok(data) => {
                    if let Err(err) = state.pty().write(&pty_id, &data) {
                        tracing::debug!(pty_id, "remote pty write failed: {err}");
                    }
                }
                Err(err) => {
                    tracing::warn!(pty_id, "invalid remote pty input: {err}");
                }
            }
        }
        ServerToNodeMsg::PtyResize { pty_id, cols, rows } => {
            if let Err(err) = state.pty().resize(&pty_id, cols, rows) {
                tracing::debug!(pty_id, "remote pty resize failed: {err}");
            }
        }
        ServerToNodeMsg::PtyKill { pty_id } => {
            if let Err(err) = state.pty().kill(&pty_id) {
                tracing::debug!(pty_id, "remote pty kill failed: {err}");
            }
        }
        ServerToNodeMsg::GetGitDiff {
            request_id,
            session_id,
            scope,
            base_branch,
        } => {
            let result = match state.db().get_session(session_id.as_str()).await {
                Ok(Some(session)) => {
                    crate::git::workspace_diff(
                        Path::new(&session.local_path),
                        scope,
                        base_branch.as_deref(),
                    )
                    .await
                }
                Ok(None) => Err(NodeError::InvalidSession(session_id.to_string())),
                Err(err) => Err(err.into()),
            };
            let response = match result {
                Ok(diff) => NodeToServerMsg::GitDiffResult {
                    request_id,
                    diff: Some(diff),
                    error: None,
                },
                Err(err) => NodeToServerMsg::GitDiffResult {
                    request_id,
                    diff: None,
                    error: Some(err.to_string()),
                },
            };
            fxg_protocol::discard_with!(
                out.send(response).await,
                "send a message to a channel that may be closed"
            );
        }
        ServerToNodeMsg::ManageWorktree {
            command_id,
            project_id,
            action,
        } => {
            let source = AuditSource::remote(state.node_id());
            let response = match action {
                WorktreeAction::Add {
                    branch,
                    base_branch,
                    new_path,
                } => match state.project_main_repo(project_id.as_str()).await {
                    Ok(repo) => {
                        match state
                            .worktree_add(
                                &repo,
                                Some(project_id.as_str()),
                                &branch,
                                base_branch,
                                new_path.map(std::path::PathBuf::from),
                                &source,
                            )
                            .await
                        {
                            Ok(outcome) => {
                                let head_commit =
                                    crate::git::head_commit(&outcome.path).await.unwrap_or(None);
                                NodeToServerMsg::WorktreeResult {
                                    command_id,
                                    worktree: Some(WorktreeInfo {
                                        node_id: state.node_id().into(),
                                        path: outcome.path.to_string_lossy().into_owned(),
                                        branch: Some(outcome.branch),
                                        head_commit,
                                        is_main: false,
                                    }),
                                    error: None,
                                }
                            }
                            Err(err) => NodeToServerMsg::WorktreeResult {
                                command_id,
                                worktree: None,
                                error: Some(err.to_string()),
                            },
                        }
                    }
                    Err(err) => NodeToServerMsg::WorktreeResult {
                        command_id,
                        worktree: None,
                        error: Some(err.to_string()),
                    },
                },
                WorktreeAction::Remove { path, force } => {
                    match state
                        .worktree_remove(None, Path::new(&path), force, &source)
                        .await
                    {
                        Ok(()) => NodeToServerMsg::WorktreeResult {
                            command_id,
                            worktree: None,
                            error: None,
                        },
                        Err(err) => NodeToServerMsg::WorktreeResult {
                            command_id,
                            worktree: None,
                            error: Some(err.to_string()),
                        },
                    }
                }
                WorktreeAction::Prune => match state.project_main_repo(project_id.as_str()).await {
                    Ok(repo) => match state.worktree_prune(&repo, &source).await {
                        Ok(_) => NodeToServerMsg::WorktreeResult {
                            command_id,
                            worktree: None,
                            error: None,
                        },
                        Err(err) => NodeToServerMsg::WorktreeResult {
                            command_id,
                            worktree: None,
                            error: Some(err.to_string()),
                        },
                    },
                    Err(err) => NodeToServerMsg::WorktreeResult {
                        command_id,
                        worktree: None,
                        error: Some(err.to_string()),
                    },
                },
            };
            // Report the worktree change to the hub (update of project_node_bindings)
            state.trigger_node_hello();
            fxg_protocol::discard_with!(
                out.send(response).await,
                "send a message to a channel that may be closed"
            );
        }
        ServerToNodeMsg::ProjectScan { request_id, dir } => {
            let result: Result<fxg_protocol::client_api::ProjectScanResponse, NodeError> =
                match state.project_scan(dir.as_deref().map(Path::new)).await {
                    Ok(scanned_dirs) => {
                        state.list_projects_with_path_state().await.map(|projects| {
                            fxg_protocol::client_api::ProjectScanResponse {
                                scanned_dirs,
                                projects,
                            }
                        })
                    }
                    Err(err) => Err(err),
                };
            let response = match result {
                Ok(response) => {
                    state
                        .record_audit(
                            fxg_db::audit::actions::PROJECT_LINK,
                            &AuditSource::remote(state.node_id()),
                            None,
                            serde_json::json!({
                                "action": "scan",
                                "scanned_dirs": &response.scanned_dirs,
                                "projects": response.projects.len(),
                            }),
                        )
                        .await;
                    NodeToServerMsg::ProjectScanResult {
                        request_id,
                        response: Some(response),
                        error: None,
                    }
                }
                Err(err) => NodeToServerMsg::ProjectScanResult {
                    request_id,
                    response: None,
                    error: Some(err.to_string()),
                },
            };
            fxg_protocol::discard_with!(
                out.send(response).await,
                "send a message to a channel that may be closed"
            );
        }
        ServerToNodeMsg::ProjectLink {
            request_id,
            project_id,
            local_path,
        } => {
            let dir = PathBuf::from(&local_path);
            let result = if dir.is_dir() {
                state.project_link(&dir, project_id.as_str()).await
            } else {
                Err(NodeError::InvalidState(format!(
                    "directory not found: {local_path}"
                )))
            };
            let response = match result {
                Ok(resolved) => {
                    state
                        .record_audit(
                            fxg_db::audit::actions::PROJECT_LINK,
                            &AuditSource::remote(state.node_id()),
                            None,
                            serde_json::json!({
                                "action": "link",
                                "project_id": &resolved.project_id,
                                "local_path": resolved.local_path.to_string_lossy(),
                            }),
                        )
                        .await;
                    NodeToServerMsg::ProjectLinkResult {
                        request_id,
                        response: Some(fxg_protocol::client_api::ProjectLinkResponse {
                            project_id: resolved.project_id.into(),
                            local_path: resolved.local_path.to_string_lossy().into_owned(),
                        }),
                        error: None,
                    }
                }
                Err(err) => NodeToServerMsg::ProjectLinkResult {
                    request_id,
                    response: None,
                    error: Some(err.to_string()),
                },
            };
            fxg_protocol::discard_with!(
                out.send(response).await,
                "send a message to a channel that may be closed"
            );
        }
        ServerToNodeMsg::ListAgents { request_id } => {
            let response = match state.agents_catalog().await {
                Ok(catalog) => NodeToServerMsg::AgentsResult {
                    request_id,
                    response: Some(catalog),
                    error: None,
                },
                Err(err) => NodeToServerMsg::AgentsResult {
                    request_id,
                    response: None,
                    error: Some(err.to_string()),
                },
            };
            fxg_protocol::discard_with!(
                out.send(response).await,
                "send a message to a channel that may be closed"
            );
        }
        ServerToNodeMsg::ManageAgent { request_id, action } => {
            let response = match state.manage_agent(&action).await {
                Ok(message) => {
                    state
                        .record_audit(
                            fxg_db::audit::actions::AGENT_MANAGE,
                            &AuditSource::remote(state.node_id()),
                            None,
                            serde_json::json!({ "action": action, "message": &message }),
                        )
                        .await;
                    // Report the change of installation state to the hub
                    state.trigger_node_hello();
                    NodeToServerMsg::AgentOpResult {
                        request_id,
                        success: true,
                        code: None,
                        message: Some(message),
                        error: None,
                    }
                }
                Err(err) => NodeToServerMsg::AgentOpResult {
                    request_id,
                    success: false,
                    code: Some(err.error_code()),
                    message: None,
                    error: Some(err.to_string()),
                },
            };
            fxg_protocol::discard_with!(
                out.send(response).await,
                "send a message to a channel that may be closed"
            );
        }
        ServerToNodeMsg::BrowseFs { request_id, path } => {
            let result = crate::fs_browse::browse_fs(path.as_deref());
            let response = match result {
                Ok(browse_resp) => NodeToServerMsg::BrowseFsResult {
                    request_id,
                    response: Some(browse_resp),
                    error: None,
                },
                Err(err) => NodeToServerMsg::BrowseFsResult {
                    request_id,
                    response: None,
                    error: Some(err.to_string()),
                },
            };
            fxg_protocol::discard_with!(
                out.send(response).await,
                "send a message to a channel that may be closed"
            );
        }
        ServerToNodeMsg::GitCredentialResponse {
            request_id,
            username,
            token,
            error,
        } => {
            state.credentials().complete(
                &request_id,
                crate::credentials::CredentialResponse {
                    username,
                    token,
                    error,
                },
            );
        }
        ServerToNodeMsg::DrainAndShutdown {
            reason,
            create_git_bundle,
        } => {
            drain_and_shutdown(state, out, ctx, &reason, create_git_bundle).await;
        }
        ServerToNodeMsg::KillAllSessions { reason } => match state.kill_all_local(&reason).await {
            Ok((killed_sessions, killed_ptys)) => {
                state
                    .record_audit(
                        fxg_db::audit::actions::KILL_SWITCH,
                        &AuditSource::remote(state.node_id()),
                        None,
                        serde_json::json!({
                            "reason": reason,
                            "killed_sessions": killed_sessions,
                            "killed_ptys": killed_ptys,
                        }),
                    )
                    .await;
            }
            Err(err) => {
                tracing::warn!("kill-all via central server failed: {err}");
            }
        },
    }
}

/// Handles a remote PTY spawn request (applies the `allow_remote_pty` policy).
async fn handle_remote_pty_spawn(
    state: &DaemonState,
    out: &mpsc::Sender<NodeToServerMsg>,
    pty_id: String,
    session_id: String,
    cols: u16,
    rows: u16,
    shell_cmd: Option<String>,
) {
    if !state.config().allow_remote_pty {
        tracing::warn!(
            node_id = state.node_id(),
            "remote pty spawn rejected by policy"
        );
        fxg_protocol::discard_with!(
            out.send(NodeToServerMsg::PtyError {
                pty_id,
                code: "PTY_DISABLED".to_owned(),
                message: "Remote PTY is disabled on this node by security policy".to_owned(),
            })
            .await,
            "send a message to the central server"
        );
        return;
    }

    let session = match state.db().get_session(session_id.as_str()).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            fxg_protocol::discard_with!(
                out.send(NodeToServerMsg::PtyError {
                    pty_id,
                    code: "NOT_FOUND".to_owned(),
                    message: format!("session not found: {session_id}"),
                })
                .await,
                "send a message to the central server"
            );
            return;
        }
        Err(err) => {
            fxg_protocol::discard_with!(
                out.send(NodeToServerMsg::PtyError {
                    pty_id,
                    code: "INTERNAL".to_owned(),
                    message: err.to_string(),
                })
                .await,
                "send a message to the central server"
            );
            return;
        }
    };

    let mut request = fxg_pty::PtySpawnRequest::new(pty_id.clone());
    request.session_id = Some(session_id.clone());
    request.cwd = Some(std::path::PathBuf::from(&session.local_path));
    request.shell_cmd = shell_cmd;
    request.cols = cols;
    request.rows = rows;
    if let Err(err) = state.pty().spawn(request) {
        fxg_protocol::discard_with!(
            out.send(NodeToServerMsg::PtyError {
                pty_id,
                code: "INTERNAL".to_owned(),
                message: err.to_string(),
            })
            .await,
            "send a message to the central server"
        );
        return;
    }

    state
        .record_audit(
            fxg_db::audit::actions::PTY_SPAWN,
            &AuditSource::remote(state.node_id()),
            Some(session_id.as_str()),
            serde_json::json!({ "pty_id": pty_id }),
        )
        .await;

    // Forward PTY output to the server
    let events = match state.pty().subscribe(&pty_id) {
        Ok(events) => events,
        Err(err) => {
            fxg_protocol::discard_with!(
                out.send(NodeToServerMsg::PtyError {
                    pty_id,
                    code: "INTERNAL".to_owned(),
                    message: err.to_string(),
                })
                .await,
                "send a message to the central server"
            );
            return;
        }
    };
    let out = out.clone();
    fxg_protocol::spawn_guarded!("pty event forwarder", async move {
        let mut events = events;
        loop {
            match events.recv().await {
                Ok(fxg_pty::PtyEvent::Output { data, .. }) => {
                    let message = NodeToServerMsg::PtyOutput {
                        pty_id: pty_id.clone(),
                        data_b64: BASE64.encode(data),
                    };
                    if out.send(message).await.is_err() {
                        break;
                    }
                }
                Ok(fxg_pty::PtyEvent::Exit { exit_code, .. }) => {
                    fxg_protocol::discard_with!(
                        out.send(NodeToServerMsg::PtyExit { pty_id, exit_code })
                            .await,
                        "send a message to the central server"
                    );
                    break;
                }
                // Dropped output is acceptable (prefer showing the latest)
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(skipped, "remote pty forwarder lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Handles `DrainAndShutdown` (best effort).
///
/// 1. Flush all unsent events
/// 2. If this is an ephemeral VM (`ctx.ephemeral`), create a `git bundle` and upload it
///    in chunks
/// 3. Send `DrainComplete` and request process shutdown
///
/// Losing intermediate events when the process crashes before drain is acceptable by
/// design (design: docs/01 §6.4).
async fn drain_and_shutdown(
    state: &DaemonState,
    out: &mpsc::Sender<NodeToServerMsg>,
    ctx: &CommandContext<'_>,
    reason: &str,
    create_git_bundle: bool,
) {
    tracing::info!(reason, create_git_bundle, "drain and shutdown requested");

    // 1. Stop sessions / PTYs (records StatusChanged(Stopped))
    if ctx.ephemeral
        && let Err(err) = state.kill_all_local(&format!("drain: {reason}")).await
    {
        tracing::warn!("failed to stop sessions on drain: {err}");
    }

    // 2. Flush all unsent events
    if let Err(err) = push_outbox(state, out).await {
        tracing::warn!("failed to flush outbox on drain: {err}");
    }

    if create_git_bundle
        && ctx.ephemeral
        && let Some(workspace) = ctx.workspace
    {
        match upload_workspace_bundle(state, out, workspace).await {
            Ok(Some(session_id)) => {
                tracing::info!(session_id, "workspace bundle uploaded");
            }
            Ok(None) => {
                tracing::info!("no session to attach workspace bundle to; skipped");
            }
            Err(err) => {
                tracing::warn!("failed to create workspace bundle: {err}");
            }
        }
    }

    fxg_protocol::discard_with!(
        out.send(NodeToServerMsg::DrainComplete {
            node_id: state.node_id().into(),
        })
        .await,
        "send a message to the central server"
    );

    if let Some(shutdown) = ctx.shutdown {
        fxg_protocol::discard_with!(shutdown.send(true), "request shutdown");
    } else {
        tracing::warn!("drain requested but no shutdown channel is attached");
    }
}

/// Generates the workspace Git bundle and uploads it in chunks as
/// `WorkspaceBundleUpload`.
///
/// Returns `Ok(None)` if there are no sessions (no backup needed).
async fn upload_workspace_bundle(
    state: &DaemonState,
    out: &mpsc::Sender<NodeToServerMsg>,
    workspace: &Path,
) -> Result<Option<String>, NodeError> {
    let sessions = state
        .db()
        .list_sessions(&SessionFilter {
            limit: Some(1),
            ..SessionFilter::default()
        })
        .await?;
    let Some(session) = sessions.first() else {
        return Ok(None);
    };

    let index_path = state
        .paths()
        .snapshot_index_path(session.session_id.as_str());
    let bundle =
        crate::bundle::create_workspace_bundle(workspace, &index_path, session.session_id.as_str())
            .await?;

    let encoded = BASE64.encode(&bundle.bytes);
    let chunks: Vec<&str> = encoded
        .as_bytes()
        .chunks(BUNDLE_CHUNK_B64_BYTES)
        .filter_map(|chunk| match std::str::from_utf8(chunk) {
            Ok(chunk) => Some(chunk),
            Err(err) => {
                tracing::error!("base64 chunk is not valid UTF-8: {err}");
                None
            }
        })
        .collect();
    for chunk in chunks {
        let message = NodeToServerMsg::WorkspaceBundleUpload {
            session_id: session.session_id.clone(),
            branch: bundle.branch.clone().unwrap_or_default(),
            head_commit: bundle.head_commit.clone().unwrap_or_default(),
            bundle_b64: chunk.to_owned(),
        };
        if out.send(message).await.is_err() {
            return Err(NodeError::Server(
                "bundle sink closed before upload completed".to_owned(),
            ));
        }
    }
    Ok(Some(session.session_id.to_string()))
}

/// Builds and sends a `CommandResult`.
async fn send_command_result(
    out: &mpsc::Sender<NodeToServerMsg>,
    command_id: CommandId,
    result: Result<fxg_protocol::SessionId, NodeError>,
) {
    let result = match result {
        Ok(session_id) => fxg_protocol::common::CommandResult::success(command_id, session_id),
        Err(err) => fxg_protocol::common::CommandResult::failure(command_id, err.info()),
    };
    let message = NodeToServerMsg::CommandResult {
        command_id: result.command_id,
        success: result.success,
        error: result.error,
        session_id: result.session_id,
        warnings: result.warnings,
        data_safety: result.data_safety,
    };
    fxg_protocol::discard_with!(
        out.send(message).await,
        "send a message to a channel that may be closed"
    );
}
