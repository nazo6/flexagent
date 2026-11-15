#![allow(
    clippy::disallowed_types,
    reason = "OpenCode v2 session/history payloads are parsed at this external boundary"
)]

use super::*;

#[async_trait]
impl AgentDriver for OpenCodeNativeDriver {
    fn driver_kind(&self) -> DriverKind {
        DriverKind::Custom
    }

    fn management(&self) -> Option<Arc<dyn AgentManagement>> {
        Some(Arc::new(Self {
            inner: Arc::clone(&self.inner),
        }))
    }

    async fn probe_capabilities(
        &self,
        launch: &AgentLaunchSpec,
        cwd: &Path,
        initial_config: &[InitialConfig],
    ) -> anyhow::Result<Option<AgentCapabilitiesPreview>> {
        self.inner.active_ops.fetch_add(1, Ordering::SeqCst);
        let _op = ActiveOp(&self.inner.active_ops);
        let server = self.inner.ensure_server(launch, cwd).await?;
        let Some((payload, _, _)) = wait_for_capabilities(&server.client, cwd, None).await else {
            return Ok(None);
        };
        let UnifiedEventPayload::CapabilitiesUpdated {
            current_mode,
            available_modes,
            mut config_options,
            ..
        } = payload
        else {
            return Ok(None);
        };
        if let Some(model) = initial_model(initial_config)
            && let Some(option) = config_options.iter_mut().find(|opt| opt.key == "model")
            && let Some(choice) = option
                .options
                .iter()
                .find(|choice| matches!(choice, ConfigOptionValue::Choice(c) if c.value == model))
                .cloned()
        {
            option.current_value = choice;
        }
        Ok(Some(AgentCapabilitiesPreview {
            current_mode,
            available_modes,
            config_options,
        }))
    }

    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession> {
        self.inner.active_ops.fetch_add(1, Ordering::SeqCst);
        let _op = ActiveOp(&self.inner.active_ops);
        let server = self.inner.ensure_server(&req.launch, &req.cwd).await?;
        let client = server.client.clone();

        let resume_id = req
            .resume
            .as_ref()
            .and_then(|resume| resume.agent_session_id.clone());
        let allow_fresh = req.resume.as_ref().is_some_and(|resume| resume.allow_fresh);
        let capture_history = req
            .resume
            .as_ref()
            .is_some_and(|resume| resume.capture_history);

        let (agent_session_id, context_restored) = if let Some(source) = req.fork_from.as_deref() {
            let id = client
                .fork_session(source, req.fork_before_message_id.as_deref())
                .await
                .map_err(|err| NativeForkUnavailable(err.to_string()))?;
            (id, true)
        } else if let Some(resume_id) = resume_id {
            if client.session_exists(&resume_id).await? {
                (resume_id, true)
            } else if allow_fresh {
                (
                    client
                        .create_session(
                            &req.cwd,
                            Some(&req.title.clone().unwrap_or_default()),
                            req.initial_mode.as_deref(),
                            initial_model(&req.initial_config).as_deref(),
                        )
                        .await?,
                    false,
                )
            } else {
                return Err(NativeResumeUnavailable(format!(
                    "OpenCode session not found: {resume_id}"
                ))
                .into());
            }
        } else if req.resume.is_some() && !allow_fresh {
            return Err(NativeResumeUnavailable(
                "no OpenCode agent session ID was recorded".to_owned(),
            )
            .into());
        } else {
            (
                client
                    .create_session(
                        &req.cwd,
                        req.title.as_deref(),
                        req.initial_mode.as_deref(),
                        initial_model(&req.initial_config).as_deref(),
                    )
                    .await?,
                false,
            )
        };

        let runtime = Arc::new(SessionRuntime {
            fxg_session_id: req.session_id.clone(),
            agent_session_id: agent_session_id.clone(),
            cwd: req.cwd.clone(),
            event_tx,
            state: std::sync::Mutex::new(RuntimeState::default()),
        });
        self.inner.register(Arc::clone(&runtime));
        // Establish a baseline before the first prompt. Without it, an initial
        // `server.connected` backfill would replay the whole session history.
        if let Ok(Some(seq)) = client.session_log_watermark(&agent_session_id).await {
            runtime
                .state
                .lock()
                .expect("state poisoned")
                .durable_seq
                .insert(agent_session_id.clone(), seq);
        }

        // The binding is also required for imports: an imported fxg session is
        // new even though the OpenCode session already existed. The projection
        // update is idempotent, and ACP emits the same event on every start.
        runtime.emit_event(UnifiedEventPayload::SessionAgentBound {
            agent_session_id: agent_session_id.clone(),
        });
        if context_restored {
            if let Some(mode) = req.initial_mode.as_deref()
                && let Err(err) = client.switch_agent(&agent_session_id, mode).await
            {
                warn!("failed to apply the initial OpenCode agent: {err:#}");
            }
            if let Some(model) = initial_model(&req.initial_config)
                && let Some(model) = parse_model_ref(&model)
                && let Err(err) = client.switch_model(&agent_session_id, model).await
            {
                warn!("failed to apply the initial OpenCode model: {err:#}");
            }
        }

        match wait_for_capabilities(&client, &req.cwd, Some(&agent_session_id)).await {
            Some((payload, commands, context_size)) => {
                {
                    let mut state = runtime.state.lock().expect("state poisoned");
                    state.commands = commands;
                    state.context_size = context_size;
                }
                runtime.emit_event(payload);
            }
            None => warn!(
                session_id = %req.session_id,
                "failed to load OpenCode capabilities"
            ),
        }

        if capture_history {
            match client.messages(&agent_session_id, 1000).await {
                Ok(messages) => {
                    emit_history(&runtime, &messages);
                    // Flush the imported history before the manager returns
                    // (the event pump waits for this status boundary).
                    runtime.emit_event(UnifiedEventPayload::StatusChanged {
                        status: SessionStatus::Idle,
                        error_message: None,
                    });
                }
                Err(err) => {
                    self.inner.unregister(&agent_session_id);
                    return Err(HistoryCaptureUnavailable(format!(
                        "failed to load OpenCode history: {err:#}"
                    ))
                    .into());
                }
            }
        }

        if context_restored {
            // A resumed/imported session may already be running or waiting for
            // a permission/form that was emitted before this client attached.
            resync_session(&self.inner, &client, &runtime).await;
        }

        Ok(StartedSession {
            handle: Box::new(OpenCodeSessionHandle {
                inner: Arc::clone(&self.inner),
                client,
                agent_session_id: agent_session_id.clone(),
                runtime,
            }),
            context_restored,
            agent_session_id: Some(agent_session_id),
        })
    }

    async fn list_sessions(
        &self,
        launch: &AgentLaunchSpec,
        cwd: &Path,
    ) -> anyhow::Result<AgentSessionList> {
        self.inner.active_ops.fetch_add(1, Ordering::SeqCst);
        let _op = ActiveOp(&self.inner.active_ops);
        let server = self.inner.ensure_server(launch, cwd).await?;
        let sessions = server.client.list_sessions(cwd).await?;
        Ok(AgentSessionList {
            sessions,
            supports_history: true,
        })
    }

    async fn shutdown_idle(&self) -> usize {
        usize::from(self.inner.stop_if_idle().await)
    }

    fn stderr_tail(&self, _agent_id: &str) -> Vec<String> {
        // Keep the same chronological order as the ACP driver; diagnostics
        // clients render the tail top-to-bottom.
        self.inner
            .stderr
            .lock()
            .expect("stderr poisoned")
            .iter()
            .rev()
            .take(50)
            .rev()
            .cloned()
            .collect()
    }

    fn stderr_snapshot(&self) -> BTreeMap<String, Vec<String>> {
        BTreeMap::from([("opencode".to_owned(), self.stderr_tail("opencode"))])
    }
}

pub(super) struct OpenCodeSessionHandle {
    pub(super) inner: Arc<DriverInner>,
    pub(super) client: OpenCodeClient,
    pub(super) agent_session_id: String,
    pub(super) runtime: Arc<SessionRuntime>,
}

impl OpenCodeSessionHandle {
    /// Whether this prompt is routed to `/command` or `/compact`, neither of
    /// which accepts a caller-provided message ID.
    fn is_native_command(&self, text: &str) -> bool {
        let Some(rest) = text.trim_start().strip_prefix('/') else {
            return false;
        };
        let name = rest.split_whitespace().next().unwrap_or_default();
        name == COMPACT_COMMAND
            || self
                .runtime
                .state
                .lock()
                .expect("state poisoned")
                .commands
                .iter()
                .any(|command| command == name)
    }
}

#[async_trait]
impl ActiveSessionHandle for OpenCodeSessionHandle {
    fn session_capabilities(&self) -> DriverSessionCapabilities {
        DriverSessionCapabilities {
            assigns_message_ids: true,
            native_features: NATIVE_SESSION_FEATURES.to_vec(),
        }
    }

    fn assigns_message_id_for(&self, input: &PromptInput) -> bool {
        // `/command` and `/compact` do not accept a caller-provided message ID.
        !self.is_native_command(&input.text)
    }

    async fn send_prompt(&self, input: PromptInput) -> anyhow::Result<()> {
        let delivery = self.runtime.is_busy().then_some("steer");
        let files = attachments_to_prompt_files(&input.attachments);
        let text = input.text.trim_start();
        if let Some(rest) = text.strip_prefix('/') {
            let name = rest.split_whitespace().next().unwrap_or_default();
            if name == COMPACT_COMMAND {
                self.client
                    .compact(&self.agent_session_id, delivery)
                    .await?;
                return Ok(());
            }
            let known = self
                .runtime
                .state
                .lock()
                .expect("state poisoned")
                .commands
                .iter()
                .any(|command| command == name);
            if known {
                {
                    self.runtime
                        .state
                        .lock()
                        .expect("state poisoned")
                        .pending_commands += 1;
                }
                let arguments = rest[name.len()..].trim_start();
                let res = self
                    .client
                    .send_command(&self.agent_session_id, name, arguments, &files, delivery)
                    .await;
                if res.is_err() {
                    let mut state = self.runtime.state.lock().expect("state poisoned");
                    state.pending_commands = state.pending_commands.saturating_sub(1);
                }
                return res;
            }
        }
        if let Some(id) = input.agent_message_id.clone() {
            self.runtime
                .state
                .lock()
                .expect("state poisoned")
                .self_messages
                .insert(id);
        }
        let res = self
            .client
            .send_prompt(&self.agent_session_id, &input, &files, delivery)
            .await;
        if res.is_err()
            && let Some(id) = input.agent_message_id.as_deref()
        {
            self.runtime
                .state
                .lock()
                .expect("state poisoned")
                .self_messages
                .remove(id);
        }
        res
    }

    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()> {
        self.client
            .reply_permission(&self.agent_session_id, &request_id, &selected_option_id)
            .await
    }

    async fn respond_elicitation(
        &self,
        elicitation_id: String,
        action: ElicitationAction,
        content: Option<ElicitationContent>,
    ) -> anyhow::Result<()> {
        match action {
            ElicitationAction::Accept => {
                let content = content.unwrap_or_default();
                self.client
                    .reply_form(&self.agent_session_id, &elicitation_id, &content)
                    .await
            }
            ElicitationAction::Decline | ElicitationAction::Cancel => {
                self.client
                    .cancel_form(&self.agent_session_id, &elicitation_id)
                    .await
            }
        }
    }

    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()> {
        self.client
            .switch_agent(&self.agent_session_id, &mode_id)
            .await
    }

    async fn set_config(&self, key: String, value: ConfigOptionValue) -> anyhow::Result<()> {
        if key != "model" {
            anyhow::bail!("OpenCode does not expose config option {key}");
        }
        let Some(value) = value.select_value() else {
            anyhow::bail!("model config value must be a select value");
        };
        let model =
            parse_model_ref(value).ok_or_else(|| anyhow!("invalid model value: {value}"))?;
        self.client
            .switch_model(&self.agent_session_id, model)
            .await
    }

    async fn revert_to_message(&self, agent_message_id: &str, files: bool) -> anyhow::Result<()> {
        self.client
            .revert(&self.agent_session_id, agent_message_id, files)
            .await
    }

    async fn compact_context(&self) -> anyhow::Result<()> {
        let delivery = self.runtime.is_busy().then_some("steer");
        self.client.compact(&self.agent_session_id, delivery).await
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        self.client.interrupt(&self.agent_session_id).await
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.inner.unregister(&self.agent_session_id);
        Ok(())
    }

    async fn dispose(&self) -> anyhow::Result<()> {
        self.shutdown().await?;
        self.inner.stop_if_idle().await;
        Ok(())
    }
}

impl OpenCodeNativeDriver {
    pub(super) async fn management_client(&self) -> anyhow::Result<OpenCodeClient> {
        if let Some(server) = self.inner.server().await {
            return Ok(server.client.clone());
        }
        if let Some(client) = &self.inner.injected {
            return Ok(client.clone());
        }
        Self::connect_management().await
    }
}

#[async_trait]
impl AgentManagement for OpenCodeNativeDriver {
    fn agent_id(&self) -> &str {
        "opencode"
    }

    async fn snapshot(&self) -> anyhow::Result<fxg_protocol::client_api::AgentManagementSnapshot> {
        let cwd = self
            .inner
            .runtimes()
            .first()
            .map(|runtime| runtime.cwd.clone());
        self.management_client()
            .await?
            .management_snapshot(cwd.as_deref())
            .await
    }

    async fn mcp_action(
        &self,
        name: &str,
        action: fxg_protocol::client_api::AgentMcpAction,
    ) -> anyhow::Result<()> {
        self.management_client()
            .await?
            .mcp_action(name, action)
            .await
    }

    async fn upsert_mcp(
        &self,
        name: &str,
        definition: &fxg_protocol::client_api::AgentMcpDefinition,
    ) -> anyhow::Result<()> {
        self.management_client()
            .await?
            .upsert_mcp(name, definition)
            .await
    }

    async fn remove_mcp(&self, name: &str) -> anyhow::Result<()> {
        self.management_client().await?.remove_mcp(name).await
    }
}

// ---------------------------------------------------------------------------
// Backfill
// ---------------------------------------------------------------------------

pub(super) async fn backfill_all(inner: &Arc<DriverInner>, server: &Arc<Server>) {
    let runtimes = inner
        .runtimes
        .lock()
        .expect("runtimes poisoned")
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for runtime in runtimes {
        let Some(after) = runtime
            .state
            .lock()
            .expect("state poisoned")
            .durable_seq
            .get(&runtime.agent_session_id)
            .copied()
        else {
            // Without a baseline, replaying from zero would duplicate the
            // session history that the node already has.
            continue;
        };
        if let Err(err) = backfill_session(inner, server, &runtime, after).await {
            debug!(
                session_id = %runtime.fxg_session_id,
                "OpenCode backfill failed: {err:#}"
            );
        }
    }
}

/// Re-syncs every runtime after an SSE connection is established.
pub(super) async fn sync_all(inner: &Arc<DriverInner>) {
    let Ok(_guard) = inner.sync.try_lock() else {
        return;
    };
    let Some(server) = inner.server().await else {
        return;
    };
    backfill_all(inner, &server).await;
    resync_all(inner, &server.client).await;
}

pub(super) async fn backfill_session(
    inner: &Arc<DriverInner>,
    server: &Arc<Server>,
    runtime: &Arc<SessionRuntime>,
    after: u64,
) -> anyhow::Result<()> {
    let path = format!(
        "/api/experimental/session/{}/log?after={after}&follow=false",
        runtime.agent_session_id
    );
    let response = server
        .client
        .auth(server.client.sse_http.get(server.client.url(&path)))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("OpenCode session log returned {}", response.status());
    }
    let text = response.text().await?;
    for frame in text.split("\n\n") {
        let data = frame
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("log.synced") {
            continue;
        }
        // Use the live apply path so child registration, status scoping and the
        // seen-event dedupe behave exactly like a live reconnect.
        apply_raw_event(inner, &value).await;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_server_url_reads_the_listen_line() {
        assert_eq!(
            parse_server_url("server listening on http://127.0.0.1:38219").as_deref(),
            Some("http://127.0.0.1:38219")
        );
        assert_eq!(parse_server_url("other output"), None);
    }

    #[test]
    fn model_values_round_trip() {
        let value = parse_model_ref("opencode-go/model#high").expect("model");
        assert_eq!(value["providerID"], "opencode-go");
        assert_eq!(value["id"], "model");
        assert_eq!(value["variant"], "high");
        assert_eq!(
            model_ref_value(&value)
                .and_then(|value| value.select_value().map(str::to_owned))
                .as_deref(),
            Some("opencode-go/model#high")
        );
        assert_eq!(
            model_ref_value(&json!({
                "providerID": "opencode",
                "id": "big-pickle",
                "variant": "default",
            }))
            .and_then(|value| value.select_value().map(str::to_owned))
            .as_deref(),
            Some("opencode/big-pickle")
        );
    }

    #[test]
    fn tool_diff_is_built_for_edit_inputs() {
        let diff = tool_diff(
            "edit",
            &json!({
                "filePath": "src/main.rs",
                "oldString": "a\n",
                "newString": "b\n"
            }),
        )
        .expect("diff");
        assert_eq!(diff.path, "src/main.rs");
        assert_eq!(diff.additions, 1);
        assert_eq!(diff.deletions, 1);
    }

    #[test]
    fn form_constraints_and_hidden_fields_are_preserved() {
        let payload = form_to_elicitation(
            &json!({
                "form": {
                    "id": "frm_2",
                    "sessionID": "ses_1",
                    "title": "Limits",
                    "fields": [
                        {"key": "count", "type": "integer", "minimum": 1, "maximum": 10,
                         "required": true},
                        {"key": "secret", "type": "string", "hidden": true, "default": "x"},
                        {"key": "code", "type": "string", "pattern": "^[A-Z]+$",
                         "minLength": 2}
                    ]
                }
            }),
            None,
        )
        .expect("elicitation");
        let UnifiedEventPayload::ElicitationRequest {
            requested_schema, ..
        } = payload
        else {
            panic!("expected a form");
        };
        let count = &requested_schema.properties["count"];
        assert_eq!(count.minimum, Some(1));
        assert_eq!(count.maximum, Some(10));
        assert_eq!(
            requested_schema.required.as_deref(),
            Some(&["count".to_owned()][..])
        );
        let secret = &requested_schema.properties["secret"];
        assert_eq!(
            secret.extra.get("hidden").map(RawJson::as_value),
            Some(&Value::Bool(true))
        );
        let code = &requested_schema.properties["code"];
        assert_eq!(
            code.extra.get("pattern").map(RawJson::as_value),
            Some(&Value::String("^[A-Z]+$".to_owned()))
        );
    }

    #[test]
    fn failed_tool_reports_its_error_and_releases_state() {
        let runtime = test_runtime();
        let _ = map_event(
            &runtime,
            "session.tool.input.started",
            &json!({"sessionID": "ses_1", "id": "call_1", "name": "bash"}),
            None,
            None,
            &json!({"id": "evt_tool_start"}),
        );
        let _ = map_event(
            &runtime,
            "session.tool.called",
            &json!({"sessionID": "ses_1", "id": "call_1", "input": {"command": "false"}}),
            None,
            None,
            &json!({"id": "evt_tool_called"}),
        );
        let events = map_event(
            &runtime,
            "session.tool.failed",
            &json!({
                "sessionID": "ses_1",
                "id": "call_1",
                "error": {"message": "command failed"}
            }),
            None,
            None,
            &json!({"id": "evt_tool_failed"}),
        );
        assert!(matches!(
            events.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::ToolCall {
                status: ToolCallStatus::Failed,
                raw_output: Some(output),
                ..
            })] if output.contains("command failed")
        ));
        let state = runtime.state.lock().expect("state");
        assert!(state.tool_names.is_empty());
        assert!(state.tool_titles.is_empty());
        assert!(state.tool_inputs.is_empty());
    }

    #[test]
    fn command_turns_do_not_assign_native_message_ids() {
        let runtime = Arc::new(test_runtime());
        runtime
            .state
            .lock()
            .expect("state")
            .commands
            .push("review".to_owned());
        let handle = OpenCodeSessionHandle {
            inner: Arc::new(DriverInner {
                server: Mutex::new(None),
                injected: None,
                runtimes: std::sync::Mutex::new(HashMap::new()),
                children: std::sync::Mutex::new(HashMap::new()),
                sse_running: AtomicBool::new(false),
                active_ops: std::sync::atomic::AtomicUsize::new(0),
                sync: Mutex::new(()),
                stderr: std::sync::Mutex::new(Vec::new()),
            }),
            client: OpenCodeClient::new("http://127.0.0.1:1", "test"),
            agent_session_id: "ses_1".to_owned(),
            runtime,
        };
        assert!(!handle.assigns_message_id_for(&PromptInput::text("/review")));
        assert!(!handle.assigns_message_id_for(&PromptInput::text("/compact")));
        assert!(handle.assigns_message_id_for(&PromptInput::text("/unknown")));
        assert!(handle.assigns_message_id_for(&PromptInput::text("hello")));
    }

    #[test]
    fn inbox_emits_only_user_items_with_native_ids() {
        let runtime = test_runtime();
        let events = map_event(
            &runtime,
            "session.inbox.enqueued",
            &json!({
                "sessionID": "ses_1",
                "inboxID": "msg_1",
                "item": {
                    "type": "user",
                    "payload": {
                        "text": "hello",
                        "files": [{
                            "data": "aGk=",
                            "mime": "text/plain",
                            "source": { "type": "inline" },
                            "name": "a.txt"
                        }]
                    }
                }
            }),
            None,
            None,
            &json!({"id": "evt_inbox_user"}),
        );
        assert!(matches!(
            events.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::UserMessage {
                text,
                attachments,
                agent_message_id: Some(id),
                ..
            })] if text == "hello"
                && attachments.len() == 1
                && attachments[0].data_b64.as_deref() == Some("aGk=")
                && id == "msg_1"
        ));

        let events = map_event(
            &runtime,
            "session.inbox.enqueued",
            &json!({
                "sessionID": "ses_1",
                "inboxID": "msg_2",
                "item": { "type": "synthetic", "payload": { "text": "hidden" } }
            }),
            None,
            None,
            &json!({"id": "evt_inbox_synthetic"}),
        );
        assert!(events.is_empty());
    }

    #[test]
    fn child_events_keep_the_parent_status_and_metadata() {
        let runtime = test_runtime();
        let info = ChildInfo {
            root: "ses_1".to_owned(),
            parent: Some("ses_1".to_owned()),
            depth: 2,
            title: Some("Explore".to_owned()),
            status: SubagentStatus::Running,
        };

        // A child start must not make the root session busy.
        let events = map_event(
            &runtime,
            "session.execution.started",
            &json!({"sessionID": "ses_child"}),
            Some("ses_child"),
            Some(&info),
            &json!({"id": "evt_child_start"}),
        );
        assert!(events.is_empty());

        // A child rename is a subagent title update, not the root title.
        let events = map_event(
            &runtime,
            "session.renamed",
            &json!({"sessionID": "ses_child", "title": "Renamed"}),
            Some("ses_child"),
            Some(&info),
            &json!({"id": "evt_child_rename"}),
        );
        assert!(matches!(
            events.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
                parent_session_id: Some(parent),
                depth: 2,
                title: Some(title),
                ..
            })] if parent == "ses_1" && title == "Renamed"
        ));

        // Completion keeps the metadata stored when the child was announced.
        let info = ChildInfo {
            status: SubagentStatus::Completed,
            ..info
        };
        let events = map_event(
            &runtime,
            "session.execution.succeeded",
            &json!({"sessionID": "ses_child"}),
            Some("ses_child"),
            Some(&info),
            &json!({"id": "evt_child_done"}),
        );
        assert!(matches!(
            events.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
                parent_session_id: Some(parent),
                depth: 2,
                title: Some(title),
                status: SubagentStatus::Completed,
                ..
            })] if parent == "ses_1" && title == "Explore"
        ));
    }

    #[test]
    fn location_query_encodes_the_project_directory() {
        assert_eq!(
            location_query(Path::new("/tmp/a b&c")),
            "location%5Bdirectory%5D=%2Ftmp%2Fa%20b%26c"
        );
    }

    #[test]
    fn attachments_use_prompt_input_uris() {
        let files = attachments_to_prompt_files(&[
            fxg_protocol::common::AttachmentMeta {
                file_name: "a.txt".to_owned(),
                mime_type: "text/plain".to_owned(),
                size_bytes: 2,
                local_path: Some("/tmp/a b.txt".to_owned()),
                data_b64: None,
            },
            fxg_protocol::common::AttachmentMeta {
                file_name: "b.txt".to_owned(),
                mime_type: "text/plain".to_owned(),
                size_bytes: 2,
                local_path: None,
                data_b64: Some("aGk=".to_owned()),
            },
        ]);
        assert_eq!(files[0]["uri"], "file:///tmp/a%20b.txt");
        assert_eq!(files[0]["name"], "a.txt");
        assert_eq!(files[1]["uri"], "data:text/plain;base64,aGk=");
        assert_eq!(files[1]["source"]["type"], "inline");
        assert_eq!(files[1]["name"], "b.txt");
    }

    #[test]
    fn form_fields_become_an_elicitation_schema() {
        let payload = form_to_elicitation(
            &json!({
                "form": {
                    "id": "frm_1",
                    "sessionID": "ses_1",
                    "title": "Pick one",
                    "fields": [
                        {"key": "choice", "type": "string", "title": "Choice", "required": true,
                         "options": [{"value": "a", "label": "A"}, {"value": "b", "label": "B"}]},
                        {"key": "flag", "type": "boolean", "title": "Flag"}
                    ]
                }
            }),
            None,
        )
        .expect("elicitation");
        let UnifiedEventPayload::ElicitationRequest {
            elicitation_id,
            requested_schema,
            ..
        } = payload
        else {
            panic!("expected an elicitation request");
        };
        assert_eq!(elicitation_id, "frm_1");
        assert_eq!(requested_schema.properties.len(), 2);
        assert_eq!(
            requested_schema.required.as_deref(),
            Some(&["choice".to_owned()][..])
        );
    }

    #[test]
    fn text_and_tool_events_map_to_normalized_payloads() {
        let runtime = test_runtime();
        let events = map_event(
            &runtime,
            "session.text.ended",
            &json!({"sessionID": "ses_1", "assistantMessageID": "msg_1", "text": "hello"}),
            None,
            None,
            &json!({"id": "evt_1"}),
        );
        assert!(matches!(
            events.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::AgentMessage { text, .. })] if text == "hello"
        ));

        let events = map_event(
            &runtime,
            "session.tool.input.started",
            &json!({"sessionID": "ses_1", "id": "call_1", "name": "edit"}),
            None,
            None,
            &json!({"id": "evt_2"}),
        );
        assert!(matches!(
            events.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::ToolCall { status, .. })]
                if *status == ToolCallStatus::Pending
        ));
    }

    #[test]
    fn usage_uses_step_tokens_and_cache_read() {
        let runtime = test_runtime();
        let events = map_event(
            &runtime,
            "session.step.ended",
            &json!({
                "sessionID": "ses_1",
                "tokens": {
                    "input": 105,
                    "output": 12,
                    "reasoning": 1,
                    "cache": { "read": 6656, "write": 0 }
                },
                "cost": 0.25
            }),
            None,
            None,
            &json!({"id": "evt_usage_step"}),
        );
        let [
            DriverEvent::Event(UnifiedEventPayload::UsageUpdated {
                used_tokens, cost, ..
            }),
        ] = events.as_slice()
        else {
            panic!("expected usage");
        };
        assert_eq!(*used_tokens, 6774);
        assert!(cost.as_ref().is_some_and(|cost| cost.amount == 0.25));

        // `session.usage.updated` is cumulative; only its cost replaces the
        // accumulated value.
        let events = map_event(
            &runtime,
            "session.usage.updated",
            &json!({
                "sessionID": "ses_1",
                "tokens": {
                    "input": 233635,
                    "output": 16676,
                    "reasoning": 29220,
                    "cache": { "read": 9372672, "write": 0 }
                },
                "cost": 1.5
            }),
            None,
            None,
            &json!({"id": "evt_usage_session"}),
        );
        let [
            DriverEvent::Event(UnifiedEventPayload::UsageUpdated {
                used_tokens, cost, ..
            }),
        ] = events.as_slice()
        else {
            panic!("expected usage");
        };
        assert_eq!(*used_tokens, 6774);
        assert!(cost.as_ref().is_some_and(|cost| cost.amount == 1.5));
    }

    #[test]
    fn seen_event_ids_evicts_oldest_without_clearing_recent() {
        let mut seen = SeenEventIds::default();
        for i in 0..MAX_SEEN_EVENT_IDS {
            assert!(seen.insert(&format!("evt_{i}")));
        }
        // Adding one more evicts evt_0 while keeping recent IDs.
        assert!(seen.insert("evt_new"));
        assert!(!seen.insert("evt_new"));
        assert!(!seen.insert(&format!("evt_{}", MAX_SEEN_EVENT_IDS - 1)));
        assert!(seen.insert("evt_0"));
    }

    fn test_runtime() -> SessionRuntime {
        let (tx, _rx) = mpsc::unbounded_channel();
        SessionRuntime {
            fxg_session_id: "fxg".to_owned(),
            agent_session_id: "ses_1".to_owned(),
            cwd: PathBuf::from("."),
            event_tx: tx,
            state: std::sync::Mutex::new(RuntimeState::default()),
        }
    }
}
