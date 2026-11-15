//! Shared per-session runtime state for the native OpenCode driver.

use super::*;

pub(super) struct SessionRuntime {
    pub(super) fxg_session_id: String,
    pub(super) agent_session_id: String,
    /// Directory that scopes OpenCode's agent / model / command catalogs.
    pub(super) cwd: PathBuf,
    pub(super) event_tx: mpsc::UnboundedSender<DriverEvent>,
    pub(super) state: std::sync::Mutex<RuntimeState>,
}

#[derive(Default)]
pub(super) struct RuntimeState {
    pub(super) busy: bool,
    pub(super) commands: Vec<String>,
    pub(super) tool_names: HashMap<String, String>,
    pub(super) tool_titles: HashMap<String, String>,
    pub(super) tool_inputs: HashMap<String, Value>,
    pub(super) self_messages: HashSet<String>,
    pub(super) pending_commands: usize,
    pub(super) seen_event_ids: SeenEventIds,
    pub(super) durable_seq: HashMap<String, u64>,
    pub(super) context_size: u64,
    /// Last completed step's finish reason (`session.step.ended`).
    pub(super) finish: Option<String>,
    /// Last failed step / execution error type (`provider.auth`, ...).
    pub(super) step_error: Option<String>,
    /// Last capabilities payload sent to the session, used to suppress
    /// redundant refreshes when commands / models did not change.
    pub(super) last_capabilities: Option<UnifiedEventPayload>,
    /// Context usage by scope (`None` = the root session).
    pub(super) usage: HashMap<Option<String>, UsageState>,
}

#[derive(Default)]
pub(super) struct UsageState {
    pub(super) used_tokens: u64,
    pub(super) cost: Option<f64>,
}

#[derive(Default)]
pub(super) struct SeenEventIds {
    pub(super) set: HashSet<String>,
    pub(super) order: VecDeque<String>,
}

impl SeenEventIds {
    /// Records `id` and returns `false` if it was already seen recently.
    pub(super) fn insert(&mut self, id: &str) -> bool {
        if self.set.contains(id) {
            return false;
        }
        let owned = id.to_owned();
        if self.order.len() >= MAX_SEEN_EVENT_IDS
            && let Some(oldest) = self.order.pop_front()
        {
            self.set.remove(&oldest);
        }
        self.order.push_back(owned.clone());
        self.set.insert(owned);
        true
    }
}

pub(super) const NATIVE_SESSION_FEATURES: [NativeSessionFeature; 3] = [
    NativeSessionFeature::Revert,
    NativeSessionFeature::Steer,
    NativeSessionFeature::Compact,
];

impl SessionRuntime {
    pub(super) fn emit(&self, events: Vec<DriverEvent>) {
        for event in events {
            drop(self.event_tx.send(event));
        }
    }

    pub(super) fn emit_event(&self, payload: UnifiedEventPayload) {
        self.emit(vec![DriverEvent::Event(payload)]);
    }

    pub(super) fn is_busy(&self) -> bool {
        self.state.lock().expect("state poisoned").busy
    }

    pub(super) fn set_busy(&self, busy: bool) {
        self.state.lock().expect("state poisoned").busy = busy;
    }

    pub(super) async fn refresh_capabilities(&self, client: &OpenCodeClient) {
        match fetch_capabilities(client, &self.cwd, Some(&self.agent_session_id)).await {
            Ok((payload, commands, context_size)) => {
                let changed = {
                    let mut state = self.state.lock().expect("state poisoned");
                    state.commands = commands;
                    state.context_size = context_size;
                    if state.last_capabilities.as_ref() == Some(&payload) {
                        false
                    } else {
                        state.last_capabilities = Some(payload.clone());
                        true
                    }
                };
                if changed {
                    self.emit_event(payload);
                }
            }
            Err(err) => warn!(
                session_id = %self.fxg_session_id,
                "failed to refresh OpenCode capabilities: {err:#}"
            ),
        }
    }

    /// Records the capabilities sent at session start so the first idle
    /// refresh does not emit a duplicate.
    pub(super) fn set_capabilities(
        &self,
        payload: UnifiedEventPayload,
        commands: Vec<String>,
        context_size: u64,
    ) {
        {
            let mut state = self.state.lock().expect("state poisoned");
            state.commands = commands;
            state.context_size = context_size;
            state.last_capabilities = Some(payload.clone());
        }
        self.emit_event(payload);
    }
}

impl RuntimeState {
    /// Takes the reason the turn was cut off, clearing the per-turn state.
    pub(super) fn take_turn_stop_reason(&mut self) -> Option<TurnStopReason> {
        let finish = self.finish.take();
        let error = self.step_error.take();
        turn_stop_reason(finish.as_deref(), error.as_deref())
    }
}

/// Maps OpenCode's finish reason / error type to the normalized stop reason.
///
/// `end_turn` and cancellation are not surfaced (matching ACP). Content
/// filtering is a refusal, not an error.
pub(super) fn turn_stop_reason(
    finish: Option<&str>,
    error: Option<&str>,
) -> Option<TurnStopReason> {
    if error == Some("provider.content-filter") || finish == Some("content-filter") {
        return Some(TurnStopReason::Refusal);
    }
    match finish {
        Some("length") => Some(TurnStopReason::MaxTokens),
        Some("max-turns" | "max_turn_requests") => Some(TurnStopReason::MaxTurnRequests),
        _ => None,
    }
}

/// Returns a user-facing error message, with a login hint for provider auth.
pub(super) fn error_message(data: &Value, fallback: &str) -> String {
    match data.pointer("/error/type").and_then(Value::as_str) {
        Some("provider.auth") => {
            "OpenCode requires authentication; run `opencode auth login` or `fxg agents login opencode`"
                .to_owned()
        }
        _ => data
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .to_owned(),
    }
}
