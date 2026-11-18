use super::prompt::*;
use super::*;

/// Persists and delivers driver events, managing status transitions and queues.
pub(super) async fn pump_events(
    inner: Arc<Inner>,
    session_id: String,
    mut event_rx: mpsc::UnboundedReceiver<DriverEvent>,
    mut initial_flushed: Option<tokio::sync::oneshot::Sender<()>>,
) {
    while let Some(event) = event_rx.recv().await {
        match event {
            DriverEvent::Delta(delta) => inner.bus.publish_delta(session_id.as_str(), delta),
            DriverEvent::Failed { message } => {
                mark_status(
                    &inner,
                    &session_id,
                    SessionStatus::Error,
                    Some(message.clone()),
                );
                if let Err(err) = inner
                    .bus
                    .record(
                        &session_id,
                        UnifiedEventPayload::StatusChanged {
                            status: SessionStatus::Error,
                            error_message: Some(message),
                        },
                    )
                    .await
                {
                    tracing::warn!(
                        session_id = %session_id,
                        "failed to record the driver failure: {err}"
                    );
                }
                if let Some(tx) = initial_flushed.take() {
                    fxg_protocol::discard_with!(tx.send(()), "notify initial event flush");
                }
            }
            DriverEvent::Event(payload) => {
                let is_status_changed =
                    matches!(&payload, UnifiedEventPayload::StatusChanged { .. });
                // A status change to a non-busy state ends the turn: commands that
                // keep running must be persisted then (their final event may be
                // minutes away, or never come).
                let turn_ended = matches!(
                    &payload,
                    UnifiedEventPayload::StatusChanged { status, .. } if !status.is_busy()
                );
                let mut auto_permission: Option<(String, String)> = None;
                match &payload {
                    UnifiedEventPayload::StatusChanged {
                        status,
                        error_message,
                    } => {
                        mark_status(&inner, &session_id, *status, error_message.clone());
                    }
                    UnifiedEventPayload::PermissionRequest {
                        request_id,
                        tool_name,
                        summary,
                        options,
                        ..
                    } => {
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        if let Some(session) = sessions.active.get_mut(&session_id) {
                            session.pending_permissions.insert(
                                request_id.clone(),
                                PendingPermissionSummary {
                                    session_id: session_id.clone(),
                                    request_id: request_id.clone(),
                                    tool_name: tool_name.clone(),
                                    summary: summary.clone(),
                                    options: options.clone(),
                                },
                            );
                            if session.permission_policy == PermissionPolicy::AutoAllowAll {
                                // `auto_allow_all` should persist the rule when
                                // the agent offers `allow_always`, not merely
                                // approve this request once.
                                auto_permission = options
                                    .iter()
                                    .find(|option| {
                                        option.kind.is_allow() && option.kind.is_always()
                                    })
                                    .or_else(|| {
                                        options.iter().find(|option| option.kind.is_allow())
                                    })
                                    .map(|option| (request_id.clone(), option.option_id.clone()));
                            }
                        }
                    }
                    UnifiedEventPayload::PermissionResolved { request_id, .. } => {
                        // Permissions resolved on the driver side (e.g. a native agent UI) also arrive here. Responses via fxg are already
                        // recorded by `SessionManager::respond_permission`,
                        // so only record it if it is still in the pending list.
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        let already_recorded =
                            sessions.active.get_mut(&session_id).is_some_and(|session| {
                                session.pending_permissions.remove(request_id).is_none()
                            });
                        if already_recorded {
                            continue;
                        }
                    }
                    UnifiedEventPayload::ElicitationRequest {
                        elicitation_id,
                        message,
                        mode,
                        requested_schema,
                        ..
                    } => {
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        if let Some(session) = sessions.active.get_mut(&session_id) {
                            session.pending_elicitations.insert(
                                elicitation_id.clone(),
                                PendingElicitationSummary {
                                    session_id: session_id.clone(),
                                    elicitation_id: elicitation_id.clone(),
                                    message: message.clone(),
                                    mode: mode.clone(),
                                    requested_schema: requested_schema.clone(),
                                },
                            );
                        }
                    }
                    UnifiedEventPayload::ElicitationResolved { elicitation_id, .. } => {
                        // Elicitations resolved on the driver side (e.g. automatic
                        // resolution on turn cancel) also arrive here. Responses via fxg
                        // are already recorded by `SessionManager::respond_elicitation`,
                        // so only record it if it is still in the pending list.
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        let already_recorded =
                            sessions.active.get_mut(&session_id).is_some_and(|session| {
                                session
                                    .pending_elicitations
                                    .remove(elicitation_id)
                                    .is_none()
                            });
                        if already_recorded {
                            continue;
                        }
                    }
                    UnifiedEventPayload::SessionTitleChanged { .. } => {
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        if let Some(session) = sessions.active.get_mut(&session_id) {
                            session.has_custom_title = true;
                        }
                    }
                    UnifiedEventPayload::CapabilitiesUpdated {
                        current_mode,
                        available_modes,
                        config_options,
                        ..
                    } => {
                        let session = {
                            let sessions = inner.sessions.lock().expect("sessions poisoned");
                            sessions
                                .active
                                .get(&session_id)
                                .map(|session| (session.agent_id.clone(), session.cwd.clone()))
                        };
                        if let Some((agent_id, cwd)) = session {
                            inner.remember_capabilities(
                                &agent_id,
                                &cwd,
                                AgentCapabilitiesPreview {
                                    available_modes: available_modes.clone(),
                                    current_mode: current_mode.clone(),
                                    config_options: config_options.clone(),
                                },
                            );
                        }
                    }
                    _ => {}
                }
                match inner.bus.record(&session_id, payload).await {
                    Ok(_) => {
                        if let Some((request_id, option_id)) = auto_permission {
                            auto_respond_permission(&inner, &session_id, request_id, option_id)
                                .await;
                        }
                        if turn_ended {
                            snapshot_terminals(&inner, &session_id).await;
                        }
                        maybe_dispatch_queued(&inner, &session_id).await;
                    }
                    Err(err) => {
                        tracing::warn!(session_id = %session_id, "failed to persist driver event: {err:#}");
                    }
                }
                if is_status_changed && let Some(tx) = initial_flushed.take() {
                    fxg_protocol::discard_with!(tx.send(()), "notify initial event flush");
                }
            }
        }
    }
    if let Some(tx) = initial_flushed.take() {
        fxg_protocol::discard_with!(tx.send(()), "notify initial event flush");
    }
    // Channel closed = the driver (agent process) exited.
    // The transition to `Stopped` must also be reflected in the DB `sessions`
    // projection, so append it as an event (leave it as-is if already Stopped / Error).
    let previous = {
        let sessions = inner.sessions.lock().expect("sessions poisoned");
        sessions
            .active
            .get(&session_id)
            .map(|session| session.status)
    };
    mark_status(&inner, &session_id, SessionStatus::Stopped, None);
    if matches!(
        previous,
        Some(
            SessionStatus::Provisioning
                | SessionStatus::Idle
                | SessionStatus::Running
                | SessionStatus::WaitingPermission
                | SessionStatus::WaitingInput
        )
    ) && let Err(err) = inner
        .bus
        .record(
            &session_id,
            UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Stopped,
                error_message: None,
            },
        )
        .await
    {
        tracing::warn!(
            session_id = %session_id,
            "failed to record the stopped status: {err}"
        );
    }
    {
        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
        sessions.active.remove(&session_id);
    }
}

/// Auto-approves a permission request according to the session policy.
///
/// The request must already be recorded: recording the resolution first would leave the
/// `permission_requests` projection pending (and show stale approval buttons in the UI).
async fn auto_respond_permission(
    inner: &Arc<Inner>,
    session_id: &str,
    request_id: String,
    option_id: String,
) {
    let handle = inner
        .sessions
        .lock()
        .expect("sessions poisoned")
        .active
        .get(session_id)
        .map(|session| Arc::clone(&session.handle));
    let Some(handle) = handle else {
        return;
    };
    if let Err(err) = handle
        .respond_permission(request_id.clone(), option_id.clone())
        .await
    {
        // Keep the request pending so the user can still decide manually.
        tracing::warn!(session_id, "auto-approval failed: {err:#}");
        return;
    }
    {
        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
        if let Some(session) = sessions.active.get_mut(session_id) {
            session.pending_permissions.remove(&request_id);
        }
    }
    if let Err(err) = inner
        .bus
        .record(
            session_id,
            UnifiedEventPayload::PermissionResolved {
                request_id,
                selected_option_id: option_id,
                resolved_by: ResolvedBy::Unknown("auto".to_owned()),
            },
        )
        .await
    {
        tracing::warn!(session_id, "failed to record auto-approval: {err}");
    }
}

/// Asks the driver to persist the output of commands that outlive the turn.
///
/// ACP terminals keep running after the turn ends (a dev server, a watcher), but
/// their next `TerminalOutput` event is the exit. Recording the output produced so
/// far keeps the card and its log after a reload.
async fn snapshot_terminals(inner: &Arc<Inner>, session_id: &str) {
    let handle = inner
        .sessions
        .lock()
        .expect("sessions poisoned")
        .active
        .get(session_id)
        .map(|session| Arc::clone(&session.handle));
    if let Some(handle) = handle {
        handle.snapshot_terminals().await;
    }
}

/// Updates the session status (busy is synchronized too).
pub(super) fn mark_status(
    inner: &Inner,
    session_id: &str,
    status: SessionStatus,
    _error_message: Option<String>,
) {
    let mut sessions = inner.sessions.lock().expect("sessions poisoned");
    if let Some(session) = sessions.active.get_mut(session_id) {
        session.busy = status.is_busy();
        session.status = status;
    }
}
