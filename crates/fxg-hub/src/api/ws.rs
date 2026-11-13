//! Shared handler for the Client WebSocket (`/api/v1/client/ws`).
//!
//! Design: `docs/03-protocol-and-api.md` §3.2. The flow connect → `Subscribe` → replay from the
//! cursor → live delivery (persisted events / `LiveStreamDelta`) → `command_id`-correlated
//! command responses is shared between the node and the hub.

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use fxg_protocol::client_api::{ClientWsMessage, HubLinkStatus, HubWsMessage};
use fxg_protocol::common::{ErrorCode, ErrorInfo, IncidentLevel, IncidentSource};
use fxg_protocol::{ClientId, TabId};
use tokio::sync::{broadcast, watch};

use super::{ClientApiBackend, ClientCommand, ClientEvent, replay_events, send_ws_json};
use crate::presence::ClientPresence;

/// Number of events per WS replay batch.
pub const REPLAY_BATCH_SIZE: u32 = 500;

/// Connection handler for Client WS (Subscribe → replay → live delivery + command responses).
pub(crate) async fn handle_client_ws<B: ClientApiBackend>(backend: B, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe first so events produced during the replay are not missed
    // (events at or below the already replayed cursor are skipped on forwarding).
    let mut subscription = backend.subscribe_client_events();
    let mut hub_link_rx = backend.hub_link_updates();
    let mut cursor: u64 = 0;
    let mut subscribed = false;
    // Last presence key of this connection, removed when the WS closes.
    let mut presence_key: Option<(ClientId, TabId)> = None;

    /// The next event to wait for in `select`.
    enum Next {
        Client(Option<Result<Message, axum::Error>>),
        Bus(Box<Result<ClientEvent, broadcast::error::RecvError>>),
        HubLink(HubLinkStatus),
    }

    loop {
        let next = if subscribed {
            tokio::select! {
                incoming = receiver.next() => Next::Client(incoming),
                event = subscription.recv() => Next::Bus(Box::new(event)),
                status = next_hub_link(&mut hub_link_rx) => Next::HubLink(status),
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
                        let counters = backend.diagnostics_counters();
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
                            if let Some(counters) = counters {
                                counters.incr(super::CounterKind::ReplayFailures);
                            }
                            let error = ErrorInfo::new(ErrorCode::SyncBacklog, message);
                            super::diagnostics::record_info(
                                &backend,
                                &error,
                                IncidentSource::Sync,
                                IncidentLevel::Error,
                                None,
                                None,
                            )
                            .await;
                            let error = HubWsMessage::Error { error };
                            fxg_protocol::discard_with!(
                                send_ws_json(&mut sender, &error).await,
                                "notify the client that replay failed"
                            );
                            break;
                        }
                        subscribed = true;
                    }
                    Ok(ClientWsMessage::Presence {
                        client_id,
                        tab_id,
                        visible,
                        focused,
                        focused_session_id,
                    }) => {
                        let key = (client_id.clone(), tab_id.clone());
                        if presence_key.as_ref() != Some(&key)
                            && let Some((old_client_id, old_tab_id)) = presence_key.take()
                        {
                            backend.clear_client_presence(&old_client_id, &old_tab_id);
                        }
                        backend.record_client_presence(ClientPresence {
                            client_id,
                            tab_id,
                            visible,
                            focused,
                            focused_session_id,
                        });
                        presence_key = Some(key);
                    }
                    Ok(ClientWsMessage::Ping) => {
                        if send_ws_json(&mut sender, &HubWsMessage::Pong)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(other) => {
                        let Some(command) = ClientCommand::from_message(&other) else {
                            let response = HubWsMessage::Error {
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
                        super::diagnostics::record_command_result(&backend, &result).await;
                        let response = HubWsMessage::CommandResult {
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
                        let response = HubWsMessage::Error {
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
                        &HubWsMessage::EventBatch {
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
                        &HubWsMessage::LiveStreamDelta { session_id, delta },
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                }
                Ok(ClientEvent::ProvisioningLog { session_id, line }) => {
                    // Temporary VM bootstrap log (ephemeral; not persisted)
                    if send_ws_json(
                        &mut sender,
                        &HubWsMessage::ProvisioningLog { session_id, line },
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // When the subscription lags, fetch history again from the cursor
                    if let Some(counters) = backend.diagnostics_counters() {
                        counters.add(super::CounterKind::DroppedEvents, skipped);
                        counters.incr(super::CounterKind::LaggedBroadcasts);
                    }
                    tracing::warn!(skipped, "client ws lagged; replaying from cursor");
                    if let Err(message) = replay_events(&backend, &mut sender, &mut cursor).await {
                        if let Some(counters) = backend.diagnostics_counters() {
                            counters.incr(super::CounterKind::ReplayFailures);
                        }
                        let error = ErrorInfo::new(ErrorCode::SyncBacklog, message);
                        super::diagnostics::record_info(
                            &backend,
                            &error,
                            IncidentSource::Sync,
                            IncidentLevel::Error,
                            None,
                            None,
                        )
                        .await;
                        let error = HubWsMessage::Error { error };
                        fxg_protocol::discard_with!(
                            send_ws_json(&mut sender, &error).await,
                            "notify the client that replay failed"
                        );
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            Next::HubLink(status) => {
                let (unsynced_event_count, last_synced_at) = match backend.system_extras().await {
                    Ok(extras) => (
                        extras.unsynced_event_count.unwrap_or(0),
                        extras.last_synced_at,
                    ),
                    Err(err) => {
                        tracing::warn!(message = %err.message, "failed to read system extras");
                        (0, None)
                    }
                };
                let message = HubWsMessage::SystemStatus {
                    hub_link: status,
                    unsynced_event_count,
                    last_synced_at,
                };
                if send_ws_json(&mut sender, &message).await.is_err() {
                    break;
                }
            }
        }
    }

    if let Some((client_id, tab_id)) = presence_key {
        backend.clear_client_presence(&client_id, &tab_id);
    }
}

/// Waits for the next node ⇔ hub link change (never resolves on the hub).
async fn next_hub_link(rx: &mut Option<watch::Receiver<HubLinkStatus>>) -> HubLinkStatus {
    match rx {
        Some(receiver) => match receiver.changed().await {
            Ok(()) => receiver.borrow_and_update().clone(),
            // The sender is gone; keep the arm waiting instead of spinning.
            Err(_) => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}
