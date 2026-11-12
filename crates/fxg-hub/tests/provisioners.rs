//! Provisioner CRUD (`/api/v1/provisioners`).

#![allow(
    clippy::disallowed_types,
    reason = "tests assert raw JSON responses; see docs/06-type-safety.md"
)]

use fxg_hub::Hub;
use fxg_hub::state::HubOptions;

async fn start_hub() -> (tempfile::TempDir, Hub) {
    let home = tempfile::tempdir().expect("home");
    let server = Hub::start(HubOptions {
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

fn auth_token(home: &tempfile::TempDir) -> String {
    std::fs::read_to_string(home.path().join("auth_token"))
        .expect("auth token")
        .trim()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn provisioner_crud_roundtrips() {
    let (home, server) = start_hub().await;
    let http = reqwest::Client::new();
    let token = auth_token(&home);
    let base = format!("http://{}/api/v1/provisioners", server.listen_addr());

    let empty: serde_json::Value = http
        .get(&base)
        .bearer_auth(&token)
        .send()
        .await
        .expect("list")
        .json()
        .await
        .expect("json");
    assert_eq!(empty["provisioners"], serde_json::json!([]));

    let definition = serde_json::json!({
        "name": "local-docker",
        "description": "Local Docker sandbox",
        "command": "docker",
        "args": ["run", "--rm", "-i", "fxg-sandbox:latest"],
        "idle_timeout_secs": 900,
        "env": { "LOG_LEVEL": "info" },
        "env_secrets": { "GH_TOKEN": "GH_TOKEN" }
    });
    let saved: serde_json::Value = http
        .put(format!("{base}/local-docker"))
        .bearer_auth(&token)
        .json(&definition)
        .send()
        .await
        .expect("save")
        .json()
        .await
        .expect("json");
    assert_eq!(saved["provisioners"][0]["name"], "local-docker");
    assert_eq!(
        saved["provisioners"][0]["env_secrets"]["GH_TOKEN"],
        "GH_TOKEN"
    );

    let listed: serde_json::Value = http
        .get(&base)
        .bearer_auth(&token)
        .send()
        .await
        .expect("list")
        .json()
        .await
        .expect("json");
    assert_eq!(listed["provisioners"][0]["command"], "docker");

    let response = http
        .delete(format!("{base}/local-docker"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("delete");
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

    let empty: serde_json::Value = http
        .get(&base)
        .bearer_auth(&token)
        .send()
        .await
        .expect("list")
        .json()
        .await
        .expect("json");
    assert_eq!(empty["provisioners"], serde_json::json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn provisioner_name_is_validated() {
    let (home, server) = start_hub().await;
    let http = reqwest::Client::new();
    let token = auth_token(&home);
    let response = http
        .put(format!(
            "http://{}/api/v1/provisioners/bad%20name",
            server.listen_addr()
        ))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "command": "docker" }))
        .send()
        .await
        .expect("save");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}
