//! Outbox sync worker (Node <-> Central Server).
//!
//! Design: `docs/01-architecture-and-sync.md` §2.2, `docs/03-protocol-and-api.md` §2.
//!
//! - **Outbound WebSocket**: connects to the hub's node hub
//!   (`/api/v1/node/ws`) with `Authorization: Bearer <NODE_TOKEN>` and reconnects with
//!   exponential backoff on disconnect
//! - **Store-and-Forward**: `NodeHello` (session sync state) -> the hub sends
//!   `ResyncRequest` when needed -> events after `synced_up_to_node_seq` are sent in
//!   batches with `EventBatchPush`, and `EventBatchAck` updates the watermark
//! - **Real-time delivery**: completed events are pushed immediately with
//!   `EventBatchPush`, streaming deltas are delivered as `LiveStreamDelta` (persistence
//!   happens only when the turn completes)
//! - **Remote command execution**: executes `StartSession` / `SendPrompt` /
//!   `RespondPermission` / `ControlSession` / `Pty*` / `KillAllSessions` directly
//!   and forwards generic operations to [`crate::rpc`] through `NodeRpc` /
//!   `NodeRpcResult`, correlated by `request_id`

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use fxg_db::SessionFilter;
use fxg_protocol::CommandId;
use fxg_protocol::client_api::{HubLinkState, HubLinkStatus};
use fxg_protocol::node_hub::{HubToNodeMsg, NodeToHubMsg, SessionSyncState};
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::daemon::{AuditSource, DaemonState, VERSION};
use crate::error::NodeError;
use crate::session::SessionBroadcast;
use crate::session_manager::StartSessionParams;

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

/// Processing context for commands from the hub (absorbs transport differences).
///
/// A resident node (WS) uses the default values for all fields; an ephemeral VM
/// (`fxg internal stdio`) passes the workspace override and the process shutdown signal used on
/// drain.
#[derive(Default, Clone, Copy)]
pub(crate) struct CommandContext<'a> {
    /// Override directory of `fxg internal stdio --workspace`
    /// (pins `StartSession.local_path` to a fixed clone destination)
    pub workspace: Option<&'a Path>,
    /// Whether this is ephemeral VM mode (stop sessions and flush on drain)
    pub ephemeral: bool,
    /// Signal that requests process shutdown when `DrainAndShutdown` is received
    pub shutdown: Option<&'a watch::Sender<bool>>,
}

/// Connection target of the outbox sync worker (owned by the pairing settings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubTarget {
    /// Node Hub WebSocket URL (derived from the paired HTTP origin)
    pub ws_url: String,
    /// Pairing token (never logged)
    pub token: String,
}

/// Supervises the Outbox sync worker.
///
/// Reconnects whenever the pairing changes (`HubTarget` watch channel) and stays
/// idle while the node is unpaired, so pairing takes effect without a restart.
pub async fn supervise(state: DaemonState, mut shutdown: watch::Receiver<bool>) {
    let mut targets = state.hub_target_updates();
    loop {
        if *shutdown.borrow() {
            break;
        }

        let current = targets.borrow().clone();
        match current {
            Some(target) => run(&state, &target, &mut targets, &mut shutdown).await,
            None => {
                tokio::select! {
                    result = targets.changed() => {
                        if result.is_err() {
                            break;
                        }
                    }
                    _ = shutdown.changed() => break,
                }
            }
        }
    }
    tracing::info!("outbox sync worker stopped");
}

/// Runs one connection target until the pairing changes or the node shuts down.
async fn run(
    state: &DaemonState,
    target: &HubTarget,
    targets: &mut watch::Receiver<Option<HubTarget>>,
    shutdown: &mut watch::Receiver<bool>,
) {
    let hub_url = Some(fxg_protocol::config::hub_http_origin(&target.ws_url));
    let mut backoff = RECONNECT_MIN;
    loop {
        if *shutdown.borrow() || targets.borrow().as_ref() != Some(target) {
            break;
        }

        state.set_hub_link(HubLinkStatus::new(
            HubLinkState::Connecting,
            hub_url.clone(),
            None,
        ));
        match connect_once(state, target, targets, shutdown).await {
            Ok(()) => {
                backoff = RECONNECT_MIN;
                // The pairing may have changed (unpair / new hub): the outer loop
                // owns the `Standalone` state, so do not overwrite it.
                if targets.borrow().as_ref() != Some(target) {
                    break;
                }
                // The connection dropped cleanly; the next attempt follows after the backoff.
                state.set_hub_link(HubLinkStatus::new(
                    HubLinkState::Connecting,
                    hub_url.clone(),
                    None,
                ));
            }
            Err(err) => {
                state
                    .diagnostics_counters()
                    .incr(fxg_hub::api::CounterKind::OutboxRetries);
                let (link_state, message) = classify_failure(&err);
                tracing::warn!(url = %target.ws_url, "hub connection failed: {message}");
                state.set_hub_link(HubLinkStatus::new(
                    link_state,
                    hub_url.clone(),
                    Some(message),
                ));
            }
        }
        // The connection is gone: hub clients can no longer be attached to the
        // terminals opened through it. Detach them so the idle timeout can reap
        // the shells.
        state.detach_hub_ptys();
        if *shutdown.borrow() {
            break;
        }

        // Exponential backoff (interrupted by a shutdown request or a pairing change)
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown.changed() => break,
            result = targets.changed() => {
                if result.is_err() {
                    break;
                }
            }
        }
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

/// Tries a WebSocket handshake without starting the sync worker (`pairing/test`).
pub async fn test_connection(ws_url: &str, token: &str) -> HubLinkStatus {
    let hub_url = Some(fxg_protocol::config::hub_http_origin(ws_url));
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        probe_connection(ws_url, token),
    )
    .await
    {
        Ok(Ok(())) => HubLinkStatus::new(HubLinkState::Connected, hub_url, None),
        Ok(Err(err)) => {
            let (state, message) = classify_failure(&err);
            HubLinkStatus::new(state, hub_url, Some(message))
        }
        Err(_) => HubLinkStatus::new(
            HubLinkState::Failed,
            hub_url,
            Some("timed out while connecting to the hub".to_owned()),
        ),
    }
}

/// One-shot WebSocket handshake used by [`test_connection`].
async fn probe_connection(ws_url: &str, token: &str) -> anyhow::Result<()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut request = ws_url.into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {token}").parse()?);
    let (mut socket, _response) = tokio_tungstenite::connect_async(request).await?;
    fxg_protocol::discard_with!(socket.close(None).await, "close the pairing probe socket");
    Ok(())
}

/// Classifies a failed connection attempt for the UI.
///
/// The hub rejects an unknown or revoked pairing token during the WebSocket
/// handshake with HTTP 401 / 403, which is reported separately from a node that
/// simply cannot reach the hub.
pub fn classify_failure(err: &anyhow::Error) -> (HubLinkState, String) {
    if let Some(tokio_tungstenite::tungstenite::Error::Http(response)) =
        err.downcast_ref::<tokio_tungstenite::tungstenite::Error>()
    {
        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            return (
                HubLinkState::Unauthorized,
                format!("the hub rejected the pairing token (HTTP {status})"),
            );
        }
    }
    (HubLinkState::Failed, format!("{err:#}"))
}

/// Performs one connection's worth of sync with the hub (does not return until the
/// connection drops).
async fn connect_once(
    state: &DaemonState,
    target: &HubTarget,
    targets: &mut watch::Receiver<Option<HubTarget>>,
    shutdown: &mut watch::Receiver<bool>,
) -> anyhow::Result<()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut request = target.ws_url.as_str().into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {}", target.token).parse()?);
    let (socket, _response) = tokio_tungstenite::connect_async(request).await?;
    let (mut sender, mut receiver) = socket.split();

    state.set_hub_link(HubLinkStatus::new(
        HubLinkState::Connected,
        Some(fxg_protocol::config::hub_http_origin(&target.ws_url)),
        None,
    ));
    tracing::info!(url = %target.ws_url, "connected to hub");

    // Send queue (shared path for the PTY forwarder / command responses)
    let (out_tx, mut out_rx) = mpsc::channel::<NodeToHubMsg>(SEND_QUEUE_CAPACITY);

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
            _ = targets.changed() => {
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
                        &NodeToHubMsg::EventBatchPush {
                            events: vec![event],
                            resync: false,
                        },
                    )
                    .await?;
                }
                Ok(SessionBroadcast::StreamDelta { session_id, delta }) => {
                    send_msg(
                        &mut sender,
                        &NodeToHubMsg::LiveStreamDelta { session_id, delta },
                    )
                    .await?;
                }
                // Ephemeral events (keystrokes and so on) are not synced
                Ok(SessionBroadcast::Ephemeral(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    state
                        .diagnostics_counters()
                        .add(fxg_hub::api::CounterKind::DroppedEvents, skipped);
                    state
                        .diagnostics_counters()
                        .incr(fxg_hub::api::CounterKind::LaggedBroadcasts);
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
                    tracing::warn!(url = %target.ws_url, "hub websocket error: {err}");
                    return Err(err.into());
                }
                _ => {}
            },
        }
    }
}

/// Builds `NodeHello` (node info, projects, and session sync state).
pub(crate) async fn build_hello(state: &DaemonState) -> NodeToHubMsg {
    // `projects_complete = false` prevents the hub from pruning bindings when the
    // enumeration failed halfway (an empty list must not mean "delete everything").
    let (projects, projects_complete) = match state.list_projects_with_path_state().await {
        Ok(projects) => (
            projects
                .into_iter()
                .map(|project| fxg_protocol::common::NodeProjectReport {
                    project_id: project.project_id.clone(),
                    name: project.name,
                    canonical_git_url: project.canonical_git_url,
                    identity: project.identity,
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
            true,
        ),
        Err(err) => {
            tracing::warn!("failed to list projects for NodeHello: {err}");
            (Vec::new(), false)
        }
    };

    let sessions = match state
        .db()
        .list_sessions(&SessionFilter {
            // Archived sessions are also synced (so the resync safety net covers them)
            include_archived: true,
            // Fetch one more row to tell "exactly at the limit" from "truncated".
            limit: Some(HELLO_SESSION_LIMIT.saturating_add(1)),
            ..SessionFilter::default()
        })
        .await
    {
        Ok(mut sessions) => {
            let limit = usize::try_from(HELLO_SESSION_LIMIT).unwrap_or(usize::MAX);
            state.set_sessions_truncated(sessions.len() > limit);
            sessions.truncate(limit);
            sessions
                .into_iter()
                .map(|session| SessionSyncState {
                    session_id: session.session_id.clone(),
                    project_id: session.project_id,
                    last_node_seq: session.last_node_seq,
                })
                .collect()
        }
        Err(err) => {
            tracing::warn!("failed to list sessions for NodeHello: {err}");
            Vec::new()
        }
    };

    NodeToHubMsg::NodeHello {
        node_id: state.node_id().into(),
        name: state.node_settings().name,
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        version: VERSION.to_owned(),
        protocol_version: fxg_protocol::node_hub::PROTOCOL_VERSION,
        is_ephemeral: state.config().ephemeral,
        installed_agents: state.installed_agents().await,
        projects,
        projects_complete,
        state_revision: state.state_revision().await.unwrap_or(0),
        sessions,
        sessions_truncated: state.sessions_truncated(),
    }
}

/// Turns unsent events (after `synced_up_to_node_seq`) into a sequence of outgoing
/// messages.
pub(crate) async fn outbox_messages(state: &DaemonState) -> anyhow::Result<Vec<NodeToHubMsg>> {
    let mut messages = Vec::new();
    for batch in state.db().extract_outbox().await? {
        for chunk in batch.events.chunks(OUTBOX_BATCH_EVENTS) {
            messages.push(NodeToHubMsg::EventBatchPush {
                events: chunk.to_vec(),
                resync: false,
            });
        }
    }
    Ok(messages)
}

/// Pushes unsent events into the send queue (for the stdio transport).
pub(crate) async fn push_outbox(
    state: &DaemonState,
    out: &mpsc::Sender<NodeToHubMsg>,
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
async fn send_msg<S>(sender: &mut S, message: &NodeToHubMsg) -> anyhow::Result<()>
where
    S: futures_util::Sink<WsMessage> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    let text = serde_json::to_string(message)?;
    sender.send(WsMessage::Text(text.into())).await?;
    Ok(())
}

/// Handles messages from the hub.
pub(crate) async fn handle_server_message(
    state: &DaemonState,
    text: &str,
    out: &mpsc::Sender<NodeToHubMsg>,
    ctx: &CommandContext<'_>,
) {
    let message: HubToNodeMsg = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(err) => {
            tracing::warn!("invalid server message: {err}");
            return;
        }
    };

    match message {
        HubToNodeMsg::EventBatchAck {
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
        HubToNodeMsg::ResyncRequest { sessions } => {
            for target in sessions {
                match state
                    .db()
                    .extract_outbox_after(target.session_id.as_str(), target.from_node_seq)
                    .await
                {
                    Ok(events) => {
                        for chunk in events.chunks(OUTBOX_BATCH_EVENTS) {
                            if out
                                .send(NodeToHubMsg::EventBatchPush {
                                    events: chunk.to_vec(),
                                    resync: true,
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
        HubToNodeMsg::StartSession {
            command_id,
            session_id,
            project_id: _,
            local_path,
            agent_id,
            initial_prompt,
            initial_config,
            mode,
            extra_args,
            fork_from_session_id,
            fork_up_to_node_seq,
        } => {
            // An ephemeral VM (`fxg internal stdio --workspace`) is pinned to the cloned directory
            // (the hub does not know the path inside the VM, so the requested
            // local_path is not used)
            let local_path = ctx
                .workspace
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(&local_path));

            // Bring the project binding of the working directory up to date
            // (an ephemeral VM already registered it at startup, but re-register on
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
                    initial_config: &initial_config,
                    mode: mode.as_deref(),
                    extra_args: extra_args.as_deref(),
                    fork_from_session_id: fork_from_session_id
                        .as_ref()
                        .map(fxg_protocol::SessionId::as_str),
                    fork_up_to_node_seq,
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
        HubToNodeMsg::SendPrompt {
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
        HubToNodeMsg::RespondPermission {
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
        HubToNodeMsg::RespondElicitation {
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
        HubToNodeMsg::ControlSession {
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
        HubToNodeMsg::NodeRpc {
            request_id,
            request,
        } => {
            let outcome = crate::rpc::execute(
                state,
                &request_id,
                request,
                &AuditSource::remote(state.node_id()),
            )
            .await;
            fxg_protocol::discard_with!(
                out.send(NodeToHubMsg::NodeRpcResult {
                    request_id,
                    outcome
                })
                .await,
                "send a message to a channel that may be closed"
            );
        }
        HubToNodeMsg::PtySpawn {
            request_id,
            pty_id,
            path,
            session_id,
            terminal_id,
            cols,
            rows,
            new,
        } => {
            handle_remote_pty_spawn(
                state,
                out,
                request_id,
                pty_id,
                path,
                session_id,
                terminal_id,
                cols,
                rows,
                new,
            )
            .await;
        }
        HubToNodeMsg::PtyAttach { pty_id } => {
            if let Err(err) = state.pty().attach(&pty_id) {
                tracing::debug!(pty_id, "remote pty attach failed: {err}");
                // Let the hub drop a stale relay route instead of leaving the
                // client subscribed to a terminal that no longer exists.
                fxg_protocol::discard_with!(
                    out.send(NodeToHubMsg::PtyError {
                        pty_id,
                        code: "NOT_FOUND".to_owned(),
                        message: err.to_string(),
                    })
                    .await,
                    "send a pty attach failure to the hub"
                );
            } else {
                state.track_hub_pty_attach(&pty_id);
            }
        }
        HubToNodeMsg::PtyDetach { pty_id } => {
            state.pty().detach(&pty_id);
            state.track_hub_pty_detach(&pty_id);
        }
        HubToNodeMsg::PtyInput { pty_id, data_b64 } => match BASE64.decode(data_b64.as_bytes()) {
            Ok(data) => {
                if let Err(err) = state.pty().write(&pty_id, &data) {
                    tracing::debug!(pty_id, "remote pty write failed: {err}");
                }
            }
            Err(err) => {
                tracing::warn!(pty_id, "invalid remote pty input: {err}");
            }
        },
        HubToNodeMsg::PtyResize { pty_id, cols, rows } => {
            if let Err(err) = state.pty().resize(&pty_id, cols, rows) {
                tracing::debug!(pty_id, "remote pty resize failed: {err}");
            }
        }
        HubToNodeMsg::PtyKill { pty_id } => match state.pty().kill(&pty_id) {
            Ok(()) => {
                state
                    .record_audit(
                        fxg_db::audit::actions::PTY_KILL,
                        &AuditSource::remote(state.node_id()),
                        None,
                        serde_json::json!({ "pty_id": pty_id }),
                    )
                    .await;
                state.forget_hub_pty(&pty_id);
            }
            Err(err) => tracing::debug!(pty_id, "remote pty kill failed: {err}"),
        },
        HubToNodeMsg::DrainAndShutdown { reason } => {
            drain_and_shutdown(state, out, ctx, &reason).await;
        }
        HubToNodeMsg::KillAllSessions { reason } => match state.kill_all_local(&reason).await {
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
                tracing::warn!("kill-all via hub failed: {err}");
            }
        },
    }
}

/// Handles a remote workspace-terminal open request (applies the
/// `allow_remote_pty` policy inside `DaemonState::pty_open`).
#[allow(clippy::too_many_arguments)]
async fn handle_remote_pty_spawn(
    state: &DaemonState,
    out: &mpsc::Sender<NodeToHubMsg>,
    request_id: CommandId,
    pty_id: String,
    path: String,
    session_id: Option<fxg_protocol::SessionId>,
    terminal_id: Option<String>,
    cols: u16,
    rows: u16,
    new: bool,
) {
    use fxg_hub::api::ClientApiBackend as _;

    let params = fxg_hub::api::PtyOpenParams {
        pty_id: pty_id.clone(),
        node_id: state.node_id().into(),
        path,
        session_id,
        terminal_id,
        cols,
        rows,
        new,
        remote: true,
    };
    let outcome = match state.pty_open(params).await {
        Ok(outcome) => outcome,
        Err(err) => {
            fxg_protocol::discard_with!(
                out.send(NodeToHubMsg::PtyOpenFailed {
                    request_id,
                    pty_id,
                    code: err.code,
                    message: err.message,
                })
                .await,
                "send a pty open failure to the hub"
            );
            return;
        }
    };
    state.track_hub_pty_attach(&outcome.pty_id);

    let (events, replay) = match state.pty().subscribe_with_replay(&outcome.pty_id) {
        Ok(subscription) => subscription,
        Err(err) => {
            state.pty().detach(&outcome.pty_id);
            state.track_hub_pty_detach(&outcome.pty_id);
            fxg_protocol::discard_with!(
                out.send(NodeToHubMsg::PtyOpenFailed {
                    request_id,
                    pty_id: outcome.pty_id,
                    code: "INTERNAL".to_owned(),
                    message: err.to_string(),
                })
                .await,
                "send a pty open failure to the hub"
            );
            return;
        }
    };

    // The retained output must reach the hub before any live chunk so it can
    // seed the replay log.
    if out
        .send(NodeToHubMsg::PtyOpened {
            request_id,
            pty_id: outcome.pty_id.clone(),
            path: outcome.path,
            cols: outcome.cols,
            rows: outcome.rows,
            attached: outcome.attached,
            snapshot_b64: BASE64.encode(&replay.data),
        })
        .await
        .is_err()
    {
        state.pty().detach(&outcome.pty_id);
        return;
    }

    // Forward PTY output to the hub (chunks already covered by the snapshot are
    // dropped).
    let pty_id = outcome.pty_id;
    let next_seq = replay.next_seq;
    let out = out.clone();
    let exit_state = state.clone();
    fxg_protocol::spawn_guarded!("pty event forwarder", async move {
        let mut events = events;
        loop {
            match events.recv().await {
                Ok(fxg_pty::PtyEvent::Output { seq, data, .. }) => {
                    if seq < next_seq {
                        continue;
                    }
                    let message = NodeToHubMsg::PtyOutput {
                        pty_id: pty_id.clone(),
                        data_b64: BASE64.encode(data),
                    };
                    if out.send(message).await.is_err() {
                        break;
                    }
                }
                Ok(fxg_pty::PtyEvent::Exit { exit_code, .. }) => {
                    exit_state.forget_hub_pty(&pty_id);
                    fxg_protocol::discard_with!(
                        out.send(NodeToHubMsg::PtyExit { pty_id, exit_code }).await,
                        "send a message to the hub"
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
/// 1. Flush all unsent events and stop sessions on an ephemeral VM
/// 2. Send `DrainComplete` and request process shutdown
///
/// Losing intermediate events when the process crashes before drain is acceptable by
/// design (design: docs/01 §6.4).
async fn drain_and_shutdown(
    state: &DaemonState,
    out: &mpsc::Sender<NodeToHubMsg>,
    ctx: &CommandContext<'_>,
    reason: &str,
) {
    tracing::info!(reason, "drain and shutdown requested");

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

    fxg_protocol::discard_with!(
        out.send(NodeToHubMsg::DrainComplete {
            node_id: state.node_id().into(),
        })
        .await,
        "send a message to the hub"
    );

    if let Some(shutdown) = ctx.shutdown {
        fxg_protocol::discard_with!(shutdown.send(true), "request shutdown");
    } else {
        tracing::warn!("drain requested but no shutdown channel is attached");
    }
}

/// Builds and sends a `CommandResult`.
async fn send_command_result(
    out: &mpsc::Sender<NodeToHubMsg>,
    command_id: CommandId,
    result: Result<fxg_protocol::SessionId, NodeError>,
) {
    let result = match result {
        Ok(session_id) => fxg_protocol::common::CommandResult::success(command_id, session_id),
        Err(err) => fxg_protocol::common::CommandResult::failure(command_id, err.info()),
    };
    let message = NodeToHubMsg::CommandResult {
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
