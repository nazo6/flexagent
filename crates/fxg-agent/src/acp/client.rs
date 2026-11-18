use super::mapping::*;
use super::process::*;
use super::session::*;
use super::subagent::*;
use super::terminal::*;
use super::*;

/// Maximum size of a text file the client reads or writes for an agent.
///
/// The ACP `fs/*` handlers run inside the node process, so an unbounded read
/// would block a runtime worker and let an agent pull arbitrarily large files
/// into memory.
const MAX_ACP_TEXT_FILE_BYTES: u64 = 8 * 1024 * 1024;

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
    events: Mutex<Option<EventSink>>,
    /// Latest modes / commands / config options (shared with the turn loop).
    capabilities: Arc<Mutex<SessionCapabilitiesState>>,
    /// PTY manager (for ACP terminal/*; shared per process)
    pty: Arc<fxg_pty::PtySessionManager>,
    /// Terminals started by the agent (shared per process, one session at a time)
    terminals: Arc<TerminalRegistry>,
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
                .set_config(
                    key,
                    ConfigOptionValue::Choice(ConfigOptionChoice::from_value(mode_id)),
                )
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

    async fn kill_terminal(&self, terminal_id: &str) -> anyhow::Result<()> {
        let pty_id = self
            .terminals
            .pty_id(terminal_id)
            .map_err(|err| anyhow!("{err}"))?;
        self.pty
            .kill(&pty_id)
            .with_context(|| format!("failed to terminate terminal {terminal_id}"))?;
        Ok(())
    }

    async fn snapshot_terminals(&self) {
        self.terminals.request_snapshots();
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
        // Unblock any in-flight permission or elicitation waiters before waiting
        // for the turn loop to end (otherwise a prompt paused on approval would
        // deadlock shutdown).
        cancel_pending_permissions(&self.permissions);
        let events_snapshot = self.events.lock().expect("session events poisoned").clone();
        cancel_pending_elicitations(&self.elicitations, events_snapshot.as_ref());
        fxg_protocol::discard_with!(
            self.commands.send(AcpCommand::Shutdown),
            "send a command to the ACP session"
        );
        if let Some(session_end) = session_end {
            const TURN_LOOP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
            if tokio::time::timeout(TURN_LOOP_SHUTDOWN_TIMEOUT, session_end)
                .await
                .is_err()
            {
                tracing::warn!(
                    session_id = %self.acp_session_id,
                    timeout = ?TURN_LOOP_SHUTDOWN_TIMEOUT,
                    "timed out waiting for the ACP turn loop to end; disposing process"
                );
                self.lease.mark_dispose();
            }
        }
        // Best-effort logical close when returning the process to the warm pool;
        // when disposing the process anyway, skip waiting for `session/close`.
        if !self.lease.dispose_requested.load(Ordering::SeqCst) {
            match tokio::time::timeout(
                Duration::from_secs(5),
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
        }
        let mut exits = self.terminals.exit_receivers();
        for terminal_id in self.pty.list() {
            fxg_protocol::discard_with!(
                self.pty.kill(&terminal_id.pty_id),
                "kill the pty terminal on session shutdown"
            );
        }
        // A terminated command still reports its final output. Wait for it (bounded)
        // so the card does not stay "running" without an exit code once the event
        // sink below is released.
        fxg_protocol::discard_with!(
            tokio::time::timeout(Duration::from_secs(1), async {
                for exit_rx in &mut exits {
                    if exit_rx.borrow().is_none() {
                        fxg_protocol::discard_with!(
                            exit_rx.changed().await,
                            "wait for a terminal to report its exit on shutdown"
                        );
                    }
                }
            })
            .await,
            "discard a terminal that did not report its exit in time"
        );
        // The process may be returned to the warm pool: do not keep the previous
        // session's terminal buffers (or let them answer a later session).
        self.terminals.clear();
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
/// waiting-for-input state in the UI (responses via fxg are already
/// recorded by the manager, so this only records driver-side auto-resolution).
pub(super) fn cancel_pending_elicitations(
    elicitations: &ElicitationRegistry,
    events: Option<&EventSink>,
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

/// Flushes open streaming chunks on the session turn loop before emitting a
/// blocking approval or elicitation request so preceding thoughts/text are persisted.
async fn flush_session_streams(
    ingress: Option<&mpsc::UnboundedSender<super::subagent::SessionIngress>>,
) {
    let Some(tx) = ingress else {
        return;
    };
    let (ack_tx, ack_rx) = oneshot::channel();
    if tx
        .send(super::subagent::SessionIngress::Flush(ack_tx))
        .is_ok()
    {
        fxg_protocol::discard_with!(
            tokio::time::timeout(std::time::Duration::from_secs(1), ack_rx).await,
            "wait for streaming accumulator flush"
        );
    }
}

pub(super) fn next_status_after_request(
    permissions: &PermissionRegistry,
    elicitations: &ElicitationRegistry,
) -> fxg_protocol::common::SessionStatus {
    if !permissions
        .lock()
        .expect("permission registry poisoned")
        .is_empty()
    {
        return fxg_protocol::common::SessionStatus::WaitingPermission;
    }
    if !elicitations
        .lock()
        .expect("elicitation registry poisoned")
        .is_empty()
    {
        return fxg_protocol::common::SessionStatus::WaitingInput;
    }
    fxg_protocol::common::SessionStatus::Running
}

fn elicitation_response(outcome: ElicitationOutcome) -> CreateElicitationResponse {
    match outcome {
        ElicitationOutcome::Accept(content) => CreateElicitationResponse::new(
            AcpElicitationAction::Accept(ElicitationAcceptAction::new().content(content)),
        ),
        ElicitationOutcome::Decline => {
            CreateElicitationResponse::new(AcpElicitationAction::Decline)
        }
        ElicitationOutcome::Cancel => CreateElicitationResponse::new(AcpElicitationAction::Cancel),
    }
}

/// Reads a text file for an ACP `fs/read_text_file` request.
///
/// Files above [`MAX_ACP_TEXT_FILE_BYTES`] are rejected instead of being loaded,
/// and the optional 1-based `line` / `limit` window is applied afterwards.
async fn read_text_file(
    path: &std::path::Path,
    line: Option<u32>,
    limit: Option<u32>,
) -> anyhow::Result<String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.len() > MAX_ACP_TEXT_FILE_BYTES {
        anyhow::bail!(
            "refusing to read {}: {} bytes exceeds the {MAX_ACP_TEXT_FILE_BYTES} byte limit",
            path.display(),
            metadata.len()
        );
    }
    let content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    if line.is_none() && limit.is_none() {
        return Ok(content);
    }
    let start = line.unwrap_or(1).saturating_sub(1) as usize;
    let take = limit.unwrap_or(u32::MAX) as usize;
    Ok(content
        .lines()
        .skip(start)
        .take(take)
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Writes a text file for an ACP `fs/write_text_file` request.
///
/// Returns the previous content for the diff (empty when the file did not
/// exist, `None` when it was too large to read for a diff).
async fn write_text_file(path: &std::path::Path, content: &str) -> anyhow::Result<Option<String>> {
    if content.len() as u64 > MAX_ACP_TEXT_FILE_BYTES {
        anyhow::bail!(
            "refusing to write {}: {} bytes exceeds the {MAX_ACP_TEXT_FILE_BYTES} byte limit",
            path.display(),
            content.len()
        );
    }
    let old_text = match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.len() > MAX_ACP_TEXT_FILE_BYTES => None,
        Ok(_) => Some(
            tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("failed to read {} before writing", path.display()))?,
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Some(String::new()),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to stat {} before writing", path.display()));
        }
    };
    tokio::fs::write(path, content)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(old_text)
}

/// Launch parameters for `run_acp_process`.
pub(super) struct AcpProcessTaskParams {
    /// Agent to launch
    pub(super) agent: AcpAgent,
    /// Agent ID (diagnostics key)
    pub(super) agent_id: String,
    /// ACP connection options (client capabilities / vendor notifications)
    pub(super) acp: crate::acp::extensions::AcpExtensionOptions,
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
                let elicitations = Arc::clone(&elicitations);
                let contexts = Arc::clone(&contexts);
                async move |request: RequestPermissionRequest, responder, cx| {
                    // If the session is already closed (race), fail safe and
                    // respond with cancel
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ));
                    };
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
                    let tool_name = tool.name.clone().unwrap_or_else(|| "tool".to_owned());
                    let details = serde_json::to_value(&request.tool_call)
                        .unwrap_or(serde_json::Value::Null)
                        .into();

                    let (decision_tx, mut decision_rx) = oneshot::channel();
                    permissions
                        .lock()
                        .expect("permission registry poisoned")
                        .insert(request_id.clone(), decision_tx);
                    let permissions = Arc::clone(&permissions);
                    let elicitations = Arc::clone(&elicitations);
                    cx.spawn(async move {
                        flush_session_streams(context.ingress.as_ref()).await;
                        match decision_rx.try_recv() {
                            Ok(outcome) => {
                                return responder.respond(RequestPermissionResponse::new(outcome));
                            }
                            Err(oneshot::error::TryRecvError::Closed) => {
                                return responder.respond(RequestPermissionResponse::new(
                                    RequestPermissionOutcome::Cancelled,
                                ));
                            }
                            Err(oneshot::error::TryRecvError::Empty) => {}
                        }
                        fxg_protocol::discard_with!(
                            context.events.send(DriverEvent::Event(
                                UnifiedEventPayload::StatusChanged {
                                    status: fxg_protocol::common::SessionStatus::WaitingPermission,
                                    error_message: None,
                                },
                            )),
                            "notify event subscribers"
                        );
                        fxg_protocol::discard_with!(
                            context.events.send(DriverEvent::Event(
                                UnifiedEventPayload::PermissionRequest {
                                    request_id,
                                    tool_name,
                                    summary,
                                    options,
                                    details,
                                    subagent_id: context.subagent_id,
                                },
                            )),
                            "notify event subscribers"
                        );
                        let outcome = decision_rx
                            .await
                            .unwrap_or(RequestPermissionOutcome::Cancelled);
                        fxg_protocol::discard_with!(
                            context.events.send(DriverEvent::Event(
                                UnifiedEventPayload::StatusChanged {
                                    status: next_status_after_request(&permissions, &elicitations),
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
                    match read_text_file(&path, request.line, request.limit).await {
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
                    match write_text_file(&path, &request.content).await {
                        Ok(old_text) => {
                            let path_string = path.to_string_lossy().into_owned();
                            let diff = old_text
                                .map(|old| build_file_diff(&path_string, &old, &request.content));
                            fxg_protocol::discard_with!(
                                context.events.send(DriverEvent::Event(
                                    UnifiedEventPayload::ToolCall {
                                        tool_call_id: format!("fs-{}", uuid_v7()),
                                        title: format!("write_text_file: {path_string}"),
                                        kind: FxgToolCallKind::Edit,
                                        status: FxgToolCallStatus::Completed,
                                        locations: vec![path_string],
                                        diff,
                                        raw_output: None,
                                        subagent_id: context.subagent_id.clone(),
                                    },
                                )),
                                "notify event subscribers"
                            );
                            responder.respond(WriteTextFileResponse::new())
                        }
                        Err(err) => responder.respond_with_internal_error(err.to_string()),
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
                    Ok((_, _, buffer, exit_rx)) => {
                        let data = buffer.lock().expect("terminal output poisoned").retained();
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
                let permissions = Arc::clone(&permissions);
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
                    let message = request.message.clone();
                    let tool_call_id = scope.tool_call_id.as_ref().map(|id| id.to_string());

                    let (decision_tx, mut decision_rx) = oneshot::channel();
                    elicitations
                        .lock()
                        .expect("elicitation registry poisoned")
                        .insert(
                            elicitation_id.clone(),
                            PendingElicitation {
                                sender: decision_tx,
                                schema: schema.clone(),
                            },
                        );
                    let permissions = Arc::clone(&permissions);
                    let elicitations = Arc::clone(&elicitations);
                    cx.spawn(async move {
                        flush_session_streams(context.ingress.as_ref()).await;
                        match decision_rx.try_recv() {
                            Ok(outcome) => {
                                return responder.respond(elicitation_response(outcome));
                            }
                            Err(oneshot::error::TryRecvError::Closed) => {
                                return responder
                                    .respond(elicitation_response(ElicitationOutcome::Cancel));
                            }
                            Err(oneshot::error::TryRecvError::Empty) => {}
                        }
                        fxg_protocol::discard_with!(
                            context.events.send(DriverEvent::Event(
                                UnifiedEventPayload::StatusChanged {
                                    status: fxg_protocol::common::SessionStatus::WaitingInput,
                                    error_message: None,
                                },
                            )),
                            "notify event subscribers"
                        );
                        fxg_protocol::discard_with!(
                            context.events.send(DriverEvent::Event(
                                UnifiedEventPayload::ElicitationRequest {
                                    elicitation_id,
                                    message,
                                    mode: FxgElicitationMode::Form,
                                    requested_schema: schema,
                                    tool_call_id,
                                    subagent_id: context.subagent_id,
                                },
                            )),
                            "notify event subscribers"
                        );
                        let outcome = decision_rx.await.unwrap_or(ElicitationOutcome::Cancel);
                        fxg_protocol::discard_with!(
                            context.events.send(DriverEvent::Event(
                                UnifiedEventPayload::StatusChanged {
                                    status: next_status_after_request(&permissions, &elicitations),
                                    error_message: None,
                                },
                            )),
                            "notify event subscribers"
                        );
                        responder.respond(elicitation_response(outcome))
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
            tracing::debug!(target: "fxg_agent::acp", "acp agent: {line}");
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
                let (ingress_tx, ingress_rx) = mpsc::unbounded_channel();
                let BuiltSession {
                    session,
                    acp_session_id,
                    context_restored,
                    capabilities_state,
                    mut runtime,
                    capture_history,
                } = match build_session(
                    &cx,
                    &agent_capabilities,
                    &params,
                    &events,
                    &contexts,
                    &ingress_tx,
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
                    terminals: Arc::clone(&terminals),
                    lease: Arc::clone(&lease),
                };
                if ready.send(Ok((handle, context_restored))).is_err() {
                    tracing::debug!("session waiter dropped before the session became ready");
                }

                // 3) Turn loop (serializes prompt sends with update reads)
                if let Some(parser) = params.acp_parser.clone() {
                    let handler = VendorNotificationHandler::new(parser, ingress_tx.clone());
                    match cx.add_dynamic_handler(handler) {
                        Ok(guard) => runtime.guards.push(guard),
                        Err(err) => {
                            tracing::warn!("failed to register vendor notification handler: {err}");
                        }
                    }
                }
                let outcome = serve_session(
                    session,
                    cmd_rx,
                    &prompt_capabilities,
                    ingress_rx,
                    runtime,
                    capture_history,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn limit_bytes() -> usize {
        usize::try_from(MAX_ACP_TEXT_FILE_BYTES).expect("limit fits in usize")
    }

    #[tokio::test]
    async fn read_text_file_applies_the_line_window() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notes.txt");
        tokio::fs::write(&path, "one\ntwo\nthree\nfour\n")
            .await
            .expect("write");

        assert_eq!(
            read_text_file(&path, None, None).await.expect("read"),
            "one\ntwo\nthree\nfour\n"
        );
        assert_eq!(
            read_text_file(&path, Some(2), Some(2))
                .await
                .expect("window"),
            "two\nthree"
        );
    }

    #[tokio::test]
    async fn read_text_file_rejects_files_above_the_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("huge.txt");
        tokio::fs::write(&path, vec![b'a'; limit_bytes() + 1])
            .await
            .expect("write");

        let err = read_text_file(&path, None, None)
            .await
            .expect_err("must reject");
        assert!(err.to_string().contains("byte limit"), "{err}");
    }

    #[tokio::test]
    async fn write_text_file_rejects_content_above_the_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("huge.txt");
        let content = "a".repeat(limit_bytes() + 1);

        let err = write_text_file(&path, &content)
            .await
            .expect_err("must reject");
        assert!(err.to_string().contains("byte limit"), "{err}");
        assert!(!path.exists(), "a rejected write must not create the file");
    }

    #[tokio::test]
    async fn write_text_file_returns_the_previous_content_and_skips_large_diffs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notes.txt");

        assert_eq!(
            write_text_file(&path, "first").await.expect("create"),
            Some(String::new())
        );
        assert_eq!(
            write_text_file(&path, "second").await.expect("update"),
            Some("first".to_owned())
        );
        assert_eq!(
            tokio::fs::read_to_string(&path).await.expect("read"),
            "second"
        );

        // An existing file above the limit is overwritten without a diff.
        let huge = dir.path().join("huge.txt");
        tokio::fs::write(&huge, vec![b'a'; limit_bytes() + 1])
            .await
            .expect("write huge");
        assert_eq!(
            write_text_file(&huge, "small").await.expect("overwrite"),
            None
        );
        assert_eq!(
            tokio::fs::read_to_string(&huge).await.expect("read"),
            "small"
        );
    }
}
