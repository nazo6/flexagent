//! 中央サーバー REST API クライアント (CLI 用)。
//!
//! ローカルIPC (デーモン直結) では提供できない操作 — 一時VMプロビジョナーの
//! 起動 (`fxg run --provisioner`) と疎通検証 (`fxg provisioners test`) — は、
//! `config.toml` の `[node] central_server_url` が指す中央サーバーの API を
//! 直接呼び出す (設計: docs/05 §1.1。中央サーバー停止中は利用不可)。
//!
//! 認証は `~/.flexagent/auth_token` の Bearer トークンを用いる。CLI を
//! 中央サーバーと同一ホストで実行する場合は同一ファイルがサーバーのトークン
//! と一致するため、そのまま認証できる。

use anyhow::{Context, Result, bail};
use fxg_protocol::client_api::{
    ApiErrorResponse, CreateSessionRequest, CreateSessionResponse, ProvisionerTestResponse,
    SessionListResponse,
};
use fxg_protocol::common::SessionSummary;
use fxg_protocol::config::{AUTH_TOKEN_FILE_NAME, EnvLookup, GlobalConfig, fxg_home};

/// 中央サーバー REST クライアント。
pub struct ServerClient {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl ServerClient {
    /// `config.toml` の `[node] central_server_url` と `auth_token` から構築する。
    pub fn from_config(env: EnvLookup<'_>) -> Result<Self> {
        let global = GlobalConfig::load(env).context("failed to load ~/.flexagent/config.toml")?;
        let server_url = global.node.central_server_url.clone().context(
            "node.central_server_url が config.toml に設定されていません (中央サーバー必須)",
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

    /// HTTP オリジン (`http://host:8080`)。
    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// GET リクエストを送り、JSON レスポンスをデコードする。
    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .get(self.endpoint(path))
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("failed to reach central server ({})", self.base))?;
        decode(response).await
    }

    /// POST リクエスト (JSON ボディ付き) を送る。
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

    /// `POST /api/v1/provisioners/:name/test`
    pub async fn provisioner_test(&self, name: &str) -> Result<ProvisionerTestResponse> {
        self.post(&format!("/api/v1/provisioners/{name}/test"), &())
            .await
    }

    /// `GET /api/v1/sessions`
    pub async fn sessions(&self) -> Result<Vec<SessionSummary>> {
        let response: SessionListResponse = self.get("/api/v1/sessions").await?;
        Ok(response.sessions)
    }

    /// `POST /api/v1/sessions`
    pub async fn create_session(
        &self,
        request: &CreateSessionRequest,
    ) -> Result<CreateSessionResponse> {
        self.post("/api/v1/sessions", request).await
    }
}

/// HTTP レスポンスをデコードする (エラーボディ `{ error: { code, message } }` を展開)。
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

/// Node Hub の WS URL (`ws://host:8080/api/v1/node/ws`) から HTTP オリジンを取り出す。
pub fn central_http_url(server_url: &str) -> Result<String> {
    let http = server_url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    if !http.starts_with("http://") && !http.starts_with("https://") {
        bail!("central_server_url は ws:// または wss:// で指定してください: {server_url}");
    }
    let base = http.split("/api/").next().unwrap_or(&http);
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        bail!("central_server_url が不正です: {server_url}");
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
