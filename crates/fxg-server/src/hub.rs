//! Node Hub (`/api/v1/node/ws`): manages outbound WebSocket connections from resident nodes.
//!
//! Design: `docs/01-architecture-and-sync.md` §2.2 and §3, `docs/03-protocol-and-api.md` §2.
//!
//! - **Authentication**: hash `Authorization: Bearer <NODE_TOKEN>`, compare it against
//!   `server.db.nodes.token_hash`, then verify that `NodeHello.node_id` matches the node the
//!   token was issued for (§7.3)
//! - **Handshake**: `NodeHello` → the hub replies with `ResyncRequest` for sessions that are
//!   missing or behind, and the node resends from the given `from_node_seq` onward
//! - **Event ingestion**: apply `EventBatchPush` idempotently with `INSERT OR IGNORE` +
//!   projection updates (same Tx), then return `EventBatchAck` (watermark). Newly inserted
//!   events are delivered to Client WS subscribers
//! - **Command relay**: correlate by `command_id` and return the node's `CommandResult` /
//!   `GitDiffResult` / `WorktreeResult` to the requesting client
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
use fxg_protocol::client_api::WorktreeInfo;
use fxg_protocol::common::{CommandResult, ErrorCode, WorkspaceDiffResponse};
use fxg_protocol::node_server::{NodeToServerMsg, ResyncTarget, ServerToNodeMsg};
use fxg_protocol::util::uuid_v7;
use fxg_protocol::{CommandId, NodeId};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, oneshot};

use crate::api::auth::{hash_token, token_matches};
use crate::api::{ApiError, ClientEvent, PtyChannelError, PtyChannelEvent, send_ws_json};
use crate::state::ServerState;

/// Timeout for the handshake (`NodeHello`) after a node connects.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for a relayed command response.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for session commands (prompt submission, permission responses, control, resume).
///
/// Sending to a stopped session and resuming start the agent on the node side, so a cold
/// start (e.g. PyInstaller extraction, ~25 s) is allowed.
const SESSION_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Timeout for project operations (scans, etc.).
///
/// Directory traversal can exceed 30 seconds depending on the target size.
const PROJECT_OP_TIMEOUT: Duration = Duration::from_secs(120);

/// Timeout for fetching the ACP Registry catalog
/// (including the network re-fetch of the index).
const AGENTS_TIMEOUT: Duration = Duration::from_secs(120);

/// Timeout for agent installs and updates.
///
/// Downloading and extracting binaries can take minutes.
const AGENT_OP_TIMEOUT: Duration = Duration::from_secs(600);

/// Outgoing queue capacity for a node connection.
const NODE_CHANNEL_CAPACITY: usize = 512;

/// Broadcast capacity for PTY output.
const PTY_CHANNEL_CAPACITY: usize = 1024;

/// Kind of response a relay is waiting for.
enum PendingResponse {
    /// Command execution result (`CommandResult`)
    Command(CommandResult),
    /// Git Diff result (`GitDiffResult`)
    GitDiff {
        /// Diff on success
        diff: Option<WorkspaceDiffResponse>,
        /// Message on failure
        error: Option<String>,
    },
    /// Worktree operation result (`WorktreeResult`)
    Worktree {
        /// Worktree info on success (`None` when deleted)
        worktree: Option<WorktreeInfo>,
        /// Message on failure
        error: Option<String>,
    },
    /// Session revert result (`RevertResult`)
    Revert {
        /// Whether it succeeded
        success: bool,
        /// Structured error code on failure
        code: Option<ErrorCode>,
        /// Restore outcome on success
        outcome: Option<fxg_protocol::client_api::SessionRevertResponse>,
        /// Message on failure
        error: Option<String>,
    },
    /// Session resume result (`ResumeResult`)
    Resume {
        /// Whether it succeeded
        success: bool,
        /// Structured error code on failure
        code: Option<ErrorCode>,
        /// Whether native restore succeeded on success (`false` = continue by replaying history)
        context_restored: Option<bool>,
        /// Message on failure
        error: Option<String>,
    },
    /// Session archive/restore result (`ArchiveResult`)
    Archive {
        /// Whether it succeeded
        success: bool,
        /// Structured error code on failure
        code: Option<ErrorCode>,
        /// Archive timestamp on success (Unix epoch ms). `None` when restoring
        archived_at: Option<i64>,
        /// Message on failure
        error: Option<String>,
    },
    /// Session delete result (`DeleteResult`)
    Delete {
        /// Whether it succeeded
        success: bool,
        /// Structured error code on failure
        code: Option<ErrorCode>,
        /// Message on failure
        error: Option<String>,
    },
    /// Filesystem browse result (`BrowseFsResult`)
    BrowseFs {
        /// Browse result on success
        response: Option<fxg_protocol::client_api::FsBrowseResponse>,
        /// Message on failure
        error: Option<String>,
    },
    /// Project bulk scan result (`ProjectScanResult`)
    ProjectScan {
        /// Scan result on success
        response: Option<fxg_protocol::client_api::ProjectScanResponse>,
        /// Message on failure
        error: Option<String>,
    },
    /// Project manual link result (`ProjectLinkResult`)
    ProjectLink {
        /// Link result on success
        response: Option<fxg_protocol::client_api::ProjectLinkResponse>,
        /// Message on failure
        error: Option<String>,
    },
    /// Agent management operation result (`AgentOpResult`)
    AgentOp {
        /// Whether it succeeded
        success: bool,
        /// Structured error code on failure
        code: Option<ErrorCode>,
        /// Display message on success
        message: Option<String>,
        /// Message on failure
        error: Option<String>,
    },
    /// ACP Registry catalog result (`AgentsResult`)
    Agents {
        /// Catalog on success
        response: Option<fxg_protocol::client_api::AgentsResponse>,
        /// Message on failure
        error: Option<String>,
    },
    /// Failure such as a node disconnect
    Failed {
        /// Structured error code
        code: ErrorCode,
        /// Human-readable message
        message: String,
    },
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
    tx: mpsc::Sender<ServerToNodeMsg>,
}

/// PTY relay destination.
struct PtyRoute {
    /// Owning node ID
    node_id: NodeId,
    /// Broadcast delivered to clients
    tx: broadcast::Sender<PtyChannelEvent>,
}

struct HubInner {
    db: Db,
    client_events: broadcast::Sender<ClientEvent>,
    nodes: RwLock<HashMap<String, NodeConnection>>,
    pending: Mutex<HashMap<String, PendingEntry>>,
    ptys: Mutex<HashMap<String, PtyRoute>>,
}

/// Node connection registry and command relay (referenced from the central server's shared state).
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
            }),
        }
    }

    /// Server DB.
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

    /// Registers a temporary VM (`fxg daemon --stdio`) connection.
    ///
    /// The provisioner manager forwards `ServerToNodeMsg` items arriving on the returned receiver
    /// to the child process's `stdin` (JSON Lines).
    pub(crate) async fn attach_ephemeral(
        &self,
        node_id: &NodeId,
    ) -> (mpsc::Receiver<ServerToNodeMsg>, String) {
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
    async fn register(&self, node_id: &NodeId, conn_id: &str, tx: mpsc::Sender<ServerToNodeMsg>) {
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
                        message: format!("node {node_id} disconnected"),
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
    pub async fn send(&self, node_id: &NodeId, message: ServerToNodeMsg) -> Result<(), ApiError> {
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
        message: ServerToNodeMsg,
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

    /// Relays a Git Diff request and waits for the result correlated by `request_id`.
    pub async fn git_diff(
        &self,
        node_id: &NodeId,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<WorkspaceDiffResponse, ApiError> {
        let response = self
            .request(node_id, request_id, message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::GitDiff { diff, error } => match (diff, error) {
                (Some(diff), _) => Ok(diff),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Err(ApiError::internal("node returned no diff")),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Relays a Worktree operation and waits for the result correlated by `command_id`.
    pub async fn worktree_op(
        &self,
        node_id: &NodeId,
        command_id: &CommandId,
        message: ServerToNodeMsg,
    ) -> Result<Option<WorktreeInfo>, ApiError> {
        let response = self
            .request(node_id, command_id.as_str(), message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::Worktree { worktree, error } => match (worktree, error) {
                (Some(worktree), _) => Ok(Some(worktree)),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Ok(None),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Relays a session revert and waits for the result correlated by `command_id`.
    pub async fn revert_session(
        &self,
        node_id: &NodeId,
        command_id: &CommandId,
        message: ServerToNodeMsg,
    ) -> Result<fxg_protocol::client_api::SessionRevertResponse, ApiError> {
        let response = self
            .request(node_id, command_id.as_str(), message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::Revert {
                success,
                code,
                outcome,
                error,
            } => {
                if success {
                    outcome.ok_or_else(|| ApiError::internal("node returned no revert outcome"))
                } else {
                    Err(ApiError::from_code(
                        code.unwrap_or(ErrorCode::Internal),
                        error.unwrap_or_else(|| "revert failed".to_owned()),
                    ))
                }
            }
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Relays a session resume and waits for the result correlated by `command_id`.
    ///
    /// The return value is whether native context restore succeeded (`false` = continue by
    /// replaying history). Waits with [`SESSION_COMMAND_TIMEOUT`] to allow agent cold starts.
    pub async fn resume_session(
        &self,
        node_id: &NodeId,
        command_id: &CommandId,
        message: ServerToNodeMsg,
    ) -> Result<bool, ApiError> {
        let response = self
            .request(
                node_id,
                command_id.as_str(),
                message,
                SESSION_COMMAND_TIMEOUT,
            )
            .await?;
        match response {
            PendingResponse::Resume {
                success,
                code,
                context_restored,
                error,
            } => {
                if success {
                    context_restored
                        .ok_or_else(|| ApiError::internal("node returned no resume outcome"))
                } else {
                    Err(ApiError::from_code(
                        code.unwrap_or(ErrorCode::Internal),
                        error.unwrap_or_else(|| "resume failed".to_owned()),
                    ))
                }
            }
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Relays a session archive/restore and waits for the result correlated by `command_id`.
    ///
    /// The return value is the archive timestamp (Unix epoch ms; `None` when restoring).
    pub async fn archive_session(
        &self,
        node_id: &NodeId,
        command_id: &CommandId,
        message: ServerToNodeMsg,
    ) -> Result<Option<i64>, ApiError> {
        let response = self
            .request(node_id, command_id.as_str(), message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::Archive {
                success,
                code,
                archived_at,
                error,
            } => {
                if success {
                    Ok(archived_at)
                } else {
                    Err(ApiError::from_code(
                        code.unwrap_or(ErrorCode::Internal),
                        error.unwrap_or_else(|| "archive failed".to_owned()),
                    ))
                }
            }
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Relays a session delete and waits for the result correlated by `command_id`.
    ///
    /// The executing node waits for a running session to stop, so wait with
    /// [`SESSION_COMMAND_TIMEOUT`].
    pub async fn delete_session(
        &self,
        node_id: &NodeId,
        command_id: &CommandId,
        message: ServerToNodeMsg,
    ) -> Result<(), ApiError> {
        let response = self
            .request(
                node_id,
                command_id.as_str(),
                message,
                SESSION_COMMAND_TIMEOUT,
            )
            .await?;
        match response {
            PendingResponse::Delete {
                success,
                code,
                error,
            } => {
                if success {
                    Ok(())
                } else {
                    Err(ApiError::from_code(
                        code.unwrap_or(ErrorCode::Internal),
                        error.unwrap_or_else(|| "delete failed".to_owned()),
                    ))
                }
            }
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Relays a filesystem browse and waits for the result correlated by `request_id`.
    pub async fn browse_fs(
        &self,
        node_id: &NodeId,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<fxg_protocol::client_api::FsBrowseResponse, ApiError> {
        let response = self
            .request(node_id, request_id, message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::BrowseFs { response, error } => match (response, error) {
                (Some(resp), _) => Ok(resp),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Err(ApiError::internal("node returned no fs browse response")),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Relays a project bulk scan and waits for the result correlated by `request_id`.
    ///
    /// Directory traversal can take a while, so wait with [`PROJECT_OP_TIMEOUT`], longer than
    /// the normal command timeout.
    pub async fn scan_projects(
        &self,
        node_id: &NodeId,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<fxg_protocol::client_api::ProjectScanResponse, ApiError> {
        let response = self
            .request(node_id, request_id, message, PROJECT_OP_TIMEOUT)
            .await?;
        match response {
            PendingResponse::ProjectScan { response, error } => match (response, error) {
                (Some(resp), _) => Ok(resp),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Err(ApiError::internal("node returned no project scan response")),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Relays a project manual link and waits for the result correlated by `request_id`.
    pub async fn link_project(
        &self,
        node_id: &NodeId,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<fxg_protocol::client_api::ProjectLinkResponse, ApiError> {
        let response = self
            .request(node_id, request_id, message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::ProjectLink { response, error } => match (response, error) {
                (Some(resp), _) => Ok(resp),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Err(ApiError::internal("node returned no project link response")),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Relays the ACP Registry catalog and waits for the result correlated by `request_id`.
    pub async fn agents(
        &self,
        node_id: &NodeId,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<fxg_protocol::client_api::AgentsResponse, ApiError> {
        let response = self
            .request(node_id, request_id, message, AGENTS_TIMEOUT)
            .await?;
        match response {
            PendingResponse::Agents { response, error } => match (response, error) {
                (Some(resp), _) => Ok(resp),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Err(ApiError::internal("node returned no agents response")),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Relays an agent management operation (install / update / remove) and waits for the
    /// result correlated by `request_id`. On success, returns the display message.
    pub async fn manage_agent(
        &self,
        node_id: &NodeId,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<Option<String>, ApiError> {
        let response = self
            .request(node_id, request_id, message, AGENT_OP_TIMEOUT)
            .await?;
        match response {
            PendingResponse::AgentOp {
                success,
                code,
                message,
                error,
            } => {
                if success {
                    Ok(message)
                } else {
                    Err(ApiError::from_code(
                        code.unwrap_or(ErrorCode::Internal),
                        error.unwrap_or_else(|| "agent operation failed".to_owned()),
                    ))
                }
            }
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Common send-then-wait path.
    async fn request(
        &self,
        node_id: &NodeId,
        correlation_id: &str,
        message: ServerToNodeMsg,
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
                format!("node {node_id} disconnected"),
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
    pub async fn broadcast(&self, message: ServerToNodeMsg) -> Vec<String> {
        let connections: Vec<(String, mpsc::Sender<ServerToNodeMsg>)> = {
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

    /// Registers a PTY relay destination (at spawn time).
    pub(crate) async fn insert_pty(&self, pty_id: &str, node_id: &NodeId) {
        let (tx, _) = broadcast::channel(PTY_CHANNEL_CAPACITY);
        self.inner.ptys.lock().await.insert(
            pty_id.to_owned(),
            PtyRoute {
                node_id: node_id.to_owned(),
                tx,
            },
        );
    }

    /// Subscribes to PTY output (`None` when not registered).
    pub(crate) async fn pty_subscribe(
        &self,
        pty_id: &str,
    ) -> Option<broadcast::Receiver<PtyChannelEvent>> {
        self.inner
            .ptys
            .lock()
            .await
            .get(pty_id)
            .map(|route| route.tx.subscribe())
    }

    /// Relays a message to the node that owns the PTY.
    pub(crate) async fn pty_send(
        &self,
        pty_id: &str,
        message: ServerToNodeMsg,
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
    ApiError::node_offline(
        node_id.as_str(),
        format!("node is not connected: {node_id}"),
    )
}

// ----------------------------------------------------------------------
// WebSocket handlers
// ----------------------------------------------------------------------

/// `/api/v1/node/ws` handler (per-node token auth + WS upgrade).
pub(crate) async fn node_ws(
    State(state): State<ServerState>,
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
            "missing node token",
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
            "invalid node token",
        )
        .into_response();
    };

    ws.on_upgrade(move |socket| handle_node_connection(state, owner, socket))
}

/// Main loop for a node connection (NodeHello → register → bidirectional messaging).
async fn handle_node_connection(state: ServerState, token_owner: String, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();

    // 1. The first message must be NodeHello (with timeout)
    let hello = match tokio::time::timeout(HELLO_TIMEOUT, receiver.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<NodeToServerMsg>(&text) {
            Ok(hello @ NodeToServerMsg::NodeHello { .. }) => hello,
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

    let NodeToServerMsg::NodeHello {
        node_id,
        name,
        os,
        arch,
        version,
        is_ephemeral,
        installed_agents,
        projects,
        sessions,
    } = hello
    else {
        unreachable!("hello was matched above");
    };

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
    };
    if let Err(err) = state.db().upsert_node(&record).await {
        tracing::warn!(
            node_id = node_id.as_str(),
            "failed to upsert node from hello: {err}"
        );
        fxg_protocol::discard_with!(sender.close().await, "close the node websocket");
        return;
    }
    if let Err(err) = apply_project_reports(&state, &node_id, &projects).await {
        tracing::warn!(
            node_id = node_id.as_str(),
            "failed to apply project reports: {err}"
        );
    }

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
            .send(
                &node_id,
                ServerToNodeMsg::ResyncRequest { sessions: targets },
            )
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
async fn handle_node_message(state: &ServerState, node_id: &NodeId, text: &str) {
    let message: NodeToServerMsg = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(err) => {
            tracing::warn!(node_id = node_id.as_str(), "invalid node message: {err}");
            return;
        }
    };
    handle_node_message_value(state, node_id, message).await;
}

/// Handles an already-parsed node message (shared by WS and stdio transports).
async fn handle_node_message_value(
    state: &ServerState,
    node_id: &NodeId,
    message: NodeToServerMsg,
) {
    match message {
        NodeToServerMsg::NodeHello {
            node_id: hello_node_id,
            name,
            os,
            arch,
            version,
            is_ephemeral,
            installed_agents,
            projects,
            sessions,
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
            };
            if let Err(err) = state.db().upsert_node(&record).await {
                tracing::warn!(
                    node_id = node_id.as_str(),
                    "failed to update node from hello: {err}"
                );
            }
            if let Err(err) = apply_project_reports(state, node_id, &projects).await {
                tracing::warn!(
                    node_id = node_id.as_str(),
                    "failed to apply project reports: {err}"
                );
            }
            let targets = compute_resync(state, node_id, &sessions).await;
            if !targets.is_empty()
                && let Err(err) = state
                    .hub()
                    .send(
                        node_id,
                        ServerToNodeMsg::ResyncRequest { sessions: targets },
                    )
                    .await
            {
                tracing::warn!(
                    node_id = node_id.as_str(),
                    "failed to request session resync: {}",
                    err.message
                );
            }
        }
        NodeToServerMsg::EventBatchPush { events } => {
            apply_event_batch(state, node_id, events).await;
        }
        NodeToServerMsg::LiveStreamDelta { session_id, delta } => {
            fxg_protocol::discard_with!(
                state
                    .hub()
                    .client_events()
                    .send(ClientEvent::StreamDelta { session_id, delta }),
                "broadcast a live stream delta to clients"
            );
        }
        NodeToServerMsg::CommandResult {
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
        NodeToServerMsg::GitDiffResult {
            request_id,
            diff,
            error,
        } => {
            state
                .hub()
                .resolve(&request_id, PendingResponse::GitDiff { diff, error })
                .await;
        }
        NodeToServerMsg::WorktreeResult {
            command_id,
            worktree,
            error,
        } => {
            state
                .hub()
                .resolve(
                    command_id.as_str(),
                    PendingResponse::Worktree { worktree, error },
                )
                .await;
        }
        NodeToServerMsg::RevertResult {
            command_id,
            success,
            code,
            outcome,
            error,
        } => {
            state
                .hub()
                .resolve(
                    command_id.as_str(),
                    PendingResponse::Revert {
                        success,
                        code,
                        outcome,
                        error,
                    },
                )
                .await;
        }
        NodeToServerMsg::ResumeResult {
            command_id,
            success,
            code,
            context_restored,
            error,
        } => {
            state
                .hub()
                .resolve(
                    command_id.as_str(),
                    PendingResponse::Resume {
                        success,
                        code,
                        context_restored,
                        error,
                    },
                )
                .await;
        }
        NodeToServerMsg::ArchiveResult {
            command_id,
            success,
            code,
            archived_at,
            error,
        } => {
            state
                .hub()
                .resolve(
                    command_id.as_str(),
                    PendingResponse::Archive {
                        success,
                        code,
                        archived_at,
                        error,
                    },
                )
                .await;
        }
        NodeToServerMsg::DeleteResult {
            command_id,
            success,
            code,
            error,
        } => {
            state
                .hub()
                .resolve(
                    command_id.as_str(),
                    PendingResponse::Delete {
                        success,
                        code,
                        error,
                    },
                )
                .await;
        }
        NodeToServerMsg::BrowseFsResult {
            request_id,
            response,
            error,
        } => {
            state
                .hub()
                .resolve(&request_id, PendingResponse::BrowseFs { response, error })
                .await;
        }
        NodeToServerMsg::ProjectScanResult {
            request_id,
            response,
            error,
        } => {
            state
                .hub()
                .resolve(
                    &request_id,
                    PendingResponse::ProjectScan { response, error },
                )
                .await;
        }
        NodeToServerMsg::ProjectLinkResult {
            request_id,
            response,
            error,
        } => {
            state
                .hub()
                .resolve(
                    &request_id,
                    PendingResponse::ProjectLink { response, error },
                )
                .await;
        }
        NodeToServerMsg::AgentsResult {
            request_id,
            response,
            error,
        } => {
            state
                .hub()
                .resolve(&request_id, PendingResponse::Agents { response, error })
                .await;
        }
        NodeToServerMsg::AgentOpResult {
            request_id,
            success,
            code,
            message,
            error,
        } => {
            state
                .hub()
                .resolve(
                    &request_id,
                    PendingResponse::AgentOp {
                        success,
                        code,
                        message,
                        error,
                    },
                )
                .await;
        }
        NodeToServerMsg::PtyOutput { pty_id, data_b64 } => {
            use base64::Engine as _;
            match base64::engine::general_purpose::STANDARD.decode(data_b64.as_bytes()) {
                Ok(data) => {
                    state
                        .hub()
                        .publish_pty(&pty_id, PtyChannelEvent::Output { data })
                        .await;
                }
                Err(err) => {
                    tracing::warn!(pty_id, "invalid pty output base64: {err}");
                }
            }
        }
        NodeToServerMsg::PtyExit { pty_id, exit_code } => {
            state
                .hub()
                .publish_pty(&pty_id, PtyChannelEvent::Exit { exit_code })
                .await;
            state.hub().remove_pty(&pty_id).await;
        }
        NodeToServerMsg::PtyError {
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
        NodeToServerMsg::GitCredentialRequest {
            request_id,
            repo_url,
            operation,
        } => {
            // Git Credential Proxy (GIT_ASKPASS for temporary VMs): resolve the token in memory
            // from the configured provider and return it over the stdio pipe (docs/01 §6.4)
            let response = crate::provisioner::resolve_git_token(state, &repo_url).await;
            let message = match response {
                Some(response) => ServerToNodeMsg::GitCredentialResponse {
                    request_id,
                    username: response.username,
                    token: response.token,
                    error: response.error,
                },
                None => ServerToNodeMsg::GitCredentialResponse {
                    request_id,
                    username: fxg_protocol::config::DEFAULT_GIT_USERNAME.to_owned(),
                    token: None,
                    error: Some(format!(
                        "no git credential configured for {repo_url} ({operation})"
                    )),
                },
            };
            if let Err(err) = state.hub().send(node_id, message).await {
                tracing::warn!(
                    node_id = node_id.as_str(),
                    "failed to send the git credential error to the node: {err:#}"
                );
            }
        }
        NodeToServerMsg::WorkspaceBundleUpload {
            session_id,
            branch,
            head_commit,
            bundle_b64,
        } => {
            // Accumulate chunked bundle uploads until DrainComplete
            crate::provisioner::on_bundle_upload(
                state,
                session_id.as_str(),
                &branch,
                &head_commit,
                &bundle_b64,
            );
        }
        NodeToServerMsg::DrainComplete { node_id: drained } => {
            tracing::info!(node_id = node_id.as_str(), "drain complete");
            crate::provisioner::on_drain_complete(state, drained.as_str()).await;
        }
    }
}

/// Handles a message received from a temporary VM (`fxg daemon --stdio`).
///
/// Uses the same message types and the same processing path ([`handle_node_message`]) as a
/// WebSocket node (design: docs/03 §2).
pub(crate) async fn handle_stdio_message(
    state: &ServerState,
    node_id: &NodeId,
    message: NodeToServerMsg,
) {
    handle_node_message_value(state, node_id, message).await;
}

/// Applies `NodeHello` project reports to `projects` / `project_node_bindings`.
async fn apply_project_reports(
    state: &ServerState,
    node_id: &NodeId,
    projects: &[fxg_protocol::common::NodeProjectReport],
) -> Result<(), fxg_db::DbError> {
    for project in projects {
        state
            .db()
            .upsert_project(&fxg_db::registration::ProjectRecord {
                project_id: project.project_id.clone(),
                name: project.name.clone(),
                canonical_git_url: project.canonical_git_url.clone(),
            })
            .await?;
        for binding in &project.bindings {
            state
                .db()
                .upsert_project_binding(&fxg_db::registration::ProjectBindingRecord {
                    project_id: project.project_id.clone(),
                    node_id: node_id.as_str().to_owned(),
                    local_path: binding.local_path.clone(),
                    is_worktree: binding.is_worktree,
                    path_exists: binding.path_exists,
                    git_branch: binding.git_branch.clone(),
                })
                .await?;
        }
    }
    Ok(())
}

/// Computes the resend targets for sessions that are missing or lagging on the hub side.
async fn compute_resync(
    state: &ServerState,
    node_id: &NodeId,
    sessions: &[fxg_protocol::node_server::SessionSyncState],
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
                tracing::warn!(
                    session_id = sync.session_id.as_str(),
                    hub_node = session.node_id.as_str(),
                    node_id = node_id.as_str(),
                    "session is owned by a different node; requesting full resync"
                );
                targets.push(ResyncTarget {
                    session_id: sync.session_id.clone(),
                    from_node_seq: 1,
                });
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
    state: &ServerState,
    node_id: &NodeId,
    events: Vec<fxg_protocol::events::SessionEventEnvelope>,
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

    // Web Push notification targets (newly applied approval requests)
    let mut notify_events: Vec<fxg_protocol::events::SessionEventEnvelope> = Vec::new();

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
                        if matches!(
                            event.payload,
                            fxg_protocol::events::UnifiedEventPayload::PermissionRequest { .. }
                        ) {
                            notify_events.push(event.clone());
                        }
                    }
                }
                if let Err(err) = state
                    .hub()
                    .send(
                        node_id,
                        ServerToNodeMsg::EventBatchAck {
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

    // Web Push notifications for approval requests and questions (elicitations)
    // (sent asynchronously so they do not block the ACK)
    if !notify_events.is_empty() {
        let push = state.push().clone();
        let db = state.db().clone();
        fxg_protocol::spawn_guarded!("web push notification", async move {
            crate::push::notify_permission_requests(&push, &db, &notify_events).await;
            crate::push::notify_elicitations(&push, &db, &notify_events).await;
        });
    }
}
