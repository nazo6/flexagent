use super::prompt::*;
use super::*;

/// Persists and delivers driver events, managing status transitions and queues.
pub(super) async fn pump_events(
    inner: Arc<Inner>,
    session_id: String,
    mut event_rx: mpsc::UnboundedReceiver<DriverEvent>,
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
            }
            DriverEvent::Event(payload) => {
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
                        let auto_option = {
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
                                (session.permission_policy == PermissionPolicy::AutoAllowAll)
                                    .then(|| {
                                        options
                                            .iter()
                                            .find(|option| option.kind.is_allow())
                                            .map(|option| option.option_id.clone())
                                    })
                                    .flatten()
                            } else {
                                None
                            }
                        };
                        if let Some(option_id) = auto_option {
                            let handle = inner
                                .sessions
                                .lock()
                                .expect("sessions poisoned")
                                .active
                                .get(&session_id)
                                .map(|session| Arc::clone(&session.handle));
                            if let Some(handle) = handle {
                                if let Err(err) = handle
                                    .respond_permission(request_id.clone(), option_id.clone())
                                    .await
                                {
                                    tracing::warn!(session_id, "auto-approval failed: {err:#}");
                                } else {
                                    inner
                                        .sessions
                                        .lock()
                                        .expect("sessions poisoned")
                                        .active
                                        .get_mut(&session_id)
                                        .map(|session| {
                                            session.pending_permissions.remove(request_id)
                                        });
                                    if let Err(err) = inner
                                        .bus
                                        .record(
                                            &session_id,
                                            UnifiedEventPayload::PermissionResolved {
                                                request_id: request_id.clone(),
                                                selected_option_id: option_id,
                                                resolved_by: ResolvedBy::Unknown("auto".to_owned()),
                                            },
                                        )
                                        .await
                                    {
                                        tracing::warn!(
                                            session_id,
                                            "failed to record auto-approval: {err}"
                                        );
                                    }
                                }
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
                    Ok(_) => maybe_dispatch_queued(&inner, &session_id).await,
                    Err(err) => {
                        tracing::warn!(session_id = %session_id, "failed to persist driver event: {err:#}");
                    }
                }
            }
        }
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

/// Updates the session status (busy is synchronized too).
pub(super) fn mark_status(
    inner: &Inner,
    session_id: &str,
    status: SessionStatus,
    _error_message: Option<String>,
) {
    let mut sessions = inner.sessions.lock().expect("sessions poisoned");
    if let Some(session) = sessions.active.get_mut(session_id) {
        session.busy = matches!(
            status,
            SessionStatus::Running | SessionStatus::WaitingPermission | SessionStatus::WaitingInput
        );
        session.status = status;
    }
}
