//! Node Hub (`/api/v1/node/ws`): manages outbound WebSocket connections from resident nodes.
//!
//! Design: `docs/01-architecture-and-sync.md` §2.2 and §3, `docs/03-protocol-and-api.md` §2.
//!
//! - **Authentication**: hash `Authorization: Bearer <NODE_TOKEN>`, compare it against
//!   `hub.db.nodes.token_hash`, then verify that `NodeHello.node_id` matches the node the
//!   token was issued for (§7.3)
//! - **Handshake**: `NodeHello` → the hub replies with `ResyncRequest` for sessions that are
//!   missing or behind, and the node resends from the given `from_node_seq` onward
//! - **Event ingestion**: apply `EventBatchPush` idempotently with `INSERT OR IGNORE` +
//!   projection updates (same Tx), then return `EventBatchAck` (watermark). Newly inserted
//!   events are delivered to Client WS subscribers
//! - **Command relay**: correlate by `command_id` and return the node's
//!   `CommandResult` / `NodeRpcResult` to the requesting client
//! - **PTY relay**: forward `PtyOutput` / `PtyExit` / `PtyError` to the client's PTY WS,
//!   keyed by `pty_id`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use fxg_db::Db;
use fxg_protocol::common::{CommandResult, ErrorCode};
use fxg_protocol::node_hub::{HubToNodeMsg, NodeToHubMsg, ResyncTarget};
use fxg_protocol::util::{PtyOutputLog, uuid_v7};
use fxg_protocol::{CommandId, NodeId};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, oneshot};

use crate::api::auth::{hash_token, token_matches};
use crate::api::{
    ApiError, ClientEvent, PtyChannelError, PtyChannelEvent, PtyOpenOutcome, PtyOpenParams,
    send_ws_json,
};
use crate::state::HubState;

/// Timeout for the handshake (`NodeHello`) after a node connects.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for session commands (prompt submission, permission responses, control, resume).
///
/// Sending to a stopped session and resuming start the agent on the node side, so a cold
/// start (e.g. PyInstaller extraction, ~25 s) is allowed.
const SESSION_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Outgoing queue capacity for a node connection.
const NODE_CHANNEL_CAPACITY: usize = 512;

/// Broadcast capacity for PTY output.
const PTY_CHANNEL_CAPACITY: usize = 1024;

/// Kind of response a relay is waiting for.
enum PendingResponse {
    /// Session command result (`CommandResult`)
    Command(CommandResult),
    /// Generic node RPC result (`NodeRpcResult`)
    Rpc(fxg_protocol::node_hub::NodeRpcOutcome),
    /// Workspace terminal open result (`PtyOpened` / `PtyOpenFailed`)
    PtyOpen(Result<PtyOpenedReply, PtyChannelError>),
    /// Failure such as a node disconnect
    Failed {
        /// Structured error code
        code: ErrorCode,
        /// Human-readable message
        message: String,
    },
}

/// Node reply to a workspace terminal open request.
struct PtyOpenedReply {
    pty_id: String,
    path: String,
    cols: u16,
    rows: u16,
    attached: bool,
    /// Retained output that seeds the hub replay log
    snapshot: Vec<u8>,
}

/// Entry for a pending relay.
struct PendingEntry {
    /// Node whose response is awaited
    node_id: NodeId,
    /// Destination for the response
    tx: oneshot::Sender<PendingResponse>,
}

/// One connected node.
#[derive(Clone)]
struct NodeConnection {
    /// Connection generation ID (invalidates cleanup of an old connection on reconnect)
    conn_id: String,
    /// Outgoing queue
    tx: mpsc::Sender<HubToNodeMsg>,
}

/// PTY relay destination.
struct PtyRoute {
    /// Owning node ID
    node_id: NodeId,
    /// Canonical workspace path
    path: String,
    /// Terminal tab identity inside the workspace (`None` = legacy single terminal)
    terminal_id: Option<String>,
    /// Number of columns at the last known size
    cols: u16,
    /// Number of rows at the last known size
    rows: u16,
    /// Broadcast delivered to clients
    tx: broadcast::Sender<PtyChannelEvent>,
    /// Retained output for attach-time replay
    log: Arc<Mutex<PtyOutputLog>>,
    /// Number of clients currently attached through this hub
    clients: usize,
}

struct HubInner {
    db: Db,
    client_events: broadcast::Sender<ClientEvent>,
    nodes: RwLock<HashMap<String, NodeConnection>>,
    pending: Mutex<HashMap<String, PendingEntry>>,
    ptys: Mutex<HashMap<String, PtyRoute>>,
    /// Serializes workspace opens so two clients cannot spawn two shells for the
    /// same path.
    pty_open_gate: Mutex<()>,
}

/// Node connection registry and command relay (referenced from the hub's shared state).
#[derive(Clone)]
pub struct NodeHub {
    inner: Arc<HubInner>,
}

impl NodeHub {
    /// Creates the hub.
    pub(crate) fn new(db: Db, client_events: broadcast::Sender<ClientEvent>) -> Self {
        Self {
            inner: Arc::new(HubInner {
                db,
                client_events,
                nodes: RwLock::new(HashMap::new()),
                pending: Mutex::new(HashMap::new()),
                ptys: Mutex::new(HashMap::new()),
                pty_open_gate: Mutex::new(()),
            }),
        }
    }

    /// Hub DB.
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// Broadcast for Client WS delivery.
    pub(crate) fn client_events(&self) -> &broadcast::Sender<ClientEvent> {
        &self.inner.client_events
    }

    /// Whether the node is connected.
    pub async fn is_online(&self, node_id: &NodeId) -> bool {
        self.inner.nodes.read().await.contains_key(node_id.as_str())
    }

    /// List of connected nodes.
    pub async fn online_nodes(&self) -> Vec<String> {
        self.inner.nodes.read().await.keys().cloned().collect()
    }

    /// Registers a temporary VM (`fxg internal stdio`) connection.
    ///
    /// The provisioner manager forwards `HubToNodeMsg` items arriving on the returned receiver
    /// to the child process's `stdin` (JSON Lines).
    pub(crate) async fn attach_ephemeral(
        &self,
        node_id: &NodeId,
    ) -> (mpsc::Receiver<HubToNodeMsg>, String) {
        let (tx, rx) = mpsc::channel(NODE_CHANNEL_CAPACITY);
        let conn_id = uuid_v7();
        self.register(node_id, &conn_id, tx).await;
        tracing::info!(
            node_id = node_id.as_str(),
            "ephemeral node attached (stdio)"
        );
        (rx, conn_id)
    }

    /// Detaches a temporary VM connection (when the child process exits).
    pub(crate) async fn detach_ephemeral(&self, node_id: &NodeId, exit_code: Option<i32>) {
        // The connection generation is unknown, so remove the registered connection and clean up
        let conn_id = {
            let nodes = self.inner.nodes.read().await;
            nodes
                .get(node_id.as_str())
                .map(|entry| entry.conn_id.clone())
        };
        if let Some(conn_id) = conn_id {
            self.disconnect(node_id, &conn_id).await;
        }
        if let Err(err) = self.db().set_node_online(node_id.as_str(), false).await {
            tracing::debug!(
                node_id = node_id.as_str(),
                "failed to mark ephemeral node offline: {err}"
            );
        }
        if let Err(err) = self
            .db()
            .set_node_lifecycle(
                node_id.as_str(),
                fxg_protocol::common::NodeLifecycleStatus::Terminated,
            )
            .await
        {
            tracing::debug!(
                node_id = node_id.as_str(),
                "failed to mark ephemeral node terminated: {err}"
            );
        }
        tracing::info!(
            node_id = node_id.as_str(),
            exit_code,
            "ephemeral node detached (stdio)"
        );
    }

    /// Removes temporary VM connections unconditionally (e.g. on server shutdown).
    pub(crate) async fn detach_all_ephemeral(&self, node_ids: &[String]) {
        for node_id in node_ids {
            self.detach_ephemeral(&NodeId::from(node_id.as_str()), None)
                .await;
        }
    }

    /// Registers a node connection (replacing any existing one).
    async fn register(&self, node_id: &NodeId, conn_id: &str, tx: mpsc::Sender<HubToNodeMsg>) {
        let mut nodes = self.inner.nodes.write().await;
        if nodes
            .insert(
                node_id.as_str().to_owned(),
                NodeConnection {
                    conn_id: conn_id.to_owned(),
                    tx,
                },
            )
            .is_some()
        {
            tracing::info!(
                node_id = node_id.as_str(),
                "node reconnected; replaced previous connection"
            );
        }
    }

    /// Detaches a node connection (only for the same generation). Fails in-flight relays and PTYs.
    async fn disconnect(&self, node_id: &NodeId, conn_id: &str) {
        {
            let mut nodes = self.inner.nodes.write().await;
            let is_current = nodes
                .get(node_id.as_str())
                .map(|entry| entry.conn_id == conn_id)
                .unwrap_or(false);
            if is_current {
                nodes.remove(node_id.as_str());
            }
        }

        // Return NODE_OFFLINE for in-flight relays
        let mut pending = self.inner.pending.lock().await;
        let failed: Vec<String> = pending
            .iter()
            .filter(|(_, entry)| entry.node_id.as_str() == node_id.as_str())
            .map(|(command_id, _)| command_id.clone())
            .collect();
        for command_id in failed {
            if let Some(entry) = pending.remove(&command_id)
                && entry
                    .tx
                    .send(PendingResponse::Failed {
                        code: ErrorCode::NodeOffline,
                        message: format!("node is offline: {node_id}"),
                    })
                    .is_err()
            {
                tracing::debug!(command_id, "pending command waiter was already gone");
            }
        }
        drop(pending);

        // Treat PTYs owned by this node as exited
        let mut ptys = self.inner.ptys.lock().await;
        let pty_ids: Vec<String> = ptys
            .iter()
            .filter(|(_, route)| route.node_id.as_str() == node_id.as_str())
            .map(|(pty_id, _)| pty_id.clone())
            .collect();
        for pty_id in pty_ids {
            if let Some(route) = ptys.remove(&pty_id) {
                fxg_protocol::discard_with!(
                    route.tx.send(PtyChannelEvent::Exit { exit_code: None }),
                    "notify pty subscribers that the node disconnected"
                );
            }
        }
    }

    /// Sends a message to a node. Returns `NODE_OFFLINE` when not connected.
    pub async fn send(&self, node_id: &NodeId, message: HubToNodeMsg) -> Result<(), ApiError> {
        let tx = {
            let nodes = self.inner.nodes.read().await;
            nodes.get(node_id.as_str()).map(|entry| entry.tx.clone())
        };
        let Some(tx) = tx else {
            return Err(offline_error(node_id));
        };
        match tx.send(message).await {
            Ok(()) => Ok(()),
            Err(_) => Err(offline_error(node_id)),
        }
    }

    /// Relays a command and waits for the `CommandResult` correlated by `command_id`.
    ///
    /// Prompt submission may include automatic resume of a stopped session, so wait with
    /// [`SESSION_COMMAND_TIMEOUT`].
    pub async fn command(
        &self,
        node_id: &NodeId,
        command_id: &CommandId,
        message: HubToNodeMsg,
    ) -> Result<CommandResult, ApiError> {
        let response = self
            .request(
                node_id,
                command_id.as_str(),
                message,
                SESSION_COMMAND_TIMEOUT,
            )
            .await?;
        match response {
            PendingResponse::Command(result) => Ok(result),
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Relays a generic node operation and waits for its typed result.
    ///
    /// The timeout comes from [`fxg_protocol::node_hub::NodeRpcRequest::timeout`].
    /// Node errors are passed through as [`ApiError::from_info`], so `ErrorCode`,
    /// recovery actions and the data-safety hint are preserved.
    pub async fn rpc(
        &self,
        node_id: &NodeId,
        request_id: &CommandId,
        request: fxg_protocol::node_hub::NodeRpcRequest,
    ) -> Result<fxg_protocol::node_hub::NodeRpcResponse, ApiError> {
        use fxg_protocol::node_hub::NodeRpcOutcome;

        let timeout = request.timeout();
        let response = self
            .request(
                node_id,
                request_id.as_str(),
                HubToNodeMsg::NodeRpc {
                    request_id: request_id.clone(),
                    request,
                },
                timeout,
            )
            .await?;
        match response {
            PendingResponse::Rpc(NodeRpcOutcome::Ok { response }) => Ok(response),
            PendingResponse::Rpc(NodeRpcOutcome::Err { error }) => Err(ApiError::from_info(error)),
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Common send-then-wait path.
    async fn request(
        &self,
        node_id: &NodeId,
        correlation_id: &str,
        message: HubToNodeMsg,
        timeout: Duration,
    ) -> Result<PendingResponse, ApiError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.inner.pending.lock().await;
            if pending.contains_key(correlation_id) {
                return Err(ApiError::from_code(
                    ErrorCode::CommandDuplicate,
                    format!("duplicate in-flight command id: {correlation_id}"),
                ));
            }
            pending.insert(
                correlation_id.to_owned(),
                PendingEntry {
                    node_id: node_id.to_owned(),
                    tx,
                },
            );
        }

        if let Err(err) = self.send(node_id, message).await {
            self.inner.pending.lock().await.remove(correlation_id);
            return Err(err);
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_canceled)) => Err(ApiError::node_offline(
                node_id.as_str(),
                format!("node is offline: {node_id}"),
            )),
            Err(_) => {
                self.inner.pending.lock().await.remove(correlation_id);
                Err(ApiError::internal(format!(
                    "timed out waiting for node response: {correlation_id}"
                )))
            }
        }
    }

    /// Resolves a pending relay response.
    async fn resolve(&self, correlation_id: &str, response: PendingResponse) {
        let entry = self.inner.pending.lock().await.remove(correlation_id);
        match entry {
            Some(entry) => {
                if entry.tx.send(response).is_err() {
                    tracing::debug!(correlation_id, "relay response waiter was already gone");
                }
            }
            None => {
                tracing::debug!(correlation_id, "late node response (no waiter)");
            }
        }
    }

    /// Delivers a message to all connected nodes and returns the IDs of the nodes it reached.
    pub async fn broadcast(&self, message: HubToNodeMsg) -> Vec<String> {
        let connections: Vec<(String, mpsc::Sender<HubToNodeMsg>)> = {
            let nodes = self.inner.nodes.read().await;
            nodes
                .iter()
                .map(|(node_id, entry)| (node_id.clone(), entry.tx.clone()))
                .collect()
        };
        let mut delivered = Vec::new();
        for (node_id, tx) in connections {
            if tx.send(message.clone()).await.is_ok() {
                delivered.push(node_id);
            }
        }
        delivered
    }

    // ------------------------------------------------------------------
    // PTY relay
    // ------------------------------------------------------------------

    /// Opens (or attaches to) the workspace terminal on its node.
    ///
    /// Opens are serialized per hub so concurrent clients of the same workspace
    /// reuse one shell. The requesting client is counted as attached.
    pub(crate) async fn pty_open(
        &self,
        params: PtyOpenParams,
    ) -> Result<PtyOpenOutcome, PtyChannelError> {
        if !self.is_online(&params.node_id).await {
            return Err(PtyChannelError::offline(format!(
                "node is offline: {}",
                params.node_id
            )));
        }

        let _gate = self.inner.pty_open_gate.lock().await;

        let existing = {
            let ptys = self.inner.ptys.lock().await;
            ptys.iter()
                .find(|(_, route)| {
                    route.node_id == params.node_id
                        && route.path == params.path
                        && route.terminal_id == params.terminal_id
                })
                .map(|(pty_id, route)| (pty_id.clone(), route.cols, route.rows))
        };
        if let Some((pty_id, cols, rows)) = existing {
            if !params.new {
                {
                    let mut ptys = self.inner.ptys.lock().await;
                    if let Some(route) = ptys.get_mut(&pty_id) {
                        route.clients = route.clients.saturating_add(1);
                    }
                }
                if let Err(err) = self
                    .send(
                        &params.node_id,
                        HubToNodeMsg::PtyAttach {
                            pty_id: pty_id.clone(),
                        },
                    )
                    .await
                {
                    let mut ptys = self.inner.ptys.lock().await;
                    if let Some(route) = ptys.get_mut(&pty_id) {
                        route.clients = route.clients.saturating_sub(1);
                    }
                    return Err(PtyChannelError::new(err.code.as_str(), err.message));
                }
                return Ok(PtyOpenOutcome {
                    pty_id,
                    path: params.path,
                    attached: true,
                    cols,
                    rows,
                });
            }
            // A new shell was requested: drop the old route (its clients see the
            // stream close) and spawn below.
            self.remove_pty(&pty_id).await;
        }

        let request_id = CommandId::from(uuid_v7());
        let response = self
            .request(
                &params.node_id,
                request_id.as_str(),
                HubToNodeMsg::PtySpawn {
                    request_id: request_id.clone(),
                    pty_id: params.pty_id,
                    path: params.path.clone(),
                    session_id: params.session_id,
                    terminal_id: params.terminal_id.clone(),
                    cols: params.cols,
                    rows: params.rows,
                    new: params.new,
                },
                Duration::from_secs(30),
            )
            .await
            .map_err(|err| PtyChannelError::new(err.code.as_str(), err.message))?;
        let reply = match response {
            PendingResponse::PtyOpen(Ok(reply)) => reply,
            PendingResponse::PtyOpen(Err(err)) => return Err(err),
            PendingResponse::Failed { code, message } => {
                return Err(PtyChannelError::new(code.as_str(), message));
            }
            _ => return Err(PtyChannelError::internal("unexpected node response kind")),
        };

        {
            let log = Arc::new(Mutex::new(PtyOutputLog::with_default_capacity()));
            if !reply.snapshot.is_empty() {
                log.lock().await.push(&reply.snapshot);
            }
            let mut ptys = self.inner.ptys.lock().await;
            if let Some(route) = ptys.get_mut(&reply.pty_id) {
                // The node reused a terminal we already track (the requested
                // path spelling differed). The node counted this client at
                // open, so only the hub-side count changes.
                route.clients = route.clients.saturating_add(1);
            } else {
                let (tx, _) = broadcast::channel(PTY_CHANNEL_CAPACITY);
                ptys.insert(
                    reply.pty_id.clone(),
                    PtyRoute {
                        node_id: params.node_id,
                        path: reply.path.clone(),
                        terminal_id: params.terminal_id,
                        cols: reply.cols,
                        rows: reply.rows,
                        tx,
                        log,
                        clients: 1,
                    },
                );
            }
        }

        Ok(PtyOpenOutcome {
            pty_id: reply.pty_id,
            path: reply.path,
            attached: reply.attached,
            cols: reply.cols,
            rows: reply.rows,
        })
    }

    /// Attaches an additional client to a known PTY.
    pub(crate) async fn pty_attach(&self, pty_id: &str) -> Result<PtyOpenOutcome, PtyChannelError> {
        let outcome = {
            let mut ptys = self.inner.ptys.lock().await;
            let Some(route) = ptys.get_mut(pty_id) else {
                return Err(PtyChannelError::not_found(format!(
                    "pty session not found: {pty_id}"
                )));
            };
            route.clients = route.clients.saturating_add(1);
            (
                route.node_id.clone(),
                route.path.clone(),
                route.cols,
                route.rows,
            )
        };
        let (node_id, path, cols, rows) = outcome;
        if let Err(err) = self
            .send(
                &node_id,
                HubToNodeMsg::PtyAttach {
                    pty_id: pty_id.to_owned(),
                },
            )
            .await
        {
            let mut ptys = self.inner.ptys.lock().await;
            if let Some(route) = ptys.get_mut(pty_id) {
                route.clients = route.clients.saturating_sub(1);
            }
            return Err(PtyChannelError::new(err.code.as_str(), err.message));
        }
        Ok(PtyOpenOutcome {
            pty_id: pty_id.to_owned(),
            path,
            attached: true,
            cols,
            rows,
        })
    }

    /// Detaches a client (the node keeps the shell until its idle timeout).
    pub(crate) async fn pty_detach(&self, pty_id: &str) {
        let node_id = {
            let mut ptys = self.inner.ptys.lock().await;
            let Some(route) = ptys.get_mut(pty_id) else {
                return;
            };
            route.clients = route.clients.saturating_sub(1);
            route.node_id.clone()
        };
        fxg_protocol::discard_with!(
            self.send(
                &node_id,
                HubToNodeMsg::PtyDetach {
                    pty_id: pty_id.to_owned(),
                },
            )
            .await,
            "notify the node that a pty client detached"
        );
    }

    /// Subscribes to PTY output with the retained output first.
    pub(crate) async fn pty_subscribe(
        &self,
        pty_id: &str,
    ) -> Option<broadcast::Receiver<PtyChannelEvent>> {
        let (live, data, next_seq) = {
            let ptys = self.inner.ptys.lock().await;
            let route = ptys.get(pty_id)?;
            let live = route.tx.subscribe();
            let (data, next_seq) = route.log.lock().await.snapshot();
            (live, data, next_seq)
        };
        Some(crate::api::subscribe_with_replay(live, data, next_seq))
    }

    /// Relays a message to the node that owns the PTY.
    pub(crate) async fn pty_send(
        &self,
        pty_id: &str,
        message: HubToNodeMsg,
    ) -> Result<(), PtyChannelError> {
        let node_id = {
            let ptys = self.inner.ptys.lock().await;
            ptys.get(pty_id).map(|route| route.node_id.clone())
        };
        let Some(node_id) = node_id else {
            return Err(PtyChannelError::not_found(format!(
                "pty session not found: {pty_id}"
            )));
        };
        self.send(&node_id, message)
            .await
            .map_err(|err| PtyChannelError::new(err.code.as_str(), err.message))
    }

    /// Removes a PTY relay registration (after kill / exit).
    pub(crate) async fn remove_pty(&self, pty_id: &str) {
        self.inner.ptys.lock().await.remove(pty_id);
    }

    /// Publishes an output chunk from a node (assigning its replay sequence).
    async fn publish_pty_output(&self, pty_id: &str, data: Vec<u8>) {
        let route = {
            let ptys = self.inner.ptys.lock().await;
            ptys.get(pty_id)
                .map(|route| (route.tx.clone(), Arc::clone(&route.log)))
        };
        if let Some((tx, log)) = route {
            let seq = log.lock().await.push(&data);
            fxg_protocol::discard_with!(
                tx.send(PtyChannelEvent::Output { data, seq }),
                "publish a pty output chunk to subscribers"
            );
        }
    }

    /// Publishes a PTY event from a node to its subscribers.
    async fn publish_pty(&self, pty_id: &str, event: PtyChannelEvent) {
        let route = {
            let ptys = self.inner.ptys.lock().await;
            ptys.get(pty_id).map(|route| route.tx.clone())
        };
        if let Some(tx) = route {
            fxg_protocol::discard_with!(tx.send(event), "publish a pty event to subscribers");
        }
    }
}

/// Builds a `NODE_OFFLINE` error.
fn offline_error(node_id: &NodeId) -> ApiError {
    ApiError::node_offline(node_id.as_str(), format!("node is offline: {node_id}"))
}

// ----------------------------------------------------------------------
// WebSocket handlers
// ----------------------------------------------------------------------

/// `/api/v1/node/ws` handler (node pairing token auth + WS upgrade).
pub(crate) async fn node_ws(
    State(state): State<HubState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(provided) = crate::api::header_str(&headers, header::AUTHORIZATION)
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
    else {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "missing node pairing token",
        )
        .into_response();
    };

    // Plaintext tokens are not stored, so hash the provided one and compare in constant time
    let provided_hash = hash_token(provided);
    let tokens = match state.db().list_node_tokens().await {
        Ok(tokens) => tokens,
        Err(err) => return ApiError::internal(err).into_response(),
    };
    let owner = tokens
        .into_iter()
        .find(|(_, stored)| token_matches(stored, &provided_hash))
        .map(|(node_id, _)| node_id);
    let Some(owner) = owner else {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "invalid node pairing token",
        )
        .into_response();
    };

    ws.on_upgrade(move |socket| handle_node_connection(state, owner, socket))
}

/// Main loop for a node connection (NodeHello → register → bidirectional messaging).
async fn handle_node_connection(state: HubState, token_owner: String, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();

    // 1. The first message must be NodeHello (with timeout)
    let hello = match tokio::time::timeout(HELLO_TIMEOUT, receiver.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<NodeToHubMsg>(&text) {
            Ok(hello @ NodeToHubMsg::NodeHello { .. }) => hello,
            Ok(other) => {
                tracing::warn!(op = ?std::mem::discriminant(&other), "node sent a non-hello first message");
                fxg_protocol::discard_with!(sender.close().await, "close the node websocket");
                return;
            }
            Err(err) => {
                tracing::warn!("invalid node hello: {err}");
                fxg_protocol::discard_with!(sender.close().await, "close the node websocket");
                return;
            }
        },
        _ => {
            tracing::warn!("node hello timeout or connection closed");
            return;
        }
    };

    let NodeToHubMsg::NodeHello {
        node_id,
        name,
        os,
        arch,
        version,
        protocol_version,
        is_ephemeral,
        installed_agents,
        projects,
        projects_complete,
        state_revision,
        sessions,
        sessions_truncated,
    } = hello
    else {
        unreachable!("hello was matched above");
    };

    if sessions_truncated {
        tracing::warn!(
            node_id = node_id.as_str(),
            reported = sessions.len(),
            "node reported more sessions than the hello limit; the remaining sessions are not synced"
        );
    }

    // Reject nodes from a different protocol generation before sending any RPC.
    if protocol_version < fxg_protocol::node_hub::PROTOCOL_VERSION {
        tracing::error!(
            node_id = node_id.as_str(),
            protocol_version,
            required = fxg_protocol::node_hub::PROTOCOL_VERSION,
            "node protocol version is too old; upgrade fxg on the node"
        );
        fxg_protocol::discard_with!(sender.close().await, "close the node websocket");
        return;
    }

    // Reject impersonation: NodeHello.node_id must match the token owner
    if node_id.as_str() != token_owner {
        tracing::warn!(
            node_id = node_id.as_str(),
            token_owner,
            "node hello node_id does not match token owner; rejecting"
        );
        fxg_protocol::discard_with!(sender.close().await, "close the node websocket");
        return;
    }

    // Stale reports (older than what the hub already applied) must not roll back
    // project bindings or sessions.
    let stored_revision = match state.db().node_state_revision(node_id.as_str()).await {
        Ok(revision) => revision.unwrap_or(0),
        Err(err) => {
            tracing::warn!(
                node_id = node_id.as_str(),
                "failed to read node state revision: {err}"
            );
            0
        }
    };
    let stale_report = state_revision < stored_revision;
    let state_revision = state_revision.max(stored_revision);

    // 2. Register the node (a node may already have registered itself before the first Hello)
    let record = fxg_db::NodeRecord {
        node_id: node_id.as_str().to_owned(),
        name,
        os,
        arch,
        version,
        installed_agents,
        is_ephemeral,
        provisioner: None,
        lifecycle_status: fxg_protocol::common::NodeLifecycleStatus::Ready,
        idle_timeout_secs: None,
        is_online: true,
        state_revision,
    };
    if let Err(err) = state.db().upsert_node(&record).await {
        tracing::warn!(
            node_id = node_id.as_str(),
            "failed to upsert node from hello: {err}"
        );
        fxg_protocol::discard_with!(sender.close().await, "close the node websocket");
        return;
    }
    sync_project_reports(
        &state,
        &node_id,
        stale_report,
        state_revision,
        stored_revision,
        &projects,
        projects_complete,
        &sessions,
    )
    .await;

    // 3. Register the connection
    let (tx, mut rx) = mpsc::channel(NODE_CHANNEL_CAPACITY);
    let conn_id = uuid_v7();
    state.hub().register(&node_id, &conn_id, tx).await;
    tracing::info!(node_id = node_id.as_str(), "node connected");

    // 4. ResyncRequest for missing or lagging sessions
    let targets = compute_resync(&state, &node_id, &sessions).await;
    if !targets.is_empty() {
        tracing::info!(
            node_id = node_id.as_str(),
            sessions = targets.len(),
            "requesting session resync from node"
        );
        if let Err(err) = state
            .hub()
            .send(&node_id, HubToNodeMsg::ResyncRequest { sessions: targets })
            .await
        {
            tracing::warn!(
                node_id = node_id.as_str(),
                "failed to request session resync: {}",
                err.message
            );
        }
    }

    // 5. Bidirectional message loop
    loop {
        tokio::select! {
            outgoing = rx.recv() => match outgoing {
                Some(message) => {
                    if send_ws_json(&mut sender, &message).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            incoming = receiver.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    handle_node_message(&state, &node_id, &text).await;
                }
                Some(Ok(Message::Ping(payload))) => {
                    if sender.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(err)) => {
                    tracing::warn!(node_id = node_id.as_str(), "node websocket error: {err}");
                    break;
                }
                _ => {}
            },
        }
    }

    // 6. Disconnect handling (only valid for the same connection generation)
    state.hub().disconnect(&node_id, &conn_id).await;
    if let Err(err) = state.db().set_node_online(node_id.as_str(), false).await {
        tracing::warn!(
            node_id = node_id.as_str(),
            "failed to mark node offline: {err}"
        );
    }
    tracing::info!(node_id = node_id.as_str(), "node disconnected");
}

/// Handles a message from a node.
async fn handle_node_message(state: &HubState, node_id: &NodeId, text: &str) {
    let message: NodeToHubMsg = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(err) => {
            tracing::warn!(node_id = node_id.as_str(), "invalid node message: {err}");
            return;
        }
    };
    handle_node_message_value(state, node_id, message).await;
}

/// Handles an already-parsed node message (shared by WS and stdio transports).
async fn handle_node_message_value(state: &HubState, node_id: &NodeId, message: NodeToHubMsg) {
    match message {
        NodeToHubMsg::NodeHello {
            node_id: hello_node_id,
            name,
            os,
            arch,
            version,
            protocol_version,
            is_ephemeral,
            installed_agents,
            projects,
            projects_complete,
            state_revision,
            sessions,
            sessions_truncated,
        } => {
            // Not a reconnect but an in-flight resend (e.g. after a Worktree change)
            if &hello_node_id != node_id {
                tracing::warn!(
                    node_id = node_id.as_str(),
                    hello_node_id = hello_node_id.as_str(),
                    "node hello node_id mismatch; ignoring"
                );
                return;
            }
            if protocol_version < fxg_protocol::node_hub::PROTOCOL_VERSION {
                tracing::error!(
                    node_id = node_id.as_str(),
                    protocol_version,
                    required = fxg_protocol::node_hub::PROTOCOL_VERSION,
                    "node protocol version is too old; ignoring hello"
                );
                return;
            }
            if sessions_truncated {
                tracing::warn!(
                    node_id = node_id.as_str(),
                    reported = sessions.len(),
                    "node reported more sessions than the hello limit"
                );
            }
            let stored_revision = match state.db().node_state_revision(node_id.as_str()).await {
                Ok(revision) => revision.unwrap_or(0),
                Err(err) => {
                    tracing::warn!(
                        node_id = node_id.as_str(),
                        "failed to read node state revision: {err}"
                    );
                    0
                }
            };
            let stale_report = state_revision < stored_revision;
            let state_revision = state_revision.max(stored_revision);
            let record = fxg_db::NodeRecord {
                node_id: node_id.as_str().to_owned(),
                name,
                os,
                arch,
                version,
                installed_agents,
                is_ephemeral,
                provisioner: None,
                lifecycle_status: fxg_protocol::common::NodeLifecycleStatus::Ready,
                idle_timeout_secs: None,
                is_online: true,
                state_revision,
            };
            if let Err(err) = state.db().upsert_node(&record).await {
                tracing::warn!(
                    node_id = node_id.as_str(),
                    "failed to update node from hello: {err}"
                );
            }
            sync_project_reports(
                state,
                node_id,
                stale_report,
                state_revision,
                stored_revision,
                &projects,
                projects_complete,
                &sessions,
            )
            .await;
            let targets = compute_resync(state, node_id, &sessions).await;
            if !targets.is_empty()
                && let Err(err) = state
                    .hub()
                    .send(node_id, HubToNodeMsg::ResyncRequest { sessions: targets })
                    .await
            {
                tracing::warn!(
                    node_id = node_id.as_str(),
                    "failed to request session resync: {}",
                    err.message
                );
            }
        }
        NodeToHubMsg::EventBatchPush { events, resync } => {
            apply_event_batch(state, node_id, events, resync).await;
        }
        NodeToHubMsg::LiveStreamDelta { session_id, delta } => {
            fxg_protocol::discard_with!(
                state
                    .hub()
                    .client_events()
                    .send(ClientEvent::StreamDelta { session_id, delta }),
                "broadcast a live stream delta to clients"
            );
        }
        NodeToHubMsg::CommandResult {
            command_id,
            success,
            error,
            session_id,
            warnings,
            data_safety,
        } => {
            let result = CommandResult {
                command_id: command_id.clone(),
                success,
                error,
                session_id,
                warnings,
                data_safety,
            };
            state
                .hub()
                .resolve(command_id.as_str(), PendingResponse::Command(result))
                .await;
        }
        NodeToHubMsg::NodeRpcResult {
            request_id,
            outcome,
        } => {
            state
                .hub()
                .resolve(request_id.as_str(), PendingResponse::Rpc(outcome))
                .await;
        }
        NodeToHubMsg::PtyOutput { pty_id, data_b64 } => {
            use base64::Engine as _;
            match base64::engine::general_purpose::STANDARD.decode(data_b64.as_bytes()) {
                Ok(data) => {
                    state.hub().publish_pty_output(&pty_id, data).await;
                }
                Err(err) => {
                    tracing::warn!(pty_id, "invalid pty output base64: {err}");
                }
            }
        }
        NodeToHubMsg::PtyOpened {
            request_id,
            pty_id,
            path,
            cols,
            rows,
            attached,
            snapshot_b64,
        } => {
            use base64::Engine as _;
            let snapshot = base64::engine::general_purpose::STANDARD
                .decode(snapshot_b64.as_bytes())
                .unwrap_or_else(|err| {
                    tracing::warn!(
                        pty_id,
                        "invalid pty snapshot base64 (continuing without replay): {err}"
                    );
                    Vec::new()
                });
            state
                .hub()
                .resolve(
                    request_id.as_str(),
                    PendingResponse::PtyOpen(Ok(PtyOpenedReply {
                        pty_id,
                        path,
                        cols,
                        rows,
                        attached,
                        snapshot,
                    })),
                )
                .await;
        }
        NodeToHubMsg::PtyOpenFailed {
            request_id,
            pty_id,
            code,
            message,
        } => {
            tracing::debug!(pty_id, code, "remote pty open failed: {message}");
            state
                .hub()
                .resolve(
                    request_id.as_str(),
                    PendingResponse::PtyOpen(Err(PtyChannelError::new(code, message))),
                )
                .await;
        }
        NodeToHubMsg::PtyExit { pty_id, exit_code } => {
            state
                .hub()
                .publish_pty(&pty_id, PtyChannelEvent::Exit { exit_code })
                .await;
            state.hub().remove_pty(&pty_id).await;
        }
        NodeToHubMsg::PtyError {
            pty_id,
            code,
            message,
        } => {
            state
                .hub()
                .publish_pty(&pty_id, PtyChannelEvent::Error { code, message })
                .await;
            state.hub().remove_pty(&pty_id).await;
        }
        NodeToHubMsg::DrainComplete { node_id: drained } => {
            tracing::info!(node_id = node_id.as_str(), "drain complete");
            crate::provisioner::on_drain_complete(state, drained.as_str());
        }
    }
}

/// Handles a message received from a temporary VM (`fxg internal stdio`).
///
/// Uses the same message types and the same processing path ([`handle_node_message`]) as a
/// WebSocket node (design: docs/03 §2).
pub(crate) async fn handle_stdio_message(
    state: &HubState,
    node_id: &NodeId,
    message: NodeToHubMsg,
) {
    handle_node_message_value(state, node_id, message).await;
}

/// Applies `NodeHello` project reports when the revision is current and notifies
/// connected UI clients on success.
#[allow(clippy::too_many_arguments)]
async fn sync_project_reports(
    state: &HubState,
    node_id: &NodeId,
    stale_report: bool,
    state_revision: u64,
    stored_revision: u64,
    projects: &[fxg_protocol::common::NodeProjectReport],
    projects_complete: bool,
    sessions: &[fxg_protocol::node_hub::SessionSyncState],
) {
    if stale_report {
        tracing::warn!(
            node_id = node_id.as_str(),
            state_revision,
            stored_revision,
            "ignoring stale NodeHello report"
        );
        return;
    }
    match apply_project_reports(state, node_id, projects, projects_complete, sessions).await {
        Ok(()) => {
            fxg_protocol::discard_with!(
                state
                    .hub()
                    .client_events()
                    .send(ClientEvent::ProjectsUpdated),
                "notify clients that projects or bindings updated"
            );
        }
        Err(err) => {
            tracing::warn!(
                node_id = node_id.as_str(),
                "failed to apply project reports: {err}"
            );
        }
    }
}

/// Applies `NodeHello` project reports to `projects` / `project_node_bindings`.
async fn apply_project_reports(
    state: &HubState,
    node_id: &NodeId,
    projects: &[fxg_protocol::common::NodeProjectReport],
    projects_complete: bool,
    sessions: &[fxg_protocol::node_hub::SessionSyncState],
) -> Result<(), fxg_db::DbError> {
    let mut assignments: Vec<(String, String)> = Vec::new();
    for project in projects {
        state
            .db()
            .upsert_project(&fxg_db::registration::ProjectRecord {
                project_id: project.project_id.to_string(),
                name: project.name.clone(),
                canonical_git_url: project.canonical_git_url.clone(),
                identity: project.identity.clone(),
            })
            .await?;
        for binding in &project.bindings {
            state
                .db()
                .upsert_project_binding(&fxg_db::registration::ProjectBindingRecord {
                    project_id: project.project_id.to_string(),
                    node_id: node_id.as_str().to_owned(),
                    local_path: binding.local_path.clone(),
                    is_worktree: binding.is_worktree,
                    path_exists: binding.path_exists,
                    git_branch: binding.git_branch.clone(),
                })
                .await?;
            assignments.push((binding.local_path.clone(), project.project_id.to_string()));
        }
    }

    // Sessions follow the current binding / explicit report of their node.
    state
        .db()
        .reassign_sessions_project(node_id.as_str(), &assignments)
        .await?;
    for session in sessions {
        state
            .db()
            .reassign_session_project(session.session_id.as_str(), session.project_id.as_str())
            .await?;
    }

    if projects_complete {
        let keep: Vec<String> = assignments
            .into_iter()
            .map(|(local_path, _)| local_path)
            .collect();
        state
            .db()
            .prune_node_project_bindings(node_id.as_str(), &keep)
            .await?;
        state.db().delete_orphan_projects().await?;
    }
    Ok(())
}

/// Computes the resend targets for sessions that are missing or lagging on the hub side.
async fn compute_resync(
    state: &HubState,
    node_id: &NodeId,
    sessions: &[fxg_protocol::node_hub::SessionSyncState],
) -> Vec<ResyncTarget> {
    let mut targets = Vec::new();
    for sync in sessions {
        if sync.last_node_seq == 0 {
            continue;
        }
        match state.db().get_session(sync.session_id.as_str()).await {
            Ok(Some(session)) if &session.node_id == node_id => {
                if session.last_node_seq < sync.last_node_seq {
                    targets.push(ResyncTarget {
                        session_id: sync.session_id.clone(),
                        from_node_seq: session.last_node_seq + 1,
                    });
                } else if session.last_node_seq > sync.last_node_seq {
                    tracing::warn!(
                        session_id = sync.session_id.as_str(),
                        hub_seq = session.last_node_seq,
                        node_seq = sync.last_node_seq,
                        "hub is ahead of node for session; skipping resync"
                    );
                }
            }
            Ok(Some(session)) => {
                // Requesting a full resync here cannot succeed: the hub rejects
                // `SessionCreated` for a session owned by another node, so the
                // batch would never be ACKed and the node would retry forever.
                tracing::error!(
                    session_id = sync.session_id.as_str(),
                    hub_node = session.node_id.as_str(),
                    node_id = node_id.as_str(),
                    "session is owned by a different node; skipping the resync (node_id conflict)"
                );
            }
            Ok(None) => {
                // Session unknown to the hub: resend everything, including SessionCreated
                targets.push(ResyncTarget {
                    session_id: sync.session_id.clone(),
                    from_node_seq: 1,
                });
            }
            Err(err) => {
                tracing::warn!(
                    session_id = sync.session_id.as_str(),
                    "failed to read session: {err}"
                );
            }
        }
    }
    targets
}

/// Applies `EventBatchPush` idempotently, sends an ACK and delivers to Client WS.
///
/// If application fails, no ACK is sent (the node's watermark does not advance and it retries).
async fn apply_event_batch(
    state: &HubState,
    node_id: &NodeId,
    events: Vec<fxg_protocol::events::SessionEventEnvelope>,
    resync: bool,
) {
    // Prevent impersonation: SessionCreated.node_id must match the connecting node
    for event in &events {
        if let fxg_protocol::events::UnifiedEventPayload::SessionCreated {
            node_id: event_node_id,
            ..
        } = &event.payload
            && event_node_id != node_id
        {
            tracing::warn!(
                node_id = node_id.as_str(),
                event_node_id = event_node_id.as_str(),
                session_id = event.session_id.as_str(),
                "rejecting event batch with mismatched node_id"
            );
            return;
        }
    }

    // Split by session (the protocol batches per session, but be defensive)
    let mut batches: Vec<(String, Vec<fxg_protocol::events::SessionEventEnvelope>)> = Vec::new();
    for event in events {
        match batches.last_mut() {
            Some((session_id, batch)) if session_id.as_str() == event.session_id.as_str() => {
                batch.push(event)
            }
            _ => batches.push((event.session_id.to_string(), vec![event])),
        }
    }

    // Newly inserted events. Notification targets are selected after all batches apply so the
    // running-turn state sees the whole message in order.
    let mut inserted_events: Vec<fxg_protocol::events::SessionEventEnvelope> = Vec::new();

    for (session_id, batch) in batches {
        let acked_up_to_node_seq = batch.last().map(|event| event.node_seq).unwrap_or(0);
        match state.db().append_events(&batch).await {
            Ok(outcomes) => {
                for (event, outcome) in batch.iter().zip(outcomes.iter()) {
                    if outcome.inserted {
                        fxg_protocol::discard_with!(
                            state.hub().client_events().send(ClientEvent::Persisted {
                                event: Box::new(event.clone()),
                                cursor: outcome.cursor,
                            }),
                            "broadcast a persisted event to clients"
                        );
                        inserted_events.push(event.clone());
                    }
                }
                if let Err(err) = state
                    .hub()
                    .send(
                        node_id,
                        HubToNodeMsg::EventBatchAck {
                            session_id: session_id.into(),
                            acked_up_to_node_seq,
                        },
                    )
                    .await
                {
                    tracing::warn!(
                        node_id = node_id.as_str(),
                        "failed to acknowledge the event batch: {err:#}"
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    node_id = node_id.as_str(),
                    session_id,
                    "failed to apply event batch (will retry): {err}"
                );
            }
        }
    }

    // Web Push notifications for approvals, questions and finished turns.
    // Sent asynchronously so they do not block the ACK. Historical events replayed during a
    // resync still update the running-turn state but never wake up devices
    // (`push::notification_events`).
    crate::push::dispatch_notifications(
        state.push(),
        state.db(),
        state.running_turns(),
        &inserted_events,
        resync,
        Some(state.diagnostics_counters_arc()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::common::{NodeProjectReport, ProjectNodeBinding};
    use fxg_protocol::paths::{Paths, Role};
    use fxg_protocol::settings::{ProjectIdentity, ProjectIdentityKind};

    #[tokio::test]
    async fn sync_project_reports_updates_db_and_notifies_clients() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = HubState::new(crate::state::HubOptions {
            paths: Paths::portable(dir.path(), Role::Hub),
            listen_addr: "127.0.0.1:0".to_owned(),
            allowed_hosts: Vec::new(),
            allowed_origins: Vec::new(),
            log: Default::default(),
        })
        .await
        .expect("hub state");

        let mut rx = state.hub().client_events().subscribe();
        let node_id = NodeId::from("node-1");
        state
            .db()
            .upsert_node(&fxg_db::NodeRecord {
                node_id: node_id.as_str().to_owned(),
                name: "node-1".to_owned(),
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                version: "0.1.0".to_owned(),
                installed_agents: Vec::new(),
                is_ephemeral: false,
                provisioner: None,
                lifecycle_status: fxg_protocol::common::NodeLifecycleStatus::Ready,
                idle_timeout_secs: None,
                is_online: true,
                state_revision: 1,
            })
            .await
            .expect("upsert node");
        let project_id = fxg_protocol::ProjectId::from("local:node-1:abc");
        let reports = vec![NodeProjectReport {
            project_id: project_id.clone(),
            name: "demo-repo".to_owned(),
            canonical_git_url: None,
            identity: ProjectIdentity {
                kind: ProjectIdentityKind::Local,
                key: Some("node-1:abc".to_owned()),
            },
            bindings: vec![ProjectNodeBinding {
                local_path: "/tmp/demo-repo".to_owned(),
                is_worktree: false,
                path_exists: true,
                git_branch: Some("main".to_owned()),
                last_used_at: 100,
            }],
        }];

        // Stale reports are ignored and must not emit notifications or write projects.
        sync_project_reports(&state, &node_id, true, 1, 2, &reports, false, &[]).await;
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(state.db().list_projects().await.expect("list").is_empty());

        // Current reports write the project + binding and broadcast ProjectsUpdated.
        sync_project_reports(&state, &node_id, false, 2, 1, &reports, false, &[]).await;
        let event = rx.try_recv().expect("ProjectsUpdated event");
        assert!(matches!(event, ClientEvent::ProjectsUpdated));

        let projects = state.db().list_projects().await.expect("list");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].project_id, project_id);
        assert_eq!(projects[0].name, "demo-repo");
        assert_eq!(projects[0].bindings.len(), 1);
        assert_eq!(projects[0].bindings[0].local_path, "/tmp/demo-repo");
    }
}
