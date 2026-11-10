//! Hub REST API client (for the CLI).
//!
//! Operations that local IPC (a direct daemon connection) cannot provide —
//! starting an ephemeral VM provisioner (`fxg session new --provisioner`) — call the
//! hub API named by `[node] hub_url` in `config.toml`
//! directly (design: docs/05 §1.1; unavailable while the hub is
//! stopped).
//!
//! Authentication uses the Bearer token from `~/.flexagent/auth_token`. When
//! the CLI runs on the same host as the hub, the same file matches
//! the hub token, so authentication works as-is.

use anyhow::{Context, Result, bail};
use fxg_protocol::client_api::{ApiErrorResponse, CreateSessionRequest, CreateSessionResponse};
use fxg_protocol::config::{AUTH_TOKEN_FILE_NAME, EnvLookup, GlobalConfig, fxg_home};

/// Hub REST client.
pub struct HubClient {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl HubClient {
    /// Build a client from `[node] hub_url` and `auth_token` in
    /// `config.toml`.
    pub fn from_config(env: EnvLookup<'_>) -> Result<Self> {
        let global = GlobalConfig::load(env).context("failed to load ~/.flexagent/config.toml")?;
        let server_url = global
            .node
            .hub_url
            .clone()
            .context("node.hub_url is not set in config.toml (a hub is required)")?;
        let base = hub_http_url(&server_url)?;
        let token =
            fxg_hub::api::auth::load_or_create_token(&fxg_home(env).join(AUTH_TOKEN_FILE_NAME))
                .context("failed to load ~/.flexagent/auth_token")?;
        Ok(Self {
            base,
            token,
            http: reqwest::Client::new(),
        })
    }

    /// Build a client for the hub reachable from this host.
    ///
    /// Prefers the address published by a running local hub (`hub.addr`), then
    /// `[node] hub_url`, then `[hub] listen_addr`.
    pub fn local(env: EnvLookup<'_>) -> Result<Self> {
        let base = local_hub_base(env)?;
        let token =
            fxg_hub::api::auth::load_or_create_token(&fxg_home(env).join(AUTH_TOKEN_FILE_NAME))
                .context("failed to load ~/.flexagent/auth_token")?;
        Ok(Self {
            base,
            token,
            http: reqwest::Client::new(),
        })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Send a POST request with a JSON body.
    async fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let response = self
            .http
            .post(self.endpoint(path))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("failed to reach hub ({})", self.base))?;
        decode(response).await
    }

    /// Send a GET request.
    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .get(self.endpoint(path))
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("failed to reach hub ({})", self.base))?;
        decode(response).await
    }

    /// `GET /api/v1/system/info`
    pub async fn system_info(&self) -> Result<fxg_protocol::client_api::SystemInfoResponse> {
        self.get("/api/v1/system/info").await
    }

    /// `GET /api/v1/nodes`
    pub async fn nodes(&self) -> Result<fxg_protocol::client_api::NodesResponse> {
        self.get("/api/v1/nodes").await
    }

    /// `POST /api/v1/system/shutdown`
    pub async fn shutdown(&self) -> Result<()> {
        let _: ShutdownResponse = self
            .post("/api/v1/system/shutdown", &serde_json::json!({}))
            .await?;
        Ok(())
    }

    /// `POST /api/v1/sessions`
    pub async fn create_session(
        &self,
        request: &CreateSessionRequest,
    ) -> Result<CreateSessionResponse> {
        self.post("/api/v1/sessions", request).await
    }
}

/// `POST /api/v1/system/shutdown` response.
#[derive(serde::Deserialize)]
struct ShutdownResponse {
    #[allow(dead_code)]
    ok: bool,
}

/// Decode an HTTP response (unwraps the error body `{ error: { code, message } }`).
async fn decode<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let bytes = response.bytes().await.context("failed to read response")?;
    if !status.is_success() {
        if let Ok(error) = serde_json::from_slice::<ApiErrorResponse>(&bytes) {
            bail!(
                "hub error: {} ({})",
                error.error.message,
                error.error.code.as_str()
            );
        }
        bail!(
            "hub returned {status}: {}",
            String::from_utf8_lossy(&bytes).trim()
        );
    }
    serde_json::from_slice(&bytes).context("failed to decode hub response")
}

/// Normalize a listen address for a loopback HTTP URL.
fn normalize_listen(addr: &str) -> String {
    match addr.rsplit_once(':') {
        Some((host, port)) if host.is_empty() || host == "0.0.0.0" || host == "[::]" => {
            format!("127.0.0.1:{port}")
        }
        _ => addr.to_owned(),
    }
}

/// Base URL of the hub reachable from this host.
///
/// Prefers the address published by a running local hub (`hub.addr`), then
/// `[node] hub_url`, then `[hub] listen_addr`.
pub fn local_hub_base(env: EnvLookup<'_>) -> Result<String> {
    let home = fxg_home(env);
    if let Ok(addr) = std::fs::read_to_string(home.join("hub.addr")) {
        let addr = addr.trim();
        if !addr.is_empty() {
            return Ok(format!("http://{}", normalize_listen(addr)));
        }
    }
    let global = GlobalConfig::load(env).context("failed to load ~/.flexagent/config.toml")?;
    if let Some(url) = global.node.hub_url.as_deref() {
        return hub_http_url(url);
    }
    Ok(format!(
        "http://{}",
        normalize_listen(global.hub.resolved_listen_addr())
    ))
}

/// Extract the HTTP origin from a Node Hub WS URL (`ws://host:8080/api/v1/node/ws`).
pub fn hub_http_url(server_url: &str) -> Result<String> {
    let http = server_url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    if !http.starts_with("http://") && !http.starts_with("https://") {
        bail!("hub_url must use ws:// or wss://: {server_url}");
    }
    let base = http.split("/api/").next().unwrap_or(&http);
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        bail!("invalid hub_url: {server_url}");
    }
    Ok(base.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_http_url_converts_ws_endpoint() {
        assert_eq!(
            hub_http_url("ws://192.168.1.5:8080/api/v1/node/ws").expect("url"),
            "http://192.168.1.5:8080"
        );
        assert_eq!(
            hub_http_url("wss://fxg.example.com/api/v1/node/ws").expect("url"),
            "https://fxg.example.com"
        );
        assert!(hub_http_url("192.168.1.5:8080").is_err());
    }
}
