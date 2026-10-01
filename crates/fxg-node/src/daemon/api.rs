//! ローカルノード (`fxg daemon`) の Client API バックエンド実装。
//!
//! `fxg-server` の共通ルーター ([`fxg_server::api::client_router`]) に対し、
//! デーモンの `SessionManager` / `PtySessionManager` / Git 操作を提供する
//! (設計: `docs/03-protocol-and-api.md` §3)。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use fxg_protocol::client_api::{
    AgentOpResponse, AgentsResponse, ConnectionRole, CreateSessionRequest, CreateSessionResponse,
    CreateWorktreeRequest, KillSwitchResponse, ProjectLinkRequest, ProjectLinkResponse,
    ProjectScanRequest, ProjectScanResponse, ProvisionersResponse, PruneWorktreesRequest,
    RemoveWorktreeRequest, RespondPermissionRequest, RespondPermissionResponse,
    ResumeSessionRequest, ResumeSessionResponse, RotateAuthTokenResponse, SessionArchiveRequest,
    SessionArchiveResponse, SessionRevertRequest, SessionRevertResponse, WorktreeInfo,
    WorktreesResponse,
};
use fxg_protocol::common::{
    AgentAction, CommandResult, DiffScope, ProjectSummary, WorkspaceDiffResponse,
};
use fxg_server::api::{
    ApiError, ClientApiBackend, ClientCommand, ClientEvent, ClientInfo, PtyChannelError,
    PtyChannelEvent, PtySpawnParams, SystemExtras,
};
use tokio::sync::broadcast;

use super::DaemonState;
use super::ops::AuditSource;
use crate::error::NodeError;
use crate::session_manager::{ResumeParams, StartSessionParams};

/// PTY WS へ転送するローカル購読のバッファ。
const PTY_CHANNEL_CAPACITY: usize = 1024;

/// ノードエラーを API エラーへ変換する。
fn api_error(err: NodeError) -> ApiError {
    ApiError::from_code(err.error_code(), err.to_string())
}

/// `ClientInfo` を監査ログの記録元へ変換する。
fn audit_source(client: &ClientInfo) -> AuditSource {
    AuditSource {
        client_ip: client.ip.clone(),
        auth_subject: client.auth_subject.clone(),
        user_agent: client.user_agent.clone(),
    }
}

#[async_trait]
impl ClientApiBackend for DaemonState {
    fn db(&self) -> &fxg_db::Db {
        DaemonState::db(self)
    }

    fn auth_token(&self) -> String {
        DaemonState::auth_token(self)
    }

    fn connection_role(&self) -> ConnectionRole {
        ConnectionRole::LocalNode
    }

    /// ローカルノードは紐付けパスの実在確認をライブで行った一覧を返す。
    async fn list_projects(&self) -> Result<Vec<ProjectSummary>, ApiError> {
        self.list_projects_with_path_state()
            .await
            .map_err(api_error)
    }

    async fn system_extras(&self) -> Result<SystemExtras, ApiError> {
        let unsynced = self
            .db()
            .unsynced_event_count()
            .await
            .map_err(ApiError::from)?;
        Ok(SystemExtras {
            vapid_public_key: None,
            unsynced_event_count: Some(unsynced),
            central_connected: Some(self.central_connected()),
        })
    }

    fn subscribe_client_events(&self) -> broadcast::Receiver<ClientEvent> {
        self.inner.client_events.subscribe()
    }

    async fn dispatch_command(&self, command: ClientCommand) -> CommandResult {
        let manager = self.session_manager();
        let (command_id, result) = match command {
            ClientCommand::SendPrompt {
                command_id,
                session_id,
                text,
                client_source,
            } => {
                let result = manager
                    .send_prompt(&command_id, &session_id, &text, &client_source)
                    .await
                    .map(|()| session_id);
                (command_id, result)
            }
            ClientCommand::RespondPermission {
                command_id,
                session_id,
                request_id,
                selected_option_id,
                resolved_by,
            } => {
                let result = manager
                    .respond_permission(
                        &command_id,
                        &session_id,
                        &request_id,
                        &selected_option_id,
                        &resolved_by,
                    )
                    .await
                    .map(|()| session_id);
                (command_id, result)
            }
            ClientCommand::ControlSession {
                command_id,
                session_id,
                action,
            } => {
                let result = manager
                    .control(&command_id, &session_id, &action)
                    .await
                    .map(|()| session_id);
                (command_id, result)
            }
        };

        match result {
            Ok(session_id) => CommandResult {
                command_id,
                success: true,
                code: None,
                error: None,
                session_id: Some(session_id),
            },
            Err(err) => CommandResult {
                command_id,
                success: false,
                code: Some(err.error_code()),
                error: Some(err.to_string()),
                session_id: None,
            },
        }
    }

    async fn create_session(
        &self,
        request: CreateSessionRequest,
        client: ClientInfo,
    ) -> Result<CreateSessionResponse, ApiError> {
        if request.provisioner.is_some() {
            return Err(ApiError::bad_request(
                "provisioners are only available on the central server",
            ));
        }
        if let Some(node_id) = &request.node_id
            && node_id != self.node_id()
        {
            return Err(ApiError::bad_request(format!(
                "requested node {node_id} but this node is {}",
                self.node_id()
            )));
        }
        let source = audit_source(&client);

        // 既存セッションからの Context Fork (同一ノード内)
        if let Some(fork) = &request.fork {
            if fork.restore_git_bundle_b64.is_some() {
                return Err(ApiError::bad_request(
                    "git bundle restore requires an ephemeral node (Phase 6)",
                ));
            }
            // Fork 元の実行ディレクトリの紐付けも最新化する
            if let Some(source) = self
                .db()
                .get_session(&fork.from_session_id)
                .await
                .map_err(ApiError::from)?
            {
                self.resolve_and_register_project(Path::new(&source.local_path))
                    .await
                    .map_err(api_error)?;
            }
            let outcome = self
                .session_manager()
                .fork(
                    &request.command_id,
                    &fork.from_session_id,
                    fork.from_node_seq,
                    Some(&request.agent_id),
                    None,
                )
                .await
                .map_err(api_error)?;
            self.record_audit(
                fxg_db::audit::actions::SESSION_START,
                &source,
                Some(&outcome.session_id),
                serde_json::json!({ "fork_from": fork.from_session_id }),
            )
            .await;
            return Ok(CreateSessionResponse {
                session_id: outcome.session_id,
                command_id: request.command_id,
            });
        }

        // 実行ディレクトリ: Worktree 同時作成 or 既存パス or ノード既定
        // (Worktree 同時作成時は `worktree_add` が紐付けを登録済みのため、
        //  ここでは既存パス・既定解決のときのみ改めて登録する)
        let (local_path, register_binding) = match (&request.worktree, &request.local_path) {
            (Some(spec), _) => {
                let repo = self
                    .project_main_repo(&request.project_id)
                    .await
                    .map_err(api_error)?;
                let outcome = self
                    .worktree_add(
                        &repo,
                        Some(&request.project_id),
                        &spec.branch,
                        spec.base_branch.clone(),
                        spec.new_path.clone().map(PathBuf::from),
                        &source,
                    )
                    .await
                    .map_err(api_error)?;
                (outcome.path, false)
            }
            (None, Some(path)) => (PathBuf::from(path), true),
            (None, None) => {
                let path = self
                    .db()
                    .resolve_default_local_path(&request.project_id, self.node_id())
                    .await
                    .map_err(ApiError::from)?
                    .ok_or_else(|| {
                        ApiError::not_found(format!(
                            "no default directory for project {} on this node; \
                             specify local_path or create a worktree",
                            request.project_id
                        ))
                    })?;
                (PathBuf::from(path), true)
            }
        };

        if register_binding {
            // 実行ディレクトリの紐付け (last_used_at / ブランチ) を最新化する
            self.resolve_and_register_project(&local_path)
                .await
                .map_err(api_error)?;
        }

        let session_id = fxg_protocol::util::uuid_v7();
        let outcome = self
            .session_manager()
            .start_session(StartSessionParams {
                command_id: &request.command_id,
                session_id: &session_id,
                local_path: &local_path,
                agent_id: &request.agent_id,
                initial_prompt: request.initial_prompt.as_deref(),
                mode: request.mode.as_deref(),
                opencode_mode: request.opencode_mode.as_deref(),
                extra_args: request.extra_args.as_deref(),
                fork_context: None,
                restore_git_bundle: None,
            })
            .await
            .map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_START,
            &source,
            Some(&outcome.session_id),
            serde_json::json!({
                "agent_id": request.agent_id,
                "local_path": local_path.to_string_lossy(),
            }),
        )
        .await;

        Ok(CreateSessionResponse {
            session_id: outcome.session_id,
            command_id: request.command_id,
        })
    }

    async fn respond_permission(
        &self,
        session_id: &str,
        request_id: &str,
        request: RespondPermissionRequest,
        client: ClientInfo,
    ) -> Result<RespondPermissionResponse, ApiError> {
        let selected = self
            .resolve_permission_option(session_id, request_id, &request)
            .await?;
        let command_id = fxg_protocol::util::uuid_v7();
        match self
            .session_manager()
            .respond_permission(
                &command_id,
                session_id,
                request_id,
                &selected,
                &request.resolved_by,
            )
            .await
        {
            Ok(()) => {
                self.record_audit(
                    fxg_db::audit::actions::PERMISSION_RESOLVED,
                    &audit_source(&client),
                    Some(session_id),
                    serde_json::json!({
                        "request_id": request_id,
                        "selected_option_id": selected,
                        "resolved_by": request.resolved_by,
                    }),
                )
                .await;
                Ok(RespondPermissionResponse {
                    already_resolved: false,
                })
            }
            // 全クライアント横断の冪等解決: 2回目以降は正常遷移として扱う
            Err(NodeError::AlreadyResolved(_)) => Ok(RespondPermissionResponse {
                already_resolved: true,
            }),
            Err(err) => Err(api_error(err)),
        }
    }

    async fn revert_session(
        &self,
        session_id: &str,
        request: SessionRevertRequest,
        client: ClientInfo,
    ) -> Result<SessionRevertResponse, ApiError> {
        let command_id = fxg_protocol::util::uuid_v7();
        let outcome = self
            .session_manager()
            .revert(&command_id, session_id, request.target_node_seq)
            .await
            .map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_REVERT,
            &audit_source(&client),
            Some(session_id),
            serde_json::json!({
                "target_node_seq": outcome.target_node_seq,
                "restored_tree_hash": outcome.restored_tree_hash,
                "restored_files": outcome.restored_files,
                "removed_files": outcome.removed_files,
            }),
        )
        .await;
        Ok(SessionRevertResponse {
            target_node_seq: outcome.target_node_seq,
            restored_tree_hash: outcome.restored_tree_hash,
            backup_tree_hash: outcome.backup_tree_hash,
            restored_files: outcome.restored_files as u64,
            removed_files: outcome.removed_files as u64,
        })
    }

    async fn resume_session(
        &self,
        session_id: &str,
        request: ResumeSessionRequest,
        client: ClientInfo,
    ) -> Result<ResumeSessionResponse, ApiError> {
        let command_id = fxg_protocol::util::uuid_v7();
        let outcome = self
            .session_manager()
            .resume(ResumeParams {
                command_id: &command_id,
                session_id,
                force_replay: request.force_replay,
            })
            .await
            .map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_RESUME,
            &audit_source(&client),
            Some(session_id),
            serde_json::json!({
                "force_replay": request.force_replay,
                "context_restored": outcome.context_restored,
            }),
        )
        .await;
        Ok(ResumeSessionResponse {
            session_id: outcome.session_id,
            context_restored: outcome.context_restored,
        })
    }

    async fn archive_session(
        &self,
        session_id: &str,
        request: SessionArchiveRequest,
        client: ClientInfo,
    ) -> Result<SessionArchiveResponse, ApiError> {
        let command_id = fxg_protocol::util::uuid_v7();
        let archived_at = self
            .set_session_archived(
                &command_id,
                session_id,
                request.archived,
                &audit_source(&client),
            )
            .await
            .map_err(api_error)?;
        Ok(SessionArchiveResponse {
            session_id: session_id.to_owned(),
            archived_at,
        })
    }

    async fn delete_session(&self, session_id: &str, client: ClientInfo) -> Result<(), ApiError> {
        let command_id = fxg_protocol::util::uuid_v7();
        self.purge_session(&command_id, session_id, &audit_source(&client))
            .await
            .map_err(api_error)?;
        Ok(())
    }

    async fn list_worktrees(&self, project_id: &str) -> Result<WorktreesResponse, ApiError> {
        let worktrees = self
            .worktree_list(None, Some(project_id))
            .await
            .map_err(api_error)?;
        Ok(WorktreesResponse { worktrees })
    }

    async fn create_worktree(
        &self,
        project_id: &str,
        request: CreateWorktreeRequest,
        client: ClientInfo,
    ) -> Result<WorktreeInfo, ApiError> {
        if request.node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {} but this node is {}",
                request.node_id,
                self.node_id()
            )));
        }
        let repo = self
            .project_main_repo(project_id)
            .await
            .map_err(api_error)?;
        let outcome = self
            .worktree_add(
                &repo,
                Some(project_id),
                &request.branch,
                request.base_branch,
                request.path.map(PathBuf::from),
                &audit_source(&client),
            )
            .await
            .map_err(api_error)?;
        let head_commit = crate::git::head_commit(&outcome.path).await.unwrap_or(None);
        Ok(WorktreeInfo {
            node_id: self.node_id().to_owned(),
            path: outcome.path.to_string_lossy().into_owned(),
            branch: Some(outcome.branch),
            head_commit,
            is_main: false,
        })
    }

    async fn remove_worktree(
        &self,
        _project_id: &str,
        request: RemoveWorktreeRequest,
        client: ClientInfo,
    ) -> Result<(), ApiError> {
        if request.node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {} but this node is {}",
                request.node_id,
                self.node_id()
            )));
        }
        self.worktree_remove(
            None,
            &PathBuf::from(&request.path),
            request.force,
            &audit_source(&client),
        )
        .await
        .map_err(api_error)
    }

    async fn prune_worktrees(
        &self,
        project_id: &str,
        request: PruneWorktreesRequest,
        client: ClientInfo,
    ) -> Result<(), ApiError> {
        if request.node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {} but this node is {}",
                request.node_id,
                self.node_id()
            )));
        }
        let repo = self
            .project_main_repo(project_id)
            .await
            .map_err(api_error)?;
        self.worktree_prune(&repo, &audit_source(&client))
            .await
            .map_err(api_error)?;
        Ok(())
    }

    async fn scan_projects(
        &self,
        node_id: &str,
        request: ProjectScanRequest,
        client: ClientInfo,
    ) -> Result<ProjectScanResponse, ApiError> {
        if node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {node_id} but this node is {}",
                self.node_id()
            )));
        }
        let scanned_dirs = self
            .project_scan(request.dir.as_deref().map(Path::new))
            .await
            .map_err(api_error)?;
        let projects = self.db().list_projects().await.map_err(ApiError::from)?;
        self.record_audit(
            fxg_db::audit::actions::PROJECT_LINK,
            &audit_source(&client),
            None,
            serde_json::json!({
                "action": "scan",
                "scanned_dirs": scanned_dirs,
            }),
        )
        .await;
        Ok(ProjectScanResponse {
            scanned_dirs,
            projects,
        })
    }

    async fn link_project(
        &self,
        node_id: &str,
        request: ProjectLinkRequest,
        client: ClientInfo,
    ) -> Result<ProjectLinkResponse, ApiError> {
        if node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {node_id} but this node is {}",
                self.node_id()
            )));
        }
        let dir = PathBuf::from(&request.local_path);
        if !dir.is_dir() {
            return Err(ApiError::not_found(format!(
                "directory not found: {}",
                request.local_path
            )));
        }
        let resolved = self
            .project_link(&dir, &request.project_id)
            .await
            .map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::PROJECT_LINK,
            &audit_source(&client),
            None,
            serde_json::json!({
                "action": "link",
                "project_id": resolved.project_id,
                "local_path": resolved.local_path.to_string_lossy(),
            }),
        )
        .await;
        Ok(ProjectLinkResponse {
            project_id: resolved.project_id,
            local_path: resolved.local_path.to_string_lossy().into_owned(),
        })
    }

    async fn list_agents(&self) -> Result<AgentsResponse, ApiError> {
        self.agents_catalog().await.map_err(api_error)
    }

    async fn manage_agent(
        &self,
        node_id: &str,
        action: AgentAction,
        client: ClientInfo,
    ) -> Result<AgentOpResponse, ApiError> {
        if node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {node_id} but this node is {}",
                self.node_id()
            )));
        }
        let message = DaemonState::manage_agent(self, &action)
            .await
            .map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::AGENT_MANAGE,
            &audit_source(&client),
            None,
            serde_json::json!({ "action": action, "message": &message }),
        )
        .await;
        // 導入状態の変化をハブへ報告する (nodes.installed_agents の更新)
        self.trigger_node_hello();
        Ok(AgentOpResponse {
            message: Some(message),
        })
    }

    async fn rotate_auth_token(
        &self,
        client: ClientInfo,
    ) -> Result<RotateAuthTokenResponse, ApiError> {
        let token = DaemonState::rotate_auth_token(self).map_err(api_error)?;
        // 監査ログにはトークン本体を記録しない
        self.record_audit(
            fxg_db::audit::actions::AUTH_TOKEN_ROTATE,
            &audit_source(&client),
            None,
            serde_json::json!({ "scope": "node" }),
        )
        .await;
        Ok(RotateAuthTokenResponse { token })
    }

    async fn workspace_diff(
        &self,
        session_id: &str,
        scope: DiffScope,
        base_branch: Option<String>,
    ) -> Result<WorkspaceDiffResponse, ApiError> {
        let session = self
            .db()
            .get_session(session_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found(format!("session not found: {session_id}")))?;
        crate::git::workspace_diff(
            Path::new(&session.local_path),
            scope,
            base_branch.as_deref(),
        )
        .await
        .map_err(api_error)
    }

    async fn browse_fs(
        &self,
        node_id: &str,
        path: Option<String>,
    ) -> Result<fxg_protocol::client_api::FsBrowseResponse, ApiError> {
        if node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {} but this node is {}",
                node_id,
                self.node_id()
            )));
        }
        crate::fs_browse::browse_fs(path.as_deref()).map_err(api_error)
    }

    async fn provisioners(&self) -> Result<ProvisionersResponse, ApiError> {
        // 一時VMプロビジョナーは中央サーバーでのみ実行される (設計: docs/01 §6.1)
        Ok(ProvisionersResponse {
            provisioners: Vec::new(),
        })
    }

    async fn kill_switch(
        &self,
        reason: &str,
        client: ClientInfo,
    ) -> Result<KillSwitchResponse, ApiError> {
        let (killed_sessions, killed_ptys) =
            self.kill_all_local(reason).await.map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::KILL_SWITCH,
            &audit_source(&client),
            None,
            serde_json::json!({
                "reason": reason,
                "killed_sessions": killed_sessions,
                "killed_ptys": killed_ptys,
            }),
        )
        .await;
        Ok(KillSwitchResponse { notified_nodes: 1 })
    }

    async fn pty_spawn(&self, params: PtySpawnParams) -> Result<(), PtyChannelError> {
        let session = self
            .db()
            .get_session(&params.session_id)
            .await
            .map_err(PtyChannelError::internal)?
            .ok_or_else(|| {
                PtyChannelError::not_found(format!("session not found: {}", params.session_id))
            })?;
        let mut request = fxg_pty::PtySpawnRequest::new(params.pty_id.clone());
        request.session_id = Some(params.session_id.clone());
        request.cwd = Some(PathBuf::from(&session.local_path));
        request.shell_cmd = params.shell_cmd.clone();
        request.cols = params.cols;
        request.rows = params.rows;
        self.pty()
            .spawn(request)
            .map_err(PtyChannelError::internal)?;
        self.record_audit(
            fxg_db::audit::actions::PTY_SPAWN,
            &AuditSource::local(),
            Some(&params.session_id),
            serde_json::json!({ "pty_id": params.pty_id }),
        )
        .await;
        Ok(())
    }

    async fn pty_attach(&self, pty_id: &str) -> Result<(), PtyChannelError> {
        if self.pty().contains(pty_id) {
            Ok(())
        } else {
            Err(PtyChannelError::not_found(format!(
                "pty session not found: {pty_id}"
            )))
        }
    }

    async fn pty_subscribe(
        &self,
        pty_id: &str,
    ) -> Result<broadcast::Receiver<PtyChannelEvent>, PtyChannelError> {
        let mut events = self.pty().subscribe(pty_id).map_err(map_pty_error)?;
        let (tx, rx) = broadcast::channel(PTY_CHANNEL_CAPACITY);
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(fxg_pty::PtyEvent::Output { data, .. }) => {
                        if tx.send(PtyChannelEvent::Output { data }).is_err() {
                            break;
                        }
                    }
                    Ok(fxg_pty::PtyEvent::Exit { exit_code, .. }) => {
                        let _ = tx.send(PtyChannelEvent::Exit { exit_code });
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "pty subscriber lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(rx)
    }

    async fn pty_write(&self, pty_id: &str, data: &[u8]) -> Result<(), PtyChannelError> {
        self.pty()
            .write(pty_id, data)
            .map(|_| ())
            .map_err(map_pty_error)
    }

    async fn pty_resize(&self, pty_id: &str, cols: u16, rows: u16) -> Result<(), PtyChannelError> {
        self.pty().resize(pty_id, cols, rows).map_err(map_pty_error)
    }

    async fn pty_kill(&self, pty_id: &str) -> Result<(), PtyChannelError> {
        self.pty().kill(pty_id).map_err(map_pty_error)
    }

    async fn pty_disconnect(&self, pty_id: &str) {
        let _ = self.pty().kill(pty_id);
    }
}

impl DaemonState {
    /// `always = true` の場合に `allow_always` 系の選択肢へ昇格した `option_id` を返す。
    ///
    /// 承認リクエストの選択肢はノードの投影 (`permission_requests`) から取得する。
    async fn resolve_permission_option(
        &self,
        session_id: &str,
        request_id: &str,
        request: &RespondPermissionRequest,
    ) -> Result<String, ApiError> {
        if !request.always {
            return Ok(request.selected_option_id.clone());
        }
        let pending = self
            .db()
            .pending_permissions()
            .await
            .map_err(ApiError::from)?;
        if let Some(entry) = pending
            .iter()
            .find(|entry| entry.session_id == session_id && entry.request_id == request_id)
            && let Some(option) = entry.options.iter().find(|option| {
                option.kind.contains("always") || option.option_id.contains("always")
            })
        {
            return Ok(option.option_id.clone());
        }
        Ok(request.selected_option_id.clone())
    }
}

/// `fxg-pty` エラーを PTY チャネルエラーへ変換する。
fn map_pty_error(err: fxg_pty::PtyError) -> PtyChannelError {
    match err {
        fxg_pty::PtyError::NotFound(id) => {
            PtyChannelError::not_found(format!("pty session not found: {id}"))
        }
        other => PtyChannelError::internal(other),
    }
}
