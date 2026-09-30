//! CLI (`fxg`) ⇔ Daemon のローカルIPCサーバー。
//!
//! - **トランスポート**: Windows は Named Pipe (`\\.\pipe\fxg-daemon-<username>`)、
//!   Linux / macOS / WSL は Unix Domain Socket (`$XDG_RUNTIME_DIR/fxg/daemon.sock`
//!   または `/tmp/fxg-<uid>/daemon.sock`)
//! - **フレーミング**: Length-prefixed JSON
//!   (4バイト LE 長さヘッダ + JSON ペイロード)
//!
//! 設計: `docs/03-protocol-and-api.md` §4。
//! Phase 2 ではリクエスト/レスポンスのみを扱う (イベントストリーミング
//! (`AttachSession`) は Phase 3 のエージェントドライバ実装と同時に追加する)。

use std::path::{Path, PathBuf};

use fxg_db::SessionFilter;
use fxg_protocol::common::ErrorCode;
use fxg_protocol::config::{GlobalConfig, ProjectConfig};
use fxg_protocol::ipc::{IpcClientMessage, IpcResult, IpcServerMessage, ProjectInfo};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;

use super::DaemonState;
use crate::error::NodeError;
use crate::{git, project, worktree};

/// 受信フレームの最大長 (過大なフレームによるメモリ枯渇を防ぐ)。
const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

/// `fxg ps` などで返すセッション一覧の上限。
const SESSION_LIST_LIMIT: u32 = 200;

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
                        tokio::spawn(async move {
                            if let Err(err) = handle_connection(state, stream).await {
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

    tracing::info!(endpoint = %endpoint, "local ipc server listening (named pipe)");
    loop {
        // 接続ごとに新しいパイプインスタンスを用意する
        let server = ServerOptions::new()
            .first_pipe_instance(false)
            .create(&endpoint)
            .map_err(|err| {
                NodeError::Server(format!("failed to create named pipe {endpoint}: {err}"))
            })?;

        tokio::select! {
            _ = shutdown.changed() => break,
            connected = server.connect() => {
                match connected {
                    Ok(()) => {
                        let state = state.clone();
                        tokio::spawn(async move {
                            if let Err(err) = handle_connection(state, server).await {
                                tracing::debug!("ipc connection ended: {err}");
                            }
                        });
                    }
                    Err(err) => tracing::warn!("named pipe connect failed: {err}"),
                }
            }
        }
    }
    Ok(())
}

/// 1接続を処理する (Length-prefixed JSON のリクエスト/レスポンスループ)。
pub(crate) async fn handle_connection<S>(state: DaemonState, mut stream: S) -> Result<(), NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let Some(frame) = read_frame(&mut stream).await? else {
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
                write_message(&mut stream, &response).await?;
                continue;
            }
        };
        let response = dispatch(&state, request).await;
        write_message(&mut stream, &response).await?;
    }
}

async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Option<Vec<u8>>, NodeError> {
    let mut length_buf = [0u8; 4];
    match stream.read_exact(&mut length_buf).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => {
            return Err(NodeError::Server(format!("ipc length read failed: {err}")));
        }
    }
    let length = u32::from_le_bytes(length_buf);
    if length > MAX_FRAME_BYTES {
        return Err(NodeError::Server(format!("ipc frame too large: {length}")));
    }
    let mut payload = vec![0u8; length as usize];
    stream
        .read_exact(&mut payload)
        .await
        .map_err(|err| NodeError::Server(format!("ipc payload read failed: {err}")))?;
    Ok(Some(payload))
}

async fn write_message<S: AsyncWrite + Unpin>(
    stream: &mut S,
    message: &IpcServerMessage,
) -> Result<(), NodeError> {
    let payload = serde_json::to_vec(message)?;
    let length = u32::try_from(payload.len())
        .map_err(|_| NodeError::Server("ipc frame too large".to_owned()))?;
    stream
        .write_all(&length.to_le_bytes())
        .await
        .map_err(|err| NodeError::Server(format!("ipc write failed: {err}")))?;
    stream
        .write_all(&payload)
        .await
        .map_err(|err| NodeError::Server(format!("ipc write failed: {err}")))?;
    stream
        .flush()
        .await
        .map_err(|err| NodeError::Server(format!("ipc flush failed: {err}")))?;
    Ok(())
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
        let code = match &err {
            NodeError::NotARepository(_) => ErrorCode::InvalidState,
            NodeError::InvalidWorktree(_) | NodeError::InvalidSession(_) => ErrorCode::InvalidState,
            NodeError::NonPersistableEvent(_) => ErrorCode::InvalidState,
            NodeError::Db(fxg_db::DbError::SessionNotFound(_)) => ErrorCode::NotFound,
            NodeError::Db(fxg_db::DbError::NodeNotFound(_)) => ErrorCode::NotFound,
            NodeError::Config(_) => ErrorCode::InvalidState,
            _ => ErrorCode::Internal,
        };
        Self::new(command_id, code, err.to_string())
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

        // エージェントセッションの起動・アタッチは Phase 3 (fxg-acp) で実装する
        IpcClientMessage::EnsureSession { command_id, .. }
        | IpcClientMessage::AttachSession { command_id, .. }
        | IpcClientMessage::SendPrompt { command_id, .. }
        | IpcClientMessage::RespondPermission { command_id, .. }
        | IpcClientMessage::ControlSession { command_id, .. } => Err(DispatchError::new(
            &command_id,
            ErrorCode::InvalidState,
            "agent sessions are implemented in phase 3 (fxg-acp)",
        )),
    }
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
    use std::time::Duration;

    async fn start_daemon() -> (NodeDaemon, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "ipc-node", "IPC Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = dir
            .path()
            .join("daemon.sock")
            .to_string_lossy()
            .into_owned();
        let daemon = NodeDaemon::start(config).await.expect("start");
        wait_for_endpoint(daemon.ipc_endpoint()).await;
        (daemon, dir)
    }

    /// IPC サーバーが接続を受け付けられるようになるまで待機する。
    #[cfg(unix)]
    async fn wait_for_endpoint(endpoint: &str) {
        for _ in 0..200 {
            if tokio::net::UnixStream::connect(endpoint).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("ipc endpoint did not become ready: {endpoint}");
    }

    #[cfg(not(unix))]
    async fn wait_for_endpoint(_endpoint: &str) {}

    /// IPC 接続して1リクエスト送信し、レスポンスを受信する。
    #[cfg(unix)]
    async fn roundtrip(endpoint: &str, request: &IpcClientMessage) -> IpcServerMessage {
        let stream = tokio::net::UnixStream::connect(endpoint)
            .await
            .expect("connect");
        let mut stream = stream;
        let payload = serde_json::to_vec(request).expect("encode");
        let length = u32::try_from(payload.len()).expect("len");
        stream
            .write_all(&length.to_le_bytes())
            .await
            .expect("write length");
        stream.write_all(&payload).await.expect("write payload");
        stream.flush().await.expect("flush");

        let mut length_buf = [0u8; 4];
        stream
            .read_exact(&mut length_buf)
            .await
            .expect("read length");
        let mut response = vec![0u8; u32::from_le_bytes(length_buf) as usize];
        stream
            .read_exact(&mut response)
            .await
            .expect("read payload");
        serde_json::from_slice(&response).expect("decode")
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
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

        // Phase 3 のコマンドは明示的にエラーを返す
        let response = roundtrip(
            &endpoint,
            &IpcClientMessage::EnsureSession {
                command_id: "e1".to_owned(),
                cwd: repo.to_string_lossy().into_owned(),
                agent_id: "opencode2".to_owned(),
                extra_args: Vec::new(),
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

    #[cfg(unix)]
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
