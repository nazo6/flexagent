//! `GET /api/v1/meta` (認証不要メタ情報) のテスト。
//!
//! 未認証の Web UI がトークン入力ダイアログを正しく表示できるよう、`meta` は
//! トークン無しで `role` を返し、Host 検証だけは維持されること。他の API は
//! 従来どおり未認証で `401 Unauthorized` となることを検証する。

use fxg_server::Server;
use fxg_server::state::ServerOptions;

async fn start_server() -> (tempfile::TempDir, Server) {
    let home = tempfile::tempdir().expect("home");
    let server = Server::start(ServerOptions {
        fxg_home: home.path().to_path_buf(),
        listen_addr: "127.0.0.1:0".to_owned(),
        allowed_hosts: Vec::new(),
        allowed_origins: Vec::new(),
        provisioners: Default::default(),
        git_credentials: Default::default(),
    })
    .await
    .expect("start server");
    (home, server)
}

#[tokio::test(flavor = "multi_thread")]
async fn meta_is_available_without_token() {
    let (_home, server) = start_server().await;
    let http = reqwest::Client::new();

    let response = http
        .get(format!("http://{}/api/v1/meta", server.listen_addr()))
        .send()
        .await
        .expect("meta");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("json");
    assert_eq!(body["role"], "central_server");
    assert_eq!(
        body.as_object().expect("object").len(),
        1,
        "role のみを返す"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn meta_still_validates_host_header() {
    let (_home, server) = start_server().await;
    let http = reqwest::Client::new();

    let response = http
        .get(format!("http://{}/api/v1/meta", server.listen_addr()))
        .header(reqwest::header::HOST, "evil.example:8080")
        .send()
        .await
        .expect("meta");
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread")]
async fn other_endpoints_still_require_token() {
    let (_home, server) = start_server().await;
    let http = reqwest::Client::new();

    let response = http
        .get(format!(
            "http://{}/api/v1/system/info",
            server.listen_addr()
        ))
        .send()
        .await
        .expect("system info");
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}
