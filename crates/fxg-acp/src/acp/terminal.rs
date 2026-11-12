use super::*;

// ----------------------------------------------------------------------
// Handler helpers (fs / terminal)
// ----------------------------------------------------------------------

/// Resolves a path relative to the session working directory.
pub(super) fn resolve_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Lightly validates the `accept` content against `requested_schema`.
///
/// Only the restricted schema types allowed by ACP elicitation (object / string
/// (enum, oneOf) / integer / number / boolean / string array) are validated (the
/// client SHOULD do this per the spec; the agent re-validates too). Unknown types
/// (future extensions) pass through.
pub(super) fn validate_elicitation_content(
    schema: &ElicitationSchema,
    content: &ElicitationContent,
) -> Result<(), String> {
    // Do not validate shape for schemas without properties.
    if schema.properties.is_empty() {
        return Ok(());
    }
    if let Some(required) = &schema.required {
        for key in required {
            if !content.contains_key(key) {
                return Err(format!("missing required property: {key}"));
            }
        }
    }
    for (key, value) in content {
        let Some(property) = schema.properties.get(key) else {
            return Err(format!("unexpected property: {key}"));
        };
        validate_elicitation_property(key, property, value)?;
    }
    Ok(())
}

/// Type validation for a single property in the elicitation restricted schema.
pub(super) fn validate_elicitation_property(
    key: &str,
    property: &fxg_protocol::common::ElicitationProperty,
    value: &ElicitationValue,
) -> Result<(), String> {
    match (&property.kind, value) {
        (ElicitationPropertyKind::String, ElicitationValue::String(text)) => {
            if !property.r#enum.is_empty() && !property.r#enum.iter().any(|value| value == text) {
                return Err(format!("property {key} is not one of the allowed values"));
            }
            if !property.one_of.is_empty()
                && !property
                    .one_of
                    .iter()
                    .any(|option| option.const_value == *text)
            {
                return Err(format!("property {key} is not one of the allowed options"));
            }
        }
        (ElicitationPropertyKind::Integer, ElicitationValue::Integer(_)) => {}
        (
            ElicitationPropertyKind::Number,
            ElicitationValue::Integer(_) | ElicitationValue::Number(_),
        ) => {}
        (ElicitationPropertyKind::Boolean, ElicitationValue::Boolean(_)) => {}
        (ElicitationPropertyKind::Array, ElicitationValue::StringArray(items)) => {
            if let Some(items_schema) = &property.items
                && !items_schema.r#enum.is_empty()
                && !items
                    .iter()
                    .all(|item| items_schema.r#enum.iter().any(|value| value == item))
            {
                return Err(format!("property {key} has a disallowed array value"));
            }
        }
        // Unknown restricted schema types (future extensions) are not validated
        (ElicitationPropertyKind::Unknown(_), _) => {}
        (kind, _) => {
            let expected = match kind {
                ElicitationPropertyKind::Integer => "an integer".to_owned(),
                other => format!("a {}", other.as_str()),
            };
            return Err(format!("property {key} must be {expected}"));
        }
    }
    Ok(())
}

/// Converts a typed answer value into the ACP wire value.
pub(super) fn elicitation_content_value(
    value: ElicitationValue,
) -> anyhow::Result<ElicitationContentValue> {
    Ok(match value {
        ElicitationValue::String(value) => ElicitationContentValue::String(value),
        ElicitationValue::Integer(value) => ElicitationContentValue::Integer(value),
        ElicitationValue::Number(value) => ElicitationContentValue::Number(value),
        ElicitationValue::Boolean(value) => ElicitationContentValue::Boolean(value),
        ElicitationValue::StringArray(value) => ElicitationContentValue::StringArray(value),
        ElicitationValue::Unknown(_) => {
            anyhow::bail!("unsupported elicitation content value")
        }
    })
}

// ----------------------------------------------------------------------
// terminal/* (fxg-pty)
// ----------------------------------------------------------------------

/// Execution state of an ACP terminal.
pub(super) struct TerminalState {
    pub(super) pty_id: String,
    command: String,
    output: Arc<Mutex<Vec<u8>>>,
    exit_rx: watch::Receiver<Option<i32>>,
}

/// ACP terminal registry.
#[derive(Default)]
pub(super) struct TerminalRegistry {
    terminals: Mutex<HashMap<String, TerminalState>>,
}

/// Return value of `TerminalRegistry::get` (pty_id, command, output, exit_rx).
pub(super) type TerminalSnapshot = (
    String,
    String,
    Arc<Mutex<Vec<u8>>>,
    watch::Receiver<Option<i32>>,
);

impl TerminalRegistry {
    pub(super) fn get(&self, terminal_id: &str) -> Result<TerminalSnapshot, String> {
        let terminals = self.terminals.lock().expect("terminal registry poisoned");
        let state = terminals
            .get(terminal_id)
            .ok_or_else(|| format!("unknown terminal: {terminal_id}"))?;
        Ok((
            state.pty_id.clone(),
            state.command.clone(),
            Arc::clone(&state.output),
            state.exit_rx.clone(),
        ))
    }

    pub(super) fn remove(&self, terminal_id: &str) -> Option<TerminalState> {
        self.terminals
            .lock()
            .expect("terminal registry poisoned")
            .remove(terminal_id)
    }
}

/// `terminal/create`: spawns an ACP terminal via fxg-pty.
pub(super) async fn create_terminal(
    pty: Arc<fxg_pty::PtySessionManager>,
    terminals: Arc<TerminalRegistry>,
    events: mpsc::UnboundedSender<DriverEvent>,
    subagent_id: Option<String>,
    request: CreateTerminalRequest,
    responder: agent_client_protocol::v1::Responder<CreateTerminalResponse>,
) -> Result<(), agent_client_protocol::Error> {
    let terminal_id = uuid_v7();
    let mut spawn = fxg_pty::PtySpawnRequest::new(terminal_id.clone());
    spawn.shell_cmd = Some(request.command.clone());
    spawn.args = request.args.clone();
    spawn.cwd = request.cwd.clone();
    spawn.env = request
        .env
        .iter()
        .map(|variable| (variable.name.clone(), variable.value.clone()))
        .collect();
    let command_line = format!("{} {}", request.command, request.args.join(" "));

    match pty.spawn(spawn) {
        Ok(_) => {
            let Ok(mut receiver) = pty.subscribe(&terminal_id) else {
                return responder.respond_with_internal_error("failed to subscribe terminal");
            };
            let output: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
            let (exit_tx, exit_rx) = watch::channel(None);
            let limit = request.output_byte_limit.map(|limit| limit as usize);
            // PTY output collection task (delta delivery + completion event on exit)
            let output_for_task = Arc::clone(&output);
            let events_for_task = events.clone();
            let command_for_task = command_line.clone();
            let terminal_id_for_task = terminal_id.clone();
            let subagent_id_for_task = subagent_id.clone();
            fxg_protocol::spawn_guarded!("terminal output collector", async move {
                loop {
                    match receiver.recv().await {
                        Ok(fxg_pty::PtyEvent::Output { data, .. }) => {
                            {
                                let mut buffer =
                                    output_for_task.lock().expect("terminal output poisoned");
                                buffer.extend_from_slice(&data);
                                if let Some(limit) = limit
                                    && buffer.len() > limit
                                {
                                    let excess = buffer.len() - limit;
                                    buffer.drain(..excess);
                                }
                            }
                            fxg_protocol::discard_with!(
                                events_for_task.send(DriverEvent::Delta(
                                    StreamDeltaPayload::TerminalOutputDelta {
                                        terminal_id: terminal_id_for_task.clone(),
                                        data_b64: base64_encode(&data),
                                    },
                                )),
                                "notify event subscribers"
                            );
                        }
                        Ok(fxg_pty::PtyEvent::Exit { exit_code, .. }) => {
                            let data = std::mem::take(
                                &mut *output_for_task.lock().expect("terminal output poisoned"),
                            );
                            fxg_protocol::discard_with!(
                                events_for_task.send(DriverEvent::Event(
                                    UnifiedEventPayload::TerminalOutput {
                                        terminal_id: terminal_id_for_task.clone(),
                                        command: command_for_task.clone(),
                                        data_b64: base64_encode(&data),
                                        exit_code,
                                        subagent_id: subagent_id_for_task.clone(),
                                    },
                                )),
                                "notify event subscribers"
                            );
                            fxg_protocol::discard_with!(
                                exit_tx.send(exit_code),
                                "discard a non-critical result"
                            );
                            break;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            terminals
                .terminals
                .lock()
                .expect("terminal registry poisoned")
                .insert(
                    terminal_id.clone(),
                    TerminalState {
                        pty_id: terminal_id.clone(),
                        command: command_line,
                        output,
                        exit_rx,
                    },
                );
            responder.respond(CreateTerminalResponse::new(TerminalId::new(terminal_id)))
        }
        Err(err) => {
            responder.respond_with_internal_error(format!("failed to spawn terminal: {err}"))
        }
    }
}

/// Builds an ACP `TerminalExitStatus` from an exit code.
pub(super) fn exit_status_of(code: Option<i32>) -> TerminalExitStatus {
    let mut status = TerminalExitStatus::new();
    if let Some(code) = code
        && let Ok(code) = u32::try_from(code)
    {
        status = status.exit_code(code);
    }
    status
}

/// Base64 encoding (no line breaks).
pub(super) fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}
