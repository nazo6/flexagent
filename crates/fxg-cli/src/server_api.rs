//! Central server REST API client (for the CLI).
//!
//! Operations that local IPC (a direct daemon connection) cannot provide —
//! starting an ephemeral VM provisioner (`fxg run --provisioner`) — call the
//! central server API named by `[node] central_server_url` in `config.toml`
//! directly (design: docs/05 §1.1; unavailable while the central server is
//! stopped).
//!
//! Authentication uses the Bearer token from `~/.flexagent/auth_token`. When
//! the CLI runs on the same host as the central server, the same file matches
//! the server token, so authentication works as-is.

use anyhow::{Context, Result, bail};
use fxg_protocol::client_api::{ApiErrorResponse, CreateSessionRequest, CreateSessionResponse};
use fxg_protocol::config::{AUTH_TOKEN_FILE_NAME, EnvLookup, GlobalConfig, fxg_home};

/// Central server REST client.
pub struct ServerClient {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl ServerClient {
    /// Build a client from `[node] central_server_url` and `auth_token` in
    /// `config.toml`.
    pub fn from_config(env: EnvLookup<'_>) -> Result<Self> {
        let global = GlobalConfig::load(env).context("failed to load ~/.flexagent/config.toml")?;
        let server_url = global.node.central_server_url.clone().context(
            "node.central_server_url is not set in config.toml (a central server is required)",
        )?;
        let base = central_http_url(&server_url)?;
        let token =
            fxg_server::api::auth::load_or_create_token(&fxg_home(env).join(AUTH_TOKEN_FILE_NAME))
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
            .with_context(|| format!("failed to reach central server ({})", self.base))?;
        decode(response).await
    }

    /// `POST /api/v1/sessions`
    pub async fn create_session(
        &self,
        request: &CreateSessionRequest,
    ) -> Result<CreateSessionResponse> {
        self.post("/api/v1/sessions", request).await
    }
}

/// Decode an HTTP response (unwraps the error body `{ error: { code, message } }`).
async fn decode<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let bytes = response.bytes().await.context("failed to read response")?;
    if !status.is_success() {
        if let Ok(error) = serde_json::from_slice::<ApiErrorResponse>(&bytes) {
            bail!(
                "central server error: {} ({})",
                error.error.message,
                error.error.code.as_str()
            );
        }
        bail!(
            "central server returned {status}: {}",
            String::from_utf8_lossy(&bytes).trim()
        );
    }
    serde_json::from_slice(&bytes).context("failed to decode central server response")
}

/// Extract the HTTP origin from a Node Hub WS URL (`ws://host:8080/api/v1/node/ws`).
pub fn central_http_url(server_url: &str) -> Result<String> {
    let http = server_url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    if !http.starts_with("http://") && !http.starts_with("https://") {
        bail!("central_server_url must use ws:// or wss://: {server_url}");
    }
    let base = http.split("/api/").next().unwrap_or(&http);
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        bail!("invalid central_server_url: {server_url}");
    }
    Ok(base.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn central_http_url_converts_ws_endpoint() {
        assert_eq!(
            central_http_url("ws://192.168.1.5:8080/api/v1/node/ws").expect("url"),
            "http://192.168.1.5:8080"
        );
        assert_eq!(
            central_http_url("wss://fxg.example.com/api/v1/node/ws").expect("url"),
            "https://fxg.example.com"
        );
        assert!(central_http_url("192.168.1.5:8080").is_err());
    }
}
