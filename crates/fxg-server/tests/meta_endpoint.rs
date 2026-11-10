//! Tests for `GET /api/v1/meta` (unauthenticated metadata).
//!
//! `meta` must return `role` without a token so an unauthenticated Web UI can show the token
//! input dialog, while Host validation still applies. Other APIs must still return
//! `401 Unauthorized` when unauthenticated.

#![allow(
    clippy::disallowed_types,
    reason = "tests assert raw JSON responses; see docs/06-type-safety.md"
)]

use fxg_server::Server;
use fxg_server::state::ServerOptions;

async fn start_server() -> (tempfile::TempDir, Server) {
    let home = tempfile::tempdir().expect("home");
    let server = Server::start(ServerOptions {
        fxg_home: home.path().to_path_buf(),
        listen_addr: "127.0.0.1:0".to_owned(),
        allowed_hosts: Vec::new(),
        allowed_origins: Vec::new(),
        log: Default::default(),
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
        "returns role only"
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

fn auth_token(home: &tempfile::TempDir) -> String {
    std::fs::read_to_string(home.path().join("auth_token"))
        .expect("auth token")
        .trim()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn diagnostics_expose_health_and_persisted_incidents() {
    let (home, server) = start_server().await;
    let db = fxg_db::Db::open(&fxg_db::hub_db_path(home.path()), fxg_db::DbRole::Hub)
        .await
        .expect("db");
    db.record_incident(
        &fxg_db::IncidentRecord::new(
            fxg_protocol::common::IncidentLevel::Error,
            fxg_protocol::common::ErrorCode::GitFailed,
            fxg_protocol::common::IncidentSource::Git,
            "git diff failed",
        )
        .with_node_id(Some("node-1")),
    )
    .await
    .expect("incident");

    let http = reqwest::Client::new();
    let token = auth_token(&home);
    let diagnostics: serde_json::Value = http
        .get(format!(
            "http://{}/api/v1/diagnostics",
            server.listen_addr()
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("diagnostics")
        .json()
        .await
        .expect("json");
    assert!(diagnostics["health"]["components"].is_array());
    assert_eq!(diagnostics["incidents"][0]["code"], "GIT_FAILED");

    let incidents: serde_json::Value = http
        .get(format!(
            "http://{}/api/v1/diagnostics/incidents",
            server.listen_addr()
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("incidents")
        .json()
        .await
        .expect("json");
    assert_eq!(incidents["retention_days"], 14);
    assert_eq!(incidents["incidents"][0]["summary"], "git diff failed");
}

#[tokio::test(flavor = "multi_thread")]
async fn support_bundle_is_redacted() {
    let (home, server) = start_server().await;
    let http = reqwest::Client::new();
    let token = auth_token(&home);
    let response = http
        .get(format!(
            "http://{}/api/v1/diagnostics/bundle",
            server.listen_addr()
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("bundle");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let text = response.text().await.expect("body");
    assert!(!text.contains(&token), "the auth token must not leak");
    let bundle: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(bundle["config"]["fxg_home"], "~");
    assert!(
        bundle["database"]["path"]
            .as_str()
            .expect("path")
            .contains('~')
    );
}
