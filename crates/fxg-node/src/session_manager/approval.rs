use super::*;

impl SessionManager {
    /// Responds to a permission request (`AlreadyResolved` on the second and later attempts).
    pub async fn respond_permission(
        &self,
        command_id: &CommandId,
        session_id: &str,
        request_id: &str,
        selected_option_id: &str,
        resolved_by: ResolvedBy,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_string()));
        }

        let handle = {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions.active.get_mut(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
            if session.pending_permissions.remove(request_id).is_none() {
                return Err(NodeError::AlreadyResolved(request_id.to_owned()));
            }
            Arc::clone(&session.handle)
        };
        handle
            .respond_permission(request_id.to_owned(), selected_option_id.to_owned())
            .await
            .map_err(|err| NodeError::Server(format!("failed to respond: {err:#}")))?;

        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::PermissionResolved {
                    request_id: request_id.to_owned(),
                    selected_option_id: selected_option_id.to_owned(),
                    resolved_by,
                },
            )
            .await?;
        Ok(())
    }

    /// Responds to an elicitation (structured input) (`AlreadyResolved` on the second and later attempts).
    pub async fn respond_elicitation(
        &self,
        command_id: &CommandId,
        session_id: &str,
        elicitation_id: &str,
        action: ElicitationAction,
        content: Option<ElicitationContent>,
        resolved_by: ResolvedBy,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_string()));
        }

        let (handle, summary) = {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions.active.get_mut(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
            let Some(summary) = session.pending_elicitations.remove(elicitation_id) else {
                return Err(NodeError::AlreadyResolved(elicitation_id.to_owned()));
            };
            (Arc::clone(&session.handle), summary)
        };
        if let Err(err) = handle
            .respond_elicitation(elicitation_id.to_owned(), action, content.clone())
            .await
        {
            // If the driver rejects it (e.g. schema violation in content), restore the
            // pending entry so the user can fix and resend it
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            if let Some(session) = sessions.active.get_mut(session_id) {
                session
                    .pending_elicitations
                    .insert(elicitation_id.to_owned(), summary);
            }
            return Err(NodeError::Server(format!("failed to respond: {err:#}")));
        }

        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::ElicitationResolved {
                    elicitation_id: elicitation_id.to_owned(),
                    action,
                    content,
                    resolved_by,
                },
            )
            .await?;
        Ok(())
    }
}
