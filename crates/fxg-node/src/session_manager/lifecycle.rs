use super::prompt::*;
use super::replay::*;
use super::stream::*;
use super::*;

impl SessionManager {
    /// Starts a new session (`fxg session new`).
    pub async fn ensure_session(
        &self,
        command_id: &CommandId,
        cwd: &Path,
        agent_id: &str,
        extra_args: &[String],
        initial_mode: Option<&str>,
    ) -> Result<EnsureSessionOutcome, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_string()));
        }

        let resolved =
            project::resolve_project(cwd, &self.inner.node_id, self.inner.bus.db()).await?;
        let session_id = uuid_v7();

        let spec = self.resolve_launch_spec(agent_id, extra_args).await?;

        // SessionCreated (node_seq = 1)
        let (git_branch, is_worktree) =
            git_metadata_or_warn(&resolved.local_path, &session_id).await;
        let title = format!("{} @ {}", spec.display_name, resolved.name);
        self.inner
            .bus
            .create_session(
                &session_id,
                UnifiedEventPayload::SessionCreated {
                    node_id: self.inner.node_id.clone().into(),
                    project_id: resolved.project_id.clone().into(),
                    project_name: resolved.name.clone(),
                    local_path: resolved.local_path.to_string_lossy().into_owned(),
                    git_branch,
                    is_worktree,
                    agent_id: spec.agent_id.clone(),
                    parent_session_id: None,
                    title: title.clone(),
                },
            )
            .await?;

        let _started = self
            .start_driver_session(StartDriverParams {
                session_id: &session_id,
                title: &title,
                cwd: &resolved.local_path,
                spec: &spec,
                extra_args: extra_args.to_vec(),
                initial_mode: initial_mode.map(str::to_owned),
                has_custom_title: false,
                resume: None,
                fork_from: None,
                initial_config: Vec::new(),
            })
            .await?;

        Ok(EnsureSessionOutcome { session_id })
    }

    /// Starts a new session with an explicit session ID (`StartSession` / local REST).
    ///
    /// Differs from [`Self::ensure_session`] in that (1) the caller assigns
    /// `session_id` and (2) it takes a resolved `local_path` instead of `cwd`.
    /// Project resolution (`project_id`) uses the node-side authoritative data.
    pub async fn start_session(
        &self,
        params: StartSessionParams<'_>,
    ) -> Result<EnsureSessionOutcome, NodeError> {
        if !self.begin_command(params.command_id) {
            return Err(NodeError::CommandDuplicate(params.command_id.to_string()));
        }
        self.start_session_inner(params).await
    }

    /// Body of [`Self::start_session`] (the caller owns command deduplication).
    pub(super) async fn start_session_inner(
        &self,
        params: StartSessionParams<'_>,
    ) -> Result<EnsureSessionOutcome, NodeError> {
        // ACP-native fork: the source session must live on this node and use the
        // same agent. The caller-provided agent ID is ignored in that case.
        let fork_source = if let Some(source_id) = params.fork_from_session_id {
            Some(
                self.inner
                    .bus
                    .db()
                    .get_session(source_id)
                    .await?
                    .ok_or_else(|| NodeError::InvalidSession(source_id.to_owned()))?,
            )
        } else {
            None
        };
        let fork_from = match fork_source.as_ref() {
            Some(source) => Some(source.agent_session_id.clone().ok_or_else(|| {
                NodeError::ForkUnsupported(format!(
                    "source session {} has no agent session binding",
                    source.session_id
                ))
            })?),
            None => None,
        };
        let effective_agent_id = fork_source
            .as_ref()
            .map_or(params.agent_id, |source| source.agent_id.as_str());

        let resolved =
            project::resolve_project(params.local_path, &self.inner.node_id, self.inner.bus.db())
                .await?;
        let extra_args_vec: Vec<String> = params.extra_args.map(|s| s.to_vec()).unwrap_or_default();
        let spec = self
            .resolve_launch_spec(effective_agent_id, &extra_args_vec)
            .await?;

        // SessionCreated (node_seq = 1)
        let (git_branch, is_worktree) =
            git_metadata_or_warn(&resolved.local_path, params.session_id).await;
        let default_title = match fork_source.as_ref() {
            Some(source) => format!("Fork of {}", source.title),
            None => format!("{} @ {}", spec.display_name, resolved.name),
        };
        let (title, has_custom_title) = if let Some(prompt) = params.initial_prompt
            && let Some(derived) = derive_title_from_prompt(&prompt.text)
        {
            (derived, true)
        } else {
            (default_title, false)
        };
        self.inner
            .bus
            .create_session(
                params.session_id,
                UnifiedEventPayload::SessionCreated {
                    node_id: self.inner.node_id.clone().into(),
                    project_id: resolved.project_id.clone().into(),
                    project_name: resolved.name.clone(),
                    local_path: resolved.local_path.to_string_lossy().into_owned(),
                    git_branch,
                    is_worktree,
                    agent_id: spec.agent_id.clone(),
                    parent_session_id: fork_source.as_ref().map(|source| source.session_id.clone()),
                    title: title.clone(),
                },
            )
            .await?;

        let _started = self
            .start_driver_session(StartDriverParams {
                session_id: params.session_id,
                title: &title,
                cwd: &resolved.local_path,
                spec: &spec,
                extra_args: extra_args_vec,
                initial_mode: params.mode.map(str::to_owned),
                has_custom_title,
                resume: None,
                fork_from: fork_from.as_deref(),
                initial_config: params.initial_config.to_vec(),
            })
            .await?;

        // Initial prompt (executed as-is when a session is started remotely)
        if let Some(prompt) = params.initial_prompt.filter(|prompt| !prompt.is_empty())
            && let Err(err) = self
                .start_turn(params.session_id, prompt, ClientSource::Web)
                .await
        {
            return Err(self
                .fail_context_injection(params.session_id, "initial prompt injection", err)
                .await);
        }

        Ok(EnsureSessionOutcome {
            session_id: params.session_id.to_owned(),
        })
    }

    /// Resolves the launch spec (custom/built-in agents need no index).
    pub(super) async fn resolve_launch_spec(
        &self,
        agent_id: &str,
        extra_args: &[String],
    ) -> Result<AgentLaunchSpec, NodeError> {
        match self
            .inner
            .registry
            .launch_spec(agent_id, &RegistryIndex::default(), extra_args)
            .await
        {
            Ok(spec) => Ok(spec),
            Err(_) => {
                let index = self
                    .inner
                    .registry
                    .index(false)
                    .await
                    .map_err(|err| NodeError::Agent(err.to_string()))?;
                self.inner
                    .registry
                    .launch_spec(agent_id, &index, extra_args)
                    .await
                    .map_err(|err| NodeError::Agent(err.to_string()))
            }
        }
    }

    /// Launches the driver, registers it in the active list, and starts the event pump.
    ///
    /// Records `StatusChanged(Error)` on launch failure. On success, returns the
    /// handle used by the CLI to decide attach mode and the native restore result.
    pub(super) async fn start_driver_session(
        &self,
        params: StartDriverParams<'_>,
    ) -> Result<StartedDriverSession, NodeError> {
        // Inject the Git credential proxy (GIT_ASKPASS for ephemeral VMs) into the agent process.
        // The script is generated at daemon startup and authenticates via
        // `fxg internal git-askpass` → local IPC → the hub's in-memory relay
        // (design: docs/01 §6.4).
        let mut spec = params.spec.clone();
        let askpass = crate::credentials::askpass_script_path(self.inner.paths.fxg_home());
        if askpass.exists() {
            for (key, value) in crate::credentials::git_env(self.inner.paths.fxg_home()) {
                spec.env.retain(|(k, _)| k != &key);
                spec.env.push((key, value));
            }
        }
        let params = StartDriverParams {
            spec: &spec,
            ..params
        };

        let driver =
            (self.inner.factory)(params.spec).map_err(|err| NodeError::Agent(err.to_string()))?;
        self.remember_driver(&driver);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let started = match driver
            .start_session(
                fxg_acp::StartSessionRequest {
                    session_id: params.session_id.to_owned(),
                    title: Some(params.title.to_owned()),
                    cwd: params.cwd.to_path_buf(),
                    launch: params.spec.clone(),
                    extra_args: params.extra_args.clone(),
                    initial_mode: params.initial_mode.clone(),
                    resume: params.resume.clone(),
                    fork_from: params.fork_from.map(str::to_owned),
                    initial_config: params.initial_config.clone(),
                },
                event_tx.clone(),
            )
            .await
        {
            Ok(started) => started,
            Err(err) => {
                // The agent requires authentication before a session can be created.
                // Surface the advertised methods so CLI / UI can guide the login flow.
                if let Some(required) = err.downcast_ref::<AgentAuthRequired>() {
                    let message = format!(
                        "agent {} requires authentication ({} method(s) advertised)",
                        params.spec.agent_id,
                        required.0.len()
                    );
                    tracing::info!(session_id = params.session_id, "{message}");
                    self.inner
                        .bus
                        .record(
                            params.session_id,
                            UnifiedEventPayload::StatusChanged {
                                status: SessionStatus::Error,
                                error_message: Some(message),
                            },
                        )
                        .await?;
                    return Err(NodeError::AuthRequired {
                        agent_id: params.spec.agent_id.clone(),
                        methods: required.0.clone(),
                    });
                }
                // ACP-native forks are the only fork implementation. Report the
                // missing capability instead of silently degrading to a replay.
                if let Some(unavailable) = err.downcast_ref::<NativeForkUnavailable>() {
                    tracing::info!(
                        session_id = params.session_id,
                        "native session fork is not available: {unavailable}"
                    );
                    return Err(NodeError::ForkUnsupported(unavailable.0.clone()));
                }
                // In native-only auto-resume (`allow_fresh = false`), return
                // "native restore unavailable" to the caller while leaving the session stopped
                // (do not implicitly run a history replay; prompt for an explicit Resume)
                if let Some(unavailable) = err.downcast_ref::<NativeResumeUnavailable>() {
                    tracing::info!(
                        session_id = params.session_id,
                        "native resume is not available: {unavailable}"
                    );
                    return Err(NodeError::ResumeRequired {
                        session_id: params.session_id.to_owned(),
                        reason: unavailable.0.clone(),
                    });
                }
                let message = format!("failed to start agent: {err:#}");
                tracing::warn!(session_id = params.session_id, "{message}");
                self.inner
                    .bus
                    .record(
                        params.session_id,
                        UnifiedEventPayload::StatusChanged {
                            status: SessionStatus::Error,
                            error_message: Some(message.clone()),
                        },
                    )
                    .await?;
                return Err(NodeError::Agent(message));
            }
        };

        let context_restored = started.context_restored;
        let handle: Arc<dyn ActiveSessionHandle> = Arc::from(started.handle);
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            sessions.active.insert(
                params.session_id.to_owned(),
                ActiveSession {
                    handle: Arc::clone(&handle),
                    cwd: params.cwd.to_path_buf(),
                    agent_id: params.spec.agent_id.clone(),
                    status: SessionStatus::Idle,
                    busy: false,
                    pending_prompts: VecDeque::new(),
                    pending_permissions: HashMap::new(),
                    pending_elicitations: HashMap::new(),
                    has_custom_title: params.has_custom_title,
                    permission_policy: PermissionPolicy::Ask,
                },
            );
        }

        // Event pump
        let inner = Arc::clone(&self.inner);
        let pump_session_id = params.session_id.to_owned();
        fxg_protocol::spawn_guarded!(
            "session event pump",
            pump_events(inner, pump_session_id, event_rx)
        );
        Ok(StartedDriverSession { context_restored })
    }
    /// Imports an existing agent session by recording its agent-side ID and
    /// resuming it natively (`session/resume` / `session/load`).
    pub async fn import_session(
        &self,
        command_id: &CommandId,
        agent_id: &str,
        agent_session_id: &str,
        cwd: &Path,
        title: Option<&str>,
    ) -> Result<String, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_string()));
        }
        let resolved =
            project::resolve_project(cwd, &self.inner.node_id, self.inner.bus.db()).await?;
        let spec = self.resolve_launch_spec(agent_id, &[]).await?;
        let session_id = uuid_v7();
        let (git_branch, is_worktree) =
            git_metadata_or_warn(&resolved.local_path, &session_id).await;
        let title = title
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{} (imported)", spec.display_name));
        self.inner
            .bus
            .create_session(
                &session_id,
                UnifiedEventPayload::SessionCreated {
                    node_id: self.inner.node_id.clone().into(),
                    project_id: resolved.project_id.clone().into(),
                    project_name: resolved.name.clone(),
                    local_path: resolved.local_path.to_string_lossy().into_owned(),
                    git_branch,
                    is_worktree,
                    agent_id: spec.agent_id.clone(),
                    parent_session_id: None,
                    title: title.clone(),
                },
            )
            .await?;
        self.start_driver_session(StartDriverParams {
            session_id: &session_id,
            title: &title,
            cwd: &resolved.local_path,
            spec: &spec,
            extra_args: Vec::new(),
            initial_mode: None,
            has_custom_title: true,
            resume: Some(ResumeRequest {
                agent_session_id: Some(agent_session_id.to_owned()),
                allow_fresh: false,
            }),
            fork_from: None,
            initial_config: Vec::new(),
        })
        .await?;
        Ok(session_id)
    }

    /// Branches the session with the ACP-native `session/fork` + `session/load`
    /// pair (`fxg session fork`).
    ///
    /// - The source session must use the same agent and have an
    ///   `agent_session_id`; otherwise [`NodeError::ForkUnsupported`] is returned.
    /// - The new session records `parent_session_id` for lineage.
    /// - No history replay is injected: the agent carries its own context.
    pub async fn fork(
        &self,
        command_id: &CommandId,
        session_id: &str,
        new_cwd: Option<&Path>,
    ) -> Result<EnsureSessionOutcome, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_string()));
        }

        let source = self
            .inner
            .bus
            .db()
            .get_session(session_id)
            .await?
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;

        let new_session_id = uuid_v7();
        let cwd = new_cwd
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(&source.local_path));
        self.start_session_inner(StartSessionParams {
            command_id,
            session_id: &new_session_id,
            local_path: &cwd,
            agent_id: &source.agent_id,
            initial_prompt: None,
            initial_config: &[],
            mode: None,
            extra_args: None,
            fork_from_session_id: Some(source.session_id.as_str()),
        })
        .await
    }

    /// Resumes a stopped session (`fxg session resume`).
    ///
    /// - Starts the agent under the same `session_id`, preferring native restore
    ///   (`session/resume` → `session/load` → bind an existing OpenCode2 session).
    ///   If restore fails, continues by injecting a history replay.
    /// - Resumability is based on `sessions.active` (the actual running state).
    ///   On forced termination no transition to `stopped` is written, so resuming
    ///   is allowed even when the DB `status` is still `idle` / `running`.
    /// - Ephemeral VM (`provisioning` / `bootstrapping`) sessions cannot be
    ///   resumed in v1 (design: `docs/04-agent-drivers-and-windows.md` §4.1).
    pub async fn resume(&self, params: ResumeParams<'_>) -> Result<ResumeOutcome, NodeError> {
        if !self.begin_command(params.command_id) {
            return Err(NodeError::CommandDuplicate(params.command_id.to_string()));
        }

        // Exclude double resumes (reject both active and currently starting sessions)
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            if sessions.active.contains_key(params.session_id)
                || !sessions.resuming.insert(params.session_id.to_owned())
            {
                return Err(NodeError::InvalidState(format!(
                    "session is already active or resuming: {}",
                    params.session_id
                )));
            }
        }
        let result = self.resume_session_state(params).await;
        self.inner
            .sessions
            .lock()
            .expect("sessions poisoned")
            .resuming
            .remove(params.session_id);
        result
    }

    /// The body of [`Self::resume`] (the caller releases the exclusive guard).
    pub(super) async fn resume_session_state(
        &self,
        params: ResumeParams<'_>,
    ) -> Result<ResumeOutcome, NodeError> {
        let (started, events) = self
            .resume_stopped_session(
                params.session_id,
                ResumeMode::AllowReplay {
                    force_replay: params.force_replay,
                },
            )
            .await?;

        // If native restore failed, continue the context via history replay
        if !started.context_restored {
            let up_to_seq = events.last().map(|event| event.node_seq).unwrap_or(0);
            if let Some(context) = build_history_replay_context(&events, up_to_seq)
                && let Err(err) = self
                    .start_turn(
                        params.session_id,
                        &PromptInput::text(context),
                        ClientSource::Resume,
                    )
                    .await
            {
                return Err(self
                    .fail_context_injection(params.session_id, "resume context injection", err)
                    .await);
            }
        }

        Ok(ResumeOutcome {
            session_id: params.session_id.to_owned(),
            context_restored: started.context_restored,
        })
    }

    /// Validates a stopped session, launches the driver, and records up to
    /// `StatusChanged(Idle)` (shared by explicit resume / auto-resume on send).
    ///
    /// - Ephemeral VM (`provisioning` / `bootstrapping`) sessions and missing
    ///   working directories are rejected with [`NodeError::InvalidState`].
    /// - When native restore is impossible in [`ResumeMode::NativeOnly`], the driver
    ///   returns [`NativeResumeUnavailable`], which reaches the caller as
    ///   [`NodeError::ResumeRequired`] (the session remains stopped).
    ///
    /// Returns the started session and the loaded events (for replay decisions).
    pub(super) async fn resume_stopped_session(
        &self,
        session_id: &str,
        mode: ResumeMode,
    ) -> Result<(StartedDriverSession, Vec<SessionEventEnvelope>), NodeError> {
        let session = self
            .inner
            .bus
            .db()
            .get_session(session_id)
            .await?
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;

        // Resuming an ephemeral VM (provisioner) session is v2 (not yet supported)
        if matches!(
            session.status,
            SessionStatus::Provisioning | SessionStatus::Bootstrapping
        ) {
            return Err(NodeError::InvalidState(format!(
                "resuming ephemeral sessions is not supported yet: {session_id}"
            )));
        }

        // Cannot resume if the working directory is gone (e.g. worktree removed)
        let cwd = PathBuf::from(&session.local_path);
        if !cwd.is_dir() {
            return Err(NodeError::InvalidState(format!(
                "session workspace does not exist: {}",
                session.local_path
            )));
        }

        let events = self.load_session_events(session_id).await?;
        let spec = self.resolve_launch_spec(&session.agent_id, &[]).await?;

        // Attempt native restore (the caller injects a replay if it fails)
        let (agent_session_id, allow_fresh) = match mode {
            // With `--replay`, do not pass the agent-side ID so a fresh session is created
            ResumeMode::AllowReplay { force_replay: true } => (None, true),
            ResumeMode::AllowReplay {
                force_replay: false,
            } => (session.agent_session_id.clone(), true),
            ResumeMode::NativeOnly => (session.agent_session_id.clone(), false),
        };
        let started = self
            .start_driver_session(StartDriverParams {
                session_id,
                title: &session.title,
                cwd: &cwd,
                spec: &spec,
                extra_args: Vec::new(),
                initial_mode: None,
                has_custom_title: true,
                resume: Some(ResumeRequest {
                    agent_session_id,
                    allow_fresh,
                }),
                fork_from: None,
                initial_config: Vec::new(),
            })
            .await?;

        // Reflect the resume in the state projection (stopped / error → idle)
        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Idle,
                    error_message: None,
                },
            )
            .await?;

        Ok((started, events))
    }

    /// Automatically resumes a stopped session via native restore (before sending a prompt).
    ///
    /// Serialized by the same `resuming` guard as explicit resume; if another
    /// resume is in progress, waits for it to finish before returning control.
    /// Returns [`NodeError::ResumeRequired`] when native restore is impossible and
    /// leaves the session stopped (does not implicitly run a history replay; prompts
    /// the client for an explicit Resume).
    pub(super) async fn auto_resume_for_prompt(&self, session_id: &str) -> Result<(), NodeError> {
        let deadline = Instant::now() + AUTO_RESUME_TIMEOUT;
        loop {
            // Confine guard acquisition and checks to a scope so the lock is not held across await
            let acquired = {
                let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
                if sessions.active.contains_key(session_id) {
                    return Ok(());
                }
                sessions.resuming.insert(session_id.to_owned())
            };
            if acquired {
                let result = self
                    .resume_stopped_session(session_id, ResumeMode::NativeOnly)
                    .await
                    .map(|(started, _events)| {
                        // Driver contract: in native-only mode (`allow_fresh = false`)
                        // launch fails (Err) when restore is impossible, so only
                        // `context_restored = true` is normal here
                        if !started.context_restored {
                            tracing::warn!(
                                session_id,
                                "native-only resume returned a fresh session"
                            );
                        }
                    });
                self.inner
                    .sessions
                    .lock()
                    .expect("sessions poisoned")
                    .resuming
                    .remove(session_id);
                return result;
            }
            if Instant::now() >= deadline {
                return Err(NodeError::InvalidState(format!(
                    "session is resuming: {session_id}"
                )));
            }
            tokio::time::sleep(RESUME_WAIT_INTERVAL).await;
        }
    }
    /// Archives/restores a session (`fxg session archive` / Web UI).
    ///
    /// Archiving is a reversible visibility flag; it does not change the running state.
    /// Returns the archive timestamp (Unix epoch ms; `None` when restoring).
    pub async fn set_archived(
        &self,
        command_id: &CommandId,
        session_id: &str,
        archived: bool,
    ) -> Result<Option<i64>, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_string()));
        }
        self.ensure_session_exists(session_id).await?;
        let envelope = self
            .inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::SessionArchived { archived },
            )
            .await?;
        Ok(archived.then_some(envelope.created_at))
    }

    /// Deletes a session (`fxg session delete` / Web UI).
    ///
    /// If active (agent process alive), waits for it to stop before appending
    /// `SessionDeleted` (tombstone). Event bodies before this event are purged in
    /// the same transaction (irreversible).
    pub async fn delete(&self, command_id: &CommandId, session_id: &str) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_string()));
        }
        self.ensure_session_exists(session_id).await?;
        {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            if sessions.resuming.contains(session_id) {
                return Err(NodeError::InvalidState(format!(
                    "session is resuming: {session_id}"
                )));
            }
        }
        self.stop_if_active(session_id).await?;
        self.inner
            .bus
            .record(session_id, UnifiedEventPayload::SessionDeleted {})
            .await?;
        Ok(())
    }
}

/// Returns the current branch and the worktree determination.
pub(super) async fn branch_and_worktree(cwd: &Path) -> Result<(Option<String>, bool), NodeError> {
    let branch = crate::git::try_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    let git_dir = crate::git::try_git(cwd, &["rev-parse", "--git-dir"]).await?;
    let is_worktree = git_dir
        .as_deref()
        .map(|dir| dir.contains("worktrees"))
        .unwrap_or(false);
    Ok((branch, is_worktree))
}

/// Resolves git metadata for `SessionCreated`, degrading to "unknown" with a
/// warning instead of silently losing the branch / worktree flags.
pub(super) async fn git_metadata_or_warn(cwd: &Path, session_id: &str) -> (Option<String>, bool) {
    match branch_and_worktree(cwd).await {
        Ok(metadata) => metadata,
        Err(err) => {
            tracing::warn!(
                session_id,
                cwd = %cwd.display(),
                "failed to resolve git metadata: {err:#}"
            );
            (None, false)
        }
    }
}
