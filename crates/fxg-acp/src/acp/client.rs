use super::mapping::*;
use super::process::*;
use super::session::*;
use super::subagent::*;
use super::terminal::*;
use super::*;

/// Handle for a started ACP session.
pub(super) struct AcpSessionHandle {
    /// ACP (agent-side) session ID
    acp_session_id: SessionId,
    /// Connection (clonable; used to issue requests/notifications)
    conn: ConnectionTo<Agent>,
    /// Commands to the session (turn loop)
    commands: mpsc::UnboundedSender<AcpCommand>,
    /// Permission resolution registry (shared per process)
    permissions: PermissionRegistry,
    /// Elicitation resolution registry (shared per process)
    elicitations: ElicitationRegistry,
    /// Event sender (for status updates).
    ///
    /// Released on session end ([`ActiveSessionHandle::shutdown`]). If it were kept,
    /// the session manager's event pump could not detect that the driver ended and
    /// the session would remain in the active list.
    events: Mutex<Option<mpsc::UnboundedSender<DriverEvent>>>,
    /// Latest modes / commands / config options (shared with the turn loop).
    capabilities: Arc<Mutex<SessionCapabilitiesState>>,
    /// PTY manager (for ACP terminal/*; shared per process)
    pty: Arc<fxg_pty::PtySessionManager>,
    /// Process lease (returned to the pool or disposed on end)
    lease: Arc<ProcessLease>,
}

impl AcpSessionHandle {
    /// ACP (agent-side) session ID.
    pub(super) fn session_id(&self) -> &SessionId {
        &self.acp_session_id
    }
}

#[async_trait]
impl ActiveSessionHandle for AcpSessionHandle {
    async fn send_prompt(&self, input: PromptInput) -> anyhow::Result<()> {
        match self.commands.send(AcpCommand::Prompt(input)) {
            Ok(()) => Ok(()),
            Err(_) => Err(anyhow!("acp session is no longer running")),
        }
    }

    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()> {
        let sender = self
            .permissions
            .lock()
            .expect("permission registry poisoned")
            .remove(&request_id);
        let Some(sender) = sender else {
            return Err(anyhow!(
                "unknown or already resolved permission: {request_id}"
            ));
        };
        let outcome = if selected_option_id.is_empty() {
            RequestPermissionOutcome::Cancelled
        } else {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(selected_option_id))
        };
        if sender.send(outcome).is_err() {
            return Err(anyhow!("permission resolver dropped: {request_id}"));
        }
        Ok(())
    }

    async fn respond_elicitation(
        &self,
        elicitation_id: String,
        action: fxg_protocol::common::ElicitationAction,
        content: Option<ElicitationContent>,
    ) -> anyhow::Result<()> {
        use fxg_protocol::common::ElicitationAction as FxgElicitationAction;

        // Read the schema without consuming the pending entry (invalid content keeps
        // the entry pending and returns an error so the client can fix and resend)
        let schema = self
            .elicitations
            .lock()
            .expect("elicitation registry poisoned")
            .get(&elicitation_id)
            .map(|pending| pending.schema.clone())
            .ok_or_else(|| anyhow!("unknown or already resolved elicitation: {elicitation_id}"))?;
        let outcome = match action {
            FxgElicitationAction::Accept => {
                let content = content.unwrap_or_default();
                validate_elicitation_content(&schema, &content)
                    .map_err(|err| anyhow!("invalid elicitation content: {err}"))?;
                let content = content
                    .into_iter()
                    .map(|(key, value)| Ok((key, elicitation_content_value(value)?)))
                    .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
                ElicitationOutcome::Accept(content)
            }
            FxgElicitationAction::Decline => ElicitationOutcome::Decline,
            FxgElicitationAction::Cancel => ElicitationOutcome::Cancel,
        };
        let sender = self
            .elicitations
            .lock()
            .expect("elicitation registry poisoned")
            .remove(&elicitation_id)
            .map(|pending| pending.sender)
            .ok_or_else(|| anyhow!("unknown or already resolved elicitation: {elicitation_id}"))?;
        if sender.send(outcome).is_err() {
            return Err(anyhow!("elicitation resolver dropped: {elicitation_id}"));
        }
        Ok(())
    }

    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()> {
        // ACP supersedes Session Modes with Session Config Options, and agents
        // such as OpenCode only expose the selector as `configOptions[category =
        // mode]` (their `session/set_mode` rejects those IDs). Route the change
        // through the same channel the agent reports.
        let mode_key = {
            let capabilities = self
                .capabilities
                .lock()
                .expect("session capabilities poisoned");
            capabilities
                .config_options
                .iter()
                .find(|option| is_mode_option(option))
                .map(|option| option.key.clone())
        };
        if let Some(key) = mode_key {
            return self
                .set_config(key, ConfigOptionValue::Select(mode_id))
                .await;
        }
        self.conn
            .send_request(SetSessionModeRequest::new(
                self.acp_session_id.clone(),
                mode_id,
            ))
            .block_task()
            .await
            .map_err(|err| anyhow!("failed to set mode: {err}"))?;
        Ok(())
    }

    async fn set_config(&self, key: String, value: ConfigOptionValue) -> anyhow::Result<()> {
        let value = parse_config_value(value)
            .map_err(|err| anyhow!("unsupported config value for {key}: {err}"))?;
        let response = self
            .conn
            .send_request(SetSessionConfigOptionRequest::new(
                self.acp_session_id.clone(),
                key,
                value,
            ))
            .block_task()
            .await
            .map_err(|err| anyhow!("failed to set config option: {err}"))?;
        // ACP agents are not required to push a `config_option_update` notification
        // after a config change, and some agents only return the new options in the
        // response. Publish them so the UI does not keep showing the old value.
        if !response.config_options.is_empty() {
            let payload = {
                let mut capabilities = self
                    .capabilities
                    .lock()
                    .expect("session capabilities poisoned");
                capabilities.set_config_options(&response.config_options);
                capabilities.to_payload()
            };
            let events = self.events.lock().expect("session events poisoned").clone();
            if let Some(events) = events.as_ref() {
                fxg_protocol::discard_with!(
                    events.send(DriverEvent::Event(payload)),
                    "notify event subscribers"
                );
            }
        }
        Ok(())
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        self.conn
            .send_notification(CancelNotification::new(self.acp_session_id.clone()))
            .map_err(|err| anyhow!("failed to cancel turn: {err}"))?;
        // The spec requires unresolved approvals to be returned as cancelled (MUST)
        cancel_pending_permissions(&self.permissions);
        // The spec also requires unresolved elicitations to be resolved as cancel,
        // clearing the waiting-for-input state
        let events = self.events.lock().expect("session events poisoned").clone();
        cancel_pending_elicitations(&self.elicitations, events.as_ref());
        if let Some(events) = events.as_ref() {
            fxg_protocol::discard_with!(
                events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: fxg_protocol::common::SessionStatus::Running,
                    error_message: None,
                })),
                "notify event subscribers"
            );
        }
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        // Close the session and return the process to the reuse pool (disposal is
        // [`Self::dispose`])
        let session_end = self.lease.take_session_end_receiver();
        fxg_protocol::discard_with!(
            self.commands.send(AcpCommand::Shutdown),
            "send a command to the ACP session"
        );
        if let Some(session_end) = session_end {
            // Wait for the turn loop to end (returns Err immediately if the loop never
            // started or failed)
            fxg_protocol::discard_with!(session_end.await, "wait for the ACP turn loop to end");
        }
        // Best-effort logical close (the process is kept for the warm pool).
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.conn
                .send_request(CloseSessionRequest::new(self.acp_session_id.clone()))
                .block_task(),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                tracing::debug!(session_id = %self.acp_session_id, "session/close failed: {err}");
            }
            Err(_) => {
                tracing::debug!(session_id = %self.acp_session_id, "session/close timed out");
            }
        }
        for terminal_id in self.pty.list() {
            fxg_protocol::discard_with!(
                self.pty.kill(&terminal_id.pty_id),
                "kill the pty terminal on session shutdown"
            );
        }
        // Release the event sender (so the session manager's pump detects that the
        // driver ended and removes the session from the active list)
        self.events.lock().expect("session events poisoned").take();
        self.lease.finish().await;
        Ok(())
    }

    async fn dispose(&self) -> anyhow::Result<()> {
        // Dispose the process instead of keeping it (`the hub kill switch` / node shutdown)
        self.lease.dispose_requested.store(true, Ordering::SeqCst);
        self.shutdown().await
    }
}
/// Closes all unresolved permission requests as `cancelled`.
pub(super) fn cancel_pending_permissions(permissions: &PermissionRegistry) {
    let mut registry = permissions.lock().expect("permission registry poisoned");
    for (_, sender) in registry.drain() {
        fxg_protocol::discard_with!(
            sender.send(RequestPermissionOutcome::Cancelled),
            "discard a non-critical result"
        );
    }
}

/// Closes all unresolved elicitations as `cancel` (the spec requires resolving
/// unresolved elicitations with cancel when a turn is interrupted).
///
/// When `events` is provided, also emits `ElicitationResolved` to clear the
/// waiting-for-input state in the UI / Inbox (responses via fxg are already
/// recorded by the manager, so this only records driver-side auto-resolution).
pub(super) fn cancel_pending_elicitations(
    elicitations: &ElicitationRegistry,
    events: Option<&mpsc::UnboundedSender<DriverEvent>>,
) {
    let mut registry = elicitations.lock().expect("elicitation registry poisoned");
    for (elicitation_id, pending) in registry.drain() {
        if pending.sender.send(ElicitationOutcome::Cancel).is_err() {
            tracing::debug!(
                elicitation_id = %elicitation_id,
                "elicitation waiter dropped before the cancel was delivered"
            );
        }
        if let Some(events) = events {
            fxg_protocol::discard_with!(
                events.send(DriverEvent::Event(
                    UnifiedEventPayload::ElicitationResolved {
                        elicitation_id,
                        action: fxg_protocol::common::ElicitationAction::Cancel,
                        content: None,
                        resolved_by: fxg_protocol::common::ResolvedBy::Unknown(
                            "system".to_owned(),
                        ),
                    },
                )),
                "notify event subscribers"
            );
        }
    }
}
/// Launch parameters for `run_acp_process`.
pub(super) struct AcpProcessTaskParams {
    /// Agent to launch
    pub(super) agent: AcpAgent,
    /// Agent ID (diagnostics key)
    pub(super) agent_id: String,
    /// ACP connection options (client capabilities / vendor notifications)
    pub(super) acp: crate::agent::extension::AcpExtensionOptions,
    /// Recent stderr buffer shared with the driver
    pub(super) stderr: StderrRegistry,
    /// Process ready (initialize complete) notification
    pub(super) ready_tx: oneshot::Sender<anyhow::Result<()>>,
    /// Command receiver for the process
    pub(super) control_rx: mpsc::UnboundedReceiver<ProcessCommand>,
}

/// Resident task that establishes the ACP connection and alternates between idle
/// waiting and session handling.
///
/// Handles one session at a time per process, and after the session ends waits
/// for the next `NewSession` (process disposal happens via the pool TTL / `Dispose`).
#[allow(clippy::too_many_lines)]
pub(super) async fn run_acp_process(
    params: AcpProcessTaskParams,
) -> Result<(), agent_client_protocol::Error> {
    let AcpProcessTaskParams {
        agent,
        agent_id,
        acp,
        stderr,
        ready_tx,
        mut control_rx,
    } = params;
    let mut ready_tx = Some(ready_tx);
    let pty = Arc::new(fxg_pty::PtySessionManager::new());
    let terminals = Arc::new(TerminalRegistry::default());
    let permissions: PermissionRegistry = Arc::new(Mutex::new(HashMap::new()));
    let elicitations: ElicitationRegistry = Arc::new(Mutex::new(HashMap::new()));
    let contexts: SessionContexts = Arc::new(Mutex::new(HashMap::new()));

    // Register handlers (fs / approval / terminal).
    // NOTE: a helper returning `impl AsyncFnMut` cannot prove `Send` for the associated
    // future, so closures are passed directly to the builder chain.
    let builder = Client
        .v1()
        .name("fxg")
        .on_receive_request(
            {
                let permissions = Arc::clone(&permissions);
                let contexts = Arc::clone(&contexts);
                async move |request: RequestPermissionRequest, responder, cx| {
                    // If the session is already closed (race), fail safe and
                    // respond with cancel
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ));
                    };
                    let events = context.events.clone();
                    let request_id = responder.id().to_string();
                    let options = request
                        .options
                        .iter()
                        .map(|option| PermissionOption {
                            option_id: option.option_id.to_string(),
                            name: option.name.clone(),
                            kind: permission_kind(option.kind),
                        })
                        .collect::<Vec<_>>();
                    let tool = &request.tool_call.fields;
                    let summary = tool
                        .title
                        .clone()
                        .unwrap_or_else(|| request.tool_call.tool_call_id.to_string());

                    fxg_protocol::discard_with!(
                        events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                            status: fxg_protocol::common::SessionStatus::WaitingPermission,
                            error_message: None,
                        })),
                        "notify event subscribers"
                    );
                    fxg_protocol::discard_with!(
                        events.send(DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
                            request_id: request_id.clone(),
                            tool_name: tool.name.clone().unwrap_or_else(|| "tool".to_owned()),
                            summary,
                            options,
                            details: serde_json::to_value(&request.tool_call)
                                .unwrap_or(serde_json::Value::Null)
                                .into(),
                            subagent_id: context.subagent_id.clone(),
                        })),
                        "notify event subscribers"
                    );

                    let (decision_tx, decision_rx) = oneshot::channel();
                    permissions
                        .lock()
                        .expect("permission registry poisoned")
                        .insert(request_id, decision_tx);
                    let events_for_task = context.events.clone();
                    cx.spawn(async move {
                        let outcome = decision_rx
                            .await
                            .unwrap_or(RequestPermissionOutcome::Cancelled);
                        fxg_protocol::discard_with!(
                            events_for_task.send(DriverEvent::Event(
                                UnifiedEventPayload::StatusChanged {
                                    status: fxg_protocol::common::SessionStatus::Running,
                                    error_message: None,
                                },
                            )),
                            "notify event subscribers"
                        );
                        responder.respond(RequestPermissionResponse::new(outcome))
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let contexts = Arc::clone(&contexts);
                async move |request: ReadTextFileRequest, responder, _cx| {
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond_with_internal_error("unknown session");
                    };
                    let path = resolve_path(&context.cwd, &request.path);
                    let result = (|| -> anyhow::Result<String> {
                        let content = std::fs::read_to_string(&path)
                            .with_context(|| format!("failed to read {}", path.display()))?;
                        if request.line.is_some() || request.limit.is_some() {
                            let start = request.line.unwrap_or(1).saturating_sub(1) as usize;
                            let take = request.limit.unwrap_or(u32::MAX) as usize;
                            Ok(content
                                .lines()
                                .skip(start)
                                .take(take)
                                .collect::<Vec<_>>()
                                .join("\n"))
                        } else {
                            Ok(content)
                        }
                    })();
                    match result {
                        Ok(content) => responder.respond(ReadTextFileResponse::new(content)),
                        Err(err) => responder.respond_with_internal_error(err.to_string()),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let contexts = Arc::clone(&contexts);
                async move |request: WriteTextFileRequest, responder, _cx| {
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond_with_internal_error("unknown session");
                    };
                    let path = resolve_path(&context.cwd, &request.path);
                    let old_text = match std::fs::read_to_string(&path) {
                        Ok(text) => text,
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
                        Err(err) => {
                            return responder.respond_with_internal_error(format!(
                                "failed to read {} before writing: {err}",
                                path.display()
                            ));
                        }
                    };
                    match std::fs::write(&path, &request.content) {
                        Ok(()) => {
                            let path_string = path.to_string_lossy().into_owned();
                            let diff = build_file_diff(&path_string, &old_text, &request.content);
                            fxg_protocol::discard_with!(
                                context.events.send(DriverEvent::Event(
                                    UnifiedEventPayload::ToolCall {
                                        tool_call_id: format!("fs-{}", uuid_v7()),
                                        title: format!("write_text_file: {path_string}"),
                                        kind: FxgToolCallKind::Edit,
                                        status: FxgToolCallStatus::Completed,
                                        locations: vec![path_string],
                                        diff: Some(diff),
                                        raw_output: None,
                                        subagent_id: context.subagent_id.clone(),
                                    },
                                )),
                                "notify event subscribers"
                            );
                            responder.respond(WriteTextFileResponse::new())
                        }
                        Err(err) => responder.respond_with_internal_error(format!(
                            "failed to write {}: {err}",
                            path.display()
                        )),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let pty = Arc::clone(&pty);
                let terminals = Arc::clone(&terminals);
                let contexts = Arc::clone(&contexts);
                async move |request: CreateTerminalRequest, responder, _cx| {
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond_with_internal_error("unknown session");
                    };
                    // `AsyncFnMut` is called multiple times, so pass cloned captures
                    create_terminal(
                        pty.clone(),
                        terminals.clone(),
                        context.events.clone(),
                        context.subagent_id.clone(),
                        request,
                        responder,
                    )
                    .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let terminals = Arc::clone(&terminals);
                async move |request: TerminalOutputRequest, responder, _cx| match terminals
                    .get(&request.terminal_id.to_string())
                {
                    Ok((_, _, output, exit_rx)) => {
                        let data = output.lock().expect("terminal output poisoned").clone();
                        let text = String::from_utf8_lossy(&data).into_owned();
                        let exit_status = exit_rx
                            .borrow()
                            .as_ref()
                            .copied()
                            .map(|code| exit_status_of(Some(code)));
                        responder.respond(
                            TerminalOutputResponse::new(text, false).exit_status(exit_status),
                        )
                    }
                    Err(err) => responder.respond_with_internal_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let terminals = Arc::clone(&terminals);
                async move |request: WaitForTerminalExitRequest, responder, cx| {
                    let Ok((_, _, _, mut exit_rx)) =
                        terminals.get(&request.terminal_id.to_string())
                    else {
                        return responder.respond_with_internal_error("unknown terminal");
                    };
                    cx.spawn(async move {
                        // Return the current value if already exited; otherwise wait for a change
                        if exit_rx.borrow().is_none() {
                            fxg_protocol::discard_with!(
                                exit_rx.changed().await,
                                "wait for a terminal exit change"
                            );
                        }
                        let status = exit_status_of(*exit_rx.borrow());
                        responder.respond(WaitForTerminalExitResponse::new(status))
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let pty = Arc::clone(&pty);
                let terminals = Arc::clone(&terminals);
                async move |request: KillTerminalRequest, responder, _cx| match terminals
                    .get(&request.terminal_id.to_string())
                {
                    Ok((pty_id, ..)) => {
                        fxg_protocol::discard_with!(
                            pty.kill(&pty_id),
                            "kill the pty terminal on a kill-terminal request"
                        );
                        responder.respond(KillTerminalResponse::new())
                    }
                    Err(err) => responder.respond_with_internal_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let pty = Arc::clone(&pty);
                let terminals = Arc::clone(&terminals);
                async move |request: ReleaseTerminalRequest, responder, _cx| {
                    if let Some(state) = terminals.remove(&request.terminal_id.to_string()) {
                        fxg_protocol::discard_with!(
                            pty.kill(&state.pty_id),
                            "kill the pty terminal on a release-terminal request"
                        );
                    }
                    responder.respond(ReleaseTerminalResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let elicitations = Arc::clone(&elicitations);
                let contexts = Arc::clone(&contexts);
                async move |request: CreateElicitationRequest, responder, cx| {
                    // Phase 1 supports form mode only (the range advertised in capabilities).
                    // Requests for unadvertised modes return Invalid params (spec).
                    let (scope, schema) = match &request.mode {
                        ElicitationMode::Form(form) => {
                            let value = serde_json::to_value(&form.requested_schema)
                                .unwrap_or(serde_json::Value::Null);
                            match serde_json::from_value::<ElicitationSchema>(value) {
                                Ok(schema) => (form.scope.clone(), schema),
                                Err(err) => {
                                    return responder.respond_with_error(
                                        agent_client_protocol::Error::invalid_params()
                                            .data(format!("unsupported elicitation schema: {err}")),
                                    );
                                }
                            }
                        }
                        other => {
                            return responder.respond_with_error(
                                agent_client_protocol::Error::invalid_params()
                                    .data(format!("unsupported elicitation mode: {other:?}")),
                            );
                        }
                    };
                    // Only session scope is supported (request scope is for auth flows
                    // and unsupported)
                    let ElicitationScope::Session(scope) = scope else {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("request-scoped elicitation is not supported"),
                        );
                    };
                    let Some(context) = session_context(&contexts, &scope.session_id) else {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params().data("unknown session"),
                        );
                    };
                    // Form mode has no protocol-level elicitation id, so use the
                    // JSON-RPC request id as the elicitation id (same as approvals).
                    let elicitation_id = responder.id().to_string();
                    let events = context.events.clone();
                    fxg_protocol::discard_with!(
                        events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                            status: fxg_protocol::common::SessionStatus::WaitingInput,
                            error_message: None,
                        })),
                        "notify event subscribers"
                    );
                    fxg_protocol::discard_with!(
                        events.send(DriverEvent::Event(
                            UnifiedEventPayload::ElicitationRequest {
                                elicitation_id: elicitation_id.clone(),
                                message: request.message.clone(),
                                mode: FxgElicitationMode::Form,
                                requested_schema: schema.clone(),
                                tool_call_id: scope.tool_call_id.as_ref().map(|id| id.to_string()),
                                subagent_id: context.subagent_id.clone(),
                            },
                        )),
                        "notify event subscribers"
                    );

                    let (decision_tx, decision_rx) = oneshot::channel();
                    elicitations
                        .lock()
                        .expect("elicitation registry poisoned")
                        .insert(
                            elicitation_id,
                            PendingElicitation {
                                sender: decision_tx,
                                schema,
                            },
                        );
                    let events_for_task = context.events.clone();
                    cx.spawn(async move {
                        let outcome = decision_rx.await.unwrap_or(ElicitationOutcome::Cancel);
                        fxg_protocol::discard_with!(
                            events_for_task.send(DriverEvent::Event(
                                UnifiedEventPayload::StatusChanged {
                                    status: fxg_protocol::common::SessionStatus::Running,
                                    error_message: None,
                                },
                            )),
                            "notify event subscribers"
                        );
                        let response = match outcome {
                            ElicitationOutcome::Accept(content) => {
                                CreateElicitationResponse::new(AcpElicitationAction::Accept(
                                    ElicitationAcceptAction::new().content(content),
                                ))
                            }
                            ElicitationOutcome::Decline => {
                                CreateElicitationResponse::new(AcpElicitationAction::Decline)
                            }
                            ElicitationOutcome::Cancel => {
                                CreateElicitationResponse::new(AcpElicitationAction::Cancel)
                            }
                        };
                        responder.respond(response)
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CompleteElicitationNotification, _cx| {
                // Phase 1 supports form mode only, so URL-mode completion notifications are
                // ignored as unknown IDs (spec: unknown IDs must be ignored).
                tracing::debug!(
                    elicitation_id = %notification.elicitation_id,
                    "elicitation/complete notification ignored (url mode not supported)"
                );
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        );

    // The closures (async move) move `permissions` / `elicitations`, so keep
    // clones for teardown
    let permissions_for_teardown = Arc::clone(&permissions);
    let elicitations_for_teardown = Arc::clone(&elicitations);

    // Forward agent stderr to the daemon log
    let stderr_for_debug = stderr.clone();
    let agent = agent.with_debug(move |line, direction| {
        if matches!(direction, agent_client_protocol::LineDirection::Stderr) {
            stderr_for_debug.push(&agent_id, line);
            tracing::debug!(target: "fxg_acp::acp", "acp agent: {line}");
        }
    });

    let run_result = builder
        .connect_with(agent, async move |cx: ConnectionTo<Agent>| {
            // 1) initialize (once per process; declare support for fs read/write +
            //    terminal + elicitation (form))
            let initialize = match cx
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1)
                        .client_capabilities(client_capabilities(&acp))
                        .client_info(Implementation::new("fxg", env!("CARGO_PKG_VERSION"))),
                )
                .block_task()
                .await
            {
                Ok(initialize) => initialize,
                Err(err) => {
                    if let Some(tx) = ready_tx.take() {
                        fxg_protocol::discard_with!(
                            tx.send(Err(anyhow!("acp initialize failed: {err}"))),
                            "discard a non-critical result"
                        );
                    }
                    return Err(err);
                }
            };
            let agent_capabilities = initialize.agent_capabilities.clone();
            let prompt_capabilities = agent_capabilities.prompt_capabilities.clone();
            let auth_methods = initialize
                .auth_methods
                .iter()
                .map(map_auth_method)
                .collect::<Vec<_>>();
            if let Some(tx) = ready_tx.take() {
                fxg_protocol::discard_with!(tx.send(Ok(())), "discard a non-critical result");
            }

            // 2) Session processing loop. Handles one session at a time per process,
            //    waiting idle for the next `NewSession` after a session ends.
            while let Some(command) = control_rx.recv().await {
                let ProcessCommand::NewSession {
                    params,
                    events,
                    lease,
                    commands: (cmd_tx, cmd_rx),
                    ready,
                } = command
                else {
                    // Dispose = process termination
                    break;
                };
                // Build the session. On failure the connection health is unknown, so
                // return the error and shut the process down (the caller falls back
                // to a cold start).
                let (session, acp_session_id, context_restored, capabilities_state) =
                    match build_session(
                        &cx,
                        &agent_capabilities,
                        &params,
                        &events,
                        &contexts,
                        &auth_methods,
                    )
                    .await
                    {
                        Ok(built) => built,
                        Err(err) => {
                            if ready.send(Err(err)).is_err() {
                                tracing::debug!(
                                    "session waiter dropped before the start failure was reported"
                                );
                            }
                            lease.mark_dispose();
                            break;
                        }
                    };
                let session_end_tx = lease.take_session_end_sender();
                let handle = AcpSessionHandle {
                    acp_session_id: acp_session_id.clone(),
                    conn: cx.clone(),
                    commands: cmd_tx,
                    permissions: Arc::clone(&permissions),
                    elicitations: Arc::clone(&elicitations),
                    events: Mutex::new(Some(events.clone())),
                    capabilities: Arc::clone(&capabilities_state),
                    pty: Arc::clone(&pty),
                    lease: Arc::clone(&lease),
                };
                if ready.send(Ok((handle, context_restored))).is_err() {
                    tracing::debug!("session waiter dropped before the session became ready");
                }

                // 3) Turn loop (serializes prompt sends with update reads)
                let (ingress_tx, ingress_rx) = mpsc::unbounded_channel();
                let mut guards = Vec::new();
                if let Some(parser) = params.acp_parser.clone() {
                    let handler = VendorNotificationHandler::new(parser, ingress_tx.clone());
                    match cx.add_dynamic_handler(handler) {
                        Ok(guard) => guards.push(guard),
                        Err(err) => {
                            tracing::warn!("failed to register vendor notification handler: {err}");
                        }
                    }
                }
                let outcome = serve_session(
                    session,
                    cmd_rx,
                    &events,
                    capabilities_state,
                    &prompt_capabilities,
                    Arc::clone(&contexts),
                    params.cwd.clone(),
                    ingress_tx,
                    ingress_rx,
                    guards,
                )
                .await;
                contexts
                    .lock()
                    .expect("session contexts poisoned")
                    .remove(&acp_session_id.to_string());
                if let Some(tx) = session_end_tx {
                    fxg_protocol::discard_with!(tx.send(()), "discard a non-critical result");
                }
                // Close unresolved approvals and questions tied to the session with cancel
                cancel_pending_permissions(&permissions);
                cancel_pending_elicitations(&elicitations, Some(&events));
                match outcome {
                    ServeOutcome::Closed => {
                        fxg_protocol::discard_with!(
                            events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                                status: fxg_protocol::common::SessionStatus::Stopped,
                                error_message: None,
                            })),
                            "notify event subscribers"
                        );
                    }
                    ServeOutcome::Fatal(err) => {
                        tracing::warn!(
                            session_id = %params.session_id,
                            "acp session ended: {err:#}"
                        );
                        fxg_protocol::discard_with!(
                            events.send(DriverEvent::Failed {
                                message: err.to_string(),
                            }),
                            "notify event subscribers"
                        );
                        // Shut the process down on connection failure (do not return it
                        // to the pool)
                        lease.mark_dispose();
                        break;
                    }
                }
            }

            // Exiting the loop = process shutdown
            Ok(())
        })
        .await;

    // Close any remaining unresolved approvals and questions with cancel (process exit)
    cancel_pending_permissions(&permissions_for_teardown);
    cancel_pending_elicitations(&elicitations_for_teardown, None);

    run_result
}
