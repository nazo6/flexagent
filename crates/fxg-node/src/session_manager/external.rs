//! Discovery of sessions that already exist on the agent side.
//!
//! ACP exposes `session/list` (stable v1). fxg does not own these sessions yet,
//! so discovery is read-only: it starts each agent briefly, lists its sessions,
//! and marks the ones a live fxg session already owns. Importing is a separate,
//! explicit operation (`POST /api/v1/sessions/import`).
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §4.5.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fxg_acp::{AgentAuthRequired, ExternalSessionsUnsupported};
use fxg_db::SessionFilter;
use fxg_protocol::SessionId;
use fxg_protocol::client_api::{
    ExternalSessionAgentInfo, ExternalSessionListResponse, ExternalSessionSummary,
};
use fxg_protocol::common::{ErrorCode, ErrorInfo};

use super::{Inner, NodeError, SessionManager};

/// Upper bound for listing one agent (process start + initialize + list).
const AGENT_TIMEOUT: Duration = Duration::from_secs(20);
/// Upper bound for a whole discovery request across every agent.
const TOTAL_BUDGET: Duration = Duration::from_secs(100);
/// Cached discovery result stays valid for this long.
const CACHE_TTL: Duration = Duration::from_secs(60);

impl Inner {
    /// Returns the cached discovery result when it is still fresh.
    fn cached_external(&self) -> Option<ExternalSessionListResponse> {
        let cache = self
            .external_cache
            .lock()
            .expect("external session cache poisoned");
        cache
            .as_ref()
            .filter(|entry| entry.cached_at.elapsed() < CACHE_TTL)
            .map(|entry| entry.response.clone())
    }

    /// Replaces the cached discovery result.
    fn store_external(&self, response: ExternalSessionListResponse) {
        let mut cache = self
            .external_cache
            .lock()
            .expect("external session cache poisoned");
        *cache = Some(super::ExternalCacheEntry {
            response,
            cached_at: Instant::now(),
        });
    }
}

impl SessionManager {
    /// Lists sessions that already exist on the agent side.
    ///
    /// `agents` restricts the result to those IDs (empty = every available
    /// agent). `force` bypasses the short-lived cache.
    pub async fn list_external_sessions(
        &self,
        agents: &[String],
        force: bool,
    ) -> Result<ExternalSessionListResponse, NodeError> {
        let wanted = normalize_filter(agents);

        // Reuse the probe lock: both operations cold-start an agent and must not
        // spawn duplicate processes for the same selection.
        let _guard = self.inner.probe_lock.lock().await;

        // Only the unfiltered response is cached, so a per-agent request never
        // hides agents from a later unfiltered request.
        let cached = if force || !wanted.is_empty() {
            None
        } else {
            self.inner.cached_external()
        };
        let response = match cached {
            Some(response) => response,
            None => {
                let response = self.discover_external(&wanted).await?;
                if wanted.is_empty() {
                    self.inner.store_external(response.clone());
                }
                response
            }
        };

        Ok(response)
    }

    /// Runs discovery for the requested agents (empty = every available agent).
    async fn discover_external(
        &self,
        wanted: &BTreeSet<String>,
    ) -> Result<ExternalSessionListResponse, NodeError> {
        let imported = self.imported_sessions().await?;
        let process_cwd = discovery_cwd();
        let deadline = Instant::now() + TOTAL_BUDGET;

        let mut sessions = Vec::new();
        let mut agents = Vec::new();
        let mut seen_agents = BTreeSet::new();

        for entry in self.inner.catalog.list() {
            if !wanted.is_empty() && !wanted.contains(&entry.id) {
                continue;
            }
            if !entry.is_available() || !seen_agents.insert(entry.id.clone()) {
                continue;
            }
            if Instant::now() >= deadline {
                agents.push(agent_error(
                    &entry.id,
                    &entry.name,
                    ErrorInfo::new(
                        ErrorCode::Timeout,
                        "external session discovery ran out of time",
                    ),
                ));
                continue;
            }

            let outcome = self
                .discover_agent(&entry.id, &entry.name, &process_cwd, &imported)
                .await;
            sessions.extend(outcome.sessions);
            agents.push(outcome.info);
        }

        // Report explicitly requested agents that are not available on this node.
        for missing in wanted.iter().filter(|id| !seen_agents.contains(*id)) {
            agents.push(agent_error(
                missing,
                missing,
                ErrorInfo::new(
                    ErrorCode::AgentNotInstalled,
                    format!("agent {missing} is not available on this node"),
                ),
            ));
        }

        // ACP reports ISO 8601 timestamps, so lexicographic order is chronological.
        sessions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.agent_id.cmp(&b.agent_id))
                .then_with(|| a.cwd.cmp(&b.cwd))
        });
        agents.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));

        Ok(ExternalSessionListResponse { sessions, agents })
    }

    /// Lists one agent and maps its sessions into the client shape.
    async fn discover_agent(
        &self,
        agent_id: &str,
        name: &str,
        process_cwd: &Path,
        imported: &HashMap<(String, String), SessionId>,
    ) -> AgentOutcome {
        let spec = match self.resolve_launch_spec(agent_id, process_cwd, &[]).await {
            Ok(spec) => spec,
            Err(err) => return AgentOutcome::error(agent_id, name, err.info(), false),
        };
        let driver = match (self.inner.factory)(&spec) {
            Ok(driver) => driver,
            Err(err) => {
                return AgentOutcome::error(
                    agent_id,
                    name,
                    NodeError::Agent(format!("failed to select driver: {err}")).info(),
                    false,
                );
            }
        };

        let listed =
            match tokio::time::timeout(AGENT_TIMEOUT, driver.list_sessions(&spec, process_cwd))
                .await
            {
                Ok(listed) => listed,
                Err(_) => {
                    return AgentOutcome::error(
                        agent_id,
                        name,
                        ErrorInfo::new(
                            ErrorCode::Timeout,
                            format!(
                                "agent {agent_id} did not answer session/list within {} seconds",
                                AGENT_TIMEOUT.as_secs()
                            ),
                        ),
                        false,
                    );
                }
            };

        let list = match listed {
            Ok(list) => list,
            Err(err) => {
                if err.downcast_ref::<ExternalSessionsUnsupported>().is_some() {
                    return AgentOutcome {
                        sessions: Vec::new(),
                        info: ExternalSessionAgentInfo {
                            agent_id: agent_id.to_owned(),
                            name: name.to_owned(),
                            supports_list: false,
                            supports_history: false,
                            error: None,
                        },
                    };
                }
                if let Some(required) = err.downcast_ref::<AgentAuthRequired>() {
                    let error = NodeError::AuthRequired {
                        agent_id: agent_id.to_owned(),
                        methods: required.0.clone(),
                    }
                    .info();
                    return AgentOutcome::error(agent_id, name, error, true);
                }
                return AgentOutcome::error(
                    agent_id,
                    name,
                    ErrorInfo::new(
                        ErrorCode::AgentProtocolError,
                        format!("failed to list agent sessions: {err:#}"),
                    ),
                    true,
                );
            }
        };

        let sessions = list
            .sessions
            .into_iter()
            .map(|session| ExternalSessionSummary {
                agent_id: agent_id.to_owned(),
                imported_session_id: imported
                    .get(&(agent_id.to_owned(), session.session_id.clone()))
                    .cloned(),
                agent_session_id: session.session_id,
                cwd: session.cwd.to_string_lossy().into_owned(),
                title: session.title,
                updated_at: session.updated_at,
            })
            .collect();

        AgentOutcome {
            sessions,
            info: ExternalSessionAgentInfo {
                agent_id: agent_id.to_owned(),
                name: name.to_owned(),
                supports_list: true,
                supports_history: list.supports_history,
                error: None,
            },
        }
    }

    /// Maps every live fxg session to its agent-side session ID.
    async fn imported_sessions(&self) -> Result<HashMap<(String, String), SessionId>, NodeError> {
        let filter = SessionFilter {
            include_archived: true,
            ..SessionFilter::default()
        };
        let sessions = self.inner.bus.db().list_sessions(&filter).await?;
        Ok(sessions
            .into_iter()
            .filter_map(|session| {
                let agent_session_id = session.agent_session_id?;
                Some(((session.agent_id, agent_session_id), session.session_id))
            })
            .collect())
    }
}

/// Discovery result for one agent.
struct AgentOutcome {
    /// Sessions reported by the agent
    sessions: Vec<ExternalSessionSummary>,
    /// Per-agent capability / error state
    info: ExternalSessionAgentInfo,
}

impl AgentOutcome {
    fn error(agent_id: &str, name: &str, error: ErrorInfo, supports_list: bool) -> Self {
        Self {
            sessions: Vec::new(),
            info: ExternalSessionAgentInfo {
                agent_id: agent_id.to_owned(),
                name: name.to_owned(),
                supports_list,
                supports_history: false,
                error: Some(error),
            },
        }
    }
}

fn agent_error(agent_id: &str, name: &str, error: ErrorInfo) -> ExternalSessionAgentInfo {
    ExternalSessionAgentInfo {
        agent_id: agent_id.to_owned(),
        name: name.to_owned(),
        supports_list: false,
        supports_history: false,
        error: Some(error),
    }
}

/// Directory used to launch agents for discovery (sessions are not cwd-filtered).
fn discovery_cwd() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| match std::env::current_dir() {
        Ok(path) => path,
        Err(_) => PathBuf::from("."),
    })
}

/// Deduplicates and normalizes the requested agent IDs.
fn normalize_filter(agents: &[String]) -> BTreeSet<String> {
    agents
        .iter()
        .map(|agent| agent.trim())
        .filter(|agent| !agent.is_empty())
        .map(str::to_owned)
        .collect()
}
