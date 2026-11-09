//! Agent capability probing (models / modes) before a session starts.
//!
//! ACP only exposes selectable models in the `session/new` response. To show
//! the model list on the new-session screen we start a short-lived agent
//! session, collect `CapabilitiesUpdated`, close it again, and cache the
//! result. No FXG session row or event is created.

use std::path::Path;
use std::time::Duration;

use fxg_acp::{AgentAuthRequired, DriverEvent, StartSessionRequest};
use fxg_protocol::client_api::AgentCapabilitiesPreview;
use fxg_protocol::common::SessionStatus;
use fxg_protocol::events::UnifiedEventPayload;
use fxg_protocol::util::uuid_v7;
use tokio::sync::mpsc;

use super::{Inner, NodeError, ProbeCacheEntry, ProbeKey, SessionManager};

/// Upper bound for starting the agent process and creating the ACP session.
///
/// Kept below the node RPC timeout (120s) so the hub receives the structured
/// timeout error instead of failing the relay first.
const PROBE_START_TIMEOUT: Duration = Duration::from_secs(100);
/// Time to wait for the initial capabilities events after `session/new`.
const PROBE_COLLECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Cached probe results stay valid for this long.
const PROBE_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// Maximum number of cached probe results.
const PROBE_CACHE_LIMIT: usize = 32;

impl Inner {
    /// Returns a cached probe result when it is still fresh.
    fn cached_probe(&self, key: &ProbeKey) -> Option<AgentCapabilitiesPreview> {
        let cache = self.probe_cache.lock().expect("probe cache poisoned");
        cache
            .get(key)
            .filter(|entry| entry.cached_at.elapsed() < PROBE_CACHE_TTL)
            .map(|entry| entry.preview.clone())
    }

    /// Stores a probe result, evicting expired / oldest entries.
    pub(super) fn store_probe(&self, key: ProbeKey, preview: AgentCapabilitiesPreview) {
        let mut cache = self.probe_cache.lock().expect("probe cache poisoned");
        cache.retain(|_, entry| entry.cached_at.elapsed() < PROBE_CACHE_TTL);
        if cache.len() >= PROBE_CACHE_LIMIT
            && let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.cached_at)
                .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
        cache.insert(
            key,
            ProbeCacheEntry {
                preview,
                cached_at: std::time::Instant::now(),
            },
        );
    }

    /// Updates the probe cache from a running session's capabilities.
    pub(super) fn remember_capabilities(
        &self,
        agent_id: &str,
        cwd: &Path,
        preview: AgentCapabilitiesPreview,
    ) {
        self.store_probe(
            ProbeKey {
                agent_id: agent_id.to_owned(),
                local_path: cwd.to_path_buf(),
            },
            preview,
        );
    }
}

impl SessionManager {
    /// Probes an agent by starting and immediately closing a temporary ACP session.
    ///
    /// The returned modes/config options match what a real session would receive
    /// from `session/new`. Results are cached per `(agent_id, local_path)`.
    pub async fn probe_agent_capabilities(
        &self,
        agent_id: &str,
        local_path: &Path,
        force: bool,
    ) -> Result<AgentCapabilitiesPreview, NodeError> {
        if agent_id.trim().is_empty() {
            return Err(NodeError::InvalidState("agent ID is empty".to_owned()));
        }
        let cwd = dunce::canonicalize(local_path)
            .map_err(|source| NodeError::io(local_path.to_path_buf(), source))?;
        if !cwd.is_dir() {
            return Err(NodeError::InvalidState(format!(
                "working directory is not a directory: {}",
                cwd.display()
            )));
        }

        let key = ProbeKey {
            agent_id: agent_id.to_owned(),
            local_path: cwd.clone(),
        };
        if !force && let Some(preview) = self.inner.cached_probe(&key) {
            return Ok(preview);
        }

        // Serialize probes: the UI requests one agent at a time, and this avoids
        // spawning duplicate agent processes for the same selection.
        let _guard = self.inner.probe_lock.lock().await;
        if !force && let Some(preview) = self.inner.cached_probe(&key) {
            return Ok(preview);
        }

        let spec = self.resolve_launch_spec(agent_id, &[]).await?;
        let driver = (self.inner.factory)(&spec)
            .map_err(|err| NodeError::Agent(format!("failed to select driver: {err}")))?;
        self.remember_driver(&driver);

        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let request = StartSessionRequest {
            session_id: format!("probe-{}", uuid_v7()),
            title: Some("fxg capability probe".to_owned()),
            cwd: cwd.clone(),
            launch: spec.clone(),
            extra_args: Vec::new(),
            initial_mode: None,
            resume: None,
            fork_from: None,
            initial_config: Vec::new(),
        };

        let started = match tokio::time::timeout(
            PROBE_START_TIMEOUT,
            driver.start_session(request, event_tx),
        )
        .await
        {
            Ok(Ok(started)) => started,
            Ok(Err(err)) => return Err(start_error(err, &spec.agent_id)),
            Err(_) => {
                return Err(NodeError::Timeout(format!(
                    "agent {} did not start within {} seconds",
                    spec.agent_id,
                    PROBE_START_TIMEOUT.as_secs()
                )));
            }
        };

        let preview = collect_capabilities(&mut event_rx).await;
        if let Err(err) = started.handle.shutdown().await {
            tracing::debug!(
                agent_id = %spec.agent_id,
                "probe session shutdown failed: {err:#}"
            );
        }
        let preview = preview?;
        self.inner.store_probe(key, preview.clone());
        Ok(preview)
    }
}

/// Converts a driver start failure into a structured node error.
fn start_error(err: anyhow::Error, agent_id: &str) -> NodeError {
    if let Some(required) = err.downcast_ref::<AgentAuthRequired>() {
        return NodeError::AuthRequired {
            agent_id: agent_id.to_owned(),
            methods: required.0.clone(),
        };
    }
    NodeError::Agent(format!("failed to start agent for probe: {err:#}"))
}

/// Collects the initial capabilities until the session reports idle.
async fn collect_capabilities(
    events: &mut mpsc::UnboundedReceiver<DriverEvent>,
) -> Result<AgentCapabilitiesPreview, NodeError> {
    let mut preview = AgentCapabilitiesPreview::default();
    let deadline = tokio::time::Instant::now() + PROBE_COLLECT_TIMEOUT;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            // The start call already waited for `session/new`; returning what we
            // have is better than failing a probe that produced model choices.
            Err(_) | Ok(None) => return Ok(preview),
            Ok(Some(event)) => match event {
                DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
                    current_mode,
                    available_modes,
                    config_options,
                    ..
                }) => {
                    preview.current_mode = current_mode;
                    preview.available_modes = available_modes;
                    preview.config_options = config_options;
                }
                DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Idle,
                    ..
                }) => return Ok(preview),
                DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Error,
                    error_message,
                }) => {
                    return Err(NodeError::Agent(error_message.unwrap_or_else(|| {
                        "agent reported an error while probing capabilities".to_owned()
                    })));
                }
                DriverEvent::Failed { message } => {
                    return Err(NodeError::Agent(message));
                }
                _ => {}
            },
        }
    }
}
