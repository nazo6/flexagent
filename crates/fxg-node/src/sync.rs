//! Outbox Sync Worker (Node ⇔ Central Server)。
//!
//! 設計: `docs/01-architecture-and-sync.md` §2.2、`docs/03-protocol-and-api.md` §2。
//!
//! - **Outbound WebSocket**: 中央サーバーの Node Hub (`/api/v1/node/ws`) へ
//!   `Authorization: Bearer <NODE_TOKEN>` で接続し、切断時は指数バックオフで再接続する
//! - **Store-and-Forward**: `NodeHello` (セッション同期状態) → サーバーが必要に応じて
//!   `ResyncRequest` → `synced_up_to_node_seq` より後のイベントを `EventBatchPush`
//!   でバッチ送信し、`EventBatchAck` で水位を更新する
//! - **リアルタイム配信**: 完成イベントは即時 `EventBatchPush`、ストリーミング
//!   差分は `LiveStreamDelta` として配信する (永続化はターン完了時のみ)
//! - **リモートコマンド実行**: `StartSession` / `SendPrompt` /
//!   `RespondPermission` / `ControlSession` / `ManageWorktree` / `GetGitDiff` /
//!   `Pty*` / `KillAllSessions` を実行し、`command_id` 相関で結果を返す

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use fxg_db::SessionFilter;
use fxg_protocol::client_api::WorktreeInfo;
use fxg_protocol::common::WorktreeAction;
use fxg_protocol::node_server::{NodeToServerMsg, ServerToNodeMsg, SessionSyncState};
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::daemon::{AuditSource, DaemonState, VERSION};
use crate::error::NodeError;
use crate::session::SessionBroadcast;
use crate::session_manager::StartSessionParams;

/// 再接続バックオフの初期値。
const RECONNECT_MIN: Duration = Duration::from_secs(1);
/// 再接続バックオフの上限。
const RECONNECT_MAX: Duration = Duration::from_secs(30);
/// `EventBatchPush` 1 メッセージあたりのイベント数上限。
const OUTBOX_BATCH_EVENTS: usize = 200;
/// 送信キュー容量 (PTY forwarder 含む)。
const SEND_QUEUE_CAPACITY: usize = 1024;
/// `NodeHello` で報告するセッション数の上限。
const HELLO_SESSION_LIMIT: u32 = 1000;
/// `WorkspaceBundleUpload` 1 メッセージあたりの Base64 チャンク上限 (8 MiB)。
pub const BUNDLE_CHUNK_B64_BYTES: usize = 8 * 1024 * 1024;

/// サーバーからのコマンド処理コンテキスト (トランスポート差の吸収)。
///
/// 常駐ノード (WS) は全フィールド既定値で、一時VM (`--stdio`) は
/// ワークスペース上書き・Drain でのプロセス終了シグナルを渡す。
#[derive(Default, Clone, Copy)]
pub(crate) struct CommandContext<'a> {
    /// `--stdio --workspace` の上書きディレクトリ
    /// (`StartSession.local_path` を一定のクローン先に固定する)
    pub workspace: Option<&'a Path>,
    /// 一時VMモードか (`DrainAndShutdown` で Git バンドルを退避する)
    pub ephemeral: bool,
    /// `DrainAndShutdown` 受信時にプロセス終了を要求するシグナル
    pub shutdown: Option<&'a watch::Sender<bool>>,
}

/// Outbox Sync Worker をシャットダウンまで稼働させる。
///
/// `NodeDaemon::start` からタスクとして起動される。
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

        // 指数バックオフ (シャットダウン要求で即時中断)
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

/// サーバーへ 1 接続分の同期を行う (接続が切れるまで戻らない)。
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

    // 送信キュー (PTY forwarder / コマンド応答の共通経路)
    let (out_tx, mut out_rx) = mpsc::channel::<NodeToServerMsg>(SEND_QUEUE_CAPACITY);
    // Git Credential Proxy (一時VMの GIT_ASKPASS) をこの接続に紐付ける
    state.credentials().attach(out_tx.clone());

    // 取りこぼし防止のため、flush より先にバスを購読する
    // (flush に含まれたイベントがライブ配信で重複しても、サーバー側の
    //  INSERT OR IGNORE と水位 ACK の MAX 更新で無害化される)
    let mut bus_rx = state.bus().subscribe();

    // 1. ハンドシェイク
    send_msg(&mut sender, &build_hello(state).await).await?;

    // 2. 未送信イベントのフラッシュ
    flush_outbox(state, &mut sender).await?;

    // 3. 双方向ループ
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
                // Worktree 変更等でプロジェクト紐付けを報告し直す
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
                // エフェメラルイベント (キーストローク等) は同期しない
                Ok(SessionBroadcast::Ephemeral(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    // 取りこぼしは Outbox (水位ベース) から回復する
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
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => {
                    return Ok(());
                }
                _ => {}
            },
        }
    }
}

/// `NodeHello` を組み立てる (ノード情報・プロジェクト・セッション同期状態)。
pub(crate) async fn build_hello(state: &DaemonState) -> NodeToServerMsg {
    let projects = match state.db().list_projects().await {
        Ok(projects) => projects
            .into_iter()
            .map(|project| fxg_protocol::common::NodeProjectReport {
                project_id: project.project_id.clone(),
                name: project.name,
                canonical_git_url: project.canonical_git_url,
                bindings: project
                    .bindings
                    .into_iter()
                    .filter(|binding| binding.node_id == state.node_id())
                    .map(|binding| fxg_protocol::common::ProjectNodeBinding {
                        local_path: binding.local_path,
                        is_worktree: binding.is_worktree,
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
        node_id: state.node_id().to_owned(),
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

/// 未送信イベント (`synced_up_to_node_seq` より後) を送信メッセージ列にする。
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

/// 未送信イベントを送信キューへ流す (stdio トランスポート用)。
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

/// 未送信イベント (`synced_up_to_node_seq` より後) をすべて送信する。
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

/// WS へ 1 メッセージ送信する。
async fn send_msg<S>(sender: &mut S, message: &NodeToServerMsg) -> anyhow::Result<()>
where
    S: futures_util::Sink<WsMessage> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    let text = serde_json::to_string(message)?;
    sender.send(WsMessage::Text(text.into())).await?;
    Ok(())
}

/// サーバーからのメッセージを処理する。
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
                .ack_watermark(&session_id, acked_up_to_node_seq)
                .await
            {
                tracing::warn!(session_id, "failed to update sync watermark: {err}");
            }
        }
        ServerToNodeMsg::ResyncRequest { sessions } => {
            for target in sessions {
                match state
                    .db()
                    .extract_outbox_after(&target.session_id, target.from_node_seq)
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
                            session_id = target.session_id,
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
            fork_context_messages,
            restore_git_bundle_b64,
        } => {
            // 一時VM (`--stdio --workspace`) はクローン済みディレクトリに固定する
            // (サーバーは VM 内パスを知らないため、要求の local_path は使わない)
            let local_path = ctx
                .workspace
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(&local_path));

            // 退避済み Git バンドルの復元 (別ノードへの引き継ぎ / 一時VM再開)
            let restore_bundle_file = match restore_git_bundle_b64.as_deref() {
                Some(b64) => match BASE64.decode(b64.as_bytes()) {
                    Ok(bytes) => match crate::bundle::write_bundle_file(
                        &state.paths().bundles_dir(),
                        &session_id,
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

            let result = state
                .session_manager()
                .start_session(StartSessionParams {
                    command_id: &command_id,
                    session_id: &session_id,
                    local_path: &local_path,
                    agent_id: &agent_id,
                    initial_prompt: initial_prompt.as_deref(),
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
                            Some(&session_id),
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
            text,
            client_source,
        } => {
            let result = state
                .session_manager()
                .send_prompt(&command_id, &session_id, &text, &client_source)
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
                    &session_id,
                    &request_id,
                    &selected_option_id,
                    &resolved_by,
                )
                .await
                .map(|()| session_id.clone());
            if result.is_ok() {
                state
                    .record_audit(
                        fxg_db::audit::actions::PERMISSION_RESOLVED,
                        &AuditSource::remote(state.node_id()),
                        Some(&session_id),
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
        ServerToNodeMsg::ControlSession {
            command_id,
            session_id,
            action,
        } => {
            let result = state
                .session_manager()
                .control(&command_id, &session_id, &action)
                .await
                .map(|()| session_id);
            send_command_result(out, command_id, result).await;
        }
        ServerToNodeMsg::PtySpawn {
            pty_id,
            session_id,
            cols,
            rows,
            shell_cmd,
        } => {
            handle_remote_pty_spawn(state, out, pty_id, session_id, cols, rows, shell_cmd).await;
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
            let result = match state.db().get_session(&session_id).await {
                Ok(Some(session)) => {
                    crate::git::workspace_diff(
                        Path::new(&session.local_path),
                        scope,
                        base_branch.as_deref(),
                    )
                    .await
                }
                Ok(None) => Err(NodeError::InvalidSession(session_id.clone())),
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
            let _ = out.send(response).await;
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
                } => match state.project_main_repo(&project_id).await {
                    Ok(repo) => {
                        match state
                            .worktree_add(
                                &repo,
                                Some(&project_id),
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
                                        node_id: state.node_id().to_owned(),
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
            };
            // Worktree 変更をハブへ報告する (project_node_bindings の更新)
            state.trigger_node_hello();
            let _ = out.send(response).await;
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

/// リモートからの PTY 起動要求を処理する (`allow_remote_pty` ポリシー適用)。
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
        let _ = out
            .send(NodeToServerMsg::PtyError {
                pty_id,
                code: "PTY_DISABLED".to_owned(),
                message: "Remote PTY is disabled on this node by security policy".to_owned(),
            })
            .await;
        return;
    }

    let session = match state.db().get_session(&session_id).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            let _ = out
                .send(NodeToServerMsg::PtyError {
                    pty_id,
                    code: "NOT_FOUND".to_owned(),
                    message: format!("session not found: {session_id}"),
                })
                .await;
            return;
        }
        Err(err) => {
            let _ = out
                .send(NodeToServerMsg::PtyError {
                    pty_id,
                    code: "INTERNAL".to_owned(),
                    message: err.to_string(),
                })
                .await;
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
        let _ = out
            .send(NodeToServerMsg::PtyError {
                pty_id,
                code: "INTERNAL".to_owned(),
                message: err.to_string(),
            })
            .await;
        return;
    }

    state
        .record_audit(
            fxg_db::audit::actions::PTY_SPAWN,
            &AuditSource::remote(state.node_id()),
            Some(&session_id),
            serde_json::json!({ "pty_id": pty_id }),
        )
        .await;

    // PTY 出力をサーバーへ転送する
    let events = match state.pty().subscribe(&pty_id) {
        Ok(events) => events,
        Err(err) => {
            let _ = out
                .send(NodeToServerMsg::PtyError {
                    pty_id,
                    code: "INTERNAL".to_owned(),
                    message: err.to_string(),
                })
                .await;
            return;
        }
    };
    let out = out.clone();
    tokio::spawn(async move {
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
                    let _ = out
                        .send(NodeToServerMsg::PtyExit { pty_id, exit_code })
                        .await;
                    break;
                }
                // 出力の取りこぼしは許容する (最新表示優先)
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(skipped, "remote pty forwarder lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// `DrainAndShutdown` を処理する (ベストエフォート)。
///
/// 1. 未送信イベントを全フラッシュ
/// 2. 一時VM (`ctx.ephemeral`) なら `git bundle` を作成して分割アップロード
/// 3. `DrainComplete` を送信してプロセス終了を要求
///
/// Drain 前にクラッシュした場合の中間イベント損失は許容仕様
/// (設計: docs/01 §6.4)。
async fn drain_and_shutdown(
    state: &DaemonState,
    out: &mpsc::Sender<NodeToServerMsg>,
    ctx: &CommandContext<'_>,
    reason: &str,
    create_git_bundle: bool,
) {
    tracing::info!(reason, create_git_bundle, "drain and shutdown requested");

    // 1. セッション / PTY を停止する (StatusChanged(Stopped) を記録する)
    if ctx.ephemeral
        && let Err(err) = state.kill_all_local(&format!("drain: {reason}")).await
    {
        tracing::warn!("failed to stop sessions on drain: {err}");
    }

    // 2. 未送信イベントを全フラッシュ
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

    let _ = out
        .send(NodeToServerMsg::DrainComplete {
            node_id: state.node_id().to_owned(),
        })
        .await;

    if let Some(shutdown) = ctx.shutdown {
        let _ = shutdown.send(true);
    } else {
        tracing::warn!("drain requested but no shutdown channel is attached");
    }
}

/// ワークスペースの Git バンドルを生成し、`WorkspaceBundleUpload`
/// として分割アップロードする。
///
/// セッションが 1 件も無い場合は `Ok(None)` (退避不要)。
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

    let index_path = state.paths().snapshot_index_path(&session.session_id);
    let bundle =
        crate::bundle::create_workspace_bundle(workspace, &index_path, &session.session_id).await?;

    let encoded = BASE64.encode(&bundle.bytes);
    let chunks: Vec<&str> = encoded
        .as_bytes()
        .chunks(BUNDLE_CHUNK_B64_BYTES)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
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
    Ok(Some(session.session_id.clone()))
}

/// `CommandResult` を組み立てて送信する。
async fn send_command_result(
    out: &mpsc::Sender<NodeToServerMsg>,
    command_id: String,
    result: Result<String, NodeError>,
) {
    let message = match result {
        Ok(session_id) => NodeToServerMsg::CommandResult {
            command_id,
            success: true,
            code: None,
            error: None,
            session_id: Some(session_id),
        },
        Err(err) => NodeToServerMsg::CommandResult {
            command_id,
            success: false,
            code: Some(err.error_code()),
            error: Some(err.to_string()),
            session_id: None,
        },
    };
    let _ = out.send(message).await;
}
