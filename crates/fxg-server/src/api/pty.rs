//! Shared handler for the bidirectional web PTY WebSocket (`/api/v1/pty/ws`).
//!
//! Design: `docs/03-protocol-and-api.md` §3.3. Connects the client's terminal directly to a
//! ConPTY / Unix PTY on the node. The node talks straight to `PtySessionManager`, while the
//! central server relays `PtySpawn` / `PtyInput` / `PtyResize` / `PtyKill` to the target node
//! (nodes with `allow_remote_pty=false` refuse to spawn).

use axum::extract::ws::{Message, WebSocket};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use fxg_protocol::client_api::{PtyClientMessage, PtyServerMessage};
use tokio::sync::broadcast;

use super::{ClientApiBackend, send_ws_json};

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

    /// `NODE_OFFLINE` (target node disconnected during central server relay)
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

/// PTY spawn parameters (`pty_id` is assigned by the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtySpawnParams {
    /// PTY ID
    pub pty_id: String,
    /// Target session ID (used to resolve the working directory)
    pub session_id: String,
    /// Number of columns
    pub cols: u16,
    /// Number of rows
    pub rows: u16,
    /// Shell command (default shell when omitted)
    pub shell_cmd: Option<String>,
}

/// PTY attached to this socket.
struct AttachedPty {
    pty_id: String,
    /// Whether this socket spawned the PTY (`false` means attached to an existing one).
    /// Spawned PTYs are killed when the socket disconnects.
    spawned: bool,
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
                    Ok(PtyClientMessage::Spawn {
                        session_id,
                        cols,
                        rows,
                        shell_cmd,
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
                        let pty_id = fxg_protocol::util::uuid_v7();
                        let params = PtySpawnParams {
                            pty_id: pty_id.clone(),
                            session_id: session_id.to_string(),
                            cols,
                            rows,
                            shell_cmd,
                        };
                        match spawn_and_subscribe(&backend, &mut sender, params).await {
                            SpawnOutcome::Ready(rx) => {
                                output_rx = Some(rx);
                                attached = Some(AttachedPty {
                                    pty_id,
                                    spawned: true,
                                });
                            }
                            SpawnOutcome::Failed => {}
                            SpawnOutcome::Closed => break,
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
                        match backend.pty_attach(&pty_id).await {
                            Ok(()) => match backend.pty_subscribe(&pty_id).await {
                                Ok(rx) => {
                                    output_rx = Some(rx);
                                    attached = Some(AttachedPty {
                                        pty_id: pty_id.clone(),
                                        spawned: false,
                                    });
                                    if send_ws_json(
                                        &mut sender,
                                        &PtyServerMessage::Spawned { pty_id },
                                    )
                                    .await
                                    .is_err()
                                    {
                                        break;
                                    }
                                }
                                Err(err) => {
                                    if send_error(&mut sender, err).await.is_err() {
                                        break;
                                    }
                                }
                            },
                            Err(err) => {
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
                Ok(PtyChannelEvent::Output { data }) => {
                    let message = PtyServerMessage::Output {
                        data_b64: BASE64.encode(data),
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
                    // allow new spawn / attach requests.
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

    // PTYs spawned by this socket are killed on disconnect (prevents zombies).
    // Existing attached PTYs are merely detached and left running.
    if let Some(current) = attached
        && current.spawned
    {
        backend.pty_disconnect(&current.pty_id).await;
    }
}

/// Result of PTY spawn → subscription setup.
enum SpawnOutcome {
    /// Subscription established
    Ready(broadcast::Receiver<PtyChannelEvent>),
    /// Error already sent (the loop continues)
    Failed,
    /// Socket send failed (the loop ends)
    Closed,
}

/// Spawns a PTY, establishes the subscription and sends `Spawned`.
async fn spawn_and_subscribe<B: ClientApiBackend>(
    backend: &B,
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    params: PtySpawnParams,
) -> SpawnOutcome {
    let pty_id = params.pty_id.clone();
    if let Err(err) = backend.pty_spawn(params).await {
        return if send_error(sender, err).await.is_err() {
            SpawnOutcome::Closed
        } else {
            SpawnOutcome::Failed
        };
    }
    match backend.pty_subscribe(&pty_id).await {
        Ok(rx) => {
            if send_ws_json(
                sender,
                &PtyServerMessage::Spawned {
                    pty_id: pty_id.clone(),
                },
            )
            .await
            .is_err()
            {
                // Spawned successfully but cannot notify, so terminate
                backend.pty_disconnect(&pty_id).await;
                return SpawnOutcome::Closed;
            }
            SpawnOutcome::Ready(rx)
        }
        Err(err) => {
            backend.pty_disconnect(&pty_id).await;
            if send_error(sender, err).await.is_err() {
                SpawnOutcome::Closed
            } else {
                SpawnOutcome::Failed
            }
        }
    }
}

/// Sends an error.
async fn send_error(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    error: PtyChannelError,
) -> Result<(), ()> {
    send_ws_json(sender, &error.to_message()).await
}
