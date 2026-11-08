//! Shared handler for the Client WebSocket (`/api/v1/client/ws`).
//!
//! Design: `docs/03-protocol-and-api.md` §3.2. The flow connect → `Subscribe` → replay from the
//! cursor → live delivery (persisted events / `LiveStreamDelta`) → `command_id`-correlated
//! command responses is shared between the node and the central server.

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use fxg_protocol::client_api::{ClientWsMessage, ServerWsMessage};
use fxg_protocol::common::{ErrorCode, ErrorInfo};
use tokio::sync::broadcast;

use super::{ClientApiBackend, ClientCommand, ClientEvent, replay_events, send_ws_json};

/// Number of events per WS replay batch.
pub const REPLAY_BATCH_SIZE: u32 = 500;

/// Connection handler for Client WS (Subscribe → replay → live delivery + command responses).
pub(crate) async fn handle_client_ws<B: ClientApiBackend>(backend: B, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe first so events produced during the replay are not missed
    // (events at or below the already replayed cursor are skipped on forwarding).
    let mut subscription = backend.subscribe_client_events();
    let mut cursor: u64 = 0;
    let mut subscribed = false;

    /// The next event to wait for in `select`.
    enum Next {
        Client(Option<Result<Message, axum::Error>>),
        Bus(Box<Result<ClientEvent, broadcast::error::RecvError>>),
    }

    loop {
        let next = if subscribed {
            tokio::select! {
                incoming = receiver.next() => Next::Client(incoming),
                event = subscription.recv() => Next::Bus(Box::new(event)),
            }
        } else {
            Next::Client(receiver.next().await)
        };

        match next {
            Next::Client(None) | Next::Client(Some(Err(_))) => break,
            Next::Client(Some(Ok(message))) => match message {
                Message::Text(text) => match serde_json::from_str::<ClientWsMessage>(&text) {
                    Ok(ClientWsMessage::Subscribe { since_cursor, .. }) => {
                        let requested = since_cursor.unwrap_or(0);
                        // After the DB is recreated or reset, the client's stored cursor exceeds
                        // the store's end. That would cause all events in the new store (with
                        // smaller cursors) to be skipped as duplicates, so resync everything from
                        // 0 (docs/03 §3.2).
                        cursor = match backend.db().latest_cursor().await {
                            Ok(latest) if requested > latest => {
                                tracing::warn!(
                                    requested,
                                    latest,
                                    "client cursor is ahead of the store; replaying from 0"
                                );
                                0
                            }
                            Ok(_) => requested,
                            Err(err) => {
                                tracing::warn!("failed to load latest cursor: {err}");
                                requested
                            }
                        };
                        if let Err(message) =
                            replay_events(&backend, &mut sender, &mut cursor).await
                        {
                            let error = ServerWsMessage::Error {
                                error: ErrorInfo::new(ErrorCode::SyncBacklog, message),
                            };
                            let _ = send_ws_json(&mut sender, &error).await;
                            break;
                        }
                        subscribed = true;
                    }
                    Ok(ClientWsMessage::Ping) => {
                        if send_ws_json(&mut sender, &ServerWsMessage::Pong)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(other) => {
                        let Some(command) = ClientCommand::from_message(&other) else {
                            let response = ServerWsMessage::Error {
                                error: ErrorInfo::new(
                                    ErrorCode::InvalidState,
                                    format!("unsupported client message: {other:?}"),
                                ),
                            };
                            if send_ws_json(&mut sender, &response).await.is_err() {
                                break;
                            }
                            continue;
                        };
                        let result = backend.dispatch_command(command).await;
                        let response = ServerWsMessage::CommandResult {
                            command_id: result.command_id,
                            success: result.success,
                            error: result.error,
                            session_id: result.session_id,
                        };
                        if send_ws_json(&mut sender, &response).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let response = ServerWsMessage::Error {
                            error: ErrorInfo::new(
                                ErrorCode::InvalidState,
                                format!("invalid client message: {err}"),
                            ),
                        };
                        if send_ws_json(&mut sender, &response).await.is_err() {
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
            Next::Bus(event) => match *event {
                Ok(ClientEvent::Persisted {
                    event,
                    cursor: event_cursor,
                }) => {
                    // Skip events already sent during replay (deduplication)
                    if event_cursor <= cursor {
                        continue;
                    }
                    cursor = event_cursor;
                    if send_ws_json(
                        &mut sender,
                        &ServerWsMessage::EventBatch {
                            events: vec![*event],
                            cursor: event_cursor,
                        },
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                }
                Ok(ClientEvent::StreamDelta { session_id, delta }) => {
                    if send_ws_json(
                        &mut sender,
                        &ServerWsMessage::LiveStreamDelta { session_id, delta },
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                }
                Ok(ClientEvent::BootstrapLog { session_id, line }) => {
                    // Temporary VM bootstrap log (ephemeral; not persisted)
                    if send_ws_json(
                        &mut sender,
                        &ServerWsMessage::BootstrapLog { session_id, line },
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // When the subscription lags, fetch history again from the cursor
                    tracing::warn!(skipped, "client ws lagged; replaying from cursor");
                    if let Err(message) = replay_events(&backend, &mut sender, &mut cursor).await {
                        let error = ServerWsMessage::Error {
                            error: ErrorInfo::new(ErrorCode::SyncBacklog, message),
                        };
                        let _ = send_ws_json(&mut sender, &error).await;
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}
