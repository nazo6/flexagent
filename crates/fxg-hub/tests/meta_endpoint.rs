//! Tests for `GET /api/v1/meta` (unauthenticated metadata).
//!
//! `meta` must return `role` without a token so an unauthenticated Web UI can show the token
//! input dialog, while Host validation still applies. Other APIs must still return
//! `401 Unauthorized` when unauthenticated.

#![allow(
    clippy::disallowed_types,
    reason = "tests assert raw JSON responses; see docs/06-type-safety.md"
)]

use fxg_hub::Hub;
use fxg_hub::state::HubOptions;
use fxg_protocol::paths::{Paths, Role};

async fn start_hub() -> (tempfile::TempDir, Hub) {
    let home = tempfile::tempdir().expect("home");
    let server = Hub::start(HubOptions {
        paths: Paths::portable(home.path(), Role::Hub),
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
    let (_home, server) = start_hub().await;
    let http = reqwest::Client::new();

    let response = http
        .get(format!("http://{}/api/v1/meta", server.listen_addr()))
        .send()
        .await
        .expect("meta");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("json");
    assert_eq!(body["role"], "hub");
    assert_eq!(
        body.as_object().expect("object").len(),
        1,
        "returns role only"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn meta_still_validates_host_header() {
    let (_home, server) = start_hub().await;
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
    let (_home, server) = start_hub().await;
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
    std::fs::read_to_string(Paths::portable(home.path(), Role::Hub).auth_token_path())
        .expect("auth token")
        .trim()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn rotate_token_sets_the_new_session_cookie() {
    let (home, server) = start_hub().await;
    let http = reqwest::Client::new();
    let old_token = auth_token(&home);

    let response = http
        .post(format!(
            "http://{}/api/v1/auth/rotate-token",
            server.listen_addr()
        ))
        .bearer_auth(&old_token)
        .send()
        .await
        .expect("rotate");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let cookie = match response.headers().get(reqwest::header::SET_COOKIE) {
        Some(value) => match value.to_str() {
            Ok(value) => value.to_owned(),
            Err(err) => panic!("Set-Cookie is not ASCII: {err}"),
        },
        None => panic!("Set-Cookie is missing"),
    };
    let body: serde_json::Value = response.json().await.expect("json");
    let new_token = body["token"].as_str().expect("token");
    assert!(cookie.contains(&format!("fxg_session={new_token}")));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));

    let old_response = http
        .get(format!(
            "http://{}/api/v1/system/info",
            server.listen_addr()
        ))
        .bearer_auth(&old_token)
        .send()
        .await
        .expect("old token");
    assert_eq!(old_response.status(), reqwest::StatusCode::UNAUTHORIZED);

    let new_response = http
        .get(format!(
            "http://{}/api/v1/system/info",
            server.listen_addr()
        ))
        .bearer_auth(new_token)
        .send()
        .await
        .expect("new token");
    assert_eq!(new_response.status(), reqwest::StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn diagnostics_expose_health_and_persisted_incidents() {
    let (home, server) = start_hub().await;
    let db = fxg_db::Db::open(
        &Paths::portable(home.path(), Role::Hub).db_path(),
        Role::Hub,
    )
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
    let (home, server) = start_hub().await;
    let paths = Paths::portable(home.path(), Role::Hub);
    let user_home = dirs::home_dir().expect("user home");
    let secret_path = user_home.join("secret.txt");
    let log_path = paths.log_path();
    std::fs::create_dir_all(log_path.parent().expect("log parent")).expect("log dir");
    std::fs::write(&log_path, format!("path={}\n", secret_path.display())).expect("write log");

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
    let secret_text = secret_path.to_string_lossy();
    assert!(
        !text.contains(secret_text.as_ref()),
        "the user home must be redacted"
    );
    let bundle: serde_json::Value = serde_json::from_str(&text).expect("json");
    let data_dir = paths.data_dir().to_string_lossy().to_string();
    assert_eq!(
        bundle["config"]["data_dir"].as_str(),
        Some(data_dir.as_str())
    );
    assert!(
        bundle["database"]["path"]
            .as_str()
            .expect("path")
            .contains(&data_dir)
    );
    let redacted_path = secret_path
        .strip_prefix(&user_home)
        .map(|rest| std::path::Path::new("~").join(rest))
        .unwrap_or_else(|_| secret_path.clone());
    let redacted_text = redacted_path.to_string_lossy();
    assert!(
        bundle["logs"]
            .as_array()
            .expect("logs")
            .iter()
            .any(|line| line
                .as_str()
                .is_some_and(|line| line.contains(redacted_text.as_ref()))),
        "the user home in logs must be redacted: {}",
        bundle["logs"]
    );
}
