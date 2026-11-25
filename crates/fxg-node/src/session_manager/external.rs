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

use futures_util::stream::{self, StreamExt};
use fxg_agent::{AgentAuthRequired, ExternalSessionsUnsupported};
use fxg_db::queries::AgentSessionBinding;
use fxg_protocol::client_api::{
    ExternalSessionAgentInfo, ExternalSessionListResponse, ExternalSessionSummary,
};
use fxg_protocol::common::{ErrorCode, ErrorInfo};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{Inner, NodeError, SessionManager};

/// Upper bound for listing one agent (process start + initialize + list).
const AGENT_TIMEOUT: Duration = Duration::from_secs(20);
/// Upper bound for a whole discovery request across every agent.
const TOTAL_BUDGET: Duration = Duration::from_secs(100);
/// Cached discovery result stays valid for this long.
const CACHE_TTL: Duration = Duration::from_secs(60);
/// Number of agents inspected concurrently.
const DISCOVERY_CONCURRENCY: usize = 3;
/// Clock difference tolerated before an agent timestamp is treated as newer.
///
/// Local prompts also bump the agent's timestamp, while fxg records the event
/// with its own clock; this margin avoids flagging those as external changes.
const SYNC_CLOCK_TOLERANCE_MS: i64 = 60_000;

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

    /// Drops the cached discovery result after a binding change.
    pub(super) fn invalidate_external(&self) {
        let mut cache = self
            .external_cache
            .lock()
            .expect("external session cache poisoned");
        *cache = None;
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
        // spawn duplicate processes for the same selection. Discovery itself
        // inspects multiple agents concurrently inside this lock.
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
        let process_cwd = &process_cwd;
        let imported = &imported;
        let deadline = Instant::now() + TOTAL_BUDGET;

        let mut seen_agents = BTreeSet::new();
        let entries: Vec<_> = self
            .inner
            .catalog
            .list()
            .into_iter()
            .filter(|entry| wanted.is_empty() || wanted.contains(&entry.id))
            .filter(|entry| entry.is_available())
            .filter(|entry| seen_agents.insert(entry.id.clone()))
            .collect();
        let considered = seen_agents.clone();

        let outcomes = stream::iter(entries)
            .map(|entry| async move {
                if Instant::now() >= deadline {
                    return AgentOutcome::error(
                        &entry.id,
                        &entry.name,
                        ErrorInfo::new(
                            ErrorCode::Timeout,
                            "external session discovery ran out of time",
                        ),
                        false,
                    );
                }
                self.discover_agent(&entry.id, &entry.name, process_cwd, imported)
                    .await
            })
            .buffer_unordered(DISCOVERY_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;

        let mut sessions = Vec::new();
        let mut agents = Vec::with_capacity(outcomes.len());
        for outcome in outcomes {
            sessions.extend(outcome.sessions);
            agents.push(outcome.info);
        }

        // Report explicitly requested agents that are not available on this node.
        for missing in wanted.iter().filter(|id| !considered.contains(*id)) {
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

        Ok(ExternalSessionListResponse {
            sessions,
            agents,
            scanned_at: fxg_protocol::util::now_ms(),
        })
    }

    /// Lists one agent and maps its sessions into the client shape.
    async fn discover_agent(
        &self,
        agent_id: &str,
        name: &str,
        process_cwd: &Path,
        imported: &HashMap<(String, String), AgentSessionBinding>,
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
            .map(|session| {
                let binding = imported.get(&(agent_id.to_owned(), session.session_id.clone()));
                ExternalSessionSummary {
                    agent_id: agent_id.to_owned(),
                    imported_session_id: binding.map(|binding| binding.session_id.clone()),
                    imported_title: binding.map(|binding| binding.title.clone()),
                    imported_updated_at: binding.map(|binding| binding.updated_at),
                    imported_status: binding.map(|binding| binding.status),
                    needs_sync: binding.is_some_and(|binding| {
                        agent_is_newer(session.updated_at.as_deref(), binding.updated_at)
                    }),
                    agent_session_id: session.session_id,
                    cwd: session.cwd.to_string_lossy().into_owned(),
                    title: session.title,
                    updated_at: session.updated_at,
                }
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

    /// Maps every live, non-archived fxg session to its agent-side session ID.
    async fn imported_sessions(
        &self,
    ) -> Result<HashMap<(String, String), AgentSessionBinding>, NodeError> {
        let bindings = self.inner.bus.db().list_agent_session_bindings().await?;
        Ok(bindings
            .into_iter()
            .map(|binding| {
                (
                    (binding.agent_id.clone(), binding.agent_session_id.clone()),
                    binding,
                )
            })
            .collect())
    }
}

/// Whether the agent reports activity newer than the fxg session's last event.
fn agent_is_newer(agent_updated_at: Option<&str>, fxg_updated_at: i64) -> bool {
    let Some(agent_updated_at) = agent_updated_at else {
        return false;
    };
    let Ok(parsed) = OffsetDateTime::parse(agent_updated_at, &Rfc3339) else {
        return false;
    };
    let Ok(agent_ms) = i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000) else {
        return false;
    };
    agent_ms > fxg_updated_at.saturating_add(SYNC_CLOCK_TOLERANCE_MS)
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
