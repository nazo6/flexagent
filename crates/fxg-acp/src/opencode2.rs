//! OpenCode2 ドライバ (`opencode2 serve` ブリッジ + 純正TUI Attach)。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §3 モードA。
//!
//! - **サーバー起動**: セッションごとに
//!   `opencode2 serve --hostname 127.0.0.1 --port <free>` を起動し、ランダムな
//!   `OPENCODE_SERVER_PASSWORD` を注入する。以降の HTTP / SSE は Basic 認証
//!   (`opencode:<password>`) で接続する。
//! - **イベント**: SSE (`GET /api/event`) を購読し、`session.*` / `permission.*`
//!   を正規化イベントへ変換する。`*.delta` はメモリ配信のみ
//!   ([`StreamDeltaPayload`])、`*.ended` / `tool.*` / `permission.*` は永続化対象
//!   ([`UnifiedEventPayload`])。
//! - **純正TUI Attach**: サーバーはデーモンが管理し続けるため、CLI は
//!   `opencode2 run --server <url> --session <id>` で 100% 純正の TUI を表示できる
//!   ([`ActiveSessionHandle::native_attach`])。
//! - **Revert**: ネイティブ API
//!   (`POST /api/session/{id}/revert/stage` + `/revert/commit`) で会話のみを
//!   巻き戻す (`files: false`。ファイル復元は Shadow Git Tree が担当)。
//! - **Resume**: 再開指定時は `GET /api/session/{id}` で存在確認し、既存
//!   セッションへ bind する (404 の場合は `allow_fresh` に従い新規作成 +
//!   履歴 Replay へフォールバック。`allow_fresh = false` のネイティブ限定
//!   再開では [`NativeResumeUnavailable`] を返す。設計: docs/04 §4.3)。
//! - **承認**: SSE の `permission.asked` を [`UnifiedEventPayload::PermissionRequest`]
//!   へ変換し、`POST /api/session/{id}/permission/{request_id}/reply` で応答する。
//!   純正TUI から応答された場合も `permission.replied` として記録される。

use std::collections::HashMap;
use std::net::TcpListener;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use base64::Engine;
use futures_util::StreamExt;
use fxg_protocol::common::{
    ConfigOptionInfo, ModeInfo, PermissionOption, SessionStatus, StreamDeltaPayload,
};
use fxg_protocol::events::UnifiedEventPayload;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::driver::{
    ActiveSessionHandle, AgentDriver, DriverEvent, NativeAttachInfo, NativeResumeUnavailable,
    StartSessionRequest, StartedSession,
};

/// `opencode2 serve` が準備完了するまでの最大待機時間。
const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(30);
/// 準備完了ポーリングの間隔。
const READY_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// 起動直後は `/api/agent` / `/api/model` が空を返すため、カタログが
/// 揃うまで再取得する上限時間。
const CAPABILITIES_READY_TIMEOUT: Duration = Duration::from_secs(10);
/// カタログ再取得の間隔。
const CAPABILITIES_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// HTTP リクエストのタイムアウト。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// SSE (`GET /api/event`) の接続確立タイムアウト。
///
/// SSE は長時間接続のため、リクエスト全体のタイムアウト ([`REQUEST_TIMEOUT`]) を
/// 適用してはならない (適用すると一定時間でストリームが強制切断され、
/// 切断中のイベントを恒久的に取りこぼす)。
const SSE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// SSE の再接続回数上限 (サーバー停止時は諦める)。
const SSE_RECONNECT_LIMIT: u32 = 3;
/// SSE 再接続の待機時間。
const SSE_RECONNECT_DELAY: Duration = Duration::from_millis(500);
/// Basic 認証のユーザー名 (opencode2 はパスワードのみを検証する)。
const AUTH_USER: &str = "opencode";
/// 純正TUI Attach 時に CLI へ注入するパスワード環境変数名。
pub const PASSWORD_ENV: &str = "OPENCODE_PASSWORD";

/// `opencode2 serve` ブリッジドライバ。
#[derive(Debug, Default)]
pub struct OpenCode2Driver;

impl OpenCode2Driver {
    /// ドライバを作成する。
    pub fn new() -> Self {
        Self
    }
}

/// サーバープロセス管理タスクへのコマンド。
#[derive(Debug)]
enum ServerCommand {
    /// サーバー (プロセスツリーごと) を終了する
    Shutdown,
}

/// ハンドルとイベントストリームで共有するセッション状態。
#[derive(Debug, Default)]
struct SessionState {
    /// `fxg` が送信したプロンプトの inboxID (`msg_...`)。
    ///
    /// - Revert 時のターン対応付け (`keep_turns` 番目 = 最初に削除するメッセージ)
    /// - 純正TUI 由来プロンプトの判定 (自己送信分は `SessionManager` が
    ///   `UserMessage` を記録済みのため、再記録しない)
    prompt_ids: Vec<String>,
    /// `tool_call_id` → ツール名 (`session.tool.called` に名前が含まれないため、
    /// `session.tool.input.started` から引き継ぐ)
    tool_names: HashMap<String, String>,
}

/// 起動済み OpenCode2 セッションの操作ハンドル。
struct OpenCode2SessionHandle {
    http: reqwest::Client,
    /// `opencode2 serve` のベースURL (例: `http://127.0.0.1:38219`)
    base_url: String,
    /// サーバーパスワード (Basic 認証)
    password: String,
    /// OpenCode2 側のセッションID (`ses_...`)
    opencode_session_id: String,
    /// 共有セッション状態
    state: Arc<Mutex<SessionState>>,
    /// サーバープロセス管理タスクへのコマンド
    commands: mpsc::UnboundedSender<ServerCommand>,
    /// サーバープロセスのプロセスツリーガード (Windows: Job Object)。
    /// ハンドルが破棄されると孫プロセスを含めて確実に終了する。
    _process_guard: Arc<ServerProcessGuard>,
}

impl std::fmt::Debug for OpenCode2SessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenCode2SessionHandle")
            .field("base_url", &self.base_url)
            .field("opencode_session_id", &self.opencode_session_id)
            .finish()
    }
}

impl OpenCode2SessionHandle {
    /// JSON API を呼び出して応答 JSON を返す。
    async fn request_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .basic_auth(AUTH_USER, Some(self.password.clone()));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            anyhow::bail!("opencode2 api {path} failed ({status}): {message}");
        }
        // 204 No Content 等の空ボディは Null として扱う
        let text = response.text().await.unwrap_or_default();
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }
}

#[async_trait]
impl ActiveSessionHandle for OpenCode2SessionHandle {
    async fn send_prompt(&self, text: String) -> anyhow::Result<()> {
        let response = self
            .request_json(
                reqwest::Method::POST,
                &format!("/api/session/{}/prompt", self.opencode_session_id),
                Some(json!({ "text": text })),
            )
            .await?;
        if let Some(inbox_id) = response.pointer("/data/id").and_then(Value::as_str) {
            self.state
                .lock()
                .expect("state poisoned")
                .prompt_ids
                .push(inbox_id.to_owned());
        }
        Ok(())
    }

    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()> {
        // `PermissionOption::option_id` は opencode2 の `decision` 値そのもの
        // (`once` / `always` / `reject`) を用いる。
        if !matches!(selected_option_id.as_str(), "once" | "always" | "reject") {
            anyhow::bail!("invalid opencode2 permission decision: {selected_option_id}");
        }
        self.request_json(
            reqwest::Method::POST,
            &format!(
                "/api/session/{}/permission/{request_id}/reply",
                self.opencode_session_id
            ),
            Some(json!({ "decision": selected_option_id })),
        )
        .await?;
        Ok(())
    }

    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()> {
        // opencode2 の "agent" (build / plan 等) が ACP の「モード」に相当する
        self.request_json(
            reqwest::Method::POST,
            &format!("/api/session/{}/agent", self.opencode_session_id),
            Some(json!({ "agent": mode_id })),
        )
        .await?;
        Ok(())
    }

    async fn set_config(&self, key: String, value: Value) -> anyhow::Result<()> {
        match key.as_str() {
            // `CapabilitiesUpdated.config_options` の `model` キー
            // (`{"providerID": ..., "id": ...}`)
            "model" => {
                if value.get("providerID").is_none() || value.get("id").is_none() {
                    anyhow::bail!("model config requires providerID and id: {value}");
                }
                self.request_json(
                    reqwest::Method::POST,
                    &format!("/api/session/{}/model", self.opencode_session_id),
                    Some(json!({ "model": value })),
                )
                .await?;
                Ok(())
            }
            other => anyhow::bail!("unsupported opencode2 config key: {other}"),
        }
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        self.request_json(
            reqwest::Method::POST,
            &format!("/api/session/{}/interrupt", self.opencode_session_id),
            None,
        )
        .await?;
        Ok(())
    }

    async fn revert_context(&self, keep_turns: u64) -> anyhow::Result<()> {
        let keep = keep_turns as usize;
        let target = {
            let state = self.state.lock().expect("state poisoned");
            if keep >= state.prompt_ids.len() {
                // 巻き戻す必要がない (全ターンが保持対象)
                return Ok(());
            }
            state.prompt_ids[keep].clone()
        };
        // `files: false` で会話のみを巻き戻す (ファイル復元は Shadow Git Tree が担当)
        self.request_json(
            reqwest::Method::POST,
            &format!("/api/session/{}/revert/stage", self.opencode_session_id),
            Some(json!({ "messageID": target, "files": false })),
        )
        .await?;
        self.request_json(
            reqwest::Method::POST,
            &format!("/api/session/{}/revert/commit", self.opencode_session_id),
            None,
        )
        .await?;
        self.state
            .lock()
            .expect("state poisoned")
            .prompt_ids
            .truncate(keep);
        Ok(())
    }

    fn native_attach(&self) -> Option<NativeAttachInfo> {
        Some(NativeAttachInfo {
            server_url: self.base_url.clone(),
            session_id: self.opencode_session_id.clone(),
            env: vec![(PASSWORD_ENV.to_owned(), self.password.clone())],
        })
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        let _ = self.commands.send(ServerCommand::Shutdown);
        Ok(())
    }
}

#[async_trait]
impl AgentDriver for OpenCode2Driver {
    fn driver_kind(&self) -> &'static str {
        "opencode2"
    }

    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession> {
        // 再開指定 (Some の場合は既存セッションへ bind を試みる)
        let resume_requested = req.resume.is_some();
        let resume_session_id = req
            .resume
            .as_ref()
            .and_then(|resume| resume.agent_session_id.clone());
        let allow_fresh = req.resume.as_ref().is_some_and(|resume| resume.allow_fresh);

        // `opencode2.cmd` 等の拡張子解決 (Windows の PATHEXT 対応)
        let program = fxg_pty::resolve_command(&req.launch.program.to_string_lossy(), &req.cwd)
            .map_err(|err| {
                anyhow!(
                    "failed to resolve opencode2 program {}: {err}",
                    req.launch.program.display()
                )
            })?;
        let port = free_port()?;
        let password = random_password();
        let base_url = format!("http://127.0.0.1:{port}");

        let mut command = Command::new(&program);
        command
            .args(&req.launch.args)
            .arg("--hostname")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .args(&req.extra_args)
            .env("OPENCODE_SERVER_PASSWORD", &password)
            .envs(req.launch.env.iter().cloned())
            .current_dir(&req.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|err| {
            anyhow!(
                "failed to start opencode2 serve ({}): {err}",
                program.display()
            )
        })?;

        let process_guard = Arc::new(ServerProcessGuard::new()?);
        process_guard.assign(&child)?;
        // サーバーログはデーモンのログへ流しつつ読み続ける (パイプ詰まり防止)
        if let Some(stderr) = child.stderr.take() {
            spawn_log_pump(stderr, "opencode2 serve");
        }
        if let Some(stdout) = child.stdout.take() {
            spawn_log_pump(stdout, "opencode2 serve");
        }

        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        wait_until_ready(&http, &base_url, &password, &mut child).await?;

        // セッション決定: 再開指定があれば既存セッションへ bind、なければ新規作成
        // (新規作成時は作業ディレクトリを location に指定)
        let (opencode_session_id, context_restored) = match resume_session_id {
            Some(agent_session_id) => {
                match opencode_session_exists(&http, &base_url, &password, &agent_session_id).await
                {
                    Ok(true) => {
                        // bind 後の初期モード適用 (新規作成時は body で渡す)
                        if let Some(mode_id) = req.initial_mode.as_ref()
                            && let Err(err) = request(
                                &http,
                                &base_url,
                                &password,
                                reqwest::Method::POST,
                                &format!("/api/session/{agent_session_id}/agent"),
                                Some(json!({ "agent": mode_id })),
                            )
                            .await
                        {
                            tracing::warn!(
                                %agent_session_id,
                                "failed to apply initial mode on resume: {err:#}"
                            );
                        }
                        (agent_session_id, true)
                    }
                    Ok(false) if !allow_fresh => {
                        // ネイティブ限定 (送信時の自動レジューム) は新規作成せず
                        // 呼び出し側に履歴 Replay での明示再開を促す
                        return Err(NativeResumeUnavailable(format!(
                            "opencode2 session not found: {agent_session_id}"
                        ))
                        .into());
                    }
                    Ok(false) => {
                        tracing::info!(
                            %agent_session_id,
                            "opencode2 session not found; starting fresh with replay"
                        );
                        (
                            create_opencode_session(&http, &base_url, &password, &req).await?,
                            false,
                        )
                    }
                    Err(err) if !allow_fresh => {
                        return Err(NativeResumeUnavailable(format!(
                            "failed to verify opencode2 session: {err:#}"
                        ))
                        .into());
                    }
                    Err(err) => {
                        tracing::warn!(
                            %agent_session_id,
                            "failed to verify opencode2 session; starting fresh with replay: {err:#}"
                        );
                        (
                            create_opencode_session(&http, &base_url, &password, &req).await?,
                            false,
                        )
                    }
                }
            }
            None if resume_requested && !allow_fresh => {
                // エージェント側ID未記録でネイティブ限定 → 復元不可
                return Err(NativeResumeUnavailable(
                    "no opencode2 agent session id recorded".to_owned(),
                )
                .into());
            }
            None => (
                create_opencode_session(&http, &base_url, &password, &req).await?,
                false,
            ),
        };

        // 新規作成時のみエージェント側セッションIDの確定を記録する
        // (既存セッションへの bind では既に記録済みのため再発行しない)
        if !context_restored {
            let _ = event_tx.send(DriverEvent::Event(UnifiedEventPayload::SessionAgentBound {
                agent_session_id: opencode_session_id.clone(),
            }));
        }

        // モード (agent) とモデルの選択肢を同期する。
        // `opencode2 serve` は起動直後しばらくカタログが空のため、揃うまで待つ
        if let Some(capabilities) = wait_for_capabilities(&http, &base_url, &password).await {
            let _ = event_tx.send(DriverEvent::Event(capabilities));
        }

        let state = Arc::new(Mutex::new(SessionState::default()));
        let (commands, command_rx) = mpsc::unbounded_channel();

        // サーバープロセス管理タスク
        let supervisor_events = event_tx.clone();
        let supervisor_session = req.session_id.clone();
        let guard = Arc::clone(&process_guard);
        tokio::spawn(async move {
            supervise_server(
                child,
                command_rx,
                supervisor_events,
                supervisor_session,
                guard,
            )
            .await;
        });

        // SSE イベントポンプ
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let stream_events = event_tx.clone();
        // 長時間接続の SSE には `REQUEST_TIMEOUT` を適用しない専用クライアントを使う
        let stream_http = reqwest::Client::builder()
            .connect_timeout(SSE_CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let stream_url = base_url.clone();
        let stream_password = password.clone();
        let stream_session = opencode_session_id.clone();
        let stream_state = Arc::clone(&state);
        tokio::spawn(async move {
            run_event_stream(
                stream_http,
                stream_url,
                stream_password,
                stream_session,
                stream_state,
                stream_events,
                ready_tx,
            )
            .await;
        });
        // SSE 接続が確立 (または失敗) するまで待ってから返す
        let _ = ready_rx.await;

        Ok(StartedSession {
            handle: Box::new(OpenCode2SessionHandle {
                http,
                base_url,
                password,
                opencode_session_id,
                state,
                commands,
                _process_guard: process_guard,
            }),
            context_restored,
        })
    }
}

/// サーバープロセスをプロセスツリーごと終了させるガード。
///
/// - **Windows**: Job Object (`kill on job close`) に割り当て、[`Self::terminate`]
///   またはガードの破棄でジョブを閉じると孫プロセスを含めて確実に終了する。
///   `opencode2` は mise/npm の `.cmd` シム経由で実際のサーバーが
///   **孫プロセス**として起動されるため、直接の子を kill するだけでは
///   サーバーが残ってしまう。
/// - **Unix**: 追加の機構は持たない (`shutdown` 時に子プロセスを kill する)。
struct ServerProcessGuard {
    /// Windows のみ: Job Object。`terminate` で `None` になる (冪等)。
    #[cfg(windows)]
    job: Mutex<Option<fxg_pty::WinJobGuard>>,
}

impl ServerProcessGuard {
    fn new() -> anyhow::Result<Self> {
        #[cfg(windows)]
        {
            Ok(Self {
                job: Mutex::new(Some(fxg_pty::WinJobGuard::new_kill_on_close()?)),
            })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }

    /// 起動直後のサーバープロセスをガードへ参加させる。
    fn assign(&self, child: &tokio::process::Child) -> anyhow::Result<()> {
        #[cfg(windows)]
        {
            if let Some(handle) = child.raw_handle() {
                let job = self.job.lock().expect("job poisoned");
                if let Some(job) = job.as_ref() {
                    job.assign_process(handle)?;
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = child;
        }
        Ok(())
    }

    /// ジョブを閉じて割り当て済みプロセス (シム経由の孫プロセス含む) を終了する。
    fn terminate(&self) {
        #[cfg(windows)]
        {
            // kill-on-close: ハンドルを閉じるだけでツリー全体が終了する
            let _ = self.job.lock().expect("job poisoned").take();
        }
    }
}

/// サーバープロセスを所有し、Shutdown を受けてプロセスツリーごと終了する。
async fn supervise_server(
    mut child: tokio::process::Child,
    mut commands: mpsc::UnboundedReceiver<ServerCommand>,
    events: mpsc::UnboundedSender<DriverEvent>,
    session_id: String,
    guard: Arc<ServerProcessGuard>,
) {
    tokio::select! {
        _ = commands.recv() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            // `.cmd` シム経由の孫プロセス (実際のサーバー) を取り残さない
            guard.terminate();
            tracing::debug!(session_id, "opencode2 serve stopped");
        }
        status = child.wait() => {
            // 自ら終了した場合も孫プロセスを残さない
            guard.terminate();
            match status {
                Ok(status) if status.success() => {
                    tracing::debug!(session_id, "opencode2 serve exited");
                }
                Ok(status) => {
                    let _ = events.send(DriverEvent::Failed {
                        message: format!("opencode2 serve exited unexpectedly: {status}"),
                    });
                }
                Err(err) => {
                    let _ = events.send(DriverEvent::Failed {
                        message: format!("opencode2 serve wait failed: {err:#}"),
                    });
                }
            }
        }
    }
}

/// サーバーの stdout / stderr をデーモンのログ (`debug`) へ流す。
fn spawn_log_pump<R>(reader: R, label: &'static str)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(target: "fxg_acp::opencode2", "{label}: {line}");
        }
    });
}

/// 空きポートを確保する (bind → 即解放。起動直前の競合は許容する)。
fn free_port() -> anyhow::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|err| anyhow!("failed to allocate a local port: {err}"))?;
    Ok(listener.local_addr()?.port())
}

/// ローカルサーバー用のランダムパスワードを生成する。
///
/// URL-safe Base64 (パディングなし) のため、環境変数・コマンドラインを
/// 経由してもエスケープ不要で安全に扱える。
fn random_password() -> String {
    let bytes: [u8; 24] = rand::random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// セッション作成リクエストのボディを組み立てる。
fn create_session_body(req: &StartSessionRequest) -> Value {
    let mut body = json!({
        "location": { "directory": req.cwd.to_string_lossy() },
    });
    if let Some(title) = &req.title {
        body["title"] = Value::String(title.clone());
    }
    if let Some(agent) = &req.initial_mode {
        body["agent"] = Value::String(agent.clone());
    }
    body
}

/// 新規 opencode2 セッションを作成し ID を返す。
async fn create_opencode_session(
    http: &reqwest::Client,
    base_url: &str,
    password: &str,
    req: &StartSessionRequest,
) -> anyhow::Result<String> {
    let body = create_session_body(req);
    let created = request(
        http,
        base_url,
        password,
        reqwest::Method::POST,
        "/api/session",
        Some(body),
    )
    .await?;
    created
        .pointer("/data/id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("opencode2 did not return a session id: {created}"))
}

/// 既存 opencode2 セッションの存在を確認する。
///
/// 404 は `Ok(false)`、成功は `Ok(true)`、その他の失敗は `Err` を返す。
async fn opencode_session_exists(
    http: &reqwest::Client,
    base_url: &str,
    password: &str,
    session_id: &str,
) -> anyhow::Result<bool> {
    let response = http
        .get(format!("{base_url}/api/session/{session_id}"))
        .basic_auth(AUTH_USER, Some(password.to_owned()))
        .send()
        .await?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !status.is_success() {
        let message = response.text().await.unwrap_or_default();
        anyhow::bail!("opencode2 api /api/session/{session_id} failed ({status}): {message}");
    }
    Ok(true)
}

/// サーバーが `/api/info` に応答するまで待つ (プロセス即死は即エラー)。
async fn wait_until_ready(
    http: &reqwest::Client,
    base_url: &str,
    password: &str,
    child: &mut tokio::process::Child,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + SERVER_READY_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("opencode2 serve exited before becoming ready: {status}");
        }
        if let Ok(response) = http
            .get(format!("{base_url}/api/info"))
            .basic_auth(AUTH_USER, Some(password.to_owned()))
            .send()
            .await
            && response.status().is_success()
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "opencode2 serve did not become ready within {}s",
                SERVER_READY_TIMEOUT.as_secs()
            );
        }
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    }
}

/// authenticated な JSON API 呼び出し (起動前後で共有するヘルパー)。
async fn request(
    http: &reqwest::Client,
    base_url: &str,
    password: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> anyhow::Result<Value> {
    let mut request = http
        .request(method, format!("{base_url}{path}"))
        .basic_auth(AUTH_USER, Some(password.to_owned()));
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let message = response.text().await.unwrap_or_default();
        anyhow::bail!("opencode2 api {path} failed ({status}): {message}");
    }
    let text = response.text().await.unwrap_or_default();
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_str(&text)?)
}

/// カタログが揃うまで [`fetch_capabilities`] をリトライする。
///
/// `opencode2 serve` は起動直後しばらく `/api/agent` / `/api/model` が空配列を
/// 返すため、内容が得られるまで短い間隔で再取得する。
async fn wait_for_capabilities(
    http: &reqwest::Client,
    base_url: &str,
    password: &str,
) -> Option<UnifiedEventPayload> {
    let deadline = tokio::time::Instant::now() + CAPABILITIES_READY_TIMEOUT;
    loop {
        if let Some(capabilities) = fetch_capabilities(http, base_url, password).await {
            return Some(capabilities);
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::debug!("opencode2 capabilities were still empty before timeout");
            return None;
        }
        tokio::time::sleep(CAPABILITIES_POLL_INTERVAL).await;
    }
}

/// 利用可能なモード (agent) とモデル選択肢を取得して
/// [`UnifiedEventPayload::CapabilitiesUpdated`] を組み立てる。
async fn fetch_capabilities(
    http: &reqwest::Client,
    base_url: &str,
    password: &str,
) -> Option<UnifiedEventPayload> {
    let available_modes = match request(
        http,
        base_url,
        password,
        reqwest::Method::GET,
        "/api/agent",
        None,
    )
    .await
    {
        Ok(value) => value
            .get("data")
            .and_then(Value::as_array)
            .map(|agents| {
                agents
                    .iter()
                    .filter(|agent| {
                        // サブエージェントはユーザー選択の対象外
                        agent.get("mode").and_then(Value::as_str) != Some("subagent")
                            && agent.get("hidden").and_then(Value::as_bool) != Some(true)
                    })
                    .filter_map(|agent| {
                        Some(ModeInfo {
                            mode_id: agent.get("id")?.as_str()?.to_owned(),
                            name: agent
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            description: agent
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        Err(err) => {
            tracing::debug!("failed to fetch opencode2 agents: {err:#}");
            Vec::new()
        }
    };

    let config_options = match request(
        http,
        base_url,
        password,
        reqwest::Method::GET,
        "/api/model",
        None,
    )
    .await
    {
        Ok(value) => {
            let options: Vec<Value> = value
                .get("data")
                .and_then(Value::as_array)
                .map(|models| {
                    models
                        .iter()
                        .filter(|model| model.get("enabled").and_then(Value::as_bool) != Some(false))
                        .filter_map(|model| {
                            Some(json!({
                                "providerID": model.get("providerID")?.as_str()?,
                                "id": model.get("modelID").or_else(|| model.get("id"))?.as_str()?,
                                "name": model.get("name").and_then(Value::as_str).unwrap_or_default(),
                            }))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if options.is_empty() {
                Vec::new()
            } else {
                vec![ConfigOptionInfo {
                    key: "model".to_owned(),
                    name: "モデル".to_owned(),
                    current_value: Value::Null,
                    options,
                }]
            }
        }
        Err(err) => {
            tracing::debug!("failed to fetch opencode2 models: {err:#}");
            Vec::new()
        }
    };

    if available_modes.is_empty() && config_options.is_empty() {
        return None;
    }
    Some(UnifiedEventPayload::CapabilitiesUpdated {
        current_mode: None,
        available_modes,
        available_commands: Vec::new(),
        config_options,
    })
}

/// SSE (`GET /api/event`) を購読し、切断時は再接続する。
#[allow(clippy::too_many_arguments)]
async fn run_event_stream(
    http: reqwest::Client,
    base_url: String,
    password: String,
    session_id: String,
    state: Arc<Mutex<SessionState>>,
    events: mpsc::UnboundedSender<DriverEvent>,
    ready: tokio::sync::oneshot::Sender<()>,
) {
    let mut ready = Some(ready);
    let mut failures = 0u32;
    loop {
        match read_event_stream(
            &http,
            &base_url,
            &password,
            &session_id,
            &state,
            &events,
            &mut ready,
        )
        .await
        {
            Ok(()) => failures = 0,
            Err(err) => {
                if events.is_closed() {
                    return;
                }
                failures += 1;
                if failures > SSE_RECONNECT_LIMIT {
                    tracing::debug!(session_id, "opencode2 event stream停止: {err:#}");
                    return;
                }
                tracing::debug!(session_id, "opencode2 event stream reconnecting: {err:#}");
            }
        }
        if events.is_closed() {
            return;
        }
        tokio::time::sleep(SSE_RECONNECT_DELAY).await;
    }
}

/// 1 回分の SSE 購読 (接続確立時に `ready` を通知する)。
async fn read_event_stream(
    http: &reqwest::Client,
    base_url: &str,
    password: &str,
    session_id: &str,
    state: &Arc<Mutex<SessionState>>,
    events: &mpsc::UnboundedSender<DriverEvent>,
    ready: &mut Option<tokio::sync::oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    let response = http
        .get(format!("{base_url}/api/event"))
        .basic_auth(AUTH_USER, Some(password.to_owned()))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)?;
    if let Some(ready) = ready.take() {
        let _ = ready.send(());
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(index) = buffer.find('\n') {
            let line: String = buffer.drain(..=index).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            let Some(data) = line.strip_prefix("data: ") else {
                // `: heartbeat` や `event:` 行は無視する
                continue;
            };
            for event in map_event(state, session_id, data) {
                if events.send(event).is_err() {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

/// SSE イベント (JSON) を正規化イベントへ変換する。
///
/// `data` に `sessionID` を持つイベントは、自セッション以外を無視する
/// (1つの `opencode2 serve` は複数セッションを多重化し得る)。
fn map_event(state: &Arc<Mutex<SessionState>>, session_id: &str, raw: &str) -> Vec<DriverEvent> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    if let Some(event_session) = data.get("sessionID").and_then(Value::as_str)
        && event_session != session_id
    {
        return Vec::new();
    }
    let text_of = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };

    match kind {
        "session.updated" | "session.title.updated" => {
            let title = data
                .get("title")
                .or_else(|| data.pointer("/session/title"))
                .or_else(|| data.pointer("/info/title"))
                .and_then(Value::as_str)
                .map(str::trim);
            if let Some(title) = title
                && !title.is_empty()
            {
                vec![DriverEvent::Event(
                    UnifiedEventPayload::SessionTitleChanged {
                        title: title.to_owned(),
                    },
                )]
            } else {
                Vec::new()
            }
        }
        "session.execution.started" => {
            vec![DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            })]
        }
        "session.execution.succeeded" | "session.execution.interrupted" | "session.idle" => {
            vec![DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Idle,
                error_message: None,
            })]
        }
        "session.execution.failed" => {
            let message = data
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("opencode2 execution failed")
                .to_owned();
            vec![DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Error,
                error_message: Some(message),
            })]
        }
        "session.error" => {
            let message = data
                .pointer("/error/message")
                .or_else(|| data.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("opencode2 session error")
                .to_owned();
            vec![DriverEvent::Failed { message }]
        }
        // テキスト / 推論のストリーミング (`*.ended` のみ永続化する)
        "session.text.delta" => vec![DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta {
            message_id: text_of("assistantMessageID"),
            text_delta: text_of("delta"),
        })],
        "session.text.ended" => vec![DriverEvent::Event(UnifiedEventPayload::AgentMessage {
            message_id: text_of("assistantMessageID"),
            text: text_of("text"),
            is_complete: true,
        })],
        "session.reasoning.delta" => {
            vec![DriverEvent::Delta(StreamDeltaPayload::AgentThoughtDelta {
                thought_id: reasoning_id(&data),
                text_delta: text_of("delta"),
            })]
        }
        "session.reasoning.ended" => vec![DriverEvent::Event(UnifiedEventPayload::AgentThought {
            thought_id: reasoning_id(&data),
            text: text_of("text"),
            is_complete: true,
        })],
        // ツール呼び出し (同一 `tool_call_id` への追記で状態遷移を表現する)
        "session.tool.input.started" => {
            let tool_call_id = text_of("id");
            let name = text_of("name");
            state
                .lock()
                .expect("state poisoned")
                .tool_names
                .insert(tool_call_id.clone(), name.clone());
            vec![DriverEvent::Event(UnifiedEventPayload::ToolCall {
                tool_call_id,
                title: name.clone(),
                kind: tool_kind(&name).to_owned(),
                status: "pending".to_owned(),
                locations: Vec::new(),
                diff: None,
                raw_output: None,
            })]
        }
        "session.tool.called" => {
            let tool_call_id = text_of("id");
            let name = tool_name(state, &tool_call_id, &data);
            let input = data.get("input").cloned().unwrap_or(Value::Null);
            vec![DriverEvent::Event(UnifiedEventPayload::ToolCall {
                tool_call_id,
                title: tool_title(&name, &input),
                kind: tool_kind(&name).to_owned(),
                status: "in_progress".to_owned(),
                locations: tool_locations(&input),
                diff: None,
                raw_output: None,
            })]
        }
        "session.tool.progress" => vec![DriverEvent::Delta(StreamDeltaPayload::ToolCallProgress {
            tool_call_id: text_of("id"),
            status: "in_progress".to_owned(),
            raw_output_delta: None,
        })],
        "session.tool.success" | "session.tool.failed" => {
            let tool_call_id = text_of("id");
            let name = tool_name(state, &tool_call_id, &data);
            let failed = kind == "session.tool.failed";
            vec![DriverEvent::Event(UnifiedEventPayload::ToolCall {
                tool_call_id,
                title: name.clone(),
                kind: tool_kind(&name).to_owned(),
                status: if failed { "failed" } else { "completed" }.to_owned(),
                locations: Vec::new(),
                diff: None,
                raw_output: tool_output(&data),
            })]
        }
        // 承認リクエスト / 解決
        "permission.asked" => vec![DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
            request_id: text_of("id"),
            tool_name: text_of("action"),
            summary: permission_summary(&data),
            options: permission_options(),
            details: data,
        })],
        "permission.replied" => vec![DriverEvent::Event(
            UnifiedEventPayload::PermissionResolved {
                request_id: text_of("requestID"),
                selected_option_id: text_of("reply"),
                // 純正TUI (opencode2 CLI) からの解決を含む
                resolved_by: "cli".to_owned(),
            },
        )],
        // 純正TUI から送信されたプロンプトは fxg 側に記録されないため、
        // 自己送信分 (SessionManager が `UserMessage` 記録済み) 以外を拾う。
        "session.inbox.enqueued" => {
            let inbox_id = text_of("inboxID");
            let known = state
                .lock()
                .expect("state poisoned")
                .prompt_ids
                .iter()
                .any(|id| id == &inbox_id);
            if known {
                return Vec::new();
            }
            let text = data
                .pointer("/item/payload/text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if text.is_empty() {
                return Vec::new();
            }
            vec![DriverEvent::Event(UnifiedEventPayload::UserMessage {
                text,
                attachments: Vec::new(),
                client_source: "cli".to_owned(),
                // スナップショットは fxg 側の送信経路でのみ取得する
                snapshot_tree_hash: None,
            })]
        }
        _ => Vec::new(),
    }
}

/// 推論 (reasoning) の識別子。同一メッセージの複数 ordinal を区別する。
fn reasoning_id(data: &Value) -> String {
    let message_id = data
        .get("assistantMessageID")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let ordinal = data.get("ordinal").and_then(Value::as_u64).unwrap_or(0);
    format!("{message_id}#reasoning-{ordinal}")
}

/// ツール名を引く (`tool.called` 以降には名前が含まれないため記録を参照する)。
fn tool_name(state: &Arc<Mutex<SessionState>>, tool_call_id: &str, data: &Value) -> String {
    if let Some(name) = data.get("name").and_then(Value::as_str) {
        return name.to_owned();
    }
    state
        .lock()
        .expect("state poisoned")
        .tool_names
        .get(tool_call_id)
        .cloned()
        .unwrap_or_else(|| "tool".to_owned())
}

/// opencode2 のツール名を fxg の種別 (`read` / `edit` / `execute` / `search`) へ合わせる。
fn tool_kind(name: &str) -> &'static str {
    match name {
        "shell" | "bash" => "execute",
        "read" | "list" => "read",
        "write" | "edit" | "patch" | "multiedit" => "edit",
        "glob" | "grep" | "search" => "search",
        _ => "other",
    }
}

/// ツール入力から表示タイトルを組み立てる。
fn tool_title(name: &str, input: &Value) -> String {
    if let Some(command) = input.get("command").and_then(Value::as_str) {
        return command.to_owned();
    }
    if let Some(path) = input
        .get("filePath")
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)
    {
        return format!("{name}: {path}");
    }
    if let Some(pattern) = input.get("pattern").and_then(Value::as_str) {
        return format!("{name}: {pattern}");
    }
    input
        .as_object()
        .filter(|object| !object.is_empty())
        .map(|object| {
            let text = Value::Object(object.clone()).to_string();
            format!("{name}: {}", truncate_chars(&text, 120))
        })
        .unwrap_or_else(|| name.to_owned())
}

/// ツール入力から対象ファイルパスを抽出する。
fn tool_locations(input: &Value) -> Vec<String> {
    let mut locations = Vec::new();
    for key in ["filePath", "file_path", "path"] {
        if let Some(path) = input.get(key).and_then(Value::as_str) {
            locations.push(path.to_owned());
        }
    }
    if let Some(paths) = input.get("paths").and_then(Value::as_array) {
        locations.extend(paths.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    locations
}

/// ツール実行結果 (`content: [{type: "text", text}]`) を生出力へ変換する。
fn tool_output(data: &Value) -> Option<String> {
    let content = data.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() { None } else { Some(text) }
}

/// 承認リクエストの要約 (対象リソース) を組み立てる。
fn permission_summary(data: &Value) -> String {
    let resources = data
        .get("resources")
        .and_then(Value::as_array)
        .map(|resources| {
            resources
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if resources.is_empty() {
        data.get("action")
            .and_then(Value::as_str)
            .unwrap_or("permission requested")
            .to_owned()
    } else {
        resources
    }
}

/// 承認の選択肢 (opencode2 の `decision` 値と対応)。
fn permission_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption {
            option_id: "once".to_owned(),
            name: "今回のみ許可".to_owned(),
            kind: "allow_once".to_owned(),
        },
        PermissionOption {
            option_id: "always".to_owned(),
            name: "常に許可".to_owned(),
            kind: "allow_always".to_owned(),
        },
        PermissionOption {
            option_id: "reject".to_owned(),
            name: "拒否".to_owned(),
            kind: "reject_once".to_owned(),
        },
    ]
}

/// 文字列を文字数上限で切り詰める (`…` 付き)。
fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Arc<Mutex<SessionState>> {
        Arc::new(Mutex::new(SessionState::default()))
    }

    fn map(state: &Arc<Mutex<SessionState>>, raw: &str) -> Vec<DriverEvent> {
        map_event(state, "ses_1", raw)
    }

    #[test]
    fn maps_session_updated_title() {
        let state = state();
        let events = map(
            &state,
            r#"{"type":"session.updated","data":{"sessionID":"ses_1","title":"New Generated Title"}}"#,
        );
        assert!(matches!(
            events.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::SessionTitleChanged { title })]
                if title == "New Generated Title"
        ));
    }

    #[test]
    fn maps_text_streaming_events() {
        let state = state();
        let events = map(
            &state,
            r#"{"type":"session.text.delta","data":{"sessionID":"ses_1","assistantMessageID":"msg_1","ordinal":0,"delta":"he"}}"#,
        );
        assert!(matches!(
            events.as_slice(),
            [DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta { message_id, text_delta })]
                if message_id == "msg_1" && text_delta == "he"
        ));

        let events = map(
            &state,
            r#"{"type":"session.text.ended","data":{"sessionID":"ses_1","assistantMessageID":"msg_1","ordinal":0,"text":"hello"}}"#,
        );
        assert!(matches!(
            events.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::AgentMessage { message_id, text, is_complete })]
                if message_id == "msg_1" && text == "hello" && *is_complete
        ));
    }

    #[test]
    fn ignores_other_sessions() {
        let state = state();
        let events = map(
            &state,
            r#"{"type":"session.text.ended","data":{"sessionID":"ses_other","assistantMessageID":"m","text":"x"}}"#,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn maps_tool_lifecycle_with_name_tracking() {
        let state = state();
        let started = map(
            &state,
            r#"{"type":"session.tool.input.started","data":{"sessionID":"ses_1","id":"call_1","name":"shell"}}"#,
        );
        assert!(matches!(
            started.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::ToolCall { tool_call_id, status, kind, .. })]
                if tool_call_id == "call_1" && status == "pending" && kind == "execute"
        ));

        let called = map(
            &state,
            r#"{"type":"session.tool.called","data":{"sessionID":"ses_1","id":"call_1","input":{"command":"echo hi"}}}"#,
        );
        assert!(matches!(
            called.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::ToolCall { title, kind, status, .. })]
                if title == "echo hi" && kind == "execute" && status == "in_progress"
        ));

        let success = map(
            &state,
            r#"{"type":"session.tool.success","data":{"sessionID":"ses_1","id":"call_1","content":[{"type":"text","text":"hi\n"}],"metadata":{"exit":0}}}"#,
        );
        assert!(matches!(
            success.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::ToolCall { status, raw_output, .. })]
                if status == "completed" && raw_output.as_deref() == Some("hi\n")
        ));
    }

    #[test]
    fn maps_permission_lifecycle() {
        let state = state();
        let asked = map(
            &state,
            r#"{"type":"permission.asked","data":{"sessionID":"ses_1","id":"per_1","action":"shell","resources":["echo *"],"save":["echo *"]}}"#,
        );
        match asked.as_slice() {
            [
                DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
                    request_id,
                    tool_name,
                    summary,
                    options,
                    ..
                }),
            ] => {
                assert_eq!(request_id, "per_1");
                assert_eq!(tool_name, "shell");
                assert_eq!(summary, "echo *");
                assert_eq!(options.len(), 3);
                assert_eq!(options[0].option_id, "once");
            }
            other => panic!("unexpected events: {other:?}"),
        }

        let replied = map(
            &state,
            r#"{"type":"permission.replied","data":{"sessionID":"ses_1","requestID":"per_1","reply":"once"}}"#,
        );
        assert!(matches!(
            replied.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::PermissionResolved { request_id, selected_option_id, .. })]
                if request_id == "per_1" && selected_option_id == "once"
        ));
    }

    #[test]
    fn maps_native_tui_prompts_only_once() {
        let state = state();
        state
            .lock()
            .expect("state")
            .prompt_ids
            .push("msg_self".to_owned());
        let raw_self = r#"{"type":"session.inbox.enqueued","data":{"sessionID":"ses_1","inboxID":"msg_self","item":{"type":"user","payload":{"text":"from fxg"}}}}"#;
        assert!(map(&state, raw_self).is_empty());

        let raw_native = r#"{"type":"session.inbox.enqueued","data":{"sessionID":"ses_1","inboxID":"msg_native","item":{"type":"user","payload":{"text":"from native tui"}}}}"#;
        assert!(matches!(
            map(&state, raw_native).as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::UserMessage { text, .. })] if text == "from native tui"
        ));
    }

    #[test]
    fn maps_status_transitions() {
        let state = state();
        for (kind, expected) in [
            ("session.execution.started", SessionStatus::Running),
            ("session.execution.succeeded", SessionStatus::Idle),
            ("session.execution.interrupted", SessionStatus::Idle),
        ] {
            let raw = format!(r#"{{"type":"{kind}","data":{{"sessionID":"ses_1"}}}}"#);
            assert!(matches!(
                map(&state, &raw).as_slice(),
                [DriverEvent::Event(UnifiedEventPayload::StatusChanged { status, .. })] if *status == expected
            ));
        }
    }

    #[test]
    fn tool_helpers_classify_names_and_inputs() {
        assert_eq!(tool_kind("shell"), "execute");
        assert_eq!(tool_kind("read"), "read");
        assert_eq!(tool_kind("edit"), "edit");
        assert_eq!(tool_kind("grep"), "search");
        assert_eq!(tool_kind("unknown"), "other");
        assert_eq!(
            tool_title("shell", &json!({"command": "cargo test"})),
            "cargo test"
        );
        assert_eq!(
            tool_title("read", &json!({"filePath": "src/main.rs"})),
            "read: src/main.rs"
        );
        assert_eq!(
            tool_locations(&json!({"paths": ["a.rs", "b.rs"]})),
            vec!["a.rs".to_owned(), "b.rs".to_owned()]
        );
    }

    #[test]
    fn create_session_body_carries_location_and_title() {
        use std::path::PathBuf;

        use crate::driver::AgentLaunchSpec;

        let req = StartSessionRequest {
            session_id: "s".to_owned(),
            title: Some("OpenCode2 @ demo".to_owned()),
            cwd: PathBuf::from("D:/work/demo"),
            launch: AgentLaunchSpec {
                agent_id: "opencode2".to_owned(),
                display_name: "OpenCode2".to_owned(),
                driver_kind: "opencode2".to_owned(),
                program: PathBuf::from("opencode2"),
                args: vec!["serve".to_owned()],
                env: Vec::new(),
            },
            extra_args: Vec::new(),
            initial_mode: Some("build".to_owned()),
            resume: None,
        };
        let body = create_session_body(&req);
        assert_eq!(body["title"], "OpenCode2 @ demo");
        assert_eq!(body["agent"], "build");
        assert!(body["location"]["directory"].as_str().is_some());
    }

    /// 実挙動の確認 (opencode2 がインストール済みの環境でのみ実行):
    /// `FXG_TEST_OPENCODE2=1 cargo test -p fxg-acp -- --ignored` で実行する。
    #[tokio::test]
    #[ignore = "requires a local opencode2 install and provider auth"]
    async fn serves_and_streams_against_real_opencode2() {
        use std::path::PathBuf;

        use crate::driver::AgentLaunchSpec;

        let root = tempfile::tempdir().expect("tempdir");
        let driver = OpenCode2Driver::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let started = driver
            .start_session(
                StartSessionRequest {
                    session_id: "test-session".to_owned(),
                    title: Some("fxg test".to_owned()),
                    cwd: PathBuf::from(root.path()),
                    launch: AgentLaunchSpec {
                        agent_id: "opencode2".to_owned(),
                        display_name: "OpenCode2".to_owned(),
                        driver_kind: "opencode2".to_owned(),
                        program: PathBuf::from("opencode2"),
                        args: vec!["serve".to_owned()],
                        env: Vec::new(),
                    },
                    extra_args: Vec::new(),
                    initial_mode: None,
                    resume: None,
                },
                tx,
            )
            .await
            .expect("start_session");
        assert!(!started.context_restored);
        let handle = started.handle;

        assert!(handle.native_attach().is_some());
        // 起動直後は `/api/agent` / `/api/model` が空を返すため、ドライバが
        // カタログ取得を待って `CapabilitiesUpdated` を送ることを検証する
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let mut has_model_options = false;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await {
                Ok(Some(DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
                    config_options,
                    ..
                }))) => {
                    has_model_options = config_options
                        .iter()
                        .any(|option| option.key == "model" && !option.options.is_empty());
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            has_model_options,
            "capabilities with model options must be emitted"
        );
        // opencode2 の既定モデルは環境によっては利用できないため、公開プロバイダ
        // (`opencode`) のモデルを選んで明示的に設定する。
        if let Some(info) = handle.native_attach() {
            let password = info
                .env
                .iter()
                .find(|(key, _)| key == PASSWORD_ENV)
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            let client = reqwest::Client::new();
            let models = request(
                &client,
                &info.server_url,
                &password,
                reqwest::Method::GET,
                "/api/model",
                None,
            )
            .await
            .expect("model list");
            let model = models
                .get("data")
                .and_then(Value::as_array)
                .and_then(|models| {
                    models
                        .iter()
                        .find(|model| model.get("providerID") == Some(&Value::from("opencode")))
                        .or_else(|| models.first())
                })
                .cloned();
            if let Some(model) = model {
                handle
                    .set_config(
                        "model".to_owned(),
                        json!({
                            "providerID": model.get("providerID"),
                            "id": model.get("modelID").or_else(|| model.get("id")),
                        }),
                    )
                    .await
                    .expect("set model");
            }
        }
        handle
            .send_prompt("Reply with exactly one word: hello".to_owned())
            .await
            .expect("send_prompt");

        // `session.text.ended` が届くまでイベントを待つ
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        let mut completed = false;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
                Ok(Some(event)) => {
                    tracing::info!("opencode2 test event: {event:?}");
                    if matches!(
                        event,
                        DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                            is_complete: true,
                            ..
                        })
                    ) {
                        completed = true;
                        break;
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        assert!(completed, "no completed agent message received");
        handle.shutdown().await.expect("shutdown");
    }

    /// 既存セッションへの bind (resume) を実 opencode2 で確認する:
    /// `FXG_TEST_OPENCODE2=1 cargo test -p fxg-acp -- --ignored` で実行する。
    #[tokio::test]
    #[ignore = "requires a local opencode2 install"]
    async fn resumes_existing_session_against_real_opencode2() {
        use std::path::PathBuf;

        use crate::driver::{AgentLaunchSpec, ResumeRequest};

        let root = tempfile::tempdir().expect("tempdir");
        let driver = OpenCode2Driver::new();
        let launch = AgentLaunchSpec {
            agent_id: "opencode2".to_owned(),
            display_name: "OpenCode2".to_owned(),
            driver_kind: "opencode2".to_owned(),
            program: PathBuf::from("opencode2"),
            args: vec!["serve".to_owned()],
            env: Vec::new(),
        };

        // 1) 新規セッションを作成し、エージェント側IDを取得して停止する
        let (tx, _rx) = mpsc::unbounded_channel();
        let started = driver
            .start_session(
                StartSessionRequest {
                    session_id: "resume-test".to_owned(),
                    title: Some("fxg resume test".to_owned()),
                    cwd: PathBuf::from(root.path()),
                    launch: launch.clone(),
                    extra_args: Vec::new(),
                    initial_mode: None,
                    resume: None,
                },
                tx,
            )
            .await
            .expect("start_session");
        assert!(!started.context_restored);
        let agent_session_id = started
            .handle
            .native_attach()
            .expect("native attach")
            .session_id;
        started.handle.shutdown().await.expect("shutdown");

        // 2) 既存セッションへ bind して再開する (新サーバーから復元)
        let (tx, _rx) = mpsc::unbounded_channel();
        let resumed = driver
            .start_session(
                StartSessionRequest {
                    session_id: "resume-test".to_owned(),
                    title: None,
                    cwd: PathBuf::from(root.path()),
                    launch,
                    extra_args: Vec::new(),
                    initial_mode: None,
                    resume: Some(ResumeRequest {
                        agent_session_id: Some(agent_session_id.clone()),
                        allow_fresh: false,
                    }),
                },
                tx,
            )
            .await
            .expect("resume session");
        assert!(resumed.context_restored);
        assert_eq!(
            resumed
                .handle
                .native_attach()
                .expect("native attach")
                .session_id,
            agent_session_id,
            "resume must bind to the recorded opencode2 session"
        );
        resumed.handle.shutdown().await.expect("shutdown");
    }
}
