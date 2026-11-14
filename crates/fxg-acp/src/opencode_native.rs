//! Native OpenCode v2 driver (Tier 3).
//!
//! OpenCode exposes a full session API over `opencode serve` (HTTP + SSE).
//! ACP only exposes a subset (no revert, no message-boundary fork, no
//! server-side steering, no provider/MCP/skill management), so this driver
//! talks to the native API directly.
//!
//! The driver reuses the OpenCode service when one is running
//! (`<state>/opencode/service.json`) and otherwise starts a private
//! `opencode serve` process owned by the node. Sessions live in OpenCode's own
//! store, so a session created here can be continued by the TUI or another
//! OpenCode client.
//!
//! Scope (Phase 1-3 of the native-driver plan):
//! - create / bind / resume / fork sessions
//! - streaming text, reasoning, tools, permissions, forms, usage, compaction
//! - model / agent selection, slash commands, attachments
//! - revert, message-boundary fork, steering, manual compaction
//! - subagent child sessions
//! - history import
//! - provider / MCP / skill / command management primitives (used by the node API)
//!
//! Design: `docs/04-agent-drivers-and-windows.md` and
//! `docs/proposals/2026-11-14-opencode-native-driver.md`.

#![allow(
    clippy::disallowed_types,
    reason = "OpenCode v2 HTTP/SSE payloads are parsed at this external boundary"
)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use base64::Engine;
use futures_util::StreamExt;
use fxg_protocol::common::{
    ClientSource, CommandInfo, ConfigOptionCategory, ConfigOptionChoice, ConfigOptionInfo,
    ConfigOptionValue, ElicitationAction, ElicitationArrayItems, ElicitationContent,
    ElicitationOption, ElicitationProperty, ElicitationPropertyKind, ElicitationSchema,
    ElicitationSchemaType, ElicitationValue, ForkScope, ModeInfo, NativeSessionFeature,
    PermissionOption, PermissionOptionKind, PromptInput, RawJson, SessionStatus,
    StreamDeltaPayload, ToolCallKind, ToolCallStatus, UsageCost,
};
use fxg_protocol::events::{SubagentStatus, UnifiedEventPayload};
use fxg_protocol::util::uuid_v7;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

use crate::driver::{
    ActiveSessionHandle, AgentDriver, AgentLaunchSpec, AgentSessionInfo, AgentSessionList,
    DriverEvent, DriverKind, HistoryCaptureUnavailable, NativeForkUnavailable,
    NativeResumeUnavailable, StartSessionRequest, StartedSession,
};

const AUTH_USER: &str = "opencode";
const OPENCODE_MAJOR: u64 = 2;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const SSE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const SSE_RECONNECT_DELAY: Duration = Duration::from_millis(750);
const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(45);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(200);
const MAX_SEEN_EVENT_IDS: usize = 4096;
const MAX_STDERR_LINES: usize = 200;
const COMPACT_COMMAND: &str = "compact";

/// Discards a failed optional read (used for OpenCode endpoints that are
/// unavailable on older versions).
#[allow(
    clippy::manual_ok_err,
    reason = "Result::ok is disallowed by the workspace lint"
)]
fn optional<T, E>(result: Result<T, E>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(_) => None,
    }
}

/// Reads an environment variable without treating a non-Unicode value as an error.
#[allow(
    clippy::manual_ok_err,
    reason = "Result::ok is disallowed by the workspace lint"
)]
fn env_var(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) => Some(value),
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

/// A typed-ish client for the OpenCode v2 HTTP API.
///
/// Responses are parsed at this boundary; the driver converts them to fxg's
/// normalized types immediately.
#[derive(Clone)]
pub struct OpenCodeClient {
    http: reqwest::Client,
    /// Long-lived stream client (no total request timeout).
    sse_http: reqwest::Client,
    base_url: Arc<str>,
    password: Arc<str>,
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

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
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
            anyhow::bail!(
                "OpenCode {path} returned {status}: {}",
                text.chars().take(500).collect::<String>()
            );
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

    async fn session_exists(&self, session_id: &str) -> anyhow::Result<bool> {
        match self.get(&format!("/api/session/{session_id}")).await {
            Ok(_) => Ok(true),
            Err(err) if err.to_string().contains("404") => Ok(false),
            Err(err) => Err(err),
        }
    }

    async fn create_session(
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

    async fn fork_session(&self, source: &str, before: Option<&str>) -> anyhow::Result<String> {
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

    async fn switch_agent(&self, session_id: &str, agent: &str) -> anyhow::Result<()> {
        self.post(
            &format!("/api/session/{session_id}/agent"),
            json!({ "agent": agent }),
        )
        .await?;
        Ok(())
    }

    async fn switch_model(&self, session_id: &str, model: Value) -> anyhow::Result<()> {
        self.post(
            &format!("/api/session/{session_id}/model"),
            json!({ "model": model }),
        )
        .await?;
        Ok(())
    }

    async fn interrupt(&self, session_id: &str) -> anyhow::Result<()> {
        self.post(&format!("/api/session/{session_id}/interrupt"), json!({}))
            .await?;
        Ok(())
    }

    async fn compact(&self, session_id: &str, delivery: Option<&str>) -> anyhow::Result<()> {
        let body = match delivery {
            Some(delivery) => json!({ "delivery": delivery }),
            None => json!({}),
        };
        self.post(&format!("/api/session/{session_id}/compact"), body)
            .await?;
        Ok(())
    }

    async fn reply_permission(
        &self,
        session_id: &str,
        request_id: &str,
        reply: &str,
    ) -> anyhow::Result<()> {
        self.post(
            &format!("/api/session/{session_id}/permission/{request_id}/reply"),
            json!({ "reply": reply }),
        )
        .await?;
        Ok(())
    }

    async fn reply_form(
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

    async fn cancel_form(&self, session_id: &str, form_id: &str) -> anyhow::Result<()> {
        self.delete(&format!("/api/session/{session_id}/form/{form_id}"))
            .await?;
        Ok(())
    }

    async fn revert(&self, session_id: &str, message_id: &str, files: bool) -> anyhow::Result<()> {
        self.post(
            &format!("/api/session/{session_id}/revert/stage"),
            json!({ "messageID": message_id, "files": files }),
        )
        .await?;
        self.post(
            &format!("/api/session/{session_id}/revert/commit"),
            json!({}),
        )
        .await?;
        Ok(())
    }

    async fn send_prompt(
        &self,
        session_id: &str,
        input: &PromptInput,
        delivery: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut body = json!({ "text": input.text });
        if let Some(id) = input.agent_message_id.as_deref() {
            body["id"] = Value::String(id.to_owned());
        }
        if let Some(delivery) = delivery {
            body["delivery"] = Value::String(delivery.to_owned());
        }
        let files = attachments_to_prompt_files(&input.attachments).await?;
        if !files.is_empty() {
            body["files"] = Value::Array(files);
        }
        self.post(&format!("/api/session/{session_id}/prompt"), body)
            .await?;
        Ok(())
    }

    async fn send_command(
        &self,
        session_id: &str,
        name: &str,
        text: &str,
        delivery: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut body = json!({ "name": name, "text": text });
        if let Some(delivery) = delivery {
            body["delivery"] = Value::String(delivery.to_owned());
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
        let path = "/api/session?limit=100&order=desc";
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
                        .map(|ms| ms.to_string()),
                })
            })
            .collect())
    }

    /// Reads the management snapshot (agents / models / providers / MCP / skills).
    pub async fn management_snapshot(
        &self,
    ) -> anyhow::Result<fxg_protocol::client_api::OpenCodeManagementSnapshot> {
        use fxg_protocol::client_api::{
            OpenCodeAgentSummary, OpenCodeManagementSnapshot, OpenCodeMcpSummary,
            OpenCodeModelSummary, OpenCodeProviderSummary, OpenCodeSkillSummary,
        };

        let agents = optional(self.get("/api/agent").await);
        let models = optional(self.get("/api/model").await);
        let providers = optional(self.get("/api/provider").await);
        let mcp = optional(self.get("/api/mcp").await);
        let skills = optional(self.get("/api/skill").await);
        let commands = optional(self.get("/api/command").await);

        let agents = agents
            .as_ref()
            .and_then(|value| value.get("data"))
            .and_then(Value::as_array)
            .map(|agents| {
                agents
                    .iter()
                    .filter_map(|agent| {
                        let id = agent.get("id")?.as_str()?.to_owned();
                        Some(OpenCodeAgentSummary {
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
                                .to_owned(),
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
                        Some(OpenCodeModelSummary {
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
                        Some(OpenCodeProviderSummary {
                            name: provider
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(&id)
                                .to_owned(),
                            activation: provider
                                .get("activation")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
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
                        Some(OpenCodeMcpSummary {
                            name: server.get("name")?.as_str()?.to_owned(),
                            status: server
                                .pointer("/status/status")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .to_owned(),
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
                        Some(OpenCodeSkillSummary {
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

        Ok(OpenCodeManagementSnapshot {
            available: true,
            version: Some(self.version().await?),
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
        action: fxg_protocol::client_api::OpenCodeMcpAction,
    ) -> anyhow::Result<()> {
        use fxg_protocol::client_api::OpenCodeMcpAction;
        let suffix = match action {
            OpenCodeMcpAction::Connect => "connect",
            OpenCodeMcpAction::Disconnect => "disconnect",
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
        definition: &fxg_protocol::client_api::OpenCodeMcpDefinition,
    ) -> anyhow::Result<()> {
        let config = match definition.kind.as_str() {
            "remote" => json!({
                "type": "remote",
                "url": definition.url.clone().unwrap_or_default(),
                "disabled": !definition.enabled,
            }),
            _ => json!({
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

    /// Fetches the full message list of a session (oldest first).
    async fn messages(&self, session_id: &str, limit: u32) -> anyhow::Result<Vec<Value>> {
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
                None => format!("/api/session/{session_id}/message?order=asc&limit={page}"),
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
        Ok(messages)
    }
}

// ---------------------------------------------------------------------------
// Server discovery / lifecycle
// ---------------------------------------------------------------------------

struct Server {
    client: OpenCodeClient,
    owned: Option<OwnedServer>,
}

struct OwnedServer {
    child: Mutex<Option<tokio::process::Child>>,
    _guard: Option<fxg_pty::ProcessTreeGuard>,
}

impl Server {
    fn is_owned(&self) -> bool {
        self.owned.is_some()
    }

    async fn stop(&self) {
        let Some(owned) = &self.owned else {
            return;
        };
        let mut child = owned.child.lock().await;
        if let Some(child) = child.as_mut() {
            if let Err(err) = child.start_kill() {
                debug!("failed to stop opencode serve: {err}");
            }
            if let Err(err) = child.wait().await {
                debug!("failed to wait for opencode serve: {err}");
            }
        }
        *child = None;
    }
}

/// Native OpenCode v2 driver.
pub struct OpenCodeNativeDriver {
    inner: Arc<DriverInner>,
}

impl Default for OpenCodeNativeDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenCodeNativeDriver {
    /// Creates a driver instance shared across all sessions of the node.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DriverInner {
                server: Mutex::new(None),
                injected: None,
                runtimes: std::sync::Mutex::new(HashMap::new()),
                children: std::sync::Mutex::new(HashMap::new()),
                sse_running: AtomicBool::new(false),
                stderr: std::sync::Mutex::new(Vec::new()),
            }),
        }
    }

    /// Creates a driver bound to an explicit server (tests / embedded clients).
    #[must_use]
    pub fn with_server(base_url: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(DriverInner {
                server: Mutex::new(None),
                injected: Some(OpenCodeClient::new(base_url, password)),
                runtimes: std::sync::Mutex::new(HashMap::new()),
                children: std::sync::Mutex::new(HashMap::new()),
                sse_running: AtomicBool::new(false),
                stderr: std::sync::Mutex::new(Vec::new()),
            }),
        }
    }

    /// Connects to an OpenCode service discovered on this machine.
    ///
    /// Used by the node management API. Does not start a server.
    pub async fn connect_management() -> anyhow::Result<OpenCodeClient> {
        if let Some(client) = env_client() {
            return Ok(client);
        }
        if let Some(client) = discover_service().await? {
            return Ok(client);
        }
        Err(anyhow!(
            "OpenCode is not running (start `opencode serve` or `opencode service start`)"
        ))
    }
}

struct DriverInner {
    server: Mutex<Option<Arc<Server>>>,
    /// Explicit server for tests / embedded clients (skips discovery).
    injected: Option<OpenCodeClient>,
    runtimes: std::sync::Mutex<HashMap<String, Arc<SessionRuntime>>>,
    /// child agent session ID -> (root agent session ID, depth, parent)
    children: std::sync::Mutex<HashMap<String, ChildInfo>>,
    sse_running: AtomicBool,
    stderr: std::sync::Mutex<Vec<String>>,
}

#[derive(Clone, Debug)]
struct ChildInfo {
    root: String,
    #[allow(dead_code, reason = "reserved for nested subagent updates")]
    parent: Option<String>,
    #[allow(dead_code, reason = "reserved for nested subagent updates")]
    depth: u32,
    #[allow(dead_code, reason = "reserved for nested subagent updates")]
    title: Option<String>,
}

impl DriverInner {
    async fn ensure_server(
        self: &Arc<Self>,
        launch: &AgentLaunchSpec,
        cwd: &Path,
    ) -> anyhow::Result<Arc<Server>> {
        {
            let guard = self.server.lock().await;
            if let Some(server) = guard.as_ref() {
                return Ok(Arc::clone(server));
            }
        }

        let server = if let Some(client) = self.injected.clone().or_else(env_client) {
            Server {
                client,
                owned: None,
            }
        } else if let Some(client) = discover_service().await? {
            Server {
                client,
                owned: None,
            }
        } else {
            spawn_server(self, launch, cwd).await?
        };
        verify_version(&server.client).await?;
        let server = Arc::new(server);
        {
            let mut guard = self.server.lock().await;
            if let Some(existing) = guard.as_ref() {
                return Ok(Arc::clone(existing));
            }
            *guard = Some(Arc::clone(&server));
        }
        if !self.sse_running.swap(true, Ordering::SeqCst) {
            let inner = Arc::clone(self);
            let server_for_pump = Arc::clone(&server);
            tokio::spawn(async move {
                run_sse(inner, server_for_pump).await;
            });
        }
        info!(base_url = %server.client.base_url, "OpenCode native driver connected");
        Ok(server)
    }

    async fn server(&self) -> Option<Arc<Server>> {
        self.server.lock().await.clone()
    }

    fn register(&self, runtime: Arc<SessionRuntime>) {
        self.runtimes
            .lock()
            .expect("runtimes poisoned")
            .insert(runtime.agent_session_id.clone(), runtime);
    }

    fn unregister(&self, agent_session_id: &str) {
        self.runtimes
            .lock()
            .expect("runtimes poisoned")
            .remove(agent_session_id);
        self.children
            .lock()
            .expect("children poisoned")
            .retain(|_, child| child.root != agent_session_id);
    }

    fn runtime_for(&self, agent_session_id: &str) -> Option<(Arc<SessionRuntime>, Option<String>)> {
        let runtimes = self.runtimes.lock().expect("runtimes poisoned");
        if let Some(runtime) = runtimes.get(agent_session_id) {
            return Some((Arc::clone(runtime), None));
        }
        let children = self.children.lock().expect("children poisoned");
        let child = children.get(agent_session_id)?;
        let runtime = runtimes.get(&child.root)?;
        Some((Arc::clone(runtime), Some(agent_session_id.to_owned())))
    }

    fn note_stderr(&self, line: String) {
        let mut lines = self.stderr.lock().expect("stderr poisoned");
        lines.push(line);
        if lines.len() > MAX_STDERR_LINES {
            let drain = lines.len() - MAX_STDERR_LINES;
            lines.drain(0..drain);
        }
    }
}

fn env_client() -> Option<OpenCodeClient> {
    let url = env_var("FXG_OPENCODE_URL")?;
    let password = env_var("FXG_OPENCODE_PASSWORD").unwrap_or_default();
    if url.trim().is_empty() {
        return None;
    }
    Some(OpenCodeClient::new(url, password))
}

async fn discover_service() -> anyhow::Result<Option<OpenCodeClient>> {
    let Some(path) = service_json_path() else {
        return Ok(None);
    };
    let text = match tokio::fs::read_to_string(&path).await {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("invalid OpenCode service file: {}", path.display()))?;
    let Some(url) = value.get("url").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(password) = value.get("password").and_then(Value::as_str) else {
        return Ok(None);
    };
    let client = OpenCodeClient::new(url, password);
    match client.version().await {
        Ok(_) => Ok(Some(client)),
        Err(err) => {
            debug!(%url, "stale OpenCode service file: {err:#}");
            Ok(None)
        }
    }
}

fn service_json_path() -> Option<PathBuf> {
    dirs::state_dir()
        .map(|dir| dir.join("opencode/service.json"))
        .or_else(|| {
            std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .map(|dir| dir.join("opencode/service.json"))
        })
        .or_else(|| dirs::home_dir().map(|dir| dir.join(".local/state/opencode/service.json")))
}

#[allow(
    clippy::manual_ok_err,
    reason = "Result::ok is disallowed by the workspace lint"
)]
async fn verify_version(client: &OpenCodeClient) -> anyhow::Result<()> {
    let version = client.version().await?;
    let major = version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|part| match part.parse::<u64>() {
            Ok(value) => Some(value),
            Err(_) => None,
        });
    if major != Some(OPENCODE_MAJOR) {
        anyhow::bail!(
            "OpenCode v2 is required by the native driver (found {})",
            if version.is_empty() {
                "unknown"
            } else {
                &version
            }
        );
    }
    Ok(())
}

async fn spawn_server(
    inner: &Arc<DriverInner>,
    launch: &AgentLaunchSpec,
    cwd: &Path,
) -> anyhow::Result<Server> {
    let program = launch
        .program
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("the OpenCode program is not installed"))?;
    let program = fxg_pty::resolve_command(&program.to_string_lossy(), cwd)?;
    let password = format!("{}{}", uuid_v7(), uuid_v7()).replace('-', "");
    let mut command = tokio::process::Command::new(&program);
    command
        .arg("serve")
        .arg("--hostname")
        .arg("127.0.0.1")
        .arg("--port")
        .arg("0")
        .env("OPENCODE_PASSWORD", &password)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in &launch.env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start opencode serve ({})", program.display()))?;
    let guard = match fxg_pty::ProcessTreeGuard::new() {
        Ok(guard) => Some(guard),
        Err(err) => {
            tracing::debug!("failed to create the OpenCode process tree guard: {err}");
            None
        }
    };
    #[cfg(windows)]
    if let (Some(guard), Some(handle)) = (guard.as_ref(), child.raw_handle())
        && let Err(err) = guard.attach_raw_handle(handle)
    {
        tracing::warn!("failed to bind opencode serve to a Windows Job Object: {err}");
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("opencode serve stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("opencode serve stderr is unavailable"))?;
    let (url_tx, mut url_rx) = mpsc::unbounded_channel::<String>();
    spawn_log_pump(inner, stdout, url_tx.clone());
    spawn_log_pump(inner, stderr, url_tx);

    let url = tokio::time::timeout(SERVER_READY_TIMEOUT, async {
        while let Some(line) = url_rx.recv().await {
            if let Some(url) = parse_server_url(&line) {
                return Some(url);
            }
        }
        None
    })
    .await
    .map_err(|err| anyhow!("opencode serve did not become ready in time: {err}"))?
    .ok_or_else(|| anyhow!("opencode serve exited before reporting its address"))?;

    let client = OpenCodeClient::new(&url, &password);
    let healthy = tokio::time::timeout(SERVER_READY_TIMEOUT, async {
        loop {
            match client.version().await {
                Ok(_) => return true,
                Err(_) => tokio::time::sleep(READY_POLL_INTERVAL).await,
            }
        }
    })
    .await
    .unwrap_or(false);
    if !healthy {
        anyhow::bail!("opencode serve did not become healthy at {url}");
    }

    Ok(Server {
        client,
        owned: Some(OwnedServer {
            child: Mutex::new(Some(child)),
            _guard: guard,
        }),
    })
}

fn parse_server_url(line: &str) -> Option<String> {
    let marker = "server listening on ";
    let index = line.find(marker)?;
    let rest = line[index + marker.len()..].trim();
    rest.split_whitespace()
        .next()
        .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
        .map(str::to_owned)
}

fn spawn_log_pump<R>(inner: &Arc<DriverInner>, reader: R, url_tx: mpsc::UnboundedSender<String>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let inner = Arc::clone(inner);
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            inner.note_stderr(line.clone());
            if line.contains("server listening on ")
                && let Err(err) = url_tx.send(line.clone())
            {
                debug!("OpenCode URL channel closed: {err}");
            }
            debug!(target: "fxg_opencode", "opencode serve: {line}");
        }
    });
}

// ---------------------------------------------------------------------------
// Session runtime and event mapping
// ---------------------------------------------------------------------------

struct SessionRuntime {
    fxg_session_id: String,
    agent_session_id: String,
    event_tx: mpsc::UnboundedSender<DriverEvent>,
    state: std::sync::Mutex<RuntimeState>,
}

#[derive(Default)]
struct RuntimeState {
    busy: bool,
    commands: Vec<String>,
    tool_names: HashMap<String, String>,
    tool_titles: HashMap<String, String>,
    tool_inputs: HashMap<String, Value>,
    self_messages: HashSet<String>,
    pending_commands: usize,
    seen_event_ids: HashSet<String>,
    durable_seq: HashMap<String, u64>,
    context_size: u64,
}

impl SessionRuntime {
    fn emit(&self, events: Vec<DriverEvent>) {
        for event in events {
            drop(self.event_tx.send(event));
        }
    }

    fn emit_event(&self, payload: UnifiedEventPayload) {
        self.emit(vec![DriverEvent::Event(payload)]);
    }

    fn is_busy(&self) -> bool {
        self.state.lock().expect("state poisoned").busy
    }

    fn set_busy(&self, busy: bool) {
        self.state.lock().expect("state poisoned").busy = busy;
    }

    async fn refresh_capabilities(&self, client: &OpenCodeClient) {
        match fetch_capabilities(client, &self.agent_session_id).await {
            Ok((payload, commands, context_size)) => {
                {
                    let mut state = self.state.lock().expect("state poisoned");
                    state.commands = commands;
                    state.context_size = context_size;
                }
                self.emit_event(payload);
            }
            Err(err) => warn!(
                session_id = %self.fxg_session_id,
                "failed to refresh OpenCode capabilities: {err:#}"
            ),
        }
    }
}

async fn fetch_capabilities(
    client: &OpenCodeClient,
    session_id: &str,
) -> anyhow::Result<(UnifiedEventPayload, Vec<String>, u64)> {
    let agents = optional(client.get("/api/agent").await);
    let models = optional(client.get("/api/model").await);
    let commands = optional(client.get("/api/command").await);
    let session = optional(client.get(&format!("/api/session/{session_id}")).await);

    let available_modes = agents
        .as_ref()
        .and_then(|value| value.get("data"))
        .and_then(Value::as_array)
        .map(|agents| {
            agents
                .iter()
                .filter(|agent| agent.get("mode").and_then(Value::as_str) != Some("subagent"))
                .filter(|agent| agent.get("hidden").and_then(Value::as_bool) != Some(true))
                .filter_map(|agent| {
                    Some(ModeInfo {
                        mode_id: agent.get("id")?.as_str()?.to_owned(),
                        name: agent
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        description: agent
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let model_options = models
        .as_ref()
        .and_then(|value| value.get("data"))
        .and_then(Value::as_array)
        .map(|models| {
            let mut options = Vec::new();
            for model in models {
                if model.get("enabled").and_then(Value::as_bool) == Some(false) {
                    continue;
                }
                let Some(model_id) = model
                    .get("modelID")
                    .or_else(|| model.get("id"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let Some(provider_id) = model.get("providerID").and_then(Value::as_str) else {
                    continue;
                };
                let name = model
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(model_id);
                let variants = model
                    .get("variants")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                options.push(ConfigOptionValue::Choice(ConfigOptionChoice {
                    value: format!("{provider_id}/{model_id}"),
                    name: name.to_owned(),
                    description: None,
                }));
                for variant in variants {
                    let Some(variant_id) = variant.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    options.push(ConfigOptionValue::Choice(ConfigOptionChoice {
                        value: format!("{provider_id}/{model_id}#{variant_id}"),
                        name: format!("{name} ({variant_id})"),
                        description: None,
                    }));
                }
            }
            options
        })
        .unwrap_or_default();

    let current_mode = session
        .as_ref()
        .and_then(|value| value.pointer("/data/agent"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| available_modes.first().map(|mode| mode.mode_id.clone()));

    let current_model_ref = session
        .as_ref()
        .and_then(|value| value.pointer("/data/model"));
    let current_model = current_model_ref.and_then(model_ref_value);
    let context_size = current_model_ref
        .and_then(|model| {
            let provider = model.get("providerID")?.as_str()?;
            let id = model.get("id")?.as_str()?;
            models
                .as_ref()?
                .get("data")?
                .as_array()?
                .iter()
                .find(|candidate| {
                    candidate.get("providerID").and_then(Value::as_str) == Some(provider)
                        && candidate
                            .get("modelID")
                            .or_else(|| candidate.get("id"))
                            .and_then(Value::as_str)
                            == Some(id)
                })
                .and_then(|model| model.pointer("/limit/context"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);

    let config_options = if model_options.is_empty() {
        Vec::new()
    } else {
        vec![ConfigOptionInfo {
            key: "model".to_owned(),
            name: "Model".to_owned(),
            description: None,
            category: Some(ConfigOptionCategory::Model),
            current_value: current_model.unwrap_or_else(|| model_options[0].clone()),
            options: model_options,
        }]
    };

    let mut available_commands = commands
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
                        input_hint: command
                            .get("input")
                            .and_then(|input| input.get("hint"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !available_commands
        .iter()
        .any(|command| command.name == COMPACT_COMMAND)
    {
        available_commands.push(CommandInfo {
            name: COMPACT_COMMAND.to_owned(),
            description: "Compact the conversation context".to_owned(),
            input_hint: None,
        });
    }
    let command_names = available_commands
        .iter()
        .map(|command| command.name.clone())
        .collect();

    Ok((
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode,
            available_modes,
            available_commands,
            config_options,
            fork_scope: Some(ForkScope::Message),
            native_features: vec![
                NativeSessionFeature::Revert,
                NativeSessionFeature::Steer,
                NativeSessionFeature::Compact,
            ],
        },
        command_names,
        context_size,
    ))
}

fn model_ref_value(value: &Value) -> Option<ConfigOptionValue> {
    let id = value.get("id").and_then(Value::as_str)?;
    let provider = value.get("providerID").and_then(Value::as_str)?;
    let variant = value.get("variant").and_then(Value::as_str);
    let wire = match variant {
        Some(variant) if !variant.is_empty() => format!("{provider}/{id}#{variant}"),
        _ => format!("{provider}/{id}"),
    };
    Some(ConfigOptionValue::Choice(ConfigOptionChoice {
        name: wire.clone(),
        value: wire,
        description: None,
    }))
}

fn parse_model_ref(value: &str) -> Option<Value> {
    let (model, variant) = match value.split_once('#') {
        Some((model, variant)) => (model, Some(variant)),
        None => (value, None),
    };
    let (provider_id, id) = model.split_once('/')?;
    let mut body = json!({ "providerID": provider_id, "id": id });
    if let Some(variant) = variant {
        body["variant"] = Value::String(variant.to_owned());
    }
    Some(body)
}

async fn run_sse(inner: Arc<DriverInner>, _initial: Arc<Server>) {
    loop {
        let Some(server) = inner.server().await else {
            tokio::time::sleep(SSE_RECONNECT_DELAY).await;
            continue;
        };
        match stream_events(&inner, &server).await {
            Ok(()) => debug!("OpenCode SSE stream ended"),
            Err(err) => debug!("OpenCode SSE stream failed: {err:#}"),
        }
        // The connection dropped: backfill every known session from its
        // durable log before reconnecting.
        backfill_all(&inner, &server).await;
        tokio::time::sleep(SSE_RECONNECT_DELAY).await;
    }
}

async fn stream_events(inner: &Arc<DriverInner>, server: &Arc<Server>) -> anyhow::Result<()> {
    let request = server
        .client
        .auth(server.client.sse_http.get(server.client.url("/api/event")));
    let response = request.send().await?;
    if !response.status().is_success() {
        anyhow::bail!("OpenCode /api/event returned {}", response.status());
    }
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(index) = buffer.find("\n\n") {
            let frame = buffer[..index].to_owned();
            buffer.drain(..index + 2);
            let data = frame
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim)
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            handle_raw_event(inner, &value).await;
        }
    }
    Ok(())
}

async fn handle_raw_event(inner: &Arc<DriverInner>, value: &Value) {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if kind == "server.connected" || kind == "log.synced" {
        return;
    }
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    if let Some(session_id) = event_session_id(kind, &data) {
        if kind == "session.created" {
            handle_session_created(inner, &data, session_id).await;
            return;
        }
        if let Some((runtime, subagent_id)) = inner.runtime_for(session_id) {
            let events = map_event(&runtime, kind, &data, subagent_id.as_deref(), value);
            runtime.emit(events);
            if subagent_id.is_none()
                && matches!(kind, "session.agent.selected" | "session.model.selected")
                && let Some(server) = inner.server().await
            {
                runtime.refresh_capabilities(&server.client).await;
            }
        }
    }
}

fn event_session_id<'a>(kind: &str, data: &'a Value) -> Option<&'a str> {
    data.get("sessionID")
        .and_then(Value::as_str)
        .or_else(|| {
            data.get("payload")
                .and_then(|payload| payload.get("sessionID"))
                .and_then(Value::as_str)
        })
        .or_else(|| data.pointer("/form/sessionID").and_then(Value::as_str))
        .or_else(|| {
            if kind.starts_with("form.") {
                data.get("sessionID").and_then(Value::as_str)
            } else {
                None
            }
        })
}

async fn handle_session_created(inner: &Arc<DriverInner>, data: &Value, session_id: &str) {
    let parent = data.get("parentID").and_then(Value::as_str);
    let Some(parent) = parent else {
        return;
    };
    let Some((runtime, _)) = inner.runtime_for(parent) else {
        return;
    };
    let depth = {
        let children = inner.children.lock().expect("children poisoned");
        children
            .get(parent)
            .map_or(1, |parent_info| parent_info.depth + 1)
    };
    let title = data.get("title").and_then(Value::as_str).map(str::to_owned);
    {
        let mut children = inner.children.lock().expect("children poisoned");
        children.insert(
            session_id.to_owned(),
            ChildInfo {
                root: runtime.agent_session_id.clone(),
                parent: Some(parent.to_owned()),
                depth,
                title: title.clone(),
            },
        );
    }
    runtime.emit_event(UnifiedEventPayload::SubagentUpdated {
        child_session_id: session_id.to_owned(),
        parent_session_id: Some(parent.to_owned()),
        depth,
        title,
        description: None,
        status: SubagentStatus::Running,
    });
}

fn map_event(
    runtime: &SessionRuntime,
    kind: &str,
    data: &Value,
    subagent_id: Option<&str>,
    raw: &Value,
) -> Vec<DriverEvent> {
    if let Some(id) = raw.get("id").and_then(Value::as_str) {
        let mut state = runtime.state.lock().expect("state poisoned");
        if !state.seen_event_ids.insert(id.to_owned()) {
            return Vec::new();
        }
        if state.seen_event_ids.len() > MAX_SEEN_EVENT_IDS {
            state.seen_event_ids.clear();
        }
        if let Some(seq) = raw.pointer("/durable/seq").and_then(Value::as_u64) {
            state.durable_seq.insert(
                data.get("sessionID")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                seq,
            );
        }
    }

    let text_of = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let scope = subagent_id.map(str::to_owned);

    match kind {
        "session.created" => Vec::new(),
        "session.renamed" | "session.title.updated" => {
            let title = data
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|title| !title.is_empty());
            match title {
                Some(title) => vec![DriverEvent::Event(
                    UnifiedEventPayload::SessionTitleChanged {
                        title: title.to_owned(),
                    },
                )],
                None => Vec::new(),
            }
        }
        "session.execution.started" => {
            runtime.set_busy(true);
            vec![DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            })]
        }
        "session.execution.succeeded" | "session.execution.interrupted" | "session.idle" => {
            if let Some(child) = subagent_id {
                let status = if kind == "session.execution.interrupted" {
                    SubagentStatus::Cancelled
                } else {
                    SubagentStatus::Completed
                };
                return vec![DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
                    child_session_id: child.to_owned(),
                    parent_session_id: None,
                    depth: 1,
                    title: None,
                    description: None,
                    status,
                })];
            }
            runtime.set_busy(false);
            vec![DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Idle,
                error_message: None,
            })]
        }
        "session.execution.failed" => {
            let message = data
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("OpenCode execution failed")
                .to_owned();
            if let Some(child) = subagent_id {
                return vec![DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
                    child_session_id: child.to_owned(),
                    parent_session_id: None,
                    depth: 1,
                    title: None,
                    description: None,
                    status: SubagentStatus::Failed,
                })];
            }
            runtime.set_busy(false);
            vec![DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Error,
                error_message: Some(message),
            })]
        }
        "session.retry.scheduled" => {
            let attempt = data.get("attempt").and_then(Value::as_u64).unwrap_or(0);
            vec![DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: Some(format!("OpenCode is retrying (attempt {attempt})")),
            })]
        }
        "session.error" => vec![DriverEvent::Failed {
            message: data
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("OpenCode session error")
                .to_owned(),
        }],
        "session.compaction.started" => {
            vec![DriverEvent::Event(UnifiedEventPayload::CompactionUpdated {
                status: fxg_protocol::common::CompactionStatus::Started,
                detail: None,
            })]
        }
        "session.compaction.ended" => {
            vec![DriverEvent::Event(UnifiedEventPayload::CompactionUpdated {
                status: fxg_protocol::common::CompactionStatus::Completed,
                detail: None,
            })]
        }
        "session.compaction.failed" => {
            vec![DriverEvent::Event(UnifiedEventPayload::CompactionUpdated {
                status: fxg_protocol::common::CompactionStatus::Failed,
                detail: data.get("error").map(ToString::to_string),
            })]
        }
        "session.compaction.delta" => Vec::new(),
        "session.text.delta" => vec![DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta {
            message_id: text_of("assistantMessageID"),
            text_delta: text_of("delta"),
            subagent_id: scope,
        })],
        "session.text.ended" => vec![DriverEvent::Event(UnifiedEventPayload::AgentMessage {
            message_id: text_of("assistantMessageID"),
            text: text_of("text"),
            is_complete: true,
            subagent_id: scope,
        })],
        "session.reasoning.delta" => {
            vec![DriverEvent::Delta(StreamDeltaPayload::AgentThoughtDelta {
                thought_id: reasoning_id(data),
                text_delta: text_of("delta"),
                subagent_id: scope,
            })]
        }
        "session.reasoning.ended" => vec![DriverEvent::Event(UnifiedEventPayload::AgentThought {
            thought_id: reasoning_id(data),
            text: text_of("text"),
            is_complete: true,
            subagent_id: scope,
        })],
        "session.tool.input.started" => {
            let tool_call_id = text_of("id");
            let name = text_of("name");
            runtime
                .state
                .lock()
                .expect("state poisoned")
                .tool_names
                .insert(tool_call_id.clone(), name.clone());
            vec![DriverEvent::Event(UnifiedEventPayload::ToolCall {
                tool_call_id,
                title: name.clone(),
                kind: tool_kind(&name),
                status: ToolCallStatus::Pending,
                locations: Vec::new(),
                diff: None,
                raw_output: None,
                subagent_id: scope,
            })]
        }
        "session.tool.input.ended" => {
            let id = text_of("id");
            let text = text_of("text");
            runtime
                .state
                .lock()
                .expect("state poisoned")
                .tool_inputs
                .insert(id, Value::String(text));
            Vec::new()
        }
        "session.tool.called" => {
            let tool_call_id = text_of("id");
            let input = data.get("input").cloned().unwrap_or(Value::Null);
            let name = {
                let state = runtime.state.lock().expect("state poisoned");
                state
                    .tool_names
                    .get(&tool_call_id)
                    .cloned()
                    .unwrap_or_else(|| "tool".to_owned())
            };
            let title = tool_title(&name, &input);
            let kind = tool_kind(&name);
            let locations = tool_locations(&input);
            let diff = tool_diff(&name, &input);
            {
                let mut state = runtime.state.lock().expect("state poisoned");
                state
                    .tool_titles
                    .insert(tool_call_id.clone(), title.clone());
                state.tool_inputs.insert(tool_call_id.clone(), input);
            }
            vec![DriverEvent::Event(UnifiedEventPayload::ToolCall {
                tool_call_id,
                title,
                kind,
                status: ToolCallStatus::InProgress,
                locations,
                diff,
                raw_output: None,
                subagent_id: scope,
            })]
        }
        "session.tool.progress" => {
            let tool_call_id = text_of("id");
            let state = runtime.state.lock().expect("state poisoned");
            let title = state
                .tool_titles
                .get(&tool_call_id)
                .cloned()
                .unwrap_or_default();
            let kind = state
                .tool_names
                .get(&tool_call_id)
                .map_or(ToolCallKind::Other, |name| tool_kind(name));
            drop(state);
            vec![DriverEvent::Event(UnifiedEventPayload::ToolCall {
                tool_call_id,
                title,
                kind,
                status: ToolCallStatus::InProgress,
                locations: Vec::new(),
                diff: None,
                raw_output: None,
                subagent_id: scope,
            })]
        }
        "session.tool.success" | "session.tool.failed" => {
            let tool_call_id = text_of("id");
            let failed = kind == "session.tool.failed";
            let state = runtime.state.lock().expect("state poisoned");
            let name = state
                .tool_names
                .get(&tool_call_id)
                .cloned()
                .unwrap_or_else(|| "tool".to_owned());
            let title = state
                .tool_titles
                .get(&tool_call_id)
                .cloned()
                .unwrap_or_else(|| name.clone());
            let input = state
                .tool_inputs
                .get(&tool_call_id)
                .cloned()
                .unwrap_or(Value::Null);
            drop(state);
            vec![DriverEvent::Event(UnifiedEventPayload::ToolCall {
                tool_call_id,
                title,
                kind: tool_kind(&name),
                status: if failed {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                },
                locations: tool_locations(&input),
                diff: tool_diff(&name, &input),
                raw_output: tool_output(data),
                subagent_id: scope,
            })]
        }
        "session.usage.updated" => {
            let used_tokens = data
                .pointer("/tokens/input")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + data
                    .pointer("/tokens/output")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
            let context_size = runtime.state.lock().expect("state poisoned").context_size;
            let cost = data.get("cost").and_then(|cost| {
                let amount = cost.as_f64()?;
                Some(UsageCost {
                    amount,
                    currency: "USD".to_owned(),
                })
            });
            vec![DriverEvent::Event(UnifiedEventPayload::UsageUpdated {
                used_tokens,
                context_size,
                cost,
                subagent_id: scope,
            })]
        }
        "permission.asked" => vec![
            DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::WaitingPermission,
                error_message: None,
            }),
            DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
                request_id: text_of("id"),
                tool_name: text_of("action"),
                summary: permission_summary(data),
                options: permission_options(),
                details: RawJson::new(data.clone()),
                subagent_id: scope,
            }),
        ],
        "permission.replied" => vec![
            DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            }),
            DriverEvent::Event(UnifiedEventPayload::PermissionResolved {
                request_id: text_of("requestID"),
                selected_option_id: text_of("reply"),
                resolved_by: fxg_protocol::common::ResolvedBy::Cli,
            }),
        ],
        "form.created" => match form_to_elicitation(data, scope.clone()) {
            Some(payload) => vec![
                DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::WaitingInput,
                    error_message: None,
                }),
                DriverEvent::Event(payload),
            ],
            None => Vec::new(),
        },
        "form.replied" => vec![
            DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            }),
            DriverEvent::Event(UnifiedEventPayload::ElicitationResolved {
                elicitation_id: text_of("id"),
                action: ElicitationAction::Accept,
                content: form_answer(data),
                resolved_by: fxg_protocol::common::ResolvedBy::Cli,
            }),
        ],
        "form.cancelled" => vec![
            DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            }),
            DriverEvent::Event(UnifiedEventPayload::ElicitationResolved {
                elicitation_id: text_of("id"),
                action: ElicitationAction::Cancel,
                content: None,
                resolved_by: fxg_protocol::common::ResolvedBy::Cli,
            }),
        ],
        "session.agent.selected" | "session.model.selected" => Vec::new(),
        "session.inbox.enqueued" => {
            let inbox_id = text_of("inboxID");
            let text = data
                .pointer("/item/payload/text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            {
                let mut state = runtime.state.lock().expect("state poisoned");
                if state.pending_commands > 0 {
                    state.pending_commands -= 1;
                    return Vec::new();
                }
                if state.self_messages.contains(&inbox_id) {
                    state.self_messages.remove(&inbox_id);
                    return Vec::new();
                }
            }
            if text.is_empty() {
                return Vec::new();
            }
            vec![DriverEvent::Event(UnifiedEventPayload::UserMessage {
                text,
                attachments: Vec::new(),
                client_source: ClientSource::Cli,
                agent_message_id: None,
            })]
        }
        "session.synthetic" => Vec::new(),
        _ => Vec::new(),
    }
}

fn reasoning_id(data: &Value) -> String {
    let message = data
        .get("assistantMessageID")
        .and_then(Value::as_str)
        .unwrap_or("reasoning");
    let ordinal = data.get("ordinal").and_then(Value::as_u64).unwrap_or(0);
    format!("{message}:reasoning:{ordinal}")
}

fn tool_kind(name: &str) -> ToolCallKind {
    match name {
        "read" | "list" | "glob" | "grep" | "find" => ToolCallKind::Read,
        "edit" | "write" | "patch" | "multiedit" => ToolCallKind::Edit,
        "bash" | "shell" | "terminal" => ToolCallKind::Execute,
        "webfetch" | "websearch" | "task" | "todowrite" | "todoread" => ToolCallKind::Search,
        _ => ToolCallKind::Other,
    }
}

fn tool_title(name: &str, input: &Value) -> String {
    match name {
        "bash" | "shell" => input
            .get("command")
            .and_then(Value::as_str)
            .map(|command| format!("$ {command}"))
            .unwrap_or_else(|| name.to_owned()),
        "read" | "edit" | "write" => input
            .get("filePath")
            .or_else(|| input.get("path"))
            .and_then(Value::as_str)
            .map(|path| format!("{name}: {path}"))
            .unwrap_or_else(|| name.to_owned()),
        "grep" | "glob" | "find" => input
            .get("pattern")
            .and_then(Value::as_str)
            .map(|pattern| format!("{name}: {pattern}"))
            .unwrap_or_else(|| name.to_owned()),
        "todowrite" => "Update todo list".to_owned(),
        _ => name.to_owned(),
    }
}

fn tool_locations(input: &Value) -> Vec<String> {
    let mut locations = Vec::new();
    for key in ["filePath", "path", "filename"] {
        if let Some(path) = input.get(key).and_then(Value::as_str) {
            locations.push(path.to_owned());
        }
    }
    if let Some(paths) = input.get("paths").and_then(Value::as_array) {
        locations.extend(paths.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    locations.sort();
    locations.dedup();
    locations
}

fn tool_diff(name: &str, input: &Value) -> Option<fxg_protocol::common::FileDiff> {
    let (path, old_text, new_text) = match name {
        "edit" => {
            let path = input.get("filePath").or_else(|| input.get("path"))?;
            let path = path.as_str()?;
            let old = input
                .get("oldString")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let new = input
                .get("newString")
                .and_then(Value::as_str)
                .unwrap_or_default();
            (path, old, new)
        }
        "write" => {
            let path = input.get("filePath").or_else(|| input.get("path"))?;
            let path = path.as_str()?;
            let new = input
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            (path, "", new)
        }
        _ => return None,
    };
    let patch = diffy::create_patch(old_text, new_text);
    let unified = patch.to_string();
    let mut additions = 0;
    let mut deletions = 0;
    for line in unified.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
        }
    }
    Some(fxg_protocol::common::FileDiff {
        path: path.to_owned(),
        old_text: Some(old_text.to_owned()),
        new_text: Some(new_text.to_owned()),
        unified_diff: unified,
        additions,
        deletions,
    })
}

fn tool_output(data: &Value) -> Option<String> {
    let content = data.get("content").and_then(Value::as_array)?;
    let mut texts = Vec::new();
    for item in content {
        if item.get("type").and_then(Value::as_str) == Some("text")
            && let Some(text) = item.get("text").and_then(Value::as_str)
        {
            texts.push(text.to_owned());
        }
    }
    if texts.is_empty() {
        data.get("metadata")
            .filter(|value| !value.is_null())
            .map(ToString::to_string)
    } else {
        Some(texts.join("\n"))
    }
}

fn permission_summary(data: &Value) -> String {
    if let Some(message) = data.get("message").and_then(Value::as_str)
        && !message.trim().is_empty()
    {
        return message.to_owned();
    }
    let action = data
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("permission");
    let resources = data
        .get("resources")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if resources.is_empty() {
        action.to_owned()
    } else {
        format!("{action}: {resources}")
    }
}

fn permission_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption {
            option_id: "once".to_owned(),
            name: "Allow once".to_owned(),
            kind: PermissionOptionKind::AllowOnce,
        },
        PermissionOption {
            option_id: "always".to_owned(),
            name: "Always allow".to_owned(),
            kind: PermissionOptionKind::AllowAlways,
        },
        PermissionOption {
            option_id: "reject".to_owned(),
            name: "Reject".to_owned(),
            kind: PermissionOptionKind::RejectOnce,
        },
    ]
}

fn form_to_elicitation(data: &Value, subagent_id: Option<String>) -> Option<UnifiedEventPayload> {
    let form = data.get("form")?;
    let form_id = form.get("id")?.as_str()?.to_owned();
    let session_id = form.get("sessionID").and_then(Value::as_str);
    session_id?;
    let title = form
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Question")
        .to_owned();
    let fields = form.get("fields").and_then(Value::as_array)?;
    let mut properties = BTreeMap::new();
    let mut required = Vec::new();
    for field in fields {
        let Some(key) = field.get("key").and_then(Value::as_str) else {
            continue;
        };
        if field.get("hidden").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let kind = field
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("string");
        let field_title = field
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let description = field
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let default = field.get("default").cloned().map(RawJson::new);
        let mut property = ElicitationProperty {
            kind: ElicitationPropertyKind::String,
            title: field_title,
            description,
            default,
            r#enum: Vec::new(),
            one_of: Vec::new(),
            items: None,
            minimum: None,
            maximum: None,
            extra: BTreeMap::new(),
        };
        match kind {
            "integer" => property.kind = ElicitationPropertyKind::Integer,
            "number" => property.kind = ElicitationPropertyKind::Number,
            "boolean" => property.kind = ElicitationPropertyKind::Boolean,
            "multiselect" => {
                property.kind = ElicitationPropertyKind::Array;
                let options = field
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|options| {
                        options
                            .iter()
                            .filter_map(|option| option.get("value")?.as_str().map(str::to_owned))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                property.items = Some(ElicitationArrayItems {
                    kind: ElicitationPropertyKind::String,
                    r#enum: options,
                    extra: BTreeMap::new(),
                });
            }
            _ => {
                if let Some(options) = field.get("options").and_then(Value::as_array) {
                    property.one_of = options
                        .iter()
                        .filter_map(|option| {
                            Some(ElicitationOption {
                                const_value: option.get("value")?.as_str()?.to_owned(),
                                title: option
                                    .get("label")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                extra: BTreeMap::new(),
                            })
                        })
                        .collect();
                }
            }
        }
        if field.get("required").and_then(Value::as_bool) == Some(true) {
            required.push(key.to_owned());
        }
        properties.insert(key.to_owned(), property);
    }
    if properties.is_empty() {
        return None;
    }
    Some(UnifiedEventPayload::ElicitationRequest {
        elicitation_id: form_id,
        message: title,
        mode: fxg_protocol::common::ElicitationMode::Form,
        requested_schema: ElicitationSchema {
            schema_type: ElicitationSchemaType::Object,
            title: None,
            description: None,
            properties,
            required: (!required.is_empty()).then_some(required),
            extra: BTreeMap::new(),
        },
        tool_call_id: None,
        subagent_id,
    })
}

fn form_answer(data: &Value) -> Option<ElicitationContent> {
    let answer = data.get("answer")?.as_object()?;
    let mut content = ElicitationContent::new();
    for (key, value) in answer {
        content.insert(key.clone(), json_to_elicitation_value(value));
    }
    Some(content)
}

fn elicitation_value_to_json(value: &ElicitationValue) -> Value {
    match value {
        ElicitationValue::String(value) => Value::String(value.clone()),
        ElicitationValue::Integer(value) => json!(value),
        ElicitationValue::Number(value) => json!(value),
        ElicitationValue::Boolean(value) => json!(value),
        ElicitationValue::StringArray(values) => json!(values),
        ElicitationValue::Unknown(value) => value.as_value().clone(),
    }
}

fn json_to_elicitation_value(value: &Value) -> ElicitationValue {
    match value {
        Value::String(value) => ElicitationValue::String(value.clone()),
        Value::Bool(value) => ElicitationValue::Boolean(*value),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                ElicitationValue::Integer(value)
            } else if let Some(value) = value.as_f64() {
                ElicitationValue::Number(value)
            } else {
                ElicitationValue::Unknown(RawJson::new(Value::Number(value.clone())))
            }
        }
        Value::Array(values) => ElicitationValue::StringArray(
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        ),
        _ => ElicitationValue::Unknown(RawJson::new(value.clone())),
    }
}

async fn attachments_to_prompt_files(
    attachments: &[fxg_protocol::common::AttachmentMeta],
) -> anyhow::Result<Vec<Value>> {
    let mut files = Vec::new();
    for attachment in attachments {
        if let Some(data) = attachment.data_b64.as_deref() {
            files.push(json!({
                "data": data,
                "mime": attachment.mime_type,
                "source": { "type": "inline" },
                "name": attachment.file_name,
            }));
            continue;
        }
        let Some(path) = attachment.local_path.as_deref() else {
            continue;
        };
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(err) => {
                debug!(%path, "failed to read the attachment; sending a URI reference: {err}");
                files.push(json!({
                    "data": "",
                    "mime": attachment.mime_type,
                    "source": { "type": "uri", "uri": file_uri(path) },
                    "name": attachment.file_name,
                }));
                continue;
            }
        };
        files.push(json!({
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            "mime": attachment.mime_type,
            "source": { "type": "inline" },
            "name": attachment.file_name,
        }));
    }
    Ok(files)
}

fn file_uri(path: &str) -> String {
    let path = Path::new(path);
    let normalized = path.to_string_lossy().replace('\\', "/");
    if normalized.starts_with('/') {
        format!("file://{normalized}")
    } else {
        format!("file:///{normalized}")
    }
}

fn urlencoded(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(char::from(byte));
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Driver implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl AgentDriver for OpenCodeNativeDriver {
    fn driver_kind(&self) -> DriverKind {
        DriverKind::Custom
    }

    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession> {
        let server = self.inner.ensure_server(&req.launch, &req.cwd).await?;
        let client = server.client.clone();

        let resume_id = req
            .resume
            .as_ref()
            .and_then(|resume| resume.agent_session_id.clone());
        let allow_fresh = req.resume.as_ref().is_some_and(|resume| resume.allow_fresh);
        let capture_history = req
            .resume
            .as_ref()
            .is_some_and(|resume| resume.capture_history);

        let (agent_session_id, context_restored) = if let Some(source) = req.fork_from.as_deref() {
            let id = client
                .fork_session(source, req.fork_before_message_id.as_deref())
                .await
                .map_err(|err| NativeForkUnavailable(err.to_string()))?;
            (id, true)
        } else if let Some(resume_id) = resume_id {
            if client.session_exists(&resume_id).await? {
                (resume_id, true)
            } else if allow_fresh {
                (
                    client
                        .create_session(
                            &req.cwd,
                            Some(&req.title.clone().unwrap_or_default()),
                            req.initial_mode.as_deref(),
                            initial_model(&req.initial_config).as_deref(),
                        )
                        .await?,
                    false,
                )
            } else {
                return Err(NativeResumeUnavailable(format!(
                    "OpenCode session not found: {resume_id}"
                ))
                .into());
            }
        } else if req.resume.is_some() && !allow_fresh {
            return Err(NativeResumeUnavailable(
                "no OpenCode agent session ID was recorded".to_owned(),
            )
            .into());
        } else {
            (
                client
                    .create_session(
                        &req.cwd,
                        req.title.as_deref(),
                        req.initial_mode.as_deref(),
                        initial_model(&req.initial_config).as_deref(),
                    )
                    .await?,
                false,
            )
        };

        let runtime = Arc::new(SessionRuntime {
            fxg_session_id: req.session_id.clone(),
            agent_session_id: agent_session_id.clone(),
            event_tx,
            state: std::sync::Mutex::new(RuntimeState::default()),
        });
        self.inner.register(Arc::clone(&runtime));

        if !context_restored {
            runtime.emit_event(UnifiedEventPayload::SessionAgentBound {
                agent_session_id: agent_session_id.clone(),
            });
        }
        if context_restored {
            if let Some(mode) = req.initial_mode.as_deref()
                && let Err(err) = client.switch_agent(&agent_session_id, mode).await
            {
                warn!("failed to apply the initial OpenCode agent: {err:#}");
            }
            if let Some(model) = initial_model(&req.initial_config)
                && let Some(model) = parse_model_ref(&model)
                && let Err(err) = client.switch_model(&agent_session_id, model).await
            {
                warn!("failed to apply the initial OpenCode model: {err:#}");
            }
        }

        match fetch_capabilities(&client, &agent_session_id).await {
            Ok((payload, commands, context_size)) => {
                {
                    let mut state = runtime.state.lock().expect("state poisoned");
                    state.commands = commands;
                    state.context_size = context_size;
                }
                runtime.emit_event(payload);
            }
            Err(err) => warn!("failed to load OpenCode capabilities: {err:#}"),
        }

        if capture_history {
            match client.messages(&agent_session_id, 1000).await {
                Ok(messages) => {
                    emit_history(&runtime, &messages);
                    // Flush the imported history before the manager returns
                    // (the event pump waits for this status boundary).
                    runtime.emit_event(UnifiedEventPayload::StatusChanged {
                        status: SessionStatus::Idle,
                        error_message: None,
                    });
                }
                Err(err) => {
                    return Err(HistoryCaptureUnavailable(format!(
                        "failed to load OpenCode history: {err:#}"
                    ))
                    .into());
                }
            }
        }

        Ok(StartedSession {
            handle: Box::new(OpenCodeSessionHandle {
                inner: Arc::clone(&self.inner),
                client,
                agent_session_id: agent_session_id.clone(),
                runtime,
            }),
            context_restored,
            agent_session_id: Some(agent_session_id),
        })
    }

    async fn list_sessions(
        &self,
        launch: &AgentLaunchSpec,
        cwd: &Path,
    ) -> anyhow::Result<AgentSessionList> {
        let server = self.inner.ensure_server(launch, cwd).await?;
        let sessions = server.client.list_sessions(cwd).await?;
        Ok(AgentSessionList {
            sessions,
            supports_history: true,
        })
    }

    async fn shutdown_idle(&self) -> usize {
        let empty = self
            .inner
            .runtimes
            .lock()
            .expect("runtimes poisoned")
            .is_empty();
        if !empty {
            return 0;
        }
        let server = self.inner.server.lock().await.take();
        if let Some(server) = server
            && server.is_owned()
        {
            server.stop().await;
            return 1;
        }
        0
    }

    fn stderr_tail(&self, _agent_id: &str) -> Vec<String> {
        self.inner
            .stderr
            .lock()
            .expect("stderr poisoned")
            .iter()
            .rev()
            .take(50)
            .cloned()
            .collect()
    }

    fn stderr_snapshot(&self) -> BTreeMap<String, Vec<String>> {
        BTreeMap::from([("opencode".to_owned(), self.stderr_tail("opencode"))])
    }
}

fn initial_model(initial_config: &[fxg_protocol::common::InitialConfig]) -> Option<String> {
    initial_config.iter().find_map(|config| {
        if config.key == "model" {
            config.value.select_value().map(str::to_owned)
        } else {
            None
        }
    })
}

fn emit_history(runtime: &SessionRuntime, messages: &[Value]) {
    for message in messages {
        let kind = message
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let message_id = message
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "user" => {
                if let Some(text) = message.get("text").and_then(Value::as_str) {
                    runtime.emit_event(UnifiedEventPayload::UserMessage {
                        text: text.to_owned(),
                        attachments: Vec::new(),
                        client_source: ClientSource::Import,
                        agent_message_id: (!message_id.is_empty()).then(|| message_id.to_owned()),
                    });
                }
            }
            "assistant" => {
                let Some(content) = message.get("content").and_then(Value::as_array) else {
                    continue;
                };
                for (index, part) in content.iter().enumerate() {
                    match part.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                runtime.emit_event(UnifiedEventPayload::AgentMessage {
                                    message_id: format!("{message_id}:text:{index}"),
                                    text: text.to_owned(),
                                    is_complete: true,
                                    subagent_id: None,
                                });
                            }
                        }
                        Some("reasoning") => {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                runtime.emit_event(UnifiedEventPayload::AgentThought {
                                    thought_id: format!("{message_id}:reasoning:{index}"),
                                    text: text.to_owned(),
                                    is_complete: true,
                                    subagent_id: None,
                                });
                            }
                        }
                        Some("tool") => {
                            let name = part
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_owned();
                            let state = part.get("state").cloned().unwrap_or(Value::Null);
                            let status = match state.get("status").and_then(Value::as_str) {
                                Some("completed") => ToolCallStatus::Completed,
                                Some("error") | Some("failed") => ToolCallStatus::Failed,
                                Some("running") => ToolCallStatus::InProgress,
                                _ => ToolCallStatus::Pending,
                            };
                            let input = state.get("input").cloned().unwrap_or(Value::Null);
                            runtime.emit_event(UnifiedEventPayload::ToolCall {
                                tool_call_id: part
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                title: tool_title(&name, &input),
                                kind: tool_kind(&name),
                                status,
                                locations: tool_locations(&input),
                                diff: tool_diff(&name, &input),
                                raw_output: tool_output(&state),
                                subagent_id: None,
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

struct OpenCodeSessionHandle {
    inner: Arc<DriverInner>,
    client: OpenCodeClient,
    agent_session_id: String,
    runtime: Arc<SessionRuntime>,
}

#[async_trait]
impl ActiveSessionHandle for OpenCodeSessionHandle {
    fn assigns_message_ids(&self) -> bool {
        true
    }

    fn supports_steering(&self) -> bool {
        true
    }

    async fn send_prompt(&self, input: PromptInput) -> anyhow::Result<()> {
        let delivery = self.runtime.is_busy().then_some("steer");
        let text = input.text.trim_start();
        if let Some(rest) = text.strip_prefix('/') {
            let name = rest.split_whitespace().next().unwrap_or_default();
            if name == COMPACT_COMMAND {
                self.client
                    .compact(&self.agent_session_id, delivery)
                    .await?;
                return Ok(());
            }
            let known = self
                .runtime
                .state
                .lock()
                .expect("state poisoned")
                .commands
                .iter()
                .any(|command| command == name);
            if known {
                self.runtime
                    .state
                    .lock()
                    .expect("state poisoned")
                    .pending_commands += 1;
                let arguments = rest[name.len()..].trim_start();
                self.client
                    .send_command(&self.agent_session_id, name, arguments, delivery)
                    .await?;
                return Ok(());
            }
        }
        if let Some(id) = input.agent_message_id.clone() {
            self.runtime
                .state
                .lock()
                .expect("state poisoned")
                .self_messages
                .insert(id);
        }
        self.client
            .send_prompt(&self.agent_session_id, &input, delivery)
            .await
    }

    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()> {
        self.client
            .reply_permission(&self.agent_session_id, &request_id, &selected_option_id)
            .await
    }

    async fn respond_elicitation(
        &self,
        elicitation_id: String,
        action: ElicitationAction,
        content: Option<ElicitationContent>,
    ) -> anyhow::Result<()> {
        match action {
            ElicitationAction::Accept => {
                let content = content.unwrap_or_default();
                self.client
                    .reply_form(&self.agent_session_id, &elicitation_id, &content)
                    .await
            }
            ElicitationAction::Decline | ElicitationAction::Cancel => {
                self.client
                    .cancel_form(&self.agent_session_id, &elicitation_id)
                    .await
            }
        }
    }

    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()> {
        self.client
            .switch_agent(&self.agent_session_id, &mode_id)
            .await
    }

    async fn set_config(&self, key: String, value: ConfigOptionValue) -> anyhow::Result<()> {
        if key != "model" {
            anyhow::bail!("OpenCode does not expose config option {key}");
        }
        let Some(value) = value.select_value() else {
            anyhow::bail!("model config value must be a select value");
        };
        let model =
            parse_model_ref(value).ok_or_else(|| anyhow!("invalid model value: {value}"))?;
        self.client
            .switch_model(&self.agent_session_id, model)
            .await
    }

    async fn revert_to_message(&self, agent_message_id: &str, files: bool) -> anyhow::Result<()> {
        self.client
            .revert(&self.agent_session_id, agent_message_id, files)
            .await
    }

    async fn compact_context(&self) -> anyhow::Result<()> {
        let delivery = self.runtime.is_busy().then_some("steer");
        self.client.compact(&self.agent_session_id, delivery).await
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        self.client.interrupt(&self.agent_session_id).await
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.inner.unregister(&self.agent_session_id);
        Ok(())
    }

    async fn dispose(&self) -> anyhow::Result<()> {
        self.shutdown().await
    }
}

// ---------------------------------------------------------------------------
// Backfill
// ---------------------------------------------------------------------------

async fn backfill_all(inner: &Arc<DriverInner>, server: &Arc<Server>) {
    let runtimes = inner
        .runtimes
        .lock()
        .expect("runtimes poisoned")
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for runtime in runtimes {
        let after = runtime
            .state
            .lock()
            .expect("state poisoned")
            .durable_seq
            .get(&runtime.agent_session_id)
            .copied()
            .unwrap_or(0);
        if let Err(err) = backfill_session(server, &runtime, after).await {
            debug!(
                session_id = %runtime.fxg_session_id,
                "OpenCode backfill failed: {err:#}"
            );
        }
    }
}

async fn backfill_session(
    server: &Arc<Server>,
    runtime: &Arc<SessionRuntime>,
    after: u64,
) -> anyhow::Result<()> {
    let path = format!(
        "/api/experimental/session/{}/log?after={after}",
        runtime.agent_session_id
    );
    let response = server
        .client
        .auth(server.client.sse_http.get(server.client.url(&path)))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("OpenCode session log returned {}", response.status());
    }
    let text = response.text().await?;
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
            continue;
        }
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        let events = map_event(runtime, kind, &data, None, &value);
        runtime.emit(events);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_server_url_reads_the_listen_line() {
        assert_eq!(
            parse_server_url("server listening on http://127.0.0.1:38219").as_deref(),
            Some("http://127.0.0.1:38219")
        );
        assert_eq!(parse_server_url("other output"), None);
    }

    #[test]
    fn model_values_round_trip() {
        let value = parse_model_ref("opencode-go/model#high").expect("model");
        assert_eq!(value["providerID"], "opencode-go");
        assert_eq!(value["id"], "model");
        assert_eq!(value["variant"], "high");
        assert_eq!(
            model_ref_value(&value)
                .and_then(|value| value.select_value().map(str::to_owned))
                .as_deref(),
            Some("opencode-go/model#high")
        );
    }

    #[test]
    fn tool_diff_is_built_for_edit_inputs() {
        let diff = tool_diff(
            "edit",
            &json!({
                "filePath": "src/main.rs",
                "oldString": "a\n",
                "newString": "b\n"
            }),
        )
        .expect("diff");
        assert_eq!(diff.path, "src/main.rs");
        assert_eq!(diff.additions, 1);
        assert_eq!(diff.deletions, 1);
    }

    #[test]
    fn form_fields_become_an_elicitation_schema() {
        let payload = form_to_elicitation(
            &json!({
                "form": {
                    "id": "frm_1",
                    "sessionID": "ses_1",
                    "title": "Pick one",
                    "fields": [
                        {"key": "choice", "type": "string", "title": "Choice", "required": true,
                         "options": [{"value": "a", "label": "A"}, {"value": "b", "label": "B"}]},
                        {"key": "flag", "type": "boolean", "title": "Flag"}
                    ]
                }
            }),
            None,
        )
        .expect("elicitation");
        let UnifiedEventPayload::ElicitationRequest {
            elicitation_id,
            requested_schema,
            ..
        } = payload
        else {
            panic!("expected an elicitation request");
        };
        assert_eq!(elicitation_id, "frm_1");
        assert_eq!(requested_schema.properties.len(), 2);
        assert_eq!(
            requested_schema.required.as_deref(),
            Some(&["choice".to_owned()][..])
        );
    }

    #[test]
    fn text_and_tool_events_map_to_normalized_payloads() {
        let runtime = test_runtime();
        let events = map_event(
            &runtime,
            "session.text.ended",
            &json!({"sessionID": "ses_1", "assistantMessageID": "msg_1", "text": "hello"}),
            None,
            &json!({"id": "evt_1"}),
        );
        assert!(matches!(
            events.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::AgentMessage { text, .. })] if text == "hello"
        ));

        let events = map_event(
            &runtime,
            "session.tool.input.started",
            &json!({"sessionID": "ses_1", "id": "call_1", "name": "edit"}),
            None,
            &json!({"id": "evt_2"}),
        );
        assert!(matches!(
            events.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::ToolCall { status, .. })]
                if *status == ToolCallStatus::Pending
        ));
    }

    fn test_runtime() -> SessionRuntime {
        let (tx, _rx) = mpsc::unbounded_channel();
        SessionRuntime {
            fxg_session_id: "fxg".to_owned(),
            agent_session_id: "ses_1".to_owned(),
            event_tx: tx,
            state: std::sync::Mutex::new(RuntimeState::default()),
        }
    }
}
