use super::*;

impl SessionManager {
    /// Starts a turn (busy reservation → `UserMessage` → send).
    ///
    /// The caller has already decided busy handling (append to the pending queue or reject).
    pub(super) async fn start_turn(
        &self,
        session_id: &str,
        input: &PromptInput,
        client_source: ClientSource,
    ) -> Result<(), NodeError> {
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions
                .active
                .get_mut(session_id)
                .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;
            session.busy = true;
        }
        match dispatch_prompt(&self.inner, session_id, input, client_source).await {
            Ok(()) => Ok(()),
            Err(err) => {
                // Clear busy on send failure to keep the state consistent
                if let Some(session) = self
                    .inner
                    .sessions
                    .lock()
                    .expect("sessions poisoned")
                    .active
                    .get_mut(session_id)
                {
                    session.busy = false;
                }
                Err(err)
            }
        }
    }

    /// Validates and normalizes prompt attachments before recording them.
    ///
    /// Local paths must stay inside the session working directory. Inline data
    /// is limited to images and a bounded size; larger files should be sent as
    /// local paths (the ACP driver reads them when building prompt blocks).
    pub(super) async fn validate_prompt_attachments(
        &self,
        session_id: &str,
        input: &mut PromptInput,
    ) -> Result<(), NodeError> {
        const MAX_INLINE_BYTES: usize = 10 * 1024 * 1024;
        if input.attachments.is_empty() {
            return Ok(());
        }
        let cwd = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            sessions
                .active
                .get(session_id)
                .map(|session| session.cwd.clone())
                .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?
        };
        let cwd = fxg_pty::canonicalize(&cwd)?;
        for attachment in &mut input.attachments {
            if let Some(data) = attachment.data_b64.as_ref() {
                if !attachment.mime_type.starts_with("image/") {
                    return Err(NodeError::InvalidState(format!(
                        "inline attachment {} must be an image; send a local path instead",
                        attachment.file_name
                    )));
                }
                if data.len().saturating_mul(3) / 4 > MAX_INLINE_BYTES {
                    return Err(NodeError::InvalidState(format!(
                        "inline attachment {} is too large (limit {MAX_INLINE_BYTES} bytes)",
                        attachment.file_name
                    )));
                }
            }
            match attachment.local_path.as_ref() {
                Some(path) => {
                    let candidate = if Path::new(path).is_absolute() {
                        PathBuf::from(path)
                    } else {
                        cwd.join(path)
                    };
                    let resolved = fxg_pty::canonicalize(&candidate)?;
                    if !resolved.starts_with(&cwd) {
                        return Err(NodeError::InvalidState(format!(
                            "attachment {} is outside the session workspace",
                            attachment.file_name
                        )));
                    }
                    if attachment.size_bytes == 0
                        && let Ok(metadata) = std::fs::metadata(&resolved)
                    {
                        attachment.size_bytes = metadata.len();
                    }
                    attachment.local_path = Some(resolved.to_string_lossy().into_owned());
                }
                None if attachment.data_b64.is_none() => {
                    return Err(NodeError::InvalidState(format!(
                        "attachment {} has neither data nor a local path",
                        attachment.file_name
                    )));
                }
                None => {}
            }
        }
        Ok(())
    }

    /// Sends a prompt (enqueues into the pending queue when busy).
    ///
    /// If the session is stopped (not active), first attempts auto-resume via
    /// native restore and then sends. Agents that do not support native restore
    /// cause [`NodeError::ResumeRequired`], so the client should guide the user to
    /// an explicit Resume (history replay).
    pub async fn send_prompt(
        &self,
        command_id: &CommandId,
        session_id: &str,
        input: &PromptInput,
        client_source: ClientSource,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_string()));
        }

        let active = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            sessions.active.contains_key(session_id)
        };
        if !active {
            // Auto-resume stopped sessions (native restore only) before sending
            self.auto_resume_for_prompt(session_id).await?;
        }
        let mut input = input.clone();
        self.validate_prompt_attachments(session_id, &mut input)
            .await?;

        // Busy check and reservation (check-and-set)
        let supports_steering = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            sessions
                .active
                .get(session_id)
                .is_some_and(|session| session.handle.supports_steering())
        };
        let steer = {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions.active.get_mut(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
            if session.busy {
                if supports_steering {
                    // Native drivers accept a prompt while the current turn is
                    // running (`steer` delivery). Keep the session busy; the
                    // driver merges the input into the active turn.
                    true
                } else {
                    session.pending_prompts.push_back(PendingPrompt {
                        input,
                        client_source,
                    });
                    return Ok(());
                }
            } else {
                false
            }
        };
        if steer {
            return dispatch_prompt(&self.inner, session_id, &input, client_source).await;
        }

        self.start_turn(session_id, &input, client_source).await
    }
    /// Returns the agent-side message ID of the latest user message at or
    /// before `up_to_node_seq`.
    async fn agent_message_id_at(
        &self,
        session_id: &str,
        up_to_node_seq: u64,
    ) -> Result<Option<String>, NodeError> {
        let mut cursor = 0;
        let mut found = None;
        loop {
            let batch = self
                .inner
                .bus
                .db()
                .session_events_after(session_id, cursor, EVENT_LOAD_BATCH)
                .await?;
            if batch.events.is_empty() {
                break;
            }
            cursor = batch.cursor;
            for event in batch.events {
                if event.node_seq > up_to_node_seq {
                    break;
                }
                if let UnifiedEventPayload::UserMessage {
                    agent_message_id: Some(id),
                    ..
                } = event.payload
                {
                    found = Some(id);
                }
            }
            if cursor == 0 {
                break;
            }
        }
        Ok(found)
    }

    /// Mode switching / config change / cancel / kill.
    pub async fn control(
        &self,
        command_id: &CommandId,
        session_id: &str,
        action: &SessionControlAction,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_string()));
        }

        // The approval policy is session metadata owned by the node rather than a
        // driver operation, so it can be changed while the session is stopped
        // (e.g. before resuming) and applies from the next turn onward.
        if let SessionControlAction::SetPermissionPolicy { policy } = action {
            {
                let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
                if let Some(session) = sessions.active.get_mut(session_id) {
                    session.permission_policy = policy.clone();
                }
            }
            self.inner
                .bus
                .record(
                    session_id,
                    UnifiedEventPayload::SessionPermissionPolicyChanged {
                        policy: policy.clone(),
                    },
                )
                .await?;
            return Ok(());
        }

        // The remaining actions operate on the running agent, so the session must
        // be active. Do not hold the lock across await (clone the handle, then operate)
        let (handle, is_busy) = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions.active.get(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
            (Arc::clone(&session.handle), session.busy)
        };
        match action {
            SessionControlAction::SetMode { mode_id } => handle
                .set_mode(mode_id.clone())
                .await
                .map_err(|err| NodeError::Hub(format!("failed to set mode: {err:#}"))),
            SessionControlAction::SetConfig { key, value } => handle
                .set_config(key.clone(), value.clone())
                .await
                .map_err(|err| NodeError::Hub(format!("failed to set config: {err:#}"))),
            SessionControlAction::SetPermissionPolicy { .. } => {
                unreachable!("the approval policy is handled before the active-session lookup")
            }
            SessionControlAction::Compact => handle
                .compact_context()
                .await
                .map_err(|err| NodeError::Hub(format!("failed to compact: {err:#}"))),
            SessionControlAction::Revert {
                up_to_node_seq,
                files,
            } => {
                if is_busy {
                    return Err(NodeError::InvalidState(
                        "revert requires an idle session".to_owned(),
                    ));
                }
                let agent_message_id = self
                    .agent_message_id_at(session_id, *up_to_node_seq)
                    .await?
                    .ok_or_else(|| {
                        NodeError::InvalidState(format!(
                            "no native user message is available at or before {up_to_node_seq}"
                        ))
                    })?;
                handle
                    .revert_to_message(&agent_message_id, *files)
                    .await
                    .map_err(|err| NodeError::Hub(format!("failed to revert: {err:#}")))?;
                self.inner
                    .bus
                    .record(
                        session_id,
                        UnifiedEventPayload::SessionReverted {
                            up_to_node_seq: *up_to_node_seq,
                            files: *files,
                        },
                    )
                    .await?;
                Ok(())
            }
            SessionControlAction::Cancel => {
                // If already idle (no turn running) there is nothing to cancel, so treat as success
                if !is_busy {
                    return Ok(());
                }
                handle
                    .cancel_turn()
                    .await
                    .map_err(|err| NodeError::Hub(format!("failed to cancel: {err:#}")))
            }
            SessionControlAction::Kill => handle
                .shutdown()
                .await
                .map_err(|err| NodeError::Hub(format!("failed to shutdown: {err:#}"))),
            SessionControlAction::Dispose => handle
                .dispose()
                .await
                .map_err(|err| NodeError::Hub(format!("failed to dispose: {err:#}"))),
        }
    }
}

/// Sends the prompt at the head of the queue after a turn ends.
pub(super) async fn maybe_dispatch_queued(inner: &Arc<Inner>, session_id: &str) {
    let next = {
        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
        let Some(session) = sessions.active.get_mut(session_id) else {
            return;
        };
        if session.busy {
            return;
        }
        match session.pending_prompts.pop_front() {
            Some(prompt) => {
                session.busy = true;
                Some(prompt)
            }
            None => None,
        }
    };
    let Some(prompt) = next else {
        return;
    };

    // busy has already been reserved by the caller. Clear it on failure to keep the state consistent.
    if let Err(err) = dispatch_prompt(
        inner,
        session_id,
        &prompt.input,
        prompt.client_source.clone(),
    )
    .await
    {
        tracing::warn!(session_id = %session_id, "failed to dispatch queued prompt: {err:#}");
        if let Err(record_err) = inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Error,
                    error_message: Some(format!("failed to dispatch queued prompt: {err:#}")),
                },
            )
            .await
        {
            tracing::warn!(
                session_id = %session_id,
                "failed to record the queued prompt failure: {record_err}"
            );
        }
        if let Some(session) = inner
            .sessions
            .lock()
            .expect("sessions poisoned")
            .active
            .get_mut(session_id)
        {
            session.busy = false;
        }
    }
}

/// Derives a human-readable session title from the beginning of the prompt.
///
/// - Trims leading whitespace, blank lines and Markdown markers (`#`, `-`, `*`, `>`, backticks, etc.)
/// - Takes the first line (up to 40 characters; appends `...` when exceeded)
/// - Returns `None` if no usable string can be extracted
pub fn derive_title_from_prompt(prompt: &str) -> Option<String> {
    for line in prompt.lines() {
        let trimmed = line.trim();
        let cleaned = trimmed.trim_start_matches(['#', '>', '-', '*', '`']).trim();
        if !cleaned.is_empty() {
            let char_count = cleaned.chars().count();
            if char_count <= 40 {
                return Some(cleaned.to_owned());
            } else {
                let truncated: String = cleaned.chars().take(40).collect();
                return Some(format!("{truncated}..."));
            }
        }
    }
    None
}

/// Starts a single turn (record `UserMessage` → send to the driver).
///
/// Busy reservation and release are handled by the caller (`SessionManager::start_turn` /
/// [`maybe_dispatch_queued`]).
pub(super) async fn dispatch_prompt(
    inner: &Arc<Inner>,
    session_id: &str,
    input: &PromptInput,
    client_source: ClientSource,
) -> Result<(), NodeError> {
    let handle = {
        let sessions = inner.sessions.lock().expect("sessions poisoned");
        sessions
            .active
            .get(session_id)
            .map(|session| Arc::clone(&session.handle))
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?
    };
    let mut input = input.clone();
    if handle.assigns_message_ids() && input.agent_message_id.is_none() {
        input.agent_message_id = Some(format!("msg_{}", uuid_v7()));
    }

    let should_update_title = {
        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
        let session = sessions
            .active
            .get_mut(session_id)
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;
        let should_update = !session.has_custom_title;
        if should_update {
            session.has_custom_title = true;
        }
        should_update
    };

    if should_update_title && let Some(title) = derive_title_from_prompt(&input.text) {
        inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::SessionTitleChanged { title },
            )
            .await?;
    }

    inner
        .bus
        .record(
            session_id,
            UnifiedEventPayload::UserMessage {
                text: input.text.clone(),
                attachments: input.attachments.clone(),
                client_source,
                agent_message_id: input.agent_message_id.clone(),
            },
        )
        .await?;

    handle
        .send_prompt(input)
        .await
        .map_err(|err| NodeError::Hub(format!("failed to send prompt: {err:#}")))
}
