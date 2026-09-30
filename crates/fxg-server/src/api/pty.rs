//! 双方向 Web PTY WebSocket (`/api/v1/pty/ws`) の共通ハンドラ。
//!
//! 設計: `docs/03-protocol-and-api.md` §3.3。クライアントのターミナルと
//! ノード上の ConPTY / Unix PTY を直接結ぶ。ノードは `PtySessionManager` へ
//! 直結し、中央サーバーは `PtySpawn` / `PtyInput` / `PtyResize` / `PtyKill` を
//! 対象ノードへ中継する (`allow_remote_pty=false` のノードは起動を拒否する)。

use axum::extract::ws::{Message, WebSocket};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use fxg_protocol::client_api::{PtyClientMessage, PtyServerMessage};
use tokio::sync::broadcast;

use super::{ClientApiBackend, send_ws_json};

/// PTY チャネル固有のエラー (コードは `ErrorCode` ではなく文字列)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyChannelError {
    /// エラーコード文字列 (`FORBIDDEN` / `PTY_DISABLED` / `NOT_FOUND` /
    /// `NODE_OFFLINE` / `INVALID_STATE` / `INTERNAL`)
    pub code: String,
    /// 人間向けメッセージ
    pub message: String,
}

impl PtyChannelError {
    /// 任意のコードで生成する。
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// `FORBIDDEN` (ポリシー拒否・対象外操作)
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new("FORBIDDEN", message)
    }

    /// `PTY_DISABLED` (リモート PTY がノード設定で無効)
    pub fn disabled(message: impl Into<String>) -> Self {
        Self::new("PTY_DISABLED", message)
    }

    /// `NOT_FOUND`
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("NOT_FOUND", message)
    }

    /// `NODE_OFFLINE` (中央サーバー中継で対象ノードが切断中)
    pub fn offline(message: impl Into<String>) -> Self {
        Self::new("NODE_OFFLINE", message)
    }

    /// `INTERNAL`
    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::warn!("pty channel error: {err}");
        Self::new("INTERNAL", err.to_string())
    }

    /// Client へ送る `PtyServerMessage::Error` に変換する。
    pub fn to_message(&self) -> PtyServerMessage {
        PtyServerMessage::Error {
            code: self.code.clone(),
            message: self.message.clone(),
        }
    }
}

/// 購読者へ配信される PTY イベント (ノード / 中継サーバー共通)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyChannelEvent {
    /// 出力チャンク
    Output {
        /// 出力バイト列
        data: Vec<u8>,
    },
    /// プロセス終了
    Exit {
        /// 終了コード
        exit_code: Option<i32>,
    },
}

/// PTY 起動パラメータ (`pty_id` は呼び出し側が採番)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtySpawnParams {
    /// PTY ID
    pub pty_id: String,
    /// 対象セッションID (作業ディレクトリ解決用)
    pub session_id: String,
    /// 列数
    pub cols: u16,
    /// 行数
    pub rows: u16,
    /// シェルコマンド (省略時は既定シェル)
    pub shell_cmd: Option<String>,
}

/// このソケットがアタッチしている PTY。
struct AttachedPty {
    pty_id: String,
    /// このソケットが spawn した PTY (`false` は既存への attach)。
    /// spawn した PTY はソケット切断時に終了する。
    spawned: bool,
}

/// PTY WS の接続処理。
pub(crate) async fn handle_pty_ws<B: ClientApiBackend>(backend: B, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let mut attached: Option<AttachedPty> = None;
    let mut output_rx: Option<broadcast::Receiver<PtyChannelEvent>> = None;

    /// select で待つ次のイベント。
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
                            session_id,
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
                            Ok(()) => match backend.pty_subscribe(&pty_id) {
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
                    // PTY は終了済み (以降の入力は無効)。ソケットは開いたまま
                    // 新規 spawn / attach を許可する。
                    attached = None;
                    output_rx = None;
                }
                // 出力の取りこぼしは許容する (PTY ストリームは最新表示優先)
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

    // このソケットが起動した PTY は切断時に終了する (ゾンビ防止)。
    // attach した既存 PTY はデタッチのみで残す。
    if let Some(current) = attached
        && current.spawned
    {
        backend.pty_disconnect(&current.pty_id).await;
    }
}

/// PTY spawn → 購読確立の結果。
enum SpawnOutcome {
    /// 購読を確立した
    Ready(broadcast::Receiver<PtyChannelEvent>),
    /// エラーを送信済み (ループは継続)
    Failed,
    /// ソケット送信に失敗 (ループ終了)
    Closed,
}

/// PTY を spawn して購読を確立し、`Spawned` を送信する。
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
    match backend.pty_subscribe(&pty_id) {
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
                // 正常に spawn 済みだが通知できないため終了する
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

/// エラーを送信する。
async fn send_error(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    error: PtyChannelError,
) -> Result<(), ()> {
    send_ws_json(sender, &error.to_message()).await
}
