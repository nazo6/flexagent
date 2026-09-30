//! CLI (`fxg`) ⇔ Daemon のローカルIPCサーバー。
//!
//! - **トランスポート**: Windows は Named Pipe (`\\.\pipe\fxg-daemon-<username>`)、
//!   Linux / macOS / WSL は Unix Domain Socket (`$XDG_RUNTIME_DIR/fxg/daemon.sock`
//!   または `/tmp/fxg-<uid>/daemon.sock`)
//! - **フレーミング**: Length-prefixed JSON
//!   (4バイト LE 長さヘッダ + JSON ペイロード)
//!
//! 設計: `docs/03-protocol-and-api.md` §4。
//!
//! - 通常コマンドは 1 リクエスト / 1 レスポンスで処理する。
//! - [`IpcClientMessage::AttachSession`] を受けた接続はイベントストリーミング
//!   モードへ切り替わり、`EventBatch` / `LiveStreamDelta` をプッシュ配信しつつ、
//!   同一接続上のコマンド (`SendPrompt` 等) にも応答する (dual-loop)。

use std::path::{Path, PathBuf};

use fxg_db::SessionFilter;
use fxg_protocol::common::ErrorCode;
use fxg_protocol::config::{GlobalConfig, ProjectConfig};
use fxg_protocol::events::SessionEventEnvelope;
use fxg_protocol::ipc::{
    IpcClientMessage, IpcResult, IpcServerMessage, ProjectInfo, SessionDetail,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{broadcast, watch};

use super::DaemonState;
use crate::error::NodeError;
use crate::ipc_framing::{read_frame, write_message};
use crate::session::SessionBroadcast;
use crate::{git, project, worktree};

/// `fxg ps` などで返すセッション一覧の上限。
const SESSION_LIST_LIMIT: u32 = 200;

/// `AttachSession` のリプレイ時に 1 バッチで送るイベント数の上限。
const ATTACH_REPLAY_BATCH: u32 = 200;

/// プロジェクトスキャンの再帰深さ上限。
const SCAN_MAX_DEPTH: usize = 3;

/// IPC エンドポイントが他デーモンに占有されていないか確認する。
///
/// 接続可能なソケットが残っている場合は別のデーモンが稼働中と見なしエラーにする。
/// 接続できない残骸ソケットは削除する。
#[cfg(unix)]
pub async fn check_endpoint_available(endpoint: &str) -> Result<(), NodeError> {
    let path = Path::new(endpoint);
    if !path.exists() {
        return Ok(());
    }
    if tokio::net::UnixStream::connect(path).await.is_ok() {
        return Err(NodeError::Server(format!(
            "another daemon is already listening on {endpoint}"
        )));
    }
    std::fs::remove_file(path).map_err(|err| NodeError::io(path, err))?;
    Ok(())
}

/// IPC エンドポイントが他デーモンに占有されていないか確認する (Windows: Named Pipe)。
#[cfg(windows)]
pub async fn check_endpoint_available(endpoint: &str) -> Result<(), NodeError> {
    use tokio::net::windows::named_pipe::ClientOptions;

    match ClientOptions::new().open(endpoint) {
        Ok(_) => Err(NodeError::Server(format!(
            "another daemon is already listening on {endpoint}"
        ))),
        // ERROR_PIPE_BUSY (231): 稼働中だが全インスタンスが接続処理中
        Err(err) if err.raw_os_error() == Some(crate::error::PIPE_BUSY) => Err(NodeError::Server(
            format!("another daemon is already listening on {endpoint}"),
        )),
        // ERROR_FILE_NOT_FOUND (2) なら未起動。その他のエラーは bind 時に検出する
        Err(_) => Ok(()),
    }
}

/// ローカルIPCサーバーを起動し、シャットダウンまで接続を受け付ける。
#[cfg(unix)]
pub async fn serve(
    state: DaemonState,
    endpoint: String,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), NodeError> {
    let path = PathBuf::from(&endpoint);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| NodeError::io(parent, err))?;
    }

    let listener = tokio::net::UnixListener::bind(&path)
        .map_err(|err| NodeError::Server(format!("failed to bind {endpoint}: {err}")))?;
    tracing::info!(endpoint = %endpoint, "local ipc server listening (unix socket)");

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        let state = state.clone();
                        let connection_shutdown = shutdown.clone();
                        tokio::spawn(async move {
                            if let Err(err) = handle_connection(state, stream, connection_shutdown).await {
                                tracing::debug!("ipc connection ended: {err}");
                            }
                        });
                    }
                    Err(err) => tracing::warn!("ipc accept failed: {err}"),
                }
            }
        }
    }

    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// ローカルIPCサーバーを起動する (Windows: Named Pipe)。
#[cfg(windows)]
pub async fn serve(
    state: DaemonState,
    endpoint: String,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), NodeError> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let create_instance = |first: bool| {
        let mut options = ServerOptions::new();
        options.first_pipe_instance(first);
        options.create(&endpoint).map_err(|err| {
            NodeError::Server(format!("failed to create named pipe {endpoint}: {err}"))
        })
    };

    tracing::info!(endpoint = %endpoint, "local ipc server listening (named pipe)");

    // 接続の合間に listening インスタンスが消えると、クライアントが
    // ERROR_PIPE_BUSY (231) を受け取る (CI: windows-latest で検出)。
    // 次のインスタンスを先に用意してから現行インスタンスの接続を待つ。
    let mut server = create_instance(false)?;
    loop {
        let next = create_instance(false)?;
        tokio::select! {
            _ = shutdown.changed() => break,
            connected = server.connect() => {
                match connected {
                    Ok(()) => {
                        let state = state.clone();
                        let connection_shutdown = shutdown.clone();
                        tokio::spawn(async move {
                            if let Err(err) = handle_connection(state, server, connection_shutdown).await {
                                tracing::debug!("ipc connection ended: {err}");
                            }
                        });
                        server = next;
                    }
                    Err(err) => tracing::warn!("named pipe connect failed: {err}"),
                }
            }
        }
    }
    Ok(())
}

/// 1接続を処理する (Length-prefixed JSON のリクエスト/レスポンスループ)。
///
/// シャットダウン要求を受けると接続を閉じる。
///
/// [`IpcClientMessage::AttachSession`] を受けると
/// [`attach_session`] へ移行し、イベント配信とコマンド応答を並行して行う。
pub(crate) async fn handle_connection<S>(
    state: DaemonState,
    stream: S,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    loop {
        let frame = tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            frame = read_frame(&mut reader) => frame?,
        };
        let Some(frame) = frame else {
            return Ok(()); // クライアントが切断した
        };
        let request: IpcClientMessage = match serde_json::from_slice(&frame) {
            Ok(request) => request,
            Err(err) => {
                let response = IpcServerMessage::Error {
                    command_id: None,
                    code: ErrorCode::InvalidState,
                    message: format!("invalid ipc message: {err}"),
                };
                write_message(&mut writer, &response).await?;
                continue;
            }
        };

        // AttachSession はこの接続をストリーミングモードへ切り替える
        if let IpcClientMessage::AttachSession {
            command_id,
            session_id,
            after_node_seq,
        } = request
        {
            attach_session(
                &state,
                &mut reader,
                &mut writer,
                &mut shutdown,
                command_id,
                session_id,
                after_node_seq,
            )
            .await?;
            return Ok(());
        }

        let response = dispatch(&state, request).await;
        write_message(&mut writer, &response).await?;
    }
}

/// `AttachSession` の接続処理 (リプレイ → ライブ配信 + コマンド応答)。
///
/// 接続が切断されるか、セッションが終了するまで戻らない。
async fn attach_session<R, W>(
    state: &DaemonState,
    reader: &mut R,
    writer: &mut W,
    shutdown: &mut watch::Receiver<bool>,
    mut command_id: String,
    mut session_id: String,
    mut after_node_seq: Option<u64>,
) -> Result<(), NodeError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // セッション切替 (ストリーム中の再 AttachSession) に対応するため外側ループを持つ
    'attach: loop {
        if state.db().get_session(&session_id).await?.is_none() {
            let response = IpcServerMessage::Error {
                command_id: Some(command_id.clone()),
                code: ErrorCode::NotFound,
                message: format!("session not found: {session_id}"),
            };
            write_message(writer, &response).await?;
            return Ok(());
        }

        // リプレイとライブ配信の間に落ちるイベントを防ぐため、先に購読する
        let mut subscription = state.bus().subscribe();

        write_message(
            writer,
            &IpcServerMessage::Result {
                command_id: command_id.clone(),
                result: IpcResult::AttachSession {
                    session_id: session_id.clone(),
                    attach_mode: state.session_manager().attach_mode(&session_id),
                },
            },
        )
        .await?;

        let mut replay_state =
            replay_session_events(state, writer, &session_id, after_node_seq).await?;

        loop {
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                frame = read_frame(reader) => {
                    let Some(frame) = frame? else {
                        return Ok(()); // クライアントが切断した
                    };
                    let request: IpcClientMessage = match serde_json::from_slice(&frame) {
                        Ok(request) => request,
                        Err(err) => {
                            let response = IpcServerMessage::Error {
                                command_id: None,
                                code: ErrorCode::InvalidState,
                                message: format!("invalid ipc message: {err}"),
                            };
                            write_message(writer, &response).await?;
                            continue;
                        }
                    };
                    // 別セッションへの切替は外側ループでリプレイからやり直す
                    if let IpcClientMessage::AttachSession { command_id: next_command_id, session_id: next, after_node_seq: next_after } = request {
                        command_id = next_command_id;
                        session_id = next;
                        after_node_seq = next_after;
                        continue 'attach;
                    }
                    let response = dispatch(state, request).await;
                    write_message(writer, &response).await?;
                }
                event = subscription.recv() => {
                    match event {
                        Ok(message) => {
                            if let Some(response) = forward_to_attached(message, &session_id, &mut replay_state) {
                                write_message(writer, &response).await?;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            // 遅延した場合はカーソル (node_seq) から履歴を取り直す
                            tracing::warn!(skipped, session_id = %session_id, "ipc attach lagged; replaying");
                            let last = replay_state.last_node_seq;
                            replay_state = replay_session_events(state, writer, &session_id, Some(last)).await?;
                        }
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    }
                }
            }
        }
    }
}

/// `AttachSession` のリプレイ進捗 (重複排除と再接続に使う)。
struct AttachReplay {
    /// 最後に送信した `node_seq`
    last_node_seq: u64,
    /// 最後に送信したイベントの `cursor`
    cursor: u64,
}

/// `after_node_seq` より後の永続イベントをバッチ送信し、進捗を返す。
async fn replay_session_events<W>(
    state: &DaemonState,
    writer: &mut W,
    session_id: &str,
    after_node_seq: Option<u64>,
) -> Result<AttachReplay, NodeError>
where
    W: AsyncWrite + Unpin,
{
    let after = after_node_seq.unwrap_or(0);
    let mut progress = AttachReplay {
        last_node_seq: after,
        cursor: 0,
    };
    loop {
        let batch = state
            .db()
            .session_events_after(session_id, progress.cursor, ATTACH_REPLAY_BATCH)
            .await?;
        let full_batch = batch.events.len() as u32 >= ATTACH_REPLAY_BATCH;
        progress.cursor = batch.cursor;
        // `after_node_seq` 以前のイベントは送らずに読み飛ばす
        let events: Vec<_> = batch
            .events
            .into_iter()
            .filter(|event| event.node_seq > after)
            .collect();
        if events.is_empty() {
            if !full_batch {
                return Ok(progress);
            }
            continue;
        }
        if let Some(last) = events.iter().map(|event| event.node_seq).max() {
            progress.last_node_seq = progress.last_node_seq.max(last);
        }
        write_message(
            writer,
            &IpcServerMessage::EventBatch {
                session_id: session_id.to_owned(),
                events,
                cursor: progress.cursor,
            },
        )
        .await?;
        if !full_batch {
            return Ok(progress);
        }
    }
}

/// ブロードキャストイベントをアタッチ中のクライアント向けメッセージへ変換する。
///
/// 対象外セッションのイベント・既にリプレイ済みのイベントは `None` を返す。
fn forward_to_attached(
    message: SessionBroadcast,
    session_id: &str,
    replay: &mut AttachReplay,
) -> Option<IpcServerMessage> {
    match message {
        SessionBroadcast::Persisted { event, cursor } => {
            if event.session_id != session_id || event.node_seq <= replay.last_node_seq {
                return None;
            }
            replay.last_node_seq = event.node_seq;
            replay.cursor = replay.cursor.max(cursor);
            Some(IpcServerMessage::EventBatch {
                session_id: session_id.to_owned(),
                events: vec![event],
                cursor,
            })
        }
        SessionBroadcast::Ephemeral(event) => {
            if event.session_id != session_id {
                return None;
            }
            Some(IpcServerMessage::EventBatch {
                session_id: session_id.to_owned(),
                events: vec![event],
                cursor: replay.cursor,
            })
        }
        SessionBroadcast::StreamDelta {
            session_id: event_session,
            delta,
        } => {
            if event_session != session_id {
                return None;
            }
            Some(IpcServerMessage::LiveStreamDelta {
                session_id: event_session,
                delta,
            })
        }
    }
}

/// ディスパッチエラー (コマンドID + 構造化エラーコード)。
#[derive(Debug)]
struct DispatchError {
    command_id: Option<String>,
    code: ErrorCode,
    message: String,
}

impl DispatchError {
    fn new(command_id: &str, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            command_id: Some(command_id.to_owned()),
            code,
            message: message.into(),
        }
    }

    fn from_node_error(command_id: &str, err: NodeError) -> Self {
        Self::new(command_id, err.error_code(), err.to_string())
    }
}

/// 1リクエストを処理してレスポンスメッセージを返す。
pub async fn dispatch(state: &DaemonState, request: IpcClientMessage) -> IpcServerMessage {
    match handle(state, request).await {
        Ok((command_id, result)) => IpcServerMessage::Result { command_id, result },
        Err(err) => IpcServerMessage::Error {
            command_id: err.command_id,
            code: err.code,
            message: err.message,
        },
    }
}

async fn handle(
    state: &DaemonState,
    request: IpcClientMessage,
) -> Result<(String, IpcResult), DispatchError> {
    match request {
        IpcClientMessage::Ping => Ok((
            "ping".to_owned(),
            IpcResult::Ack {
                message: Some("pong".to_owned()),
            },
        )),

        IpcClientMessage::GetLocalStatus { command_id } => {
            let sessions = state
                .db()
                .list_sessions(&SessionFilter {
                    statuses: DaemonState::active_statuses(),
                    limit: Some(SESSION_LIST_LIMIT),
                    ..SessionFilter::default()
                })
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            let unsynced = state
                .db()
                .unsynced_event_count()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            Ok((
                command_id,
                IpcResult::LocalStatus {
                    status: fxg_protocol::ipc::LocalStatus {
                        sessions,
                        // Outbox 同期ワーカーは Phase 4 で実装する
                        central_connected: false,
                        unsynced_event_count: unsynced,
                        last_synced_at: None,
                    },
                },
            ))
        }

        IpcClientMessage::ProjectInfo { command_id, cwd } => {
            let resolved = resolve_and_register(state, Path::new(&cwd), &command_id).await?;
            Ok((
                command_id,
                IpcResult::ProjectInfo {
                    project: to_project_info(&resolved),
                },
            ))
        }

        IpcClientMessage::ProjectList { command_id } => {
            let projects = state
                .db()
                .list_projects()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            Ok((command_id, IpcResult::Projects { projects }))
        }

        IpcClientMessage::ProjectLink {
            command_id,
            cwd,
            project_id,
        } => {
            let dir = PathBuf::from(&cwd);
            if !dir.is_dir() {
                return Err(DispatchError::new(
                    &command_id,
                    ErrorCode::NotFound,
                    format!("directory not found: {cwd}"),
                ));
            }
            patch_project_key(&dir, &project_id)
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            // 紐付けを即時反映する
            let resolved = resolve_and_register(state, &dir, &command_id).await?;
            Ok((
                command_id,
                IpcResult::Ack {
                    message: Some(format!(
                        "linked {} to {}",
                        resolved.local_path.display(),
                        resolved.project_id
                    )),
                },
            ))
        }

        IpcClientMessage::ProjectScan { command_id, dir } => {
            let config = GlobalConfig::load_from_path(&state.paths().config_path())
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            let scan_dirs: Vec<PathBuf> = match dir {
                Some(dir) => vec![PathBuf::from(dir)],
                None => config
                    .node
                    .project_scan_dirs
                    .iter()
                    .map(PathBuf::from)
                    .collect(),
            };
            if scan_dirs.is_empty() {
                return Err(DispatchError::new(
                    &command_id,
                    ErrorCode::InvalidState,
                    "no scan directories configured (set node.project_scan_dirs in config.toml)",
                ));
            }
            for scan_dir in &scan_dirs {
                let repositories = git::find_repositories(scan_dir, SCAN_MAX_DEPTH)
                    .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
                for repo in repositories {
                    resolve_and_register(state, &repo, &command_id).await?;
                }
            }
            let projects = state
                .db()
                .list_projects()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            Ok((
                command_id,
                IpcResult::ProjectScan {
                    projects,
                    scanned_dirs: scan_dirs
                        .iter()
                        .map(|dir| dir.to_string_lossy().into_owned())
                        .collect(),
                },
            ))
        }

        IpcClientMessage::WorktreeList {
            command_id,
            cwd,
            project_id,
        } => {
            let worktrees =
                list_worktrees(state, cwd.as_deref(), project_id.as_deref(), &command_id).await?;
            Ok((command_id, IpcResult::Worktrees { worktrees }))
        }

        IpcClientMessage::WorktreeAdd {
            command_id,
            cwd,
            project_id,
            branch,
            base_branch,
            path,
        } => {
            let repo = PathBuf::from(&cwd);
            let resolved = resolve_and_register(state, &repo, &command_id).await?;
            let repo_root = resolved.git_root.clone().ok_or_else(|| {
                DispatchError::new(
                    &command_id,
                    ErrorCode::InvalidState,
                    format!("not a git repository: {cwd}"),
                )
            })?;
            let resolved = if let Some(project_id) = project_id
                && project_id != resolved.project_id
            {
                return Err(DispatchError::new(
                    &command_id,
                    ErrorCode::InvalidState,
                    format!(
                        "project_id mismatch: {project_id} (resolved: {})",
                        resolved.project_id
                    ),
                ));
            } else {
                resolved
            };

            let project_config = project::load_project_config(&resolved.local_path)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?
                .unwrap_or_default();
            let global_config = GlobalConfig::load_from_path(&state.paths().config_path())
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            let dir_template = project_config
                .worktree
                .dir_template
                .clone()
                .unwrap_or_else(|| {
                    global_config
                        .node
                        .resolved_worktree_dir_template()
                        .to_owned()
                });
            let base_branch = base_branch.or_else(|| project_config.worktree.base_branch.clone());
            let path_buf = path.map(PathBuf::from);

            let outcome = worktree::ensure_worktree(&worktree::WorktreeAddRequest {
                repo: &repo_root,
                project_id: &resolved.project_id,
                branch: &branch,
                base_branch: base_branch.as_deref(),
                dir_template: &dir_template,
                new_path: path_buf.as_deref(),
                fxg_home: state.paths().fxg_home(),
                copy_files: &project_config.worktree.copy_files,
                post_create: &project_config.worktree.post_create,
            })
            .await
            .map_err(|err| DispatchError::from_node_error(&command_id, err))?;

            // Worktree をノードの紐付けとして登録する
            state
                .db()
                .upsert_project(&resolved.to_project_record())
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            let mut binding =
                resolved.to_binding_record(state.node_id(), true, Some(branch.as_str()));
            binding.local_path = outcome.path.to_string_lossy().into_owned();
            state
                .db()
                .upsert_project_binding(&binding)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;

            record_worktree_audit(
                state,
                "add",
                serde_json::json!({
                    "branch": branch,
                    "path": outcome.path.to_string_lossy(),
                    "created": outcome.created,
                }),
            )
            .await;

            Ok((
                command_id,
                IpcResult::WorktreeAdded {
                    path: outcome.path.to_string_lossy().into_owned(),
                    branch: outcome.branch,
                    created: outcome.created,
                    hook_logs: outcome.hook_logs,
                },
            ))
        }

        IpcClientMessage::WorktreeRemove {
            command_id,
            cwd,
            target,
            force,
        } => {
            let repo = PathBuf::from(&cwd);
            let entries = worktree::list_worktrees(&repo)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            let target_path = entries
                .iter()
                .find_map(|entry| {
                    if entry.branch.as_deref() == Some(target.as_str()) {
                        return Some(entry.path.clone());
                    }
                    let path = Path::new(&target);
                    if path.is_absolute() && entry.path == path {
                        return Some(entry.path.clone());
                    }
                    if entry
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy() == target.as_str())
                        .unwrap_or(false)
                    {
                        return Some(entry.path.clone());
                    }
                    None
                })
                .ok_or_else(|| {
                    DispatchError::new(
                        &command_id,
                        ErrorCode::NotFound,
                        format!("worktree not found: {target}"),
                    )
                })?;

            worktree::remove_worktree(&repo, &target_path, force)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            let _ = state
                .db()
                .delete_project_binding(state.node_id(), &target_path.to_string_lossy())
                .await;

            record_worktree_audit(
                state,
                "remove",
                serde_json::json!({
                    "target": target,
                    "path": target_path.to_string_lossy(),
                    "force": force,
                }),
            )
            .await;

            Ok((
                command_id,
                IpcResult::Ack {
                    message: Some(format!("removed worktree {}", target_path.display())),
                },
            ))
        }

        IpcClientMessage::WorktreePrune { command_id, cwd } => {
            let output = worktree::prune_worktrees(Path::new(&cwd))
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                IpcResult::Ack {
                    message: Some(if output.trim().is_empty() {
                        "pruned".to_owned()
                    } else {
                        output
                    }),
                },
            ))
        }

        IpcClientMessage::ListSessions {
            command_id,
            include_stopped,
        } => {
            let filter = SessionFilter {
                statuses: if include_stopped {
                    Vec::new()
                } else {
                    DaemonState::active_statuses()
                },
                limit: Some(SESSION_LIST_LIMIT),
                ..SessionFilter::default()
            };
            let sessions = state
                .db()
                .list_sessions(&filter)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            Ok((command_id, IpcResult::Sessions { sessions }))
        }

        IpcClientMessage::KillAll { command_id } => {
            let (killed_sessions, killed_ptys) = state
                .kill_all_local("killed by local kill-all")
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;

            let mut audit = fxg_db::AuditLogRecord::new(
                fxg_db::audit::actions::KILL_SWITCH,
                "local",
                "local-ipc",
            );
            audit.node_id = Some(state.node_id().to_owned());
            audit.details = serde_json::json!({
                "reason": "local kill-all",
                "killed_sessions": killed_sessions,
                "killed_ptys": killed_ptys,
            });
            if let Err(err) = state.db().append_audit_log(&audit).await {
                tracing::warn!("failed to record kill-switch audit log: {err}");
            }

            Ok((
                command_id,
                IpcResult::KillAll {
                    killed_sessions,
                    killed_ptys,
                },
            ))
        }

        IpcClientMessage::AuthToken { command_id, rotate } => {
            let token = if rotate {
                state
                    .rotate_auth_token()
                    .map_err(|err| DispatchError::from_node_error(&command_id, err))?
            } else {
                state.auth_token()
            };
            Ok((command_id, IpcResult::AuthToken { token }))
        }

        IpcClientMessage::EnsureSession {
            command_id,
            cwd,
            agent_id,
            extra_args,
            initial_mode,
            acp,
        } => {
            let outcome = state
                .session_manager()
                .ensure_session(
                    &command_id,
                    Path::new(&cwd),
                    &agent_id,
                    &extra_args,
                    initial_mode.as_deref(),
                    acp,
                )
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                IpcResult::EnsureSession {
                    session_id: outcome.session_id,
                    attach_mode: outcome.attach_mode,
                },
            ))
        }

        IpcClientMessage::SendPrompt {
            command_id,
            session_id,
            text,
            client_source,
        } => {
            state
                .session_manager()
                .send_prompt(&command_id, &session_id, &text, &client_source)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                IpcResult::CommandAccepted {
                    session_id: Some(session_id),
                },
            ))
        }

        IpcClientMessage::RespondPermission {
            command_id,
            session_id,
            request_id,
            selected_option_id,
            resolved_by,
        } => {
            state
                .session_manager()
                .respond_permission(
                    &command_id,
                    &session_id,
                    &request_id,
                    &selected_option_id,
                    &resolved_by,
                )
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                IpcResult::CommandAccepted {
                    session_id: Some(session_id),
                },
            ))
        }

        IpcClientMessage::ControlSession {
            command_id,
            session_id,
            action,
        } => {
            state
                .session_manager()
                .control(&command_id, &session_id, &action)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                IpcResult::CommandAccepted {
                    session_id: Some(session_id),
                },
            ))
        }

        IpcClientMessage::SessionRevert {
            command_id,
            session_id,
            target_node_seq,
        } => {
            let outcome = state
                .session_manager()
                .revert(&command_id, &session_id, target_node_seq)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                IpcResult::SessionReverted {
                    target_node_seq: outcome.target_node_seq,
                    restored_tree_hash: outcome.restored_tree_hash,
                    backup_tree_hash: outcome.backup_tree_hash,
                    restored_files: outcome.restored_files as u64,
                    removed_files: outcome.removed_files as u64,
                },
            ))
        }

        IpcClientMessage::SessionFork {
            command_id,
            session_id,
            from_node_seq,
            agent_id,
            cwd,
        } => {
            let outcome = state
                .session_manager()
                .fork(
                    &command_id,
                    &session_id,
                    from_node_seq,
                    agent_id.as_deref(),
                    cwd.as_deref().map(Path::new),
                )
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                IpcResult::SessionForked {
                    session_id: outcome.session_id,
                    attach_mode: outcome.attach_mode,
                },
            ))
        }

        IpcClientMessage::SessionShow {
            command_id,
            session_id,
            recent_events,
        } => {
            let session = state
                .db()
                .get_session(&session_id)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?
                .ok_or_else(|| {
                    DispatchError::new(
                        &command_id,
                        ErrorCode::NotFound,
                        format!("session not found: {session_id}"),
                    )
                })?;
            let (event_count, recent_events) =
                session_event_tail(state.db(), &session_id, recent_events)
                    .await
                    .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            let pending_permissions = state
                .db()
                .pending_permissions()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?
                .into_iter()
                .filter(|entry| entry.session_id == session_id)
                .collect();
            Ok((
                command_id,
                IpcResult::SessionDetail(Box::new(SessionDetail {
                    session,
                    event_count,
                    recent_events,
                    pending_permissions,
                })),
            ))
        }

        IpcClientMessage::InboxList { command_id } => {
            let requests = state
                .db()
                .pending_permissions()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            Ok((command_id, IpcResult::Inbox { requests }))
        }

        // `AttachSession` は接続ループ ([`handle_connection`]) が
        // ストリーミングモードへ切り替えて処理するため、ここには到達しない。
        IpcClientMessage::AttachSession { command_id, .. } => Err(DispatchError::new(
            &command_id,
            ErrorCode::InvalidState,
            "AttachSession must be handled by the connection loop",
        )),
    }
}

/// セッションのイベント総数と、末尾 `recent` 件 (古い順) を取得する。
///
/// バッチ読み込みで末尾まで進めながらリングバッファに直近分のみを保持する。
async fn session_event_tail(
    db: &fxg_db::Db,
    session_id: &str,
    recent: u32,
) -> Result<(u64, Vec<SessionEventEnvelope>), NodeError> {
    const BATCH: u32 = 200;
    let mut cursor = 0u64;
    let mut tail: std::collections::VecDeque<SessionEventEnvelope> =
        std::collections::VecDeque::new();
    loop {
        let batch = db.session_events_after(session_id, cursor, BATCH).await?;
        let fetched = batch.events.len() as u32;
        if fetched == 0 {
            break;
        }
        cursor = batch.cursor;
        for event in batch.events {
            tail.push_back(event);
            if tail.len() > recent as usize {
                tail.pop_front();
            }
        }
        if fetched < BATCH {
            break;
        }
    }
    Ok((cursor, tail.into_iter().collect()))
}

/// プロジェクトを解決し、`projects` / `project_node_bindings` を更新する。
async fn resolve_and_register(
    state: &DaemonState,
    dir: &Path,
    command_id: &str,
) -> Result<project::ResolvedProject, DispatchError> {
    let resolved = project::resolve_project(dir, state.node_id())
        .await
        .map_err(|err| DispatchError::from_node_error(command_id, err))?;
    state
        .db()
        .upsert_project(&resolved.to_project_record())
        .await
        .map_err(|err| DispatchError::from_node_error(command_id, err.into()))?;

    // Worktree 内での実行かどうかと現在ブランチを記録する
    // (取得に失敗した場合はメインリポジトリ扱いで登録する)
    let is_worktree = match &resolved.git_root {
        Some(root) => git::is_worktree(root).await.unwrap_or(false),
        None => false,
    };
    let branch = match &resolved.git_root {
        Some(root) => git::current_branch(root).await.unwrap_or(None),
        None => None,
    };
    state
        .db()
        .upsert_project_binding(&resolved.to_binding_record(
            state.node_id(),
            is_worktree,
            branch.as_deref(),
        ))
        .await
        .map_err(|err| DispatchError::from_node_error(command_id, err.into()))?;

    // 再リンク時に古いプロジェクトの紐付けを残さない
    // (`fxg project link` で project_key を変更した場合など)
    state
        .db()
        .delete_other_project_bindings(
            state.node_id(),
            &resolved.local_path.to_string_lossy(),
            &resolved.project_id,
        )
        .await
        .map_err(|err| DispatchError::from_node_error(command_id, err.into()))?;
    state
        .db()
        .delete_orphan_projects()
        .await
        .map_err(|err| DispatchError::from_node_error(command_id, err.into()))?;
    Ok(resolved)
}

fn to_project_info(resolved: &project::ResolvedProject) -> ProjectInfo {
    ProjectInfo {
        project_id: resolved.project_id.clone(),
        name: resolved.name.clone(),
        canonical_git_url: resolved.canonical_git_url.clone(),
        git_root: resolved
            .git_root
            .as_ref()
            .map(|root| root.to_string_lossy().into_owned()),
        relative_subpath: resolved.relative_subpath.clone(),
        local_path: resolved.local_path.to_string_lossy().into_owned(),
        is_git_repo: resolved.is_git_repo,
        source: resolved.source,
    }
}

/// Worktree 一覧を返す (`cwd` または登録済みプロジェクトのメインリポジトリから解決)。
async fn list_worktrees(
    state: &DaemonState,
    cwd: Option<&str>,
    project_id: Option<&str>,
    command_id: &str,
) -> Result<Vec<fxg_protocol::client_api::WorktreeInfo>, DispatchError> {
    let repos: Vec<PathBuf> = match cwd {
        Some(cwd) => vec![PathBuf::from(cwd)],
        None => {
            let projects = state
                .db()
                .list_projects()
                .await
                .map_err(|err| DispatchError::from_node_error(command_id, err.into()))?;
            projects
                .into_iter()
                .filter(|project| project_id.is_none_or(|id| id == project.project_id.as_str()))
                .flat_map(|project| {
                    project
                        .bindings
                        .into_iter()
                        .filter(|binding| {
                            binding.node_id == state.node_id() && !binding.is_worktree
                        })
                        .map(|binding| PathBuf::from(binding.local_path))
                })
                .collect()
        }
    };

    let mut worktrees = Vec::new();
    for repo in repos {
        let entries = worktree::list_worktrees(&repo)
            .await
            .map_err(|err| DispatchError::from_node_error(command_id, err))?;
        for entry in entries {
            worktrees.push(fxg_protocol::client_api::WorktreeInfo {
                node_id: state.node_id().to_owned(),
                path: entry.path.to_string_lossy().into_owned(),
                branch: entry.branch,
                head_commit: entry.head,
                is_main: entry.is_main,
            });
        }
    }
    worktrees.sort_by(|a, b| a.path.cmp(&b.path));
    worktrees.dedup_by(|a, b| a.path == b.path);
    Ok(worktrees)
}

/// Worktree 操作を監査ログへ記録する (`audit_logs`)。
async fn record_worktree_audit(state: &DaemonState, action: &str, details: serde_json::Value) {
    let mut audit = fxg_db::AuditLogRecord::new(
        fxg_db::audit::actions::WORKTREE_MANAGE,
        "local",
        "local-ipc",
    );
    audit.node_id = Some(state.node_id().to_owned());
    audit.details = serde_json::json!({ "action": action, "details": details });
    if let Err(err) = state.db().append_audit_log(&audit).await {
        tracing::warn!("failed to record worktree audit log: {err}");
    }
}

/// `.fxg.toml` に `project_key` を書き込む (既存の設定は保持する)。
fn patch_project_key(dir: &Path, project_key: &str) -> Result<(), NodeError> {
    let path = dir.join(fxg_protocol::config::PROJECT_CONFIG_FILE_NAME);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(str::to_owned).collect();

    // TOML 文字列リテラルとして安全に埋め込む
    let literal = format!(
        "\"{}\"",
        project_key.replace('\\', "\\\\").replace('"', "\\\"")
    );

    let mut replaced = false;
    for line in lines.iter_mut() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("project_key") && line.contains('=') {
            *line = format!("project_key = {literal}");
            replaced = true;
            break;
        }
    }
    if !replaced {
        lines.insert(0, format!("project_key = {literal}"));
    }

    let mut output = lines.join("\n");
    output.push('\n');
    std::fs::write(&path, output).map_err(|err| NodeError::io(&path, err))?;

    // 実際に読み戻せることを検証する (壊れた TOML を書かない)
    ProjectConfig::load_from_path(&path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonConfig, NodeDaemon};
    use crate::ipc_client::IpcClient;
    use std::time::Duration;

    async fn start_daemon() -> (NodeDaemon, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "ipc-node", "IPC Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let daemon = NodeDaemon::start(config).await.expect("start");
        wait_for_endpoint(daemon.ipc_endpoint()).await;
        (daemon, dir)
    }

    /// IPC サーバーが接続を受け付けられるようになるまで待機する。
    async fn wait_for_endpoint(endpoint: &str) {
        for _ in 0..200 {
            if IpcClient::connect(endpoint).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("ipc endpoint did not become ready: {endpoint}");
    }

    /// IPC 接続して1リクエスト送信し、レスポンスを受信する。
    ///
    /// クライアントは `IpcClient` を使う (Unix Domain Socket / Named Pipe を
    /// 同一のテストコードで検証する)。
    async fn roundtrip(endpoint: &str, request: &IpcClientMessage) -> IpcServerMessage {
        let mut client = IpcClient::connect(endpoint).await.expect("connect");
        client.request(request).await.expect("request")
    }

    /// モックエージェントドライバを注入したデーモンを起動する (セッション操作用)。
    async fn start_session_daemon() -> (NodeDaemon, crate::testutil::MockAgent, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        crate::testutil::write_mock_agent_config(dir.path());
        let mut config =
            DaemonConfig::new(dir.path().to_path_buf(), "session-node", "Session Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let mock = crate::testutil::MockAgent::default();
        let daemon = NodeDaemon::start_with_driver_factory(config, mock.factory())
            .await
            .expect("start");
        wait_for_endpoint(daemon.ipc_endpoint()).await;
        (daemon, mock, dir)
    }

    /// 条件を満たすメッセージを受信するまで読み進める (アタッチ中のストリーム用)。
    async fn recv_until<F>(client: &mut IpcClient, mut predicate: F) -> IpcServerMessage
    where
        F: FnMut(&IpcServerMessage) -> bool,
    {
        for _ in 0..50 {
            let message = client
                .recv()
                .await
                .expect("recv")
                .expect("daemon closed the connection");
            if predicate(&message) {
                return message;
            }
        }
        panic!("expected message was not received");
    }

    #[tokio::test]
    async fn ipc_project_info_and_link_via_socket() {
        let (daemon, dir) = start_daemon().await;
        let endpoint = daemon.ipc_endpoint().to_owned();

        // Git リポジトリを作成してから問い合わせる
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        git::run_git(&repo, &["init", "-q", "-b", "main"])
            .await
            .expect("git init");
        git::run_git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:nazo6/flexagent.git",
            ],
        )
        .await
        .expect("remote");

        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::ProjectInfo {
                command_id: "c1".to_owned(),
                cwd: repo.to_string_lossy().into_owned(),
            },
        )
        .await;
        match response {
            IpcServerMessage::Result {
                result: IpcResult::ProjectInfo { project },
                ..
            } => {
                assert_eq!(project.project_id, "github.com/nazo6/flexagent");
                assert_eq!(project.source.as_str(), "git_remote");
                assert!(project.is_git_repo);
            }
            other => panic!("unexpected response: {other:?}"),
        }

        // project link は .fxg.toml を書き込む
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::ProjectLink {
                command_id: "c2".to_owned(),
                cwd: repo.to_string_lossy().into_owned(),
                project_id: "internal/agent-tools".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            response,
            IpcServerMessage::Result {
                result: IpcResult::Ack { .. },
                ..
            }
        ));
        let config = ProjectConfig::load_from_dir(&repo)
            .expect("load")
            .expect(".fxg.toml exists");
        assert_eq!(config.project_key.as_deref(), Some("internal/agent-tools"));

        // 一覧に載る
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::ProjectList {
                command_id: "c3".to_owned(),
            },
        )
        .await;
        match response {
            IpcServerMessage::Result {
                result: IpcResult::Projects { projects },
                ..
            } => {
                assert_eq!(projects.len(), 1);
                assert_eq!(projects[0].project_id, "internal/agent-tools");
                assert_eq!(projects[0].bindings.len(), 1);
                assert_eq!(projects[0].bindings[0].node_id, "ipc-node");
            }
            other => panic!("unexpected response: {other:?}"),
        }

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn ipc_worktree_add_list_remove_and_sessions() {
        let (daemon, dir) = start_daemon().await;
        let endpoint = daemon.ipc_endpoint().to_owned();

        // Git リポジトリ + 1コミット
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        git::run_git(&repo, &["init", "-q", "-b", "main"])
            .await
            .expect("git init");
        git::run_git(&repo, &["config", "user.email", "t@example.com"])
            .await
            .expect("email");
        git::run_git(&repo, &["config", "user.name", "t"])
            .await
            .expect("name");
        std::fs::write(repo.join("README.md"), "hello\n").expect("write");
        git::run_git(&repo, &["add", "-A"]).await.expect("add");
        git::run_git(&repo, &["commit", "-q", "-m", "init"])
            .await
            .expect("commit");

        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::WorktreeAdd {
                command_id: "w1".to_owned(),
                cwd: repo.to_string_lossy().into_owned(),
                project_id: None,
                branch: "feat/ipc".to_owned(),
                base_branch: None,
                path: None,
            },
        )
        .await;
        let worktree_path = match response {
            IpcServerMessage::Result {
                result: IpcResult::WorktreeAdded { path, created, .. },
                ..
            } => {
                assert!(created);
                path
            }
            other => panic!("unexpected response: {other:?}"),
        };
        assert!(Path::new(&worktree_path).join("README.md").is_file());

        // 一覧 (cwd 指定)
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::WorktreeList {
                command_id: "w2".to_owned(),
                cwd: Some(repo.to_string_lossy().into_owned()),
                project_id: None,
            },
        )
        .await;
        match response {
            IpcServerMessage::Result {
                result: IpcResult::Worktrees { worktrees },
                ..
            } => {
                assert_eq!(worktrees.len(), 2, "main + feat/ipc");
                assert!(
                    worktrees
                        .iter()
                        .any(|w| w.branch.as_deref() == Some("main") && w.is_main)
                );
                assert!(
                    worktrees
                        .iter()
                        .any(|w| w.branch.as_deref() == Some("feat/ipc"))
                );
            }
            other => panic!("unexpected response: {other:?}"),
        }

        // ブランチ名で削除
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::WorktreeRemove {
                command_id: "w3".to_owned(),
                cwd: repo.to_string_lossy().into_owned(),
                target: "feat/ipc".to_owned(),
                force: true,
            },
        )
        .await;
        assert!(matches!(
            response,
            IpcServerMessage::Result {
                result: IpcResult::Ack { .. },
                ..
            }
        ));
        assert!(!Path::new(&worktree_path).exists());

        // セッション一覧 (空)
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::ListSessions {
                command_id: "s1".to_owned(),
                include_stopped: true,
            },
        )
        .await;
        match response {
            IpcServerMessage::Result {
                result: IpcResult::Sessions { sessions },
                ..
            } => assert!(sessions.is_empty()),
            other => panic!("unexpected response: {other:?}"),
        }

        // 認証トークンの取得とローテーション
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::AuthToken {
                command_id: "a1".to_owned(),
                rotate: false,
            },
        )
        .await;
        let token = match response {
            IpcServerMessage::Result {
                result: IpcResult::AuthToken { token },
                ..
            } => token,
            other => panic!("unexpected response: {other:?}"),
        };
        assert_eq!(token, daemon.state().auth_token());

        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::AuthToken {
                command_id: "a2".to_owned(),
                rotate: true,
            },
        )
        .await;
        match response {
            IpcServerMessage::Result {
                result: IpcResult::AuthToken { token: rotated },
                ..
            } => assert_ne!(rotated, token),
            other => panic!("unexpected response: {other:?}"),
        }

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn second_daemon_cannot_bind_same_socket() {
        let (daemon, dir) = start_daemon().await;
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "other", "Other");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = daemon.ipc_endpoint().to_owned();
        let err = NodeDaemon::start(config).await.expect_err("must fail");
        assert!(
            matches!(err, NodeError::Server(_)),
            "unexpected error: {err}"
        );

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn ipc_session_ensure_attach_prompt_and_permissions() {
        use fxg_acp::DriverEvent;
        use fxg_protocol::common::{PermissionOption, SessionControlAction};
        use fxg_protocol::events::UnifiedEventPayload;

        let (daemon, mock, dir) = start_session_daemon().await;
        let endpoint = daemon.ipc_endpoint().to_owned();
        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");

        // EnsureSession: セッション開始 (内蔵TUI モード)
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::EnsureSession {
                command_id: "e1".to_owned(),
                cwd: workdir.to_string_lossy().into_owned(),
                agent_id: "mock".to_owned(),
                extra_args: Vec::new(),
                initial_mode: None,
                acp: false,
            },
        )
        .await;
        let session_id = match response {
            IpcServerMessage::Result {
                result:
                    IpcResult::EnsureSession {
                        session_id,
                        attach_mode,
                    },
                ..
            } => {
                assert_eq!(attach_mode, fxg_protocol::ipc::AttachMode::AcpTui);
                session_id
            }
            other => panic!("unexpected response: {other:?}"),
        };

        // command_id の重複送信は COMMAND_DUPLICATE (冪等性)
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::EnsureSession {
                command_id: "e1".to_owned(),
                cwd: workdir.to_string_lossy().into_owned(),
                agent_id: "mock".to_owned(),
                extra_args: Vec::new(),
                initial_mode: None,
                acp: false,
            },
        )
        .await;
        match response {
            IpcServerMessage::Error { code, .. } => assert_eq!(code, ErrorCode::CommandDuplicate),
            other => panic!("unexpected response: {other:?}"),
        }

        // AttachSession: 結果 + 履歴リプレイ (SessionCreated)
        let mut attach = IpcClient::connect(&endpoint).await.expect("connect");
        attach
            .send(&IpcClientMessage::AttachSession {
                command_id: "a1".to_owned(),
                session_id: session_id.clone(),
                after_node_seq: None,
            })
            .await
            .expect("attach");
        let message = attach
            .recv()
            .await
            .expect("recv")
            .expect("daemon closed the connection");
        match message {
            IpcServerMessage::Result {
                command_id,
                result:
                    IpcResult::AttachSession {
                        session_id: attached,
                        attach_mode,
                    },
            } => {
                assert_eq!(command_id, "a1");
                assert_eq!(attached, session_id);
                // テスト用モックは内蔵TUIアタッチ
                assert_eq!(attach_mode, fxg_protocol::ipc::AttachMode::AcpTui);
            }
            other => panic!("unexpected response: {other:?}"),
        }
        let message = recv_until(&mut attach, |message| {
            matches!(message, IpcServerMessage::EventBatch { .. })
        })
        .await;
        match message {
            IpcServerMessage::EventBatch { events, .. } => {
                assert_eq!(events.len(), 1, "SessionCreated のみが履歴にある");
                assert_eq!(events[0].node_seq, 1);
                assert!(matches!(
                    events[0].payload,
                    UnifiedEventPayload::SessionCreated { .. }
                ));
            }
            _ => unreachable!(),
        }

        // SendPrompt: アタッチ中の同一接続から送信できる
        attach
            .send(&IpcClientMessage::SendPrompt {
                command_id: "p1".to_owned(),
                session_id: session_id.clone(),
                text: "hello".to_owned(),
                client_source: "cli".to_owned(),
            })
            .await
            .expect("send prompt");
        let mut saw_user_message = false;
        let mut saw_accepted = false;
        for _ in 0..20 {
            let message = attach
                .recv()
                .await
                .expect("recv")
                .expect("daemon closed the connection");
            match &message {
                IpcServerMessage::EventBatch { events, .. } => {
                    if events.iter().any(|event| {
                        matches!(&event.payload, UnifiedEventPayload::UserMessage { text, .. } if text == "hello")
                    }) {
                        saw_user_message = true;
                    }
                }
                IpcServerMessage::Result {
                    command_id,
                    result: IpcResult::CommandAccepted { session_id: Some(sid) },
                } => {
                    assert_eq!(command_id, "p1");
                    assert_eq!(sid, &session_id);
                    saw_accepted = true;
                }
                other => panic!("unexpected message: {other:?}"),
            }
            if saw_user_message && saw_accepted {
                break;
            }
        }
        assert!(saw_user_message, "UserMessage が配信される");
        assert!(saw_accepted, "SendPrompt は受理される");

        // モックドライバへプロンプトが届いている
        for _ in 0..100 {
            if !mock.prompts().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(mock.prompts(), vec!["hello"]);

        // 承認要求 → イベント配信 → 応答 → 2回目は ALREADY_RESOLVED
        mock.emit(DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
            request_id: "req-1".to_owned(),
            tool_name: "terminal".to_owned(),
            summary: "Run tests".to_owned(),
            options: vec![PermissionOption {
                option_id: "allow_once".to_owned(),
                name: "Allow once".to_owned(),
                kind: "allow_once".to_owned(),
            }],
            details: serde_json::json!({}),
        }));
        recv_until(&mut attach, |message| {
            matches!(
                message,
                IpcServerMessage::EventBatch { events, .. }
                    if events.iter().any(|event| matches!(
                        event.payload,
                        UnifiedEventPayload::PermissionRequest { .. }
                    ))
            )
        })
        .await;

        attach
            .send(&IpcClientMessage::RespondPermission {
                command_id: "r1".to_owned(),
                session_id: session_id.clone(),
                request_id: "req-1".to_owned(),
                selected_option_id: "allow_once".to_owned(),
                resolved_by: "cli".to_owned(),
            })
            .await
            .expect("respond");
        recv_until(&mut attach, |message| {
            matches!(message, IpcServerMessage::Result { command_id, .. } if command_id == "r1")
        })
        .await;
        assert_eq!(
            mock.permissions(),
            vec![("req-1".to_owned(), "allow_once".to_owned())]
        );

        attach
            .send(&IpcClientMessage::RespondPermission {
                command_id: "r2".to_owned(),
                session_id: session_id.clone(),
                request_id: "req-1".to_owned(),
                selected_option_id: "allow_once".to_owned(),
                resolved_by: "web".to_owned(),
            })
            .await
            .expect("respond again");
        let response = recv_until(&mut attach, |message| {
            matches!(message, IpcServerMessage::Error { code, .. } if *code == ErrorCode::AlreadyResolved)
        })
        .await;
        let _ = response;

        // ControlSession (SetMode) はドライバへ転送される
        attach
            .send(&IpcClientMessage::ControlSession {
                command_id: "m1".to_owned(),
                session_id: session_id.clone(),
                action: SessionControlAction::SetMode {
                    mode_id: "plan".to_owned(),
                },
            })
            .await
            .expect("set mode");
        recv_until(&mut attach, |message| {
            matches!(message, IpcServerMessage::Result { command_id, .. } if command_id == "m1")
        })
        .await;
        assert_eq!(mock.modes(), vec!["plan".to_owned()]);

        drop(attach);
        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn ipc_attach_unknown_session_returns_not_found() {
        let (daemon, _mock, _dir) = start_session_daemon().await;
        let endpoint = daemon.ipc_endpoint().to_owned();

        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::AttachSession {
                command_id: "a1".to_owned(),
                session_id: "missing-session".to_owned(),
                after_node_seq: None,
            },
        )
        .await;
        match response {
            IpcServerMessage::Error { code, .. } => assert_eq!(code, ErrorCode::NotFound),
            other => panic!("unexpected response: {other:?}"),
        }

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[test]
    fn patch_project_key_preserves_other_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path()
                .join(fxg_protocol::config::PROJECT_CONFIG_FILE_NAME),
            "name = \"flexagent\"\n\n[worktree]\nbase_branch = \"main\"\n",
        )
        .expect("write");
        patch_project_key(dir.path(), "github.com/nazo6/flexagent").expect("patch");
        let config = ProjectConfig::load_from_dir(dir.path())
            .expect("load")
            .expect("exists");
        assert_eq!(
            config.project_key.as_deref(),
            Some("github.com/nazo6/flexagent")
        );
        assert_eq!(config.name.as_deref(), Some("flexagent"));
        assert_eq!(config.worktree.base_branch.as_deref(), Some("main"));

        // 再実行しても重複行を作らない
        patch_project_key(dir.path(), "other/key").expect("patch again");
        let config = ProjectConfig::load_from_dir(dir.path())
            .expect("load")
            .expect("exists");
        assert_eq!(config.project_key.as_deref(), Some("other/key"));
        let text = std::fs::read_to_string(
            dir.path()
                .join(fxg_protocol::config::PROJECT_CONFIG_FILE_NAME),
        )
        .expect("read");
        assert_eq!(text.matches("project_key").count(), 1);
    }
}
