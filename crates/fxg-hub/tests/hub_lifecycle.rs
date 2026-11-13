//! Hub lifecycle tests.
//!
//! The requirement for `Hub::wait` is that it keeps running until shutdown is requested
//! (this guards against regressions that return without a request).

#![allow(
    clippy::disallowed_types,
    reason = "tests assert raw JSON responses; see docs/06-type-safety.md"
)]

use std::time::Duration;

use fxg_hub::Hub;
use fxg_hub::state::HubOptions;
use fxg_protocol::paths::{Paths, Role};

#[tokio::test(flavor = "multi_thread")]
async fn hub_keeps_running_until_shutdown_is_requested() {
    let home = tempfile::tempdir().expect("home");
    let hub = Hub::start(HubOptions {
        paths: Paths::portable(home.path(), Role::Hub),
        listen_addr: "127.0.0.1:0".to_owned(),
        allowed_hosts: Vec::new(),
        allowed_origins: Vec::new(),
        log: Default::default(),
    })
    .await
    .expect("start hub");
    let addr = hub.listen_addr();
    let token = fxg_hub::api::auth::load_or_create_token(
        &Paths::portable(home.path(), Role::Hub).auth_token_path(),
    )
    .expect("token");
    let http = reqwest::Client::new();

    let info_url = format!("http://{addr}/api/v1/system/info");
    let response = http
        .get(&info_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("system info");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let role: serde_json::Value = response.json().await.expect("json");
    assert_eq!(role["role"], "hub");

    // Even after calling `wait()`, the hub must keep running while no shutdown is requested.
    // (This task is discarded together with the runtime when the test ends.)
    let waiter = tokio::spawn(async move {
        hub.wait().await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!waiter.is_finished(), "wait() must not return early");
    let response = http
        .get(&info_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("hub must stay reachable");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
}
