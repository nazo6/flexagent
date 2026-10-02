//! OpenCode2 ドライバ (`opencode serve` ブリッジ + 純正TUI Attach)。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §3 モードA。
//!
//! - **バージョン検証**: opencode v1 / v2 はどちらもコマンド名が `opencode`
//!   のため、起動前に `opencode --version` で v2 系であることを検証する
//!   ([`ensure_opencode_v2`]。v1 は `serve` API を持たない)。
//! - **サーバー起動**: セッションごとに
//!   `opencode serve --hostname 127.0.0.1 --port <free>` を起動し、ランダムな
//!   `OPENCODE_SERVER_PASSWORD` を注入する。以降の HTTP / SSE は Basic 認証
//!   (`opencode:<password>`) で接続する。
//! - **イベント**: SSE (`GET /api/event`) を購読し、`session.*` / `permission.*`
//!   を正規化イベントへ変換する。`*.delta` はメモリ配信のみ
//!   ([`StreamDeltaPayload`])、`*.ended` / `tool.*` / `permission.*` は永続化対象
//!   ([`UnifiedEventPayload`])。
//! - **純正TUI Attach**: サーバーはデーモンが管理し続けるため、CLI は
//!   `opencode run --server <url> --session <id>` で 100% 純正の TUI を表示できる
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
//! - **質問・フォーム (elicitation)**: SSE の `form.created` (エージェントの
//!   `question` ツール呼び出し等) を [`UnifiedEventPayload::ElicitationRequest`] へ
//!   変換し、`POST /api/session/{id}/form/{id}/reply` または
//!   `DELETE /api/session/{id}/form/{id}` で応答する。純正TUI 等からの解決も
//!   `form.replied` / `form.cancelled` として同期する。
//! - **スラッシュコマンド**: `GET /api/command` の一覧を capabilities として同期し、
//!   `/name <本文>` は `POST /api/session/{id}/command` へ振り分ける。opencode の
//!   ACP / 純正TUI と同じく 1 プロンプトにつき 1 コマンドだけを扱う。
//! - **コンテキスト圧縮**: `/compact` (または `ControlSession(Compact)`) を
//!   `POST /api/session/{id}/compact` へ振り分け、SSE の `session.compaction.*` を
//!   [`UnifiedEventPayload::CompactionUpdated`] として同期する (要約本文の
//!   `*.delta` はエフェメラル扱いでイベント化しない)。

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use base64::Engine;
use futures_util::StreamExt;
use fxg_protocol::common::{
    CommandInfo, CompactionStatus, ConfigOptionInfo, ElicitationAction, ModeInfo, PermissionOption,
    SessionStatus, StreamDeltaPayload,
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

/// `opencode serve` が準備完了するまでの最大待機時間。
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
/// 対応する `opencode` のメジャーバージョン。
///
/// opencode v1 / v2 はどちらもコマンド名が `opencode` のため、`serve` API を
/// 持たない v1 を誤って起動しないようメジャーバージョンで判別する。
const OPENCODE_MAJOR: u64 = 2;

/// コンテキスト圧縮のローカルコマンド名。
///
/// opencode の TUI がローカル処理するため `GET /api/command` には現れないが、
/// 専用 API (`POST /api/session/{id}/compact`) で実行できる。
const COMPACT_COMMAND: &str = "compact";

/// `/name <本文>` を、既知のコマンド一覧に一致する場合のみ
/// `(コマンド名, 本文)` へ分割する。
///
/// opencode (`opencode acp` / 純正TUI) と同じく 1 プロンプトにつき 1 コマンドだけを
/// 扱い、2 つ目以降は 1 つ目の引数 (`$ARGUMENTS`) として渡す。本文の改行は保持する。
/// 未知の名前は `None` を返し、通常のプロンプトとして送信される。
fn split_command<'a>(text: &'a str, commands: &[String]) -> Option<(&'a str, &'a str)> {
    let rest = text.trim_start().strip_prefix('/')?;
    let name = rest.split_whitespace().next()?;
    if !commands.iter().any(|command| command == name) {
        return None;
    }
    Some((name, rest[name.len()..].trim_start()))
}

/// `/compact` (引数付きも許容) か。
///
/// `/compact` は opencode の TUI がローカル処理するコマンドで `/api/command`
/// には現れないため、ドライバが専用 API へ振り分ける。API 由来の同名コマンドが
/// 定義されている場合は [`split_command`] 側 (`/command` 送信) が優先される。
fn is_compact_command(text: &str) -> bool {
    text.trim_start()
        .strip_prefix('/')
        .and_then(|rest| rest.split_whitespace().next())
        == Some(COMPACT_COMMAND)
}

/// `opencode` 実行ファイルが対応バージョン (v2 系) であることを検証する。
///
/// v1 と v2 は同名コマンドだが、v1 は `serve` API・ネイティブセッション管理を
/// 持たないため、起動前に `opencode --version` を実行してメジャーバージョンを
/// 確認する。v2 系以外の場合は起動せずエラーを返す。
pub async fn ensure_opencode_v2(program: &str, cwd: &Path) -> anyhow::Result<()> {
    let resolved = fxg_pty::resolve_command(program, cwd).map_err(|err| {
        anyhow!("opencode が見つかりません (OpenCode v{OPENCODE_MAJOR} を起動できません): {err}")
    })?;
    let output = Command::new(&resolved)
        .arg("--version")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|err| {
            anyhow!(
                "failed to run opencode --version ({}): {err}",
                resolved.display()
            )
        })?;
    if !output.status.success() {
        anyhow::bail!(
            "opencode --version failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let major = parse_opencode_major(&stdout)
        .ok_or_else(|| anyhow!("failed to parse opencode version from {:?}", stdout.trim()))?;
    if major != OPENCODE_MAJOR {
        anyhow::bail!(
            "OpenCode v{OPENCODE_MAJOR} が必要です (検出: {})",
            stdout.trim()
        );
    }
    Ok(())
}

/// `opencode --version` 出力 (例: `opencode v2.0.21`) からメジャーバージョンを抽出する。
fn parse_opencode_major(output: &str) -> Option<u64> {
    output.split_whitespace().find_map(|token| {
        let token = token.trim_start_matches(['v', 'V']);
        token.split('.').next()?.parse::<u64>().ok()
    })
}

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
    /// 利用可能なスラッシュコマンド名 (`GET /api/command`)。
    ///
    /// `/name <本文>` を `/command` へ振り分ける判定に用いる。
    commands: Vec<String>,
    /// 送信済みだが inboxID 未記録の `/command` 数。
    ///
    /// `/command` は 204 を返し inboxID を伴わないため、`/prompt` のように
    /// 応答から自己送信分を判別できない。送信前にここで予約し、SSE
    /// (`session.inbox.enqueued`) 側で消費して ID を記録する。
    pending_commands: usize,
    /// `tool_call_id` → ツール名 (`session.tool.called` に名前が含まれないため、
    /// `session.tool.input.started` から引き継ぐ)
    tool_names: HashMap<String, String>,
}

/// 起動済み OpenCode2 セッションの操作ハンドル。
struct OpenCode2SessionHandle {
    http: reqwest::Client,
    /// `opencode serve` のベースURL (例: `http://127.0.0.1:38219`)
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
        // 既知のスラッシュコマンド (`/name <本文>`) は `/command` へ振り分ける
        // (`/prompt` はスラッシュを解釈せず、本文がそのまま LLM へ渡る)。
        let command = {
            let state = self.state.lock().expect("state poisoned");
            split_command(&text, &state.commands)
                .map(|(name, args)| (name.to_owned(), args.to_owned()))
        };
        if let Some((name, args)) = command {
            // `/command` は inboxID を返さないため、自己送信分の判別と ID 記録は
            // 送信前の予約カウンタを介して SSE 側で行う (下記 `session.inbox.enqueued`)。
            self.state.lock().expect("state poisoned").pending_commands += 1;
            if let Err(err) = self
                .request_json(
                    reqwest::Method::POST,
                    &format!("/api/session/{}/command", self.opencode_session_id),
                    Some(json!({ "name": name, "text": args })),
                )
                .await
            {
                self.state.lock().expect("state poisoned").pending_commands -= 1;
                return Err(err);
            }
            return Ok(());
        }
        // `/compact` はローカルコマンドのため専用 API へ振り分ける (引数は無視する。
        // opencode の ACP 実装も `detectSlashCommand` 後に `session.summarize` へ
        // 特別振り分けする)。
        if is_compact_command(&text) {
            return self.compact_context().await;
        }
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

    async fn respond_elicitation(
        &self,
        elicitation_id: String,
        action: ElicitationAction,
        content: Value,
    ) -> anyhow::Result<()> {
        match action {
            ElicitationAction::Accept => {
                let answer = if content.is_object() {
                    content
                } else {
                    json!({})
                };
                self.request_json(
                    reqwest::Method::POST,
                    &format!(
                        "/api/session/{}/form/{elicitation_id}/reply",
                        self.opencode_session_id
                    ),
                    Some(json!({ "answer": answer })),
                )
                .await?;
            }
            ElicitationAction::Decline | ElicitationAction::Cancel => {
                self.request_json(
                    reqwest::Method::DELETE,
                    &format!(
                        "/api/session/{}/form/{elicitation_id}",
                        self.opencode_session_id
                    ),
                    None,
                )
                .await?;
            }
        }
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

    /// 会話コンテキストを圧縮する (`/compact`)。
    ///
    /// `POST /api/session/{id}/compact` を呼び出す (busy 中は次のステップ境界で
    /// 実行される steer 配送)。進捗は SSE の `session.compaction.*` から
    /// [`UnifiedEventPayload::CompactionUpdated`] として送出される。
    async fn compact_context(&self) -> anyhow::Result<()> {
        self.request_json(
            reqwest::Method::POST,
            &format!("/api/session/{}/compact", self.opencode_session_id),
            Some(json!({})),
        )
        .await?;
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
                    "failed to resolve opencode program {}: {err}",
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
                "failed to start opencode serve ({}): {err}",
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

        // モード (agent)・スラッシュコマンド・モデルの選択肢を同期する。
        // `opencode serve` は起動直後しばらくカタログが空のため、揃うまで待つ
        let mut session_state = SessionState::default();
        if let Some(mut capabilities) = wait_for_capabilities(&http, &base_url, &password).await {
            // 振り分け対象は `/api/command` 由来のコマンドのみとする。
            // 合成エントリ `compact` は専用 API (`/compact`) へ振り分けるため、
            // ここで `split_command` の対象に含めてはならない。
            session_state.commands = command_names(&capabilities);
            add_compact_command(&mut capabilities);
            let _ = event_tx.send(DriverEvent::Event(capabilities));
        }

        let state = Arc::new(Mutex::new(session_state));
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
/// `opencode serve` は起動直後しばらく `/api/agent` / `/api/command` /
/// `/api/model` が空配列を返す。さらに `/api/command` は組み込みコマンドの後に
/// プロジェクト定義のコマンドが追加されるため、コマンド一覧が 2 回連続で同じに
/// なるまで待ってから確定する。揃いきらないままタイムアウトした場合は、それまでに
/// 得られた最新の内容を返す (カタログ取得の失敗でセッションを開始できなくしない)。
async fn wait_for_capabilities(
    http: &reqwest::Client,
    base_url: &str,
    password: &str,
) -> Option<UnifiedEventPayload> {
    let deadline = tokio::time::Instant::now() + CAPABILITIES_READY_TIMEOUT;
    let mut latest = None;
    let mut previous_commands: Option<Vec<String>> = None;
    loop {
        if let Some(capabilities) = fetch_capabilities(http, base_url, password).await {
            let commands = command_names(&capabilities);
            let settled = previous_commands.as_ref() == Some(&commands);
            previous_commands = Some(commands);
            if !settled || !is_capabilities_ready(&capabilities) {
                latest = Some(capabilities);
            } else {
                return Some(capabilities);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::debug!("opencode2 capabilities were still incomplete before timeout");
            return latest;
        }
        tokio::time::sleep(CAPABILITIES_POLL_INTERVAL).await;
    }
}

/// モード / モデルのカタログが揃っているか。
///
/// コマンド一覧は組み込みの後にプロジェクト定義が追加されるため、ここでは
/// 判定せず [`wait_for_capabilities`] が安定 (2 回連続で同一) を待つ。
fn is_capabilities_ready(capabilities: &UnifiedEventPayload) -> bool {
    match capabilities {
        UnifiedEventPayload::CapabilitiesUpdated {
            available_modes,
            config_options,
            ..
        } => !available_modes.is_empty() || !config_options.is_empty(),
        _ => true,
    }
}

/// [`UnifiedEventPayload::CapabilitiesUpdated`] からスラッシュコマンド名を取り出す。
fn command_names(capabilities: &UnifiedEventPayload) -> Vec<String> {
    match capabilities {
        UnifiedEventPayload::CapabilitiesUpdated {
            available_commands, ..
        } => available_commands
            .iter()
            .map(|command| command.name.clone())
            .collect(),
        _ => Vec::new(),
    }
}

/// `GET /api/command` の応答 (`{ data: [{ name, description? }] }`) を
/// [`CommandInfo`] へ変換する (`name` を持たない要素は無視する)。
fn commands_from_api(value: &Value) -> Vec<CommandInfo> {
    value
        .get("data")
        .and_then(Value::as_array)
        .map(|commands| {
            commands
                .iter()
                .filter_map(|command| {
                    Some(CommandInfo {
                        name: command.get("name")?.as_str()?.to_owned(),
                        description: command
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        input_hint: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// capabilities の `available_commands` に、ローカルコマンド `compact`
/// (要約によるコンテキスト圧縮) の合成エントリを加える。
///
/// `/compact` は opencode の TUI がローカル処理するため `GET /api/command` には
/// 現れないが、ブリッジは専用 API (`POST /api/session/{id}/compact`) で実行できる。
/// UI のスラッシュ候補・能力判定に含めるため一覧へ合成する (同名の
/// ユーザー定義コマンドが存在する場合はそれを優先する)。
///
/// 実行の振り分け ([`split_command`]) はこの合成エントリを含めず、
/// `/api/command` 由来の一覧のみを対象にすること。
fn add_compact_command(capabilities: &mut UnifiedEventPayload) {
    let UnifiedEventPayload::CapabilitiesUpdated {
        available_commands, ..
    } = capabilities
    else {
        return;
    };
    if !available_commands
        .iter()
        .any(|command| command.name == COMPACT_COMMAND)
    {
        available_commands.push(CommandInfo {
            name: COMPACT_COMMAND.to_owned(),
            description: "会話を要約してコンテキストを圧縮".to_owned(),
            input_hint: None,
        });
    }
}

/// 利用可能なモード (agent)・スラッシュコマンド・モデル選択肢を取得して
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

    let available_commands = match request(
        http,
        base_url,
        password,
        reqwest::Method::GET,
        "/api/command",
        None,
    )
    .await
    {
        Ok(value) => commands_from_api(&value),
        Err(err) => {
            tracing::debug!("failed to fetch opencode2 commands: {err:#}");
            Vec::new()
        }
    };
    // 合成エントリ `compact` はここでは加えない (振り分け用の一覧と混ざらないように
    // 送信時に [`add_compact_command`] で追加する)

    if available_modes.is_empty() && available_commands.is_empty() && config_options.is_empty() {
        return None;
    }
    Some(UnifiedEventPayload::CapabilitiesUpdated {
        current_mode: None,
        available_modes,
        available_commands,
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
/// (1つの `opencode serve` は複数セッションを多重化し得る)。
fn map_event(state: &Arc<Mutex<SessionState>>, session_id: &str, raw: &str) -> Vec<DriverEvent> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    let event_session = data
        .get("sessionID")
        .or_else(|| data.pointer("/form/sessionID"))
        .and_then(Value::as_str);
    if let Some(event_session) = event_session
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
        // コンテキスト圧縮 (`/compact` / `ControlSession(Compact)`)。
        // 圧縮自体は1ターンとして実行されるため `StatusChanged` で実行中表示も
        // 別途更新される。要約本文の `session.compaction.delta` はエフェメラル扱いで
        // イベント化しない (要約はエージェント側の履歴に残る)。
        "session.compaction.started" => {
            vec![DriverEvent::Event(UnifiedEventPayload::CompactionUpdated {
                status: CompactionStatus::Started,
                detail: None,
            })]
        }
        "session.compaction.ended" => {
            vec![DriverEvent::Event(UnifiedEventPayload::CompactionUpdated {
                status: CompactionStatus::Completed,
                detail: None,
            })]
        }
        "session.compaction.delta" => Vec::new(),
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
        // 構造化入力リクエスト (question ツール / MCP elicitation)
        "form.created" => {
            if let Some(event) = form_to_elicitation_request(&data) {
                vec![
                    DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                        status: SessionStatus::WaitingInput,
                        error_message: None,
                    }),
                    DriverEvent::Event(event),
                ]
            } else {
                Vec::new()
            }
        }
        "form.replied" => {
            let form_id = text_of("id");
            vec![
                DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Running,
                    error_message: None,
                }),
                DriverEvent::Event(UnifiedEventPayload::ElicitationResolved {
                    elicitation_id: form_id,
                    action: ElicitationAction::Accept,
                    content: data.get("answer").cloned().unwrap_or(Value::Null),
                    resolved_by: "cli".to_owned(),
                }),
            ]
        }
        "form.cancelled" => {
            let form_id = text_of("id");
            vec![
                DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Running,
                    error_message: None,
                }),
                DriverEvent::Event(UnifiedEventPayload::ElicitationResolved {
                    elicitation_id: form_id,
                    action: ElicitationAction::Cancel,
                    content: Value::Null,
                    resolved_by: "cli".to_owned(),
                }),
            ]
        }
        // 純正TUI から送信されたプロンプトは fxg 側に記録されないため、
        // 自己送信分 (SessionManager が `UserMessage` 記録済み) 以外を拾う。
        "session.inbox.enqueued" => {
            let inbox_id = text_of("inboxID");
            {
                let mut state = state.lock().expect("state poisoned");
                // `/command` は inboxID を返さないため、`/prompt` のように応答から
                // 自己送信分を判別できない。送信前に予約したカウンタを消費し、
                // ここで Revert 対応付け用の ID を記録する。
                if state.pending_commands > 0 {
                    state.pending_commands -= 1;
                    state.prompt_ids.push(inbox_id);
                    return Vec::new();
                }
                if state.prompt_ids.iter().any(|id| id == &inbox_id) {
                    return Vec::new();
                }
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

/// OpenCode2 の `form.created` イベントデータから `ElicitationRequest` payload を構築する。
fn form_to_elicitation_request(data: &Value) -> Option<UnifiedEventPayload> {
    let form = data.get("form").unwrap_or(data);
    let form_id = form.get("id").and_then(Value::as_str)?.to_owned();
    let title = form
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Questions");
    let fields = form.get("fields").and_then(Value::as_array)?;

    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    let mut message_parts = Vec::new();

    for field in fields {
        let key = field.get("key").and_then(Value::as_str).unwrap_or("");
        if key.is_empty() {
            continue;
        }
        let field_type = field
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("string");
        let field_title = field.get("title").and_then(Value::as_str);
        let field_desc = field.get("description").and_then(Value::as_str);
        // question ツールのフォームフィールドは既定で回答必須とする (明示的に false の場合のみ任意)
        let is_required = field
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        if is_required {
            required.push(Value::String(key.to_owned()));
        }

        if let Some(desc) = field_desc {
            if let Some(header) = field_title
                && !header.trim().is_empty()
            {
                message_parts.push(format!("{header}: {desc}"));
            } else {
                message_parts.push(desc.to_owned());
            }
        }

        let options = field.get("options").and_then(Value::as_array);
        let mut prop = serde_json::Map::new();
        if let Some(t) = field_title {
            prop.insert("title".to_owned(), Value::String(t.to_owned()));
        }
        if let Some(d) = field_desc {
            prop.insert("description".to_owned(), Value::String(d.to_owned()));
        }

        if field_type == "multiselect" {
            prop.insert("type".to_owned(), Value::String("array".to_owned()));
            let mut items = serde_json::Map::new();
            items.insert("type".to_owned(), Value::String("string".to_owned()));
            if let Some(opts) = options {
                let enum_values: Vec<Value> = opts
                    .iter()
                    .filter_map(|opt| {
                        opt.get("value")
                            .or_else(|| opt.get("label"))
                            .and_then(Value::as_str)
                            .map(|s| Value::String(s.to_owned()))
                    })
                    .collect();
                if !enum_values.is_empty() {
                    items.insert("enum".to_owned(), Value::Array(enum_values));
                }
            }
            prop.insert("items".to_owned(), Value::Object(items));
        } else {
            prop.insert("type".to_owned(), Value::String("string".to_owned()));
            if let Some(opts) = options {
                let one_of: Vec<Value> = opts
                    .iter()
                    .filter_map(|opt| {
                        let val = opt
                            .get("value")
                            .or_else(|| opt.get("label"))
                            .and_then(Value::as_str)?;
                        let label = opt.get("label").and_then(Value::as_str).unwrap_or(val);
                        let desc = opt.get("description").and_then(Value::as_str);
                        let title = match desc {
                            Some(d) if !d.trim().is_empty() => format!("{label} ({d})"),
                            _ => label.to_owned(),
                        };
                        Some(json!({
                            "const": val,
                            "title": title,
                        }))
                    })
                    .collect();
                if !one_of.is_empty() {
                    prop.insert("oneOf".to_owned(), Value::Array(one_of));
                }
            }
        }

        properties.insert(key.to_owned(), Value::Object(prop));
    }

    let message = if message_parts.is_empty() {
        title.to_owned()
    } else if message_parts.len() == 1 {
        message_parts[0].clone()
    } else {
        format!("{title}\n{}", message_parts.join("\n"))
    };

    let mut schema = serde_json::Map::new();
    schema.insert("type".to_owned(), Value::String("object".to_owned()));
    schema.insert("properties".to_owned(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".to_owned(), Value::Array(required));
    }

    let tool_call_id = form
        .pointer("/metadata/tool/id")
        .and_then(Value::as_str)
        .map(str::to_owned);

    Some(UnifiedEventPayload::ElicitationRequest {
        elicitation_id: form_id,
        message,
        mode: "form".to_owned(),
        requested_schema: Value::Object(schema),
        tool_call_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_opencode_major_version() {
        assert_eq!(parse_opencode_major("opencode v2.0.21\n"), Some(2));
        assert_eq!(parse_opencode_major("opencode 2.1.0"), Some(2));
        assert_eq!(parse_opencode_major("opencode v1.4.7"), Some(1));
        assert_eq!(parse_opencode_major("opencode"), None);
        assert_eq!(parse_opencode_major(""), None);
    }

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
    fn maps_form_lifecycle() {
        let state = state();
        let form_json = r#"{
            "type": "form.created",
            "data": {
                "form": {
                    "id": "frm_1",
                    "sessionID": "ses_1",
                    "title": "Questions",
                    "metadata": {
                        "kind": "question",
                        "tool": { "messageID": "msg_1", "id": "call_1" }
                    },
                    "fields": [
                        {
                            "key": "q0",
                            "title": "Layout",
                            "description": "Which footer view should be the reference?",
                            "type": "string",
                            "options": [
                                { "value": "Form", "label": "Form", "description": "Form footer" },
                                { "value": "Prompt", "label": "Prompt", "description": "Normal composer" }
                            ]
                        }
                    ]
                }
            }
        }"#;

        let created = map(&state, form_json);
        match created.as_slice() {
            [
                DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::WaitingInput,
                    ..
                }),
                DriverEvent::Event(UnifiedEventPayload::ElicitationRequest {
                    elicitation_id,
                    message,
                    mode,
                    requested_schema,
                    tool_call_id,
                }),
            ] => {
                assert_eq!(elicitation_id, "frm_1");
                assert_eq!(
                    message,
                    "Layout: Which footer view should be the reference?"
                );
                assert_eq!(mode, "form");
                assert_eq!(tool_call_id.as_deref(), Some("call_1"));
                assert!(requested_schema.pointer("/properties/q0").is_some());
                assert_eq!(
                    requested_schema
                        .pointer("/properties/q0/type")
                        .and_then(Value::as_str),
                    Some("string")
                );
                assert_eq!(
                    requested_schema
                        .pointer("/properties/q0/oneOf/0/const")
                        .and_then(Value::as_str),
                    Some("Form")
                );
                assert_eq!(
                    requested_schema
                        .pointer("/properties/q0/oneOf/0/title")
                        .and_then(Value::as_str),
                    Some("Form (Form footer)")
                );
            }
            other => panic!("unexpected events: {other:?}"),
        }

        // 他セッションの form.created は無視する
        let other_session = r#"{
            "type": "form.created",
            "data": {
                "form": {
                    "id": "frm_2",
                    "sessionID": "ses_other",
                    "fields": []
                }
            }
        }"#;
        assert!(map(&state, other_session).is_empty());

        let replied = map(
            &state,
            r#"{"type":"form.replied","data":{"sessionID":"ses_1","id":"frm_1","answer":{"q0":"Form"}}}"#,
        );
        match replied.as_slice() {
            [
                DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Running,
                    ..
                }),
                DriverEvent::Event(UnifiedEventPayload::ElicitationResolved {
                    elicitation_id,
                    action,
                    content,
                    ..
                }),
            ] => {
                assert_eq!(elicitation_id, "frm_1");
                assert_eq!(*action, ElicitationAction::Accept);
                assert_eq!(content.get("q0").and_then(Value::as_str), Some("Form"));
            }
            other => panic!("unexpected events: {other:?}"),
        }

        let cancelled = map(
            &state,
            r#"{"type":"form.cancelled","data":{"sessionID":"ses_1","id":"frm_1"}}"#,
        );
        match cancelled.as_slice() {
            [
                DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Running,
                    ..
                }),
                DriverEvent::Event(UnifiedEventPayload::ElicitationResolved {
                    elicitation_id,
                    action,
                    content,
                    ..
                }),
            ] => {
                assert_eq!(elicitation_id, "frm_1");
                assert_eq!(*action, ElicitationAction::Cancel);
                assert!(content.is_null());
            }
            other => panic!("unexpected events: {other:?}"),
        }
    }

    #[test]
    fn form_to_elicitation_request_handles_multiselect_and_custom_options() {
        let form_json = json!({
            "id": "frm_multi",
            "sessionID": "ses_1",
            "title": "Configuration",
            "fields": [
                {
                    "key": "q0",
                    "title": "Features",
                    "description": "Select features to enable",
                    "type": "multiselect",
                    "options": [
                        { "value": "feat_a", "label": "Feature A" },
                        { "value": "feat_b", "label": "Feature B" }
                    ]
                },
                {
                    "key": "q1",
                    "description": "Plain text question",
                    "type": "string",
                    "required": false
                }
            ]
        });

        let payload =
            form_to_elicitation_request(&form_json).expect("should build elicitation request");
        let UnifiedEventPayload::ElicitationRequest {
            elicitation_id,
            requested_schema,
            message,
            ..
        } = payload
        else {
            panic!("unexpected payload");
        };

        assert_eq!(elicitation_id, "frm_multi");
        assert!(message.contains("Features: Select features to enable"));
        assert!(message.contains("Plain text question"));

        // q0: multiselect => array with items.enum
        assert_eq!(
            requested_schema
                .pointer("/properties/q0/type")
                .and_then(Value::as_str),
            Some("array")
        );
        let items_enum = requested_schema
            .pointer("/properties/q0/items/enum")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(items_enum.len(), 2);
        assert_eq!(items_enum[0].as_str(), Some("feat_a"));
        assert_eq!(items_enum[1].as_str(), Some("feat_b"));

        // q0 は既定で required
        let req_array = requested_schema
            .get("required")
            .and_then(Value::as_array)
            .unwrap();
        assert!(req_array.iter().any(|v| v.as_str() == Some("q0")));
        // q1 は required: false 指定なので含まれない
        assert!(!req_array.iter().any(|v| v.as_str() == Some("q1")));
    }

    #[test]
    fn split_command_requires_known_name_and_keeps_arguments() {
        let commands = vec!["review".to_owned(), "init".to_owned()];
        assert_eq!(
            split_command("/review branch", &commands),
            Some(("review", "branch"))
        );
        assert_eq!(split_command("/review", &commands), Some(("review", "")));
        // 引数の改行は保持する (`$ARGUMENTS` へそのまま渡す)
        assert_eq!(
            split_command("/review\nline1\nline2", &commands),
            Some(("review", "line1\nline2"))
        );
        // 1 プロンプト 1 コマンド: 2 つ目以降は引数として扱う
        assert_eq!(
            split_command("/review /init", &commands),
            Some(("review", "/init"))
        );
        // 未知の名前・コマンド形式でない入力は通常のプロンプト
        assert_eq!(split_command("/unknown arg", &commands), None);
        assert_eq!(split_command("plain text", &commands), None);
        assert_eq!(split_command("/", &commands), None);
        assert_eq!(split_command("/review branch", &[]), None);
    }

    #[test]
    fn commands_from_api_skips_entries_without_name() {
        let commands = commands_from_api(&json!({
            "data": [
                { "name": "init", "description": "setup" },
                { "name": "ping" },
                { "description": "no name" },
            ]
        }));
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].name, "init");
        assert_eq!(commands[0].description, "setup");
        assert_eq!(commands[1].name, "ping");
        assert_eq!(commands[1].description, "");
        assert!(commands_from_api(&json!({})).is_empty());
    }

    #[test]
    fn is_compact_command_requires_slash_and_exact_name() {
        assert!(is_compact_command("/compact"));
        assert!(is_compact_command("/compact 引数は無視される"));
        assert!(is_compact_command("  /compact"));
        assert!(!is_compact_command("/compactx"));
        assert!(!is_compact_command("/other"));
        assert!(!is_compact_command("compact"));
        assert!(!is_compact_command("/"));
    }

    #[test]
    fn add_compact_command_appends_synthetic_entry_once() {
        let mut capabilities = UnifiedEventPayload::CapabilitiesUpdated {
            current_mode: None,
            available_modes: Vec::new(),
            available_commands: commands_from_api(&json!({
                "data": [{ "name": "init" }]
            })),
            config_options: Vec::new(),
        };
        add_compact_command(&mut capabilities);
        let UnifiedEventPayload::CapabilitiesUpdated {
            available_commands, ..
        } = &capabilities
        else {
            panic!("unexpected payload");
        };
        assert_eq!(available_commands.len(), 2);
        assert_eq!(available_commands[1].name, "compact");
        assert!(!available_commands[1].description.is_empty());

        // ユーザー定義の `compact` が存在する場合は合成しない
        let mut capabilities = UnifiedEventPayload::CapabilitiesUpdated {
            current_mode: None,
            available_modes: Vec::new(),
            available_commands: commands_from_api(&json!({
                "data": [{ "name": "compact", "description": "custom" }]
            })),
            config_options: Vec::new(),
        };
        add_compact_command(&mut capabilities);
        let UnifiedEventPayload::CapabilitiesUpdated {
            available_commands, ..
        } = &capabilities
        else {
            panic!("unexpected payload");
        };
        assert_eq!(available_commands.len(), 1);
        assert_eq!(available_commands[0].description, "custom");
    }

    #[test]
    fn maps_compaction_lifecycle() {
        let state = state();
        let started = map(
            &state,
            r#"{"type":"session.compaction.started","data":{"sessionID":"ses_1","reason":"manual","recent":"","inputID":"msg_1"}}"#,
        );
        assert!(matches!(
            started.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::CompactionUpdated {
                status: CompactionStatus::Started,
                detail: None,
            })]
        ));
        // 要約本文のストリーミングはエフェメラル扱いでイベント化しない
        assert!(
            map(
                &state,
                r#"{"type":"session.compaction.delta","data":{"sessionID":"ses_1","text":"part"}}"#,
            )
            .is_empty()
        );
        let ended = map(
            &state,
            r#"{"type":"session.compaction.ended","data":{"sessionID":"ses_1","reason":"manual","text":"summary"}}"#,
        );
        assert!(matches!(
            ended.as_slice(),
            [DriverEvent::Event(UnifiedEventPayload::CompactionUpdated {
                status: CompactionStatus::Completed,
                detail: None,
            })]
        ));
        // 他セッションのイベントは無視する
        assert!(
            map(
                &state,
                r#"{"type":"session.compaction.started","data":{"sessionID":"ses_2"}}"#,
            )
            .is_empty()
        );
    }

    #[test]
    fn maps_command_turns_from_sse_without_duplicate() {
        let state = state();
        state.lock().expect("state").pending_commands = 1;
        let raw = r#"{"type":"session.inbox.enqueued","data":{"sessionID":"ses_1","inboxID":"msg_cmd","item":{"type":"user","payload":{"text":"expanded template"}}}}"#;
        // 予約カウンタを消費しつつ、Revert 対応付け用の ID を記録する
        assert!(map(&state, raw).is_empty());
        {
            let guard = state.lock().expect("state");
            assert_eq!(guard.pending_commands, 0);
            assert_eq!(guard.prompt_ids, vec!["msg_cmd".to_owned()]);
        }
        // 同じイベントが再送されても重複記録しない
        assert!(map(&state, raw).is_empty());
        assert_eq!(
            state.lock().expect("state").prompt_ids,
            vec!["msg_cmd".to_owned()]
        );
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
                program: PathBuf::from("opencode"),
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

    /// 実 opencode2 でモデルを選択する (既定モデルが利用できない環境向け)。
    ///
    /// `/api/model` から公開プロバイダ (`opencode`) のモデルを優先して選び、
    /// 明示的に設定する。利用可能なモデルが無い場合は何もしない。
    async fn select_opencode_model(handle: &dyn ActiveSessionHandle) {
        let Some(info) = handle.native_attach() else {
            return;
        };
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

    /// スラッシュコマンドの一覧取得・実行・自己送信の重複防止を実 opencode2 で
    /// 確認する: `FXG_TEST_OPENCODE2=1 cargo test -p fxg-acp -- --ignored` で実行する。
    #[tokio::test]
    #[ignore = "requires a local opencode2 install and provider auth"]
    async fn runs_slash_command_against_real_opencode2() {
        use std::path::PathBuf;

        use crate::driver::AgentLaunchSpec;

        let root = tempfile::tempdir().expect("tempdir");
        let command_dir = root.path().join(".opencode/commands");
        std::fs::create_dir_all(&command_dir).expect("create command dir");
        std::fs::write(
            command_dir.join("ping.md"),
            "---\ndescription: ping test\n---\nReply with exactly: pong [$ARGUMENTS]\n",
        )
        .expect("write command");

        let driver = OpenCode2Driver::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let started = driver
            .start_session(
                StartSessionRequest {
                    session_id: "test-command".to_owned(),
                    title: Some("fxg command test".to_owned()),
                    cwd: PathBuf::from(root.path()),
                    launch: AgentLaunchSpec {
                        agent_id: "opencode2".to_owned(),
                        display_name: "OpenCode2".to_owned(),
                        driver_kind: "opencode2".to_owned(),
                        program: PathBuf::from("opencode"),
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
        let handle = started.handle;
        select_opencode_model(handle.as_ref()).await;

        // `GET /api/command` の一覧 (プロジェクトのカスタムコマンドを含む) が
        // 起動時の capabilities に含まれること
        let mut commands = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some(DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
                    available_commands,
                    ..
                }))) => {
                    commands = available_commands
                        .into_iter()
                        .map(|command| command.name)
                        .collect();
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            commands.iter().any(|name| name == "ping"),
            "custom command must be listed: {commands:?}"
        );

        // `/ping hello world` が `/command` として実行され、`$ARGUMENTS` が展開された
        // ユーザーメッセージが opencode2 側に残ること (LLM 応答の内容には依存しない)
        handle
            .send_prompt("/ping hello world".to_owned())
            .await
            .expect("send command");
        let info = handle.native_attach().expect("native attach");
        let password = info
            .env
            .iter()
            .find(|(key, _)| key == PASSWORD_ENV)
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut expanded = false;
        while tokio::time::Instant::now() < deadline {
            // 自己送信分のターンは SSE から二重記録されないこと
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(250), rx.recv()).await
            {
                assert!(
                    !matches!(
                        event,
                        DriverEvent::Event(UnifiedEventPayload::UserMessage { .. })
                    ),
                    "command turn must not be re-recorded from SSE: {event:?}"
                );
            }
            let messages = request(
                &client,
                &info.server_url,
                &password,
                reqwest::Method::GET,
                &format!("/api/session/{}/message", info.session_id),
                None,
            )
            .await
            .expect("messages");
            expanded = messages
                .get("data")
                .and_then(Value::as_array)
                .map(|messages| {
                    messages.iter().any(|message| {
                        message.get("type").and_then(Value::as_str) == Some("user")
                            && message.get("text").and_then(Value::as_str)
                                == Some("Reply with exactly: pong [hello world]")
                    })
                })
                .unwrap_or_default();
            if expanded {
                break;
            }
        }
        assert!(expanded, "command template must be expanded with arguments");
        handle.shutdown().await.expect("shutdown");
    }

    /// `/compact` (専用 API) によるコンテキスト圧縮を実 opencode2 で確認する:
    /// `FXG_TEST_OPENCODE2=1 cargo test -p fxg-acp -- --ignored` で実行する。
    #[tokio::test]
    #[ignore = "requires a local opencode2 install and provider auth"]
    async fn compacts_context_against_real_opencode2() {
        use std::path::PathBuf;

        use crate::driver::AgentLaunchSpec;

        let root = tempfile::tempdir().expect("tempdir");
        let driver = OpenCode2Driver::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let started = driver
            .start_session(
                StartSessionRequest {
                    session_id: "test-compact".to_owned(),
                    title: Some("fxg compact test".to_owned()),
                    cwd: PathBuf::from(root.path()),
                    launch: AgentLaunchSpec {
                        agent_id: "opencode2".to_owned(),
                        display_name: "OpenCode2".to_owned(),
                        driver_kind: "opencode2".to_owned(),
                        program: PathBuf::from("opencode"),
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
        let handle = started.handle;

        // 合成エントリ `compact` が capabilities に含まれること
        let mut has_compact = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await {
                Ok(Some(DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
                    available_commands,
                    ..
                }))) => {
                    has_compact = available_commands
                        .iter()
                        .any(|command| command.name == "compact");
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(has_compact, "synthetic compact command must be listed");

        select_opencode_model(handle.as_ref()).await;
        // 圧縮対象の会話を作る (内容は問わない) ため1ターン完了を待つ
        handle
            .send_prompt("Reply with exactly one word: hello".to_owned())
            .await
            .expect("send_prompt");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        let mut first_turn_done = false;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
                Ok(Some(DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                    is_complete: true,
                    ..
                }))) => {
                    first_turn_done = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            first_turn_done,
            "first turn must complete before compacting"
        );

        // `/compact` 入力が専用 API へ振り分けられ、開始/完了イベントが届くこと
        handle
            .send_prompt("/compact".to_owned())
            .await
            .expect("send compact");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        let (mut started_event, mut completed_event) = (false, false);
        while tokio::time::Instant::now() < deadline && !(started_event && completed_event) {
            match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
                Ok(Some(DriverEvent::Event(UnifiedEventPayload::CompactionUpdated {
                    status,
                    ..
                }))) => match status {
                    CompactionStatus::Started => started_event = true,
                    CompactionStatus::Completed => completed_event = true,
                    CompactionStatus::Failed => panic!("compaction failed"),
                },
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(started_event, "session.compaction.started must be mapped");
        assert!(completed_event, "session.compaction.ended must be mapped");
        handle.shutdown().await.expect("shutdown");
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
                        program: PathBuf::from("opencode"),
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
        let mut command_names = Vec::new();
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await {
                Ok(Some(DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
                    config_options,
                    available_commands,
                    ..
                }))) => {
                    has_model_options = config_options
                        .iter()
                        .any(|option| option.key == "model" && !option.options.is_empty());
                    command_names = available_commands
                        .into_iter()
                        .map(|command| command.name)
                        .collect();
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
        // `GET /api/command` の一覧 (組み込みの `init` / `review` を含む) が
        // 起動時の capabilities に含まれること
        assert!(
            command_names.iter().any(|name| name == "init"),
            "available commands must include built-ins: {command_names:?}"
        );
        // opencode2 の既定モデルは環境によっては利用できないため明示的に設定する。
        select_opencode_model(handle.as_ref()).await;
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
            program: PathBuf::from("opencode"),
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
