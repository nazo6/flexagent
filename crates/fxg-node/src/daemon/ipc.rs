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

use std::path::{Path, PathBuf};

use fxg_db::SessionFilter;
use fxg_protocol::common::ErrorCode;
use fxg_protocol::events::SessionEventEnvelope;
use fxg_protocol::ipc::{
    IpcClientMessage, IpcResult, IpcServerMessage, ProjectInfo, SessionDetail,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

use super::DaemonState;
use super::ops::AuditSource;
use crate::error::NodeError;
use crate::ipc_framing::{read_frame, write_message};
use crate::session_manager::ResumeParams;
use crate::{project, worktree};

/// `fxg ps` などで返すセッション一覧の上限。
const SESSION_LIST_LIMIT: u32 = 200;

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

        let response = dispatch(&state, request).await;
        write_message(&mut writer, &response).await?;
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
            let last_synced_at = state
                .db()
                .last_synced_at()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            Ok((
                command_id,
                IpcResult::LocalStatus {
                    status: fxg_protocol::ipc::LocalStatus {
                        sessions,
                        central_connected: state.central_connected(),
                        unsynced_event_count: unsynced,
                        last_synced_at,
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
                .list_projects_with_path_state()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
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
            // 紐付けを即時反映する
            let resolved = state
                .project_link(&dir, &project_id)
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
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
            let scanned_dirs = state
                .project_scan(dir.as_deref().map(Path::new))
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            let projects = state
                .list_projects_with_path_state()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                IpcResult::ProjectScan {
                    projects,
                    scanned_dirs,
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
            let outcome = state
                .worktree_add(
                    Path::new(&cwd),
                    project_id.as_deref(),
                    &branch,
                    base_branch,
                    path.map(PathBuf::from),
                    &AuditSource::local(),
                )
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
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

            state
                .worktree_remove(Some(&repo), &target_path, force, &AuditSource::local())
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;

            Ok((
                command_id,
                IpcResult::Ack {
                    message: Some(format!("removed worktree {}", target_path.display())),
                },
            ))
        }

        IpcClientMessage::WorktreePrune { command_id, cwd } => {
            let output = state
                .worktree_prune(Path::new(&cwd), &AuditSource::local())
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
            include_archived,
        } => {
            let filter = SessionFilter {
                statuses: if include_stopped {
                    Vec::new()
                } else {
                    DaemonState::active_statuses()
                },
                include_archived,
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

            state
                .record_audit(
                    fxg_db::audit::actions::KILL_SWITCH,
                    &AuditSource::local(),
                    None,
                    serde_json::json!({
                        "reason": "local kill-all",
                        "killed_sessions": killed_sessions,
                        "killed_ptys": killed_ptys,
                    }),
                )
                .await;

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

        IpcClientMessage::GitCredential { command_id, prompt } => {
            // Git Credential Proxy: 中央サーバーへオンメモリ中継して解決する
            // (VM内ディスクにトークンを残さない。設計: docs/01 §6.4)
            let result = match state.credentials().resolve(&prompt).await {
                Ok(response) => IpcResult::GitCredential {
                    username: if response.username.is_empty() {
                        crate::credentials::default_username().to_owned()
                    } else {
                        response.username
                    },
                    token: response.token,
                    error: response.error,
                },
                Err(err) => IpcResult::GitCredential {
                    username: crate::credentials::default_username().to_owned(),
                    token: None,
                    error: Some(err.to_string()),
                },
            };
            Ok((command_id, result))
        }

        IpcClientMessage::EnsureSession {
            command_id,
            cwd,
            agent_id,
            extra_args,
            initial_mode,
        } => {
            // 実行ディレクトリのプロジェクト紐付けを最新化する
            // (既定パス解決・直近使用の記録をセッション開始時に更新する)
            state
                .resolve_and_register_project(Path::new(&cwd))
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            let outcome = state
                .session_manager()
                .ensure_session(
                    &command_id,
                    Path::new(&cwd),
                    &agent_id,
                    &extra_args,
                    initial_mode.as_deref(),
                )
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            state
                .record_audit(
                    fxg_db::audit::actions::SESSION_START,
                    &AuditSource::local(),
                    Some(&outcome.session_id),
                    serde_json::json!({ "cwd": cwd, "agent_id": agent_id }),
                )
                .await;
            Ok((
                command_id,
                IpcResult::EnsureSession {
                    session_id: outcome.session_id,
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

        IpcClientMessage::RespondElicitation {
            command_id,
            session_id,
            elicitation_id,
            action,
            content,
            resolved_by,
        } => {
            state
                .session_manager()
                .respond_elicitation(
                    &command_id,
                    &session_id,
                    &elicitation_id,
                    action,
                    content,
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
                },
            ))
        }

        IpcClientMessage::SessionResume {
            command_id,
            session_id,
            force_replay,
        } => {
            let started = state
                .session_manager()
                .resume(ResumeParams {
                    command_id: &command_id,
                    session_id: &session_id,
                    force_replay,
                })
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            state
                .record_audit(
                    fxg_db::audit::actions::SESSION_RESUME,
                    &AuditSource::local(),
                    Some(&session_id),
                    serde_json::json!({ "force_replay": force_replay }),
                )
                .await;
            Ok((
                command_id,
                IpcResult::SessionResumed {
                    session_id: started.session_id,
                    context_restored: started.context_restored,
                },
            ))
        }

        IpcClientMessage::SessionArchive {
            command_id,
            session_id,
            archived,
        } => {
            let archived_at = state
                .set_session_archived(&command_id, &session_id, archived, &AuditSource::local())
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((
                command_id,
                IpcResult::SessionArchived {
                    session_id,
                    archived_at,
                },
            ))
        }

        IpcClientMessage::SessionDelete {
            command_id,
            session_id,
        } => {
            state
                .purge_session(&command_id, &session_id, &AuditSource::local())
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err))?;
            Ok((command_id, IpcResult::SessionDeleted { session_id }))
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
            let pending_elicitations = state
                .db()
                .pending_elicitations()
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
                    pending_elicitations,
                })),
            ))
        }

        IpcClientMessage::InboxList { command_id } => {
            let requests = state
                .db()
                .pending_permissions()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            let elicitations = state
                .db()
                .pending_elicitations()
                .await
                .map_err(|err| DispatchError::from_node_error(&command_id, err.into()))?;
            Ok((
                command_id,
                IpcResult::Inbox {
                    requests,
                    elicitations,
                },
            ))
        }
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
    state
        .resolve_and_register_project(dir)
        .await
        .map_err(|err| DispatchError::from_node_error(command_id, err))
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
    state
        .worktree_list(cwd.map(Path::new), project_id)
        .await
        .map_err(|err| DispatchError::from_node_error(command_id, err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonConfig, NodeDaemon};
    use crate::git;
    use crate::ipc_client::IpcClient;
    use fxg_protocol::config::ProjectConfig;
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
                include_archived: true,
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
        use fxg_protocol::common::{ElicitationAction, PermissionOption, SessionControlAction};
        use fxg_protocol::events::UnifiedEventPayload;

        let (daemon, mock, dir) = start_session_daemon().await;
        let endpoint = daemon.ipc_endpoint().to_owned();
        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");

        // EnsureSession: セッション開始
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::EnsureSession {
                command_id: "e1".to_owned(),
                cwd: workdir.to_string_lossy().into_owned(),
                agent_id: "mock".to_owned(),
                extra_args: Vec::new(),
                initial_mode: None,
            },
        )
        .await;
        let session_id = match response {
            IpcServerMessage::Result {
                result: IpcResult::EnsureSession { session_id },
                ..
            } => session_id,
            other => panic!("unexpected response: {other:?}"),
        };

        // セッション開始時にプロジェクト紐付けが登録される
        // (既定パス解決・直近使用記録の基礎データ)
        let canonical_workdir = fxg_pty::canonicalize(&workdir).expect("canonicalize");
        let projects = daemon
            .state()
            .db()
            .list_projects()
            .await
            .expect("list projects");
        assert!(
            projects.iter().any(|project| project
                .bindings
                .iter()
                .any(|binding| binding.local_path == canonical_workdir.to_string_lossy())),
            "EnsureSession must register a project binding: {projects:?}"
        );

        // command_id の重複送信は COMMAND_DUPLICATE (冪等性)
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::EnsureSession {
                command_id: "e1".to_owned(),
                cwd: workdir.to_string_lossy().into_owned(),
                agent_id: "mock".to_owned(),
                extra_args: Vec::new(),
                initial_mode: None,
            },
        )
        .await;
        match response {
            IpcServerMessage::Error { code, .. } => assert_eq!(code, ErrorCode::CommandDuplicate),
            other => panic!("unexpected response: {other:?}"),
        }

        // SendPrompt: プロンプト送信
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::SendPrompt {
                command_id: "p1".to_owned(),
                session_id: session_id.clone(),
                text: "hello".to_owned(),
                client_source: "cli".to_owned(),
            },
        )
        .await;
        match response {
            IpcServerMessage::Result {
                command_id,
                result:
                    IpcResult::CommandAccepted {
                        session_id: Some(sid),
                    },
            } => {
                assert_eq!(command_id, "p1");
                assert_eq!(sid, session_id);
            }
            other => panic!("unexpected response: {other:?}"),
        }

        // モックドライバへプロンプトが届いている
        for _ in 0..100 {
            if !mock.prompts().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(mock.prompts(), vec!["hello"]);

        // 承認要求 → 応答 → 2回目は ALREADY_RESOLVED
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
        tokio::time::sleep(Duration::from_millis(50)).await;

        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::RespondPermission {
                command_id: "r1".to_owned(),
                session_id: session_id.clone(),
                request_id: "req-1".to_owned(),
                selected_option_id: "allow_once".to_owned(),
                resolved_by: "cli".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            response,
            IpcServerMessage::Result { command_id, .. } if command_id == "r1"
        ));
        assert_eq!(
            mock.permissions(),
            vec![("req-1".to_owned(), "allow_once".to_owned())]
        );

        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::RespondPermission {
                command_id: "r2".to_owned(),
                session_id: session_id.clone(),
                request_id: "req-1".to_owned(),
                selected_option_id: "allow_once".to_owned(),
                resolved_by: "web".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            response,
            IpcServerMessage::Error { code, .. } if code == ErrorCode::AlreadyResolved
        ));

        // 質問 (elicitation) → 応答 → 2回目は ALREADY_RESOLVED
        mock.emit(DriverEvent::Event(
            UnifiedEventPayload::ElicitationRequest {
                elicitation_id: "elic-1".to_owned(),
                message: "どの戦略で進めますか?".to_owned(),
                mode: "form".to_owned(),
                requested_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "strategy": { "type": "string", "enum": ["a", "b"] } },
                    "required": ["strategy"]
                }),
                tool_call_id: None,
            },
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::RespondElicitation {
                command_id: "q1".to_owned(),
                session_id: session_id.clone(),
                elicitation_id: "elic-1".to_owned(),
                action: ElicitationAction::Accept,
                content: serde_json::json!({ "strategy": "a" }),
                resolved_by: "cli".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            response,
            IpcServerMessage::Result { command_id, .. } if command_id == "q1"
        ));
        assert_eq!(
            mock.elicitations(),
            vec![(
                "elic-1".to_owned(),
                ElicitationAction::Accept,
                serde_json::json!({ "strategy": "a" })
            )]
        );

        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::RespondElicitation {
                command_id: "q2".to_owned(),
                session_id: session_id.clone(),
                elicitation_id: "elic-1".to_owned(),
                action: ElicitationAction::Decline,
                content: serde_json::Value::Null,
                resolved_by: "web".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            response,
            IpcServerMessage::Error { code, .. } if code == ErrorCode::AlreadyResolved
        ));

        // ControlSession (SetMode) はドライバへ転送される
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::ControlSession {
                command_id: "m1".to_owned(),
                session_id: session_id.clone(),
                action: SessionControlAction::SetMode {
                    mode_id: "plan".to_owned(),
                },
            },
        )
        .await;
        assert!(matches!(
            response,
            IpcServerMessage::Result { command_id, .. } if command_id == "m1"
        ));
        assert_eq!(mock.modes(), vec!["plan".to_owned()]);

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn ipc_session_resume_replays_context_and_rejects_active() {
        use fxg_acp::DriverEvent;
        use fxg_protocol::common::SessionStatus;
        use fxg_protocol::events::UnifiedEventPayload;

        let (daemon, mock, dir) = start_session_daemon().await;
        let endpoint = daemon.ipc_endpoint().to_owned();
        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");

        // EnsureSession → SendPrompt で会話を 1 ターン残す
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::EnsureSession {
                command_id: "e1".to_owned(),
                cwd: workdir.to_string_lossy().into_owned(),
                agent_id: "mock".to_owned(),
                extra_args: Vec::new(),
                initial_mode: None,
            },
        )
        .await;
        let session_id = match response {
            IpcServerMessage::Result {
                result: IpcResult::EnsureSession { session_id, .. },
                ..
            } => session_id,
            other => panic!("unexpected response: {other:?}"),
        };
        roundtrip(
            &endpoint,
            &IpcClientMessage::SendPrompt {
                command_id: "p1".to_owned(),
                session_id: session_id.clone(),
                text: "hello resume".to_owned(),
                client_source: "cli".to_owned(),
            },
        )
        .await;

        // ドライバ終了で停止させ、再開可能な状態にする
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        mock.close_events();
        for _ in 0..200 {
            let row = daemon
                .state()
                .db()
                .get_session(&session_id)
                .await
                .expect("query");
            if row.is_some_and(|session| session.status == SessionStatus::Stopped) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // 再開 (モックドライバはネイティブ復元非対応 → 履歴 Replay で継続)
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::SessionResume {
                command_id: "r1".to_owned(),
                session_id: session_id.clone(),
                force_replay: false,
            },
        )
        .await;
        match response {
            IpcServerMessage::Result {
                result:
                    IpcResult::SessionResumed {
                        session_id: resumed,
                        context_restored,
                        ..
                    },
                ..
            } => {
                assert_eq!(resumed, session_id);
                assert!(!context_restored, "mock driver cannot restore natively");
            }
            other => panic!("unexpected response: {other:?}"),
        }

        // Replay コンテキストが新エージェントセッションへ送信される
        let mut injected = false;
        for _ in 0..200 {
            if mock
                .prompts()
                .iter()
                .any(|prompt| prompt.contains("hello resume"))
            {
                injected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(injected, "resume context is injected to the agent");

        // 稼働中 (再開済み) セッションへの再実行は INVALID_STATE
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::SessionResume {
                command_id: "r2".to_owned(),
                session_id: session_id.clone(),
                force_replay: false,
            },
        )
        .await;
        match response {
            IpcServerMessage::Error { code, .. } => assert_eq!(code, ErrorCode::InvalidState),
            other => panic!("unexpected response: {other:?}"),
        }

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }
}
