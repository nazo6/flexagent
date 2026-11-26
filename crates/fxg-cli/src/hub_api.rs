//! Hub REST API client (for the CLI).
//!
//! Two resolutions are kept apart on purpose:
//!
//! - **Local hub**: `fxg hub ...` manages the hub process **on this host**.
//!   The base comes only from `<state>/hub/hub.addr`, never from the node's
//!   pairing (which may point at a remote hub).
//! - **Connected hub**: client operations that target the hub a node is paired
//!   with (`fxg session new --provisioner`). The base is resolved with
//!   `--hub` → `hub.addr` → the node's pairing → `hub.toml`.
//!
//! Authentication uses the Bearer token from the hub's
//! `<data>/hub/auth_token`. When the CLI runs on the same host as the
//! hub, that file matches the hub token, so authentication works as-is.

use anyhow::{Context, Result, bail};
use fxg_protocol::client_api::{
    ApiErrorResponse, CreateSessionRequest, CreateSessionResponse, RotateAuthTokenResponse,
};
use fxg_protocol::config::{EnvLookup, HubConfig};
use fxg_protocol::paths::{Paths, Role};

/// Hub REST client.
pub struct HubClient {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl HubClient {
    /// Build a client for the hub process **on this host**.
    ///
    /// `fxg hub ...` commands use this. The address comes from
    /// `<state>/hub/hub.addr`; the node's pairing is deliberately ignored.
    /// The token is read from the same host without creating one (a running
    /// hub always wrote it at startup).
    pub async fn local(env: EnvLookup<'_>) -> Result<Self> {
        let base = local_hub_base(env)?;
        let token = fxg_api::auth::load_token(&Paths::resolve(env, Role::Hub).auth_token_path())
            .context("failed to load the hub's auth_token")?;
        Ok(Self::new(base, token))
    }

    /// Build a client for the hub a client operation explicitly targets
    /// (for example `fxg session new --provisioner`).
    ///
    /// Resolution: `hub` → `hub.addr` → the local node's pairing →
    /// `hub.toml` `listen_addr`.
    pub async fn connected(env: EnvLookup<'_>, hub: Option<&str>) -> Result<Self> {
        let base = resolve_connected_hub_base(env, hub).await?;
        let token =
            fxg_api::auth::load_or_create_token(&Paths::resolve(env, Role::Hub).auth_token_path())
                .context("failed to load the hub's auth_token (run the CLI on the hub host)")?;
        Ok(Self::new(base, token))
    }

    fn new(base: String, token: String) -> Self {
        Self {
            base,
            token,
            http: reqwest::Client::new(),
        }
    }

    /// Base URL of the connected hub (`http://host:port`).
    pub fn base(&self) -> &str {
        &self.base
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

    /// `POST /api/v1/auth/rotate-token`
    ///
    /// The hub saves the new token and updates its in-memory copy, so later
    /// requests from this client use it too.
    pub async fn rotate_auth_token(&mut self) -> Result<String> {
        let response: RotateAuthTokenResponse = self
            .post("/api/v1/auth/rotate-token", &serde_json::json!({}))
            .await?;
        self.token = response.token.clone();
        Ok(response.token)
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
pub(crate) fn normalize_listen(addr: &str) -> String {
    match addr.rsplit_once(':') {
        Some((host, port)) if host.is_empty() || host == "0.0.0.0" || host == "[::]" => {
            format!("127.0.0.1:{port}")
        }
        _ => addr.to_owned(),
    }
}

/// Base URL of the hub process on this host.
///
/// `hub.addr` is written when the hub starts and removed when it shuts down, so
/// its presence is the liveness signal for local `fxg hub ...` commands.
pub fn local_hub_base(env: EnvLookup<'_>) -> Result<String> {
    match read_local_hub_addr(env) {
        Some(addr) => Ok(format!("http://{addr}")),
        None => bail!("the local hub is not running; start it with `fxg hub start --detach`"),
    }
}

/// Reads the bound address of the local hub (`<state>/hub/hub.addr`).
///
/// Returns `None` when the hub is not running or the file is empty.
pub fn read_local_hub_addr(env: EnvLookup<'_>) -> Option<String> {
    let addr_path = Paths::resolve(env, Role::Hub).addr_path();
    let raw = match std::fs::read_to_string(addr_path) {
        Ok(raw) => raw,
        Err(err) => {
            tracing::debug!(error = %err, "failed to read hub.addr");
            return None;
        }
    };
    let addr = raw.trim();
    if addr.is_empty() {
        return None;
    }
    Some(normalize_listen(addr))
}

/// Base URL the local hub is configured to listen on (`hub.toml`).
///
/// Used by `fxg hub status` when the hub is stopped; it is not a connection
/// target (the hub's presence is detected through `hub.addr`).
pub fn configured_hub_base(env: EnvLookup<'_>) -> Result<String> {
    let config = HubConfig::load(env).context("failed to load hub.toml")?;
    Ok(format!(
        "http://{}",
        normalize_listen(config.resolved_listen_addr())
    ))
}

/// Base URL of the hub a client operation targets (possibly remote).
///
/// Resolution order:
/// 1. `hub` (a `--hub` CLI override)
/// 2. `hub.addr` (a hub running on this host)
/// 3. the local node's pairing (`node.db`, read through the client API)
/// 4. `hub.toml` `listen_addr`
///
/// This is for hub-targeting client operations such as
/// `fxg session new --provisioner`; `fxg hub ...` process commands must use
/// [`local_hub_base`] instead.
pub async fn resolve_connected_hub_base(env: EnvLookup<'_>, hub: Option<&str>) -> Result<String> {
    if let Some(url) = hub.map(str::trim).filter(|url| !url.is_empty()) {
        return hub_http_url(url);
    }
    if let Ok(addr) = std::fs::read_to_string(Paths::resolve(env, Role::Hub).addr_path()) {
        let addr = addr.trim();
        if !addr.is_empty() {
            return Ok(format!("http://{}", normalize_listen(addr)));
        }
    }
    // The node owns the pairing, so ask it instead of re-reading files.
    // Hub commands never start a node; a missing node falls through to `hub.toml`.
    if let Ok(mut node) = crate::client::NodeClient::connect().await
        && let Ok(snapshot) = node.node_settings().await
        && let Some(url) = snapshot.pairing.hub_url
    {
        return Ok(url);
    }
    let config = HubConfig::load(env).context("failed to load hub.toml")?;
    Ok(format!(
        "http://{}",
        normalize_listen(config.resolved_listen_addr())
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

    fn portable_env(home: &std::path::Path) -> impl Fn(&str) -> Option<String> + '_ {
        move |key: &str| {
            (key == fxg_protocol::paths::ENV_FXG_HOME).then(|| home.display().to_string())
        }
    }

    #[test]
    fn local_hub_base_uses_only_hub_addr() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let env = portable_env(&home);
        let paths = Paths::portable(&home, Role::Hub);

        assert!(read_local_hub_addr(&env).is_none());
        assert!(local_hub_base(&env).is_err());

        std::fs::create_dir_all(paths.state_dir()).expect("mkdir state");
        std::fs::write(paths.addr_path(), "0.0.0.0:8080\n").expect("write addr");

        assert_eq!(read_local_hub_addr(&env).as_deref(), Some("127.0.0.1:8080"));
        assert_eq!(
            local_hub_base(&env).expect("local base"),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn normalize_listen_replaces_wildcard_hosts_with_loopback() {
        assert_eq!(normalize_listen("127.0.0.1:7860"), "127.0.0.1:7860");
        assert_eq!(normalize_listen("0.0.0.0:7860"), "127.0.0.1:7860");
        assert_eq!(normalize_listen("[::]:7860"), "127.0.0.1:7860");
        assert_eq!(normalize_listen(":7860"), "127.0.0.1:7860");
    }

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
