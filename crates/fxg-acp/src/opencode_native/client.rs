#![allow(
    clippy::disallowed_types,
    reason = "OpenCode v2 HTTP payloads are parsed at this external boundary"
)]

use super::*;
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;

#[derive(Debug, thiserror::Error)]
pub(super) enum OpenCodeError {
    #[error("OpenCode {path} returned {status}: {body}")]
    Status {
        path: String,
        status: reqwest::StatusCode,
        body: String,
    },
}

fn is_http_status(err: &anyhow::Error, status: u16) -> bool {
    err.downcast_ref::<OpenCodeError>()
        .is_some_and(|error| error.status().as_u16() == status)
}

impl OpenCodeError {
    fn status(&self) -> reqwest::StatusCode {
        match self {
            Self::Status { status, .. } => *status,
        }
    }
}

#[derive(Clone)]
pub struct OpenCodeClient {
    pub(super) http: reqwest::Client,
    /// Long-lived stream client (no total request timeout).
    pub(super) sse_http: reqwest::Client,
    pub(super) base_url: Arc<str>,
    pub(super) password: Arc<str>,
}

impl std::fmt::Debug for OpenCodeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenCodeClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl OpenCodeClient {
    /// Creates a client for a running OpenCode server.
    #[must_use]
    pub fn new(base_url: impl Into<String>, password: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let sse_http = reqwest::Client::builder()
            .connect_timeout(SSE_CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            http,
            sse_http,
            base_url: Arc::from(base_url.into().trim_end_matches('/')),
            password: Arc::from(password.into()),
        }
    }

    pub(super) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    pub(super) fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.basic_auth(AUTH_USER, Some(self.password.as_ref()))
    }

    /// Sends a request and returns the raw JSON body.
    pub async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        let mut request = self.auth(self.http.request(method, self.url(path)));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("OpenCode request failed: {path}"))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .with_context(|| format!("OpenCode response could not be read: {path}"))?;
        if !status.is_success() {
            return Err(OpenCodeError::Status {
                path: path.to_owned(),
                status,
                body: text.chars().take(500).collect(),
            }
            .into());
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .with_context(|| format!("OpenCode {path} returned invalid JSON"))
    }

    /// GET helper.
    pub async fn get(&self, path: &str) -> anyhow::Result<Value> {
        self.json(reqwest::Method::GET, path, None).await
    }

    /// POST helper.
    pub async fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        self.json(reqwest::Method::POST, path, Some(body)).await
    }

    /// DELETE helper.
    pub async fn delete(&self, path: &str) -> anyhow::Result<Value> {
        self.json(reqwest::Method::DELETE, path, None).await
    }

    /// Returns the OpenCode version from `/api/info`.
    pub async fn version(&self) -> anyhow::Result<String> {
        let value = self.get("/api/info").await?;
        Ok(value
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    pub(super) async fn session_exists(&self, session_id: &str) -> anyhow::Result<bool> {
        match self.get(&format!("/api/session/{session_id}")).await {
            Ok(_) => Ok(true),
            Err(err) if is_http_status(&err, 404) => Ok(false),
            Err(err) => Err(err),
        }
    }

    pub(super) async fn create_session(
        &self,
        cwd: &Path,
        title: Option<&str>,
        agent: Option<&str>,
        model: Option<&str>,
    ) -> anyhow::Result<String> {
        let mut body = json!({
            "location": { "directory": cwd.to_string_lossy() },
        });
        if let Some(title) = title.filter(|title| !title.trim().is_empty()) {
            body["title"] = Value::String(title.to_owned());
        }
        if let Some(agent) = agent.filter(|agent| !agent.trim().is_empty()) {
            body["agent"] = Value::String(agent.to_owned());
        }
        if let Some(model) = model.and_then(parse_model_ref) {
            body["model"] = model;
        }
        let value = self.post("/api/session", body).await?;
        value
            .pointer("/data/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("OpenCode session/create did not return an id"))
    }

    pub(super) async fn fork_session(
        &self,
        source: &str,
        before: Option<&str>,
    ) -> anyhow::Result<String> {
        let body = json!({ "before": before });
        let value = self
            .post(&format!("/api/session/{source}/fork"), body)
            .await?;
        value
            .pointer("/data/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("OpenCode session/fork did not return an id"))
    }

    pub(super) async fn switch_agent(&self, session_id: &str, agent: &str) -> anyhow::Result<()> {
        self.post(
            &format!("/api/session/{session_id}/agent"),
            json!({ "agent": agent }),
        )
        .await?;
        Ok(())
    }

    pub(super) async fn switch_model(&self, session_id: &str, model: Value) -> anyhow::Result<()> {
        self.post(
            &format!("/api/session/{session_id}/model"),
            json!({ "model": model }),
        )
        .await?;
        Ok(())
    }

    pub(super) async fn interrupt(&self, session_id: &str) -> anyhow::Result<()> {
        self.post(&format!("/api/session/{session_id}/interrupt"), json!({}))
            .await?;
        Ok(())
    }

    pub(super) async fn compact(
        &self,
        session_id: &str,
        delivery: Option<&str>,
    ) -> anyhow::Result<()> {
        let body = match delivery {
            Some(delivery) => json!({ "delivery": delivery }),
            None => json!({}),
        };
        self.post(&format!("/api/session/{session_id}/compact"), body)
            .await?;
        Ok(())
    }

    pub(super) async fn reply_permission(
        &self,
        session_id: &str,
        request_id: &str,
        decision: &str,
    ) -> anyhow::Result<()> {
        // OpenCode v2 names the permission decision `decision`, not `reply`
        // (`Permission.Reply = once | always | reject`).
        self.post(
            &format!("/api/session/{session_id}/permission/{request_id}/reply"),
            json!({ "decision": decision }),
        )
        .await?;
        Ok(())
    }

    pub(super) async fn reply_form(
        &self,
        session_id: &str,
        form_id: &str,
        answer: &ElicitationContent,
    ) -> anyhow::Result<()> {
        let answer = answer
            .iter()
            .map(|(key, value)| (key.clone(), elicitation_value_to_json(value)))
            .collect::<serde_json::Map<String, Value>>();
        self.post(
            &format!("/api/session/{session_id}/form/{form_id}/reply"),
            json!({ "answer": answer }),
        )
        .await?;
        Ok(())
    }

    pub(super) async fn cancel_form(&self, session_id: &str, form_id: &str) -> anyhow::Result<()> {
        self.delete(&format!("/api/session/{session_id}/form/{form_id}"))
            .await?;
        Ok(())
    }

    pub(super) async fn revert(
        &self,
        session_id: &str,
        message_id: &str,
        files: bool,
    ) -> anyhow::Result<()> {
        if let Err(err) = self
            .post(
                &format!("/api/session/{session_id}/revert/stage"),
                json!({ "messageID": message_id, "files": files }),
            )
            .await
        {
            if is_http_status(&err, 404) || is_http_status(&err, 409) {
                return Err(NativeRevertUnavailable(err.to_string()).into());
            }
            return Err(err);
        }
        if let Err(err) = self
            .post(
                &format!("/api/session/{session_id}/revert/commit"),
                json!({}),
            )
            .await
        {
            // Do not leave a staged revert behind when the commit fails.
            if let Err(clear) = self
                .delete(&format!("/api/session/{session_id}/revert"))
                .await
            {
                debug!("failed to clear a staged OpenCode revert: {clear:#}");
            }
            return Err(err);
        }
        Ok(())
    }

    pub(super) async fn send_prompt(
        &self,
        session_id: &str,
        input: &PromptInput,
        files: &[Value],
        delivery: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut body = json!({ "text": input.text });
        if let Some(id) = input.agent_message_id.as_deref() {
            body["id"] = Value::String(id.to_owned());
        }
        if let Some(delivery) = delivery {
            body["delivery"] = Value::String(delivery.to_owned());
        }
        if !files.is_empty() {
            body["files"] = Value::Array(files.to_vec());
        }
        self.post(&format!("/api/session/{session_id}/prompt"), body)
            .await?;
        Ok(())
    }

    pub(super) async fn send_command(
        &self,
        session_id: &str,
        name: &str,
        text: &str,
        files: &[Value],
        delivery: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut body = json!({ "name": name, "text": text });
        if let Some(delivery) = delivery {
            body["delivery"] = Value::String(delivery.to_owned());
        }
        if !files.is_empty() {
            body["files"] = Value::Array(files.to_vec());
        }
        self.post(&format!("/api/session/{session_id}/command"), body)
            .await?;
        Ok(())
    }

    /// Lists sessions known to OpenCode (newest first).
    ///
    /// Discovery is intentionally not directory-scoped: a user may continue any
    /// imported session, and each row carries its own working directory.
    pub async fn list_sessions(&self, cwd: &Path) -> anyhow::Result<Vec<AgentSessionInfo>> {
        let path = "/api/session?limit=100&order=desc&parentID=null&archived=false";
        let value = self.get(path).await?;
        let rows = value
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter(|row| row.get("parentID").is_none_or(Value::is_null))
            .filter_map(|row| {
                Some(AgentSessionInfo {
                    session_id: row.get("id")?.as_str()?.to_owned(),
                    cwd: row
                        .pointer("/location/directory")
                        .and_then(Value::as_str)
                        .map(PathBuf::from)
                        .unwrap_or_else(|| cwd.to_path_buf()),
                    title: row.get("title").and_then(Value::as_str).map(str::to_owned),
                    updated_at: row
                        .pointer("/time/updated")
                        .and_then(Value::as_i64)
                        .map(epoch_millis_to_rfc3339),
                })
            })
            .collect())
    }

    /// Reads the management snapshot (agents / models / providers / MCP / skills).
    pub async fn management_snapshot(
        &self,
        cwd: Option<&Path>,
    ) -> anyhow::Result<fxg_protocol::client_api::AgentManagementSnapshot> {
        use fxg_protocol::client_api::{
            AgentManagementAgentMode, AgentManagementAgentSummary, AgentManagementSnapshot,
            AgentMcpStatus, AgentMcpSummary, AgentModelSummary, AgentProviderActivation,
            AgentProviderSummary, AgentSkillSummary,
        };

        let location = cwd
            .map(|cwd| format!("?{}", location_query(cwd)))
            .unwrap_or_default();
        let agents = optional(self.get(&format!("/api/agent{location}")).await);
        let models = optional(self.get(&format!("/api/model{location}")).await);
        let providers = optional(self.get(&format!("/api/provider{location}")).await);
        let mcp = optional(self.get(&format!("/api/mcp{location}")).await);
        let skills = optional(self.get(&format!("/api/skill{location}")).await);
        let commands = optional(self.get(&format!("/api/command{location}")).await);

        let agents = agents
            .as_ref()
            .and_then(|value| value.get("data"))
            .and_then(Value::as_array)
            .map(|agents| {
                agents
                    .iter()
                    .filter_map(|agent| {
                        let id = agent.get("id")?.as_str()?.to_owned();
                        Some(AgentManagementAgentSummary {
                            name: agent
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(&id)
                                .to_owned(),
                            description: agent
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            mode: agent
                                .get("mode")
                                .and_then(Value::as_str)
                                .unwrap_or("primary")
                                .parse::<AgentManagementAgentMode>()
                                .expect("infallible"),
                            id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let models = models
            .as_ref()
            .and_then(|value| value.get("data"))
            .and_then(Value::as_array)
            .map(|models| {
                models
                    .iter()
                    .filter_map(|model| {
                        let id = model
                            .get("modelID")
                            .or_else(|| model.get("id"))
                            .and_then(Value::as_str)?
                            .to_owned();
                        Some(AgentModelSummary {
                            provider_id: model
                                .get("providerID")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            name: model
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(&id)
                                .to_owned(),
                            enabled: model.get("enabled").and_then(Value::as_bool) != Some(false),
                            variants: model
                                .get("variants")
                                .and_then(Value::as_array)
                                .map(|variants| {
                                    variants
                                        .iter()
                                        .filter_map(|variant| {
                                            variant.get("id")?.as_str().map(str::to_owned)
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                            id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let providers = providers
            .as_ref()
            .and_then(|value| value.get("data"))
            .and_then(Value::as_array)
            .map(|providers| {
                providers
                    .iter()
                    .filter_map(|provider| {
                        let id = provider.get("id")?.as_str()?.to_owned();
                        Some(AgentProviderSummary {
                            name: provider
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(&id)
                                .to_owned(),
                            activation: provider
                                .get("activation")
                                .and_then(Value::as_str)
                                .map(|s| s.parse::<AgentProviderActivation>().expect("infallible")),
                            package: provider
                                .get("package")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mcp_servers = mcp
            .as_ref()
            .and_then(|value| value.get("data"))
            .and_then(Value::as_array)
            .map(|servers| {
                servers
                    .iter()
                    .filter_map(|server| {
                        Some(AgentMcpSummary {
                            name: server.get("name")?.as_str()?.to_owned(),
                            status: server
                                .pointer("/status/status")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .parse::<AgentMcpStatus>()
                                .expect("infallible"),
                            error: server
                                .pointer("/status/error")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let skills = skills
            .as_ref()
            .and_then(|value| value.get("data"))
            .and_then(Value::as_array)
            .map(|skills| {
                skills
                    .iter()
                    .filter_map(|skill| {
                        let id = skill.get("id")?.as_str()?.to_owned();
                        Some(AgentSkillSummary {
                            name: skill
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(&id)
                                .to_owned(),
                            description: skill
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let commands = commands
            .as_ref()
            .and_then(|value| value.get("data"))
            .and_then(Value::as_array)
            .map(|commands| {
                commands
                    .iter()
                    .filter_map(|command| {
                        Some(CommandInfo {
                            name: command.get("name")?.as_str()?.to_owned(),
                            description: command
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            input_hint: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(AgentManagementSnapshot {
            agent_id: "opencode".to_owned(),
            agent_name: "OpenCode".to_owned(),
            available: true,
            version: Some(self.version().await?),
            capabilities: fxg_protocol::client_api::AgentManagementCapabilities {
                mcp_write: true,
                provider_write: false,
            },
            agents,
            models,
            providers,
            mcp_servers,
            skills,
            commands,
        })
    }

    /// Connects or disconnects an MCP server.
    pub async fn mcp_action(
        &self,
        name: &str,
        action: fxg_protocol::client_api::AgentMcpAction,
    ) -> anyhow::Result<()> {
        use fxg_protocol::client_api::AgentMcpAction;
        let suffix = match action {
            AgentMcpAction::Connect => "connect",
            AgentMcpAction::Disconnect => "disconnect",
        };
        self.post(
            &format!("/api/experimental/mcp/{}/{}", urlencoded(name), suffix),
            json!({}),
        )
        .await?;
        Ok(())
    }

    /// Adds or replaces an MCP server definition.
    pub async fn upsert_mcp(
        &self,
        name: &str,
        definition: &fxg_protocol::client_api::AgentMcpDefinition,
    ) -> anyhow::Result<()> {
        use fxg_protocol::client_api::AgentMcpKind;
        let config = match definition.kind {
            AgentMcpKind::Remote => json!({
                "type": "remote",
                "url": definition.url.clone().unwrap_or_default(),
                "disabled": !definition.enabled,
            }),
            AgentMcpKind::Local | AgentMcpKind::Unknown(_) => json!({
                "type": "local",
                "command": definition.command,
                "environment": definition.environment,
                "disabled": !definition.enabled,
            }),
        };
        self.json(
            reqwest::Method::PUT,
            &format!("/api/experimental/mcp/{}", urlencoded(name)),
            Some(json!({ "config": config })),
        )
        .await?;
        Ok(())
    }

    /// Removes an MCP server.
    pub async fn remove_mcp(&self, name: &str) -> anyhow::Result<()> {
        self.delete(&format!("/api/experimental/mcp/{}", urlencoded(name)))
            .await?;
        Ok(())
    }

    /// Returns the foreground sessions currently owned by this OpenCode
    /// process (`/api/session/active`).
    pub(super) async fn active_sessions(
        &self,
    ) -> anyhow::Result<std::collections::HashSet<String>> {
        let value = self.get("/api/session/active").await?;
        Ok(value
            .get("data")
            .and_then(Value::as_object)
            .map(|sessions| sessions.keys().cloned().collect())
            .unwrap_or_default())
    }

    pub(super) async fn pending_permissions(&self, session_id: &str) -> anyhow::Result<Vec<Value>> {
        let value = self
            .get(&format!("/api/session/{session_id}/permission"))
            .await?;
        Ok(value
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub(super) async fn pending_forms(&self, session_id: &str) -> anyhow::Result<Vec<Value>> {
        let value = self.get(&format!("/api/session/{session_id}/form")).await?;
        Ok(value
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Reads the current durable-log sequence without fetching the history.
    ///
    /// A fresh runtime records this baseline so the initial SSE connection can
    /// backfill only events that happened after the session was started,
    /// instead of replaying the whole conversation.
    pub(super) async fn session_log_watermark(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<u64>> {
        let path = format!(
            "/api/experimental/session/{session_id}/log?after={LOG_WATERMARK_PROBE}&follow=false"
        );
        let response = tokio::time::timeout(
            SERVER_HEALTH_TIMEOUT,
            self.auth(self.sse_http.get(self.url(&path))).send(),
        )
        .await??;
        if !response.status().is_success() {
            anyhow::bail!(
                "OpenCode session log watermark returned {}",
                response.status()
            );
        }
        let text = tokio::time::timeout(SERVER_HEALTH_TIMEOUT, response.text()).await??;
        Ok(parse_log_watermark(&text))
    }

    /// Fetches the newest `limit` messages of a session in chronological order.
    ///
    /// The API pages from the requested end. Starting with `order=desc` keeps
    /// the latest context of a long session instead of truncating at the oldest
    /// 1000 messages; the collected pages are reversed before returning.
    pub(super) async fn messages(
        &self,
        session_id: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<Value>> {
        let mut messages = Vec::new();
        let mut cursor: Option<String> = None;
        let mut remaining = limit;
        while remaining > 0 {
            let page = remaining.min(100);
            let path = match cursor.as_deref() {
                // The API rejects `order` together with a cursor.
                Some(cursor) => {
                    let mut path =
                        format!("/api/session/{session_id}/message?limit={page}&cursor=");
                    path.push_str(&urlencoded(cursor));
                    path
                }
                None => format!("/api/session/{session_id}/message?order=desc&limit={page}"),
            };
            let value = self.get(&path).await?;
            let data = value
                .get("data")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let count = data.len();
            messages.extend(data);
            remaining = remaining.saturating_sub(count as u32);
            let next = value
                .pointer("/cursor/next")
                .and_then(Value::as_str)
                .map(str::to_owned);
            match next {
                Some(next) if count > 0 => cursor = Some(next),
                _ => break,
            }
        }
        messages.reverse();
        Ok(messages)
    }
}

fn parse_log_watermark(text: &str) -> Option<u64> {
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
            return value.get("seq").and_then(Value::as_u64);
        }
    }
    None
}

/// Formats an OpenCode millisecond timestamp as RFC 3339 (UTC).
///
/// `AgentSessionInfo.updated_at` is consumed as ISO 8601 by the external-session
/// dialog and by lexicographic sorting in the node. A fixed millisecond width
/// keeps that sort stable.
fn epoch_millis_to_rfc3339(ms: i64) -> String {
    const FORMAT: &[FormatItem<'static>] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
    let Ok(time) = OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000) else {
        return "1970-01-01T00:00:00.000Z".to_owned();
    };
    match time.format(FORMAT) {
        Ok(value) => value,
        Err(_) => "1970-01-01T00:00:00.000Z".to_owned(),
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
