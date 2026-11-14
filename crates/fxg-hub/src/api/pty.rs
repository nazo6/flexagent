//! Shared handler for the bidirectional web PTY WebSocket (`/api/v1/pty/ws`).
//!
//! Design: `docs/03-protocol-and-api.md` §3.3 and
//! `docs/proposals/2026-11-14-terminal-workspace-scope.md`. Connects the
//! client's terminal to the ConPTY / Unix PTY of a workspace. A terminal is
//! scoped to `(node_id, path)`: opening the same workspace again attaches to the
//! running shell. Closing the socket detaches; the node keeps the shell until its
//! idle timeout expires.

use axum::extract::ws::{Message, WebSocket};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use fxg_protocol::NodeId;
use fxg_protocol::SessionId;
use fxg_protocol::client_api::{PtyClientMessage, PtyServerMessage};
use tokio::sync::broadcast;

use super::{ClientApiBackend, send_ws_json};

/// Broadcast capacity of a client PTY subscription.
pub const PTY_CHANNEL_CAPACITY: usize = 1024;

/// PTY channel specific error (the code is a string rather than an `ErrorCode`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyChannelError {
    /// Error code string (`FORBIDDEN` / `PTY_DISABLED` / `NOT_FOUND` /
    /// `NODE_OFFLINE` / `INVALID_STATE` / `INTERNAL`)
    pub code: String,
    /// Human-readable message
    pub message: String,
}

impl PtyChannelError {
    /// Builds an error with an arbitrary code.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// `FORBIDDEN` (policy rejection / unsupported operation)
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new("FORBIDDEN", message)
    }

    /// `PTY_DISABLED` (remote PTY disabled by node configuration)
    pub fn disabled(message: impl Into<String>) -> Self {
        Self::new("PTY_DISABLED", message)
    }

    /// `NOT_FOUND`
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("NOT_FOUND", message)
    }

    /// `NODE_OFFLINE` (target node disconnected during hub relay)
    pub fn offline(message: impl Into<String>) -> Self {
        Self::new("NODE_OFFLINE", message)
    }

    /// `INTERNAL`
    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::warn!("pty channel error: {err}");
        Self::new("INTERNAL", err.to_string())
    }

    /// Converts to the `PtyServerMessage::Error` sent to the client.
    pub fn to_message(&self) -> PtyServerMessage {
        PtyServerMessage::Error {
            code: self.code.clone(),
            message: self.message.clone(),
        }
    }
}

/// PTY event delivered to subscribers (shared by nodes / relay servers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyChannelEvent {
    /// Output chunk
    Output {
        /// Output bytes
        data: Vec<u8>,
        /// Monotonic chunk sequence number (0-based per PTY)
        seq: u64,
    },
    /// Process exit
    Exit {
        /// Exit code
        exit_code: Option<i32>,
    },
    /// Asynchronous failure notification (e.g. when a relay node returns `PtyError`).
    ///
    /// The receiving Client WS sends `PtyServerMessage::Error` and discards the PTY state.
    Error {
        /// Error code string
        code: String,
        /// Human-readable message
        message: String,
    },
}

/// PTY open parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyOpenParams {
    /// Proposed PTY ID (the node may reuse an existing terminal instead)
    pub pty_id: String,
    /// Node that owns the workspace
    pub node_id: NodeId,
    /// Workspace directory
    pub path: String,
    /// Session that opened the terminal (origin)
    pub session_id: Option<SessionId>,
    /// Number of columns
    pub cols: u16,
    /// Number of rows
    pub rows: u16,
    /// Replace a running terminal with a new shell
    pub new: bool,
    /// Whether the request arrives through the hub (applies `allow_remote_pty`)
    pub remote: bool,
}

/// Result of opening or attaching to a PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyOpenOutcome {
    /// Actual PTY ID
    pub pty_id: String,
    /// Canonical workspace path
    pub path: String,
    /// Whether an existing terminal was reused
    pub attached: bool,
    /// Number of columns
    pub cols: u16,
    /// Number of rows
    pub rows: u16,
}

/// PTY attached to this socket.
struct AttachedPty {
    pty_id: String,
}

/// Connection handler for the PTY WS.
pub(crate) async fn handle_pty_ws<B: ClientApiBackend>(backend: B, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let mut attached: Option<AttachedPty> = None;
    let mut output_rx: Option<broadcast::Receiver<PtyChannelEvent>> = None;

    /// The next event to wait for in `select`.
    enum Next {
        Client(Option<Result<Message, axum::Error>>),
        Pty(Result<PtyChannelEvent, broadcast::error::RecvError>),
    }

    loop {
        let next = match output_rx.as_mut() {
            Some(rx) => tokio::select! {
                incoming = receiver.next() => Next::Client(incoming),
                event = rx.recv() => Next::Pty(event),
            },
            None => Next::Client(receiver.next().await),
        };

        match next {
            Next::Client(None) | Next::Client(Some(Err(_))) => break,
            Next::Client(Some(Ok(message))) => match message {
                Message::Text(text) => match serde_json::from_str::<PtyClientMessage>(&text) {
                    Ok(PtyClientMessage::Open {
                        node_id,
                        path,
                        session_id,
                        cols,
                        rows,
                        new,
                    }) => {
                        if attached.is_some() {
                            if send_error(
                                &mut sender,
                                PtyChannelError::forbidden("already attached to a pty"),
                            )
                            .await
                            .is_err()
                            {
                                break;
                            }
                            continue;
                        }
                        let params = PtyOpenParams {
                            pty_id: fxg_protocol::util::uuid_v7(),
                            node_id,
                            path,
                            session_id,
                            cols,
                            rows,
                            new,
                            remote: false,
                        };
                        let outcome = match backend.pty_open(params).await {
                            Ok(outcome) => outcome,
                            Err(err) => {
                                if send_error(&mut sender, err).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        };
                        match attach_subscription(&backend, &outcome.pty_id).await {
                            Ok(rx) => {
                                output_rx = Some(rx);
                                attached = Some(AttachedPty {
                                    pty_id: outcome.pty_id.clone(),
                                });
                                if send_ws_json(
                                    &mut sender,
                                    &PtyServerMessage::Opened {
                                        pty_id: outcome.pty_id,
                                        path: outcome.path,
                                        attached: outcome.attached,
                                        cols: outcome.cols,
                                        rows: outcome.rows,
                                    },
                                )
                                .await
                                .is_err()
                                {
                                    break;
                                }
                            }
                            Err(err) => {
                                backend.pty_detach(&outcome.pty_id).await;
                                if send_error(&mut sender, err).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Ok(PtyClientMessage::Attach { pty_id }) => {
                        if attached.is_some() {
                            if send_error(
                                &mut sender,
                                PtyChannelError::forbidden("already attached to a pty"),
                            )
                            .await
                            .is_err()
                            {
                                break;
                            }
                            continue;
                        }
                        let outcome = match backend.pty_attach(&pty_id).await {
                            Ok(outcome) => outcome,
                            Err(err) => {
                                if send_error(&mut sender, err).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        };
                        match attach_subscription(&backend, &outcome.pty_id).await {
                            Ok(rx) => {
                                output_rx = Some(rx);
                                attached = Some(AttachedPty {
                                    pty_id: outcome.pty_id.clone(),
                                });
                                if send_ws_json(
                                    &mut sender,
                                    &PtyServerMessage::Opened {
                                        pty_id: outcome.pty_id,
                                        path: outcome.path,
                                        attached: true,
                                        cols: outcome.cols,
                                        rows: outcome.rows,
                                    },
                                )
                                .await
                                .is_err()
                                {
                                    break;
                                }
                            }
                            Err(err) => {
                                backend.pty_detach(&outcome.pty_id).await;
                                if send_error(&mut sender, err).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Ok(PtyClientMessage::Input { data_b64 }) => {
                        let Some(current) = &attached else {
                            if send_error(
                                &mut sender,
                                PtyChannelError::new("INVALID_STATE", "no pty attached"),
                            )
                            .await
                            .is_err()
                            {
                                break;
                            }
                            continue;
                        };
                        let data = match BASE64.decode(data_b64.as_bytes()) {
                            Ok(data) => data,
                            Err(err) => {
                                if send_error(
                                    &mut sender,
                                    PtyChannelError::new(
                                        "INVALID_STATE",
                                        format!("invalid base64 input: {err}"),
                                    ),
                                )
                                .await
                                .is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                        };
                        if let Err(err) = backend.pty_write(&current.pty_id, &data).await
                            && send_error(&mut sender, err).await.is_err()
                        {
                            break;
                        }
                    }
                    Ok(PtyClientMessage::Resize { cols, rows }) => {
                        let Some(current) = &attached else {
                            if send_error(
                                &mut sender,
                                PtyChannelError::new("INVALID_STATE", "no pty attached"),
                            )
                            .await
                            .is_err()
                            {
                                break;
                            }
                            continue;
                        };
                        if let Err(err) = backend.pty_resize(&current.pty_id, cols, rows).await
                            && send_error(&mut sender, err).await.is_err()
                        {
                            break;
                        }
                    }
                    Ok(PtyClientMessage::Kill) => {
                        if let Some(current) = &attached
                            && let Err(err) = backend.pty_kill(&current.pty_id).await
                            && send_error(&mut sender, err).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(err) => {
                        if send_error(
                            &mut sender,
                            PtyChannelError::new(
                                "INVALID_STATE",
                                format!("invalid pty message: {err}"),
                            ),
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                },
                Message::Ping(payload) => {
                    if sender.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            },
            Next::Pty(event) => match event {
                Ok(PtyChannelEvent::Output { data, seq }) => {
                    let message = PtyServerMessage::Output {
                        data_b64: BASE64.encode(data),
                        seq,
                    };
                    if send_ws_json(&mut sender, &message).await.is_err() {
                        break;
                    }
                }
                Ok(PtyChannelEvent::Exit { exit_code }) => {
                    if send_ws_json(&mut sender, &PtyServerMessage::Exit { exit_code })
                        .await
                        .is_err()
                    {
                        break;
                    }
                    // The PTY has exited (subsequent input is ignored). Keep the socket open and
                    // allow new open / attach requests.
                    attached = None;
                    output_rx = None;
                }
                Ok(PtyChannelEvent::Error { code, message }) => {
                    if send_error(&mut sender, PtyChannelError::new(code, message))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    attached = None;
                    output_rx = None;
                }
                // Dropped output is tolerated (the PTY stream favors the latest view)
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(skipped, "pty output lagged");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    attached = None;
                    output_rx = None;
                }
            },
        }
    }

    // Closing the socket only detaches: the shell keeps running (and keeps its
    // scrollback) until the idle timeout expires.
    if let Some(current) = attached {
        backend.pty_detach(&current.pty_id).await;
    }
}

/// Subscribes to a PTY and fails when the PTY disappeared in between.
async fn attach_subscription<B: ClientApiBackend>(
    backend: &B,
    pty_id: &str,
) -> Result<broadcast::Receiver<PtyChannelEvent>, PtyChannelError> {
    backend.pty_subscribe(pty_id).await
}

/// Wraps a live subscription so the retained output is delivered first.
///
/// The caller must have subscribed to the live stream *before* taking the
/// snapshot. Chunks with `seq < next_seq` are already included in `data` and are
/// dropped.
pub fn subscribe_with_replay(
    mut live: broadcast::Receiver<PtyChannelEvent>,
    data: Vec<u8>,
    next_seq: u64,
) -> broadcast::Receiver<PtyChannelEvent> {
    let (tx, rx) = broadcast::channel(PTY_CHANNEL_CAPACITY);
    if !data.is_empty() {
        fxg_protocol::discard_with!(
            tx.send(PtyChannelEvent::Output {
                data,
                seq: next_seq.saturating_sub(1),
            }),
            "deliver retained pty output"
        );
    }
    fxg_protocol::spawn_guarded!("pty replay forwarder", async move {
        loop {
            match live.recv().await {
                Ok(PtyChannelEvent::Output { data, seq }) => {
                    if seq < next_seq {
                        continue;
                    }
                    if tx.send(PtyChannelEvent::Output { data, seq }).is_err() {
                        break;
                    }
                }
                Ok(event) => {
                    if tx.send(event).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(skipped, "pty replay forwarder lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    rx
}

/// Sends an error.
async fn send_error(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    error: PtyChannelError,
) -> Result<(), ()> {
    send_ws_json(sender, &error.to_message()).await
}
