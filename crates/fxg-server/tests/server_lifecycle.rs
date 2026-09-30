//! 中央サーバーのライフサイクルテスト。
//!
//! `Server::wait` は「シャットダウン要求があるまで稼働し続ける」ことが
//! 要件である (要求なしに return してしまう回帰を防ぐ)。

use std::time::Duration;

use fxg_server::Server;
use fxg_server::state::ServerOptions;

#[tokio::test(flavor = "multi_thread")]
async fn server_keeps_running_until_shutdown_is_requested() {
    let home = tempfile::tempdir().expect("home");
    let server = Server::start(ServerOptions {
        fxg_home: home.path().to_path_buf(),
        listen_addr: "127.0.0.1:0".to_owned(),
        allowed_hosts: Vec::new(),
        allowed_origins: Vec::new(),
        provisioners: Default::default(),
    })
    .await
    .expect("start server");
    let addr = server.listen_addr();
    let token = fxg_server::api::auth::load_or_create_token(
        &home.path().join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME),
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
    assert_eq!(role["role"], "central_server");

    // `wait()` を呼んでもシャットダウン要求が無いうちは稼働し続けること。
    // (このタスクはテスト終了時にランタイムごと破棄される)
    let waiter = tokio::spawn(async move {
        server.wait().await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!waiter.is_finished(), "wait() must not return early");
    let response = http
        .get(&info_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("server must stay reachable");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
}
