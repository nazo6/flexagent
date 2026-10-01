//! Client WebSocket (`/api/v1/client/ws`) の共通ハンドラ。
//!
//! 設計: `docs/03-protocol-and-api.md` §3.2。接続 → `Subscribe` →
//! カーソル以降のリプレイ → ライブ配信 (永続イベント / `LiveStreamDelta`) →
//! コマンドの `command_id` 相関応答、の流れをノード / 中央サーバーで共有する。

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use fxg_protocol::client_api::{ClientWsMessage, ServerWsMessage};
use fxg_protocol::common::ErrorCode;
use tokio::sync::broadcast;

use super::{ClientApiBackend, ClientCommand, ClientEvent, replay_events, send_ws_json};

/// WS リプレイの1バッチあたりの件数。
pub const REPLAY_BATCH_SIZE: u32 = 500;

/// Client WS の接続処理 (Subscribe → リプレイ → ライブ配信 + コマンド応答)。
pub(crate) async fn handle_client_ws<B: ClientApiBackend>(backend: B, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();

    // リプレイ完了までの間に発生したイベントも取りこぼさないよう先に購読する
    // (リプレイ済みのカーソル以下のイベントは転送時にスキップされる)。
    let mut subscription = backend.subscribe_client_events();
    let mut cursor: u64 = 0;
    let mut subscribed = false;

    /// select で待つ次のイベント。
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
                        // DB 再作成・リセット後はクライアントの保存済みカーソルが
                        // ストアの末尾を超える。そのままでは新ストアのイベント
                        // (cursor が小さい) を全て重複としてスキップしてしまうため、
                        // 0 から全量を再同期する (docs/03 §3.2)。
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
                        if replay_events(&backend, &mut sender, &mut cursor)
                            .await
                            .is_err()
                        {
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
                                code: ErrorCode::InvalidState,
                                message: format!("unsupported client message: {other:?}"),
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
                            code: result.code,
                            error: result.error,
                            session_id: result.session_id,
                        };
                        if send_ws_json(&mut sender, &response).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let response = ServerWsMessage::Error {
                            code: ErrorCode::InvalidState,
                            message: format!("invalid client message: {err}"),
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
                    // リプレイで送信済みのイベントはスキップする (重複排除)
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
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // 購読が遅延した場合はカーソルから履歴を取り直す
                    tracing::warn!(skipped, "client ws lagged; replaying from cursor");
                    if replay_events(&backend, &mut sender, &mut cursor)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}
