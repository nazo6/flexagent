//! テスト用ヘルパー (`cfg(test)` のみ)。
//!
//! 一時ディレクトリに Git リポジトリを構築するなど、複数モジュールの
//! テストで共有する処理をまとめる。

#![cfg(test)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use fxg_acp::{
    ActiveSessionHandle, AgentDriver, AgentLaunchSpec, DriverEvent, ResumeRequest,
    StartSessionRequest, StartedSession,
};
use tokio::sync::mpsc;

use crate::git;
use crate::session_manager::DriverFactory;

/// 一時ディレクトリに Git リポジトリを作成し、1コミット入れておく。
pub(crate) async fn init_test_repo(dir: &Path) {
    git::run_git(dir, &["init", "-q", "-b", "main"])
        .await
        .expect("git init");
    git::run_git(dir, &["config", "user.email", "test@example.com"])
        .await
        .expect("git config email");
    git::run_git(dir, &["config", "user.name", "fxg test"])
        .await
        .expect("git config name");
    std::fs::write(dir.join("README.md"), "# test\n").expect("write readme");
    git::run_git(dir, &["add", "-A"]).await.expect("git add");
    git::run_git(dir, &["commit", "-q", "-m", "initial"])
        .await
        .expect("git commit");
}

/// Git リポジトリを作成し、`origin` remote を設定する。
pub(crate) async fn init_test_repo_with_remote(dir: &Path, remote_url: &str) {
    init_test_repo(dir).await;
    git::run_git(dir, &["remote", "add", "origin", remote_url])
        .await
        .expect("git remote add");
}

/// テスト用の Shadow Git Index パスを作る (一時ディレクトリ配下)。
pub(crate) fn snapshot_index_path(dir: &Path, session_id: &str) -> std::path::PathBuf {
    dir.join(format!("{session_id}.index"))
}

/// テスト用のローカルIPCエンドポイントを返す。
///
/// - **Windows**: Named Pipe (`\\.\pipe\fxg-test-<pid>-<n>`)。テストは同一
///   プロセス内で並列実行されるため、連番で一意化する。
/// - **Unix**: `dir` 配下の一時ソケット (テストごとに個別の tempdir を使う)。
pub(crate) fn test_ipc_endpoint(dir: &Path) -> String {
    if cfg!(windows) {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!(r"\\.\pipe\fxg-test-{}-{n}", std::process::id())
    } else {
        dir.join("daemon.sock").to_string_lossy().into_owned()
    }
}

/// `fxg_home` の `config.toml` にテスト用カスタムエージェント (`mock`) を書き込む。
///
/// デーモン経由の `EnsureSession` はレジストリから起動スペックを解決するため、
/// モックドライバのテストでもエージェント定義が必要になる。
pub(crate) fn write_mock_agent_config(fxg_home: &Path) {
    std::fs::write(
        fxg_home.join(fxg_protocol::config::CONFIG_FILE_NAME),
        "[agents.custom.mock]\nname = \"Mock Agent\"\ncommand = \"mock-agent\"\n",
    )
    .expect("write config.toml");
}

/// テスト用モックエージェントドライバの共有状態 (ドライバ本体も兼ねる)。
///
/// 実プロセスを起動せず、送信されたプロンプト・承認応答・モード変更を記録し、
/// テストから [`Self::emit`] でドライバイベント (ターン進行・承認要求等) を
/// 注入できる。`SessionManager` へは [`Self::factory`] で渡す。
#[derive(Clone, Default)]
pub(crate) struct MockAgent {
    inner: Arc<MockAgentInner>,
}

/// テスト記録用の `start_session` 呼び出し情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MockStart {
    /// 対象セッションID
    pub session_id: String,
    /// 作業ディレクトリ
    pub cwd: PathBuf,
    /// 再開指定 (`None` は新規セッション)
    pub resume: Option<ResumeRequest>,
}

#[derive(Default)]
struct MockAgentInner {
    prompts: Mutex<Vec<String>>,
    permissions: Mutex<Vec<(String, String)>>,
    elicitations: Mutex<
        Vec<(
            String,
            fxg_protocol::common::ElicitationAction,
            serde_json::Value,
        )>,
    >,
    modes: Mutex<Vec<String>>,
    reverted: Mutex<Vec<u64>>,
    events: Mutex<Vec<mpsc::UnboundedSender<DriverEvent>>>,
    starts: Mutex<Vec<MockStart>>,
    /// ネイティブ復元 (resume) に対応しているか
    resume_supported: AtomicBool,
}

impl MockAgent {
    /// 稼働中セッションへドライバイベントを注入する。
    pub(crate) fn emit(&self, event: DriverEvent) {
        for tx in self.inner.events.lock().expect("events").iter() {
            let _ = tx.send(event.clone());
        }
    }

    /// `start_session` の呼び出し一覧。
    pub(crate) fn starts(&self) -> Vec<MockStart> {
        self.inner.starts.lock().expect("starts").clone()
    }

    /// ネイティブ復元 (resume) への対応可否を設定する。
    pub(crate) fn set_resume_supported(&self, supported: bool) {
        self.inner
            .resume_supported
            .store(supported, Ordering::SeqCst);
    }

    /// 送信されたプロンプト一覧。
    pub(crate) fn prompts(&self) -> Vec<String> {
        self.inner.prompts.lock().expect("prompts").clone()
    }

    /// 稼働中セッションのイベントチャネルを閉じる (ドライバ終了の再現)。
    pub(crate) fn close_events(&self) {
        self.inner.events.lock().expect("events").clear();
    }

    /// 承認応答 (`request_id`, `option_id`) 一覧。
    pub(crate) fn permissions(&self) -> Vec<(String, String)> {
        self.inner.permissions.lock().expect("permissions").clone()
    }

    /// elicitation 応答 (`elicitation_id`, `action`, `content`) 一覧。
    pub(crate) fn elicitations(
        &self,
    ) -> Vec<(
        String,
        fxg_protocol::common::ElicitationAction,
        serde_json::Value,
    )> {
        self.inner
            .elicitations
            .lock()
            .expect("elicitations")
            .clone()
    }

    /// 適用されたモード変更一覧。
    pub(crate) fn modes(&self) -> Vec<String> {
        self.inner.modes.lock().expect("modes").clone()
    }

    /// `revert_context` が呼ばれた `keep_turns` (先頭から残すターン数) 一覧。
    pub(crate) fn reverted(&self) -> Vec<u64> {
        self.inner.reverted.lock().expect("reverted").clone()
    }

    /// `SessionManager` へ渡すドライバファクトリ。
    pub(crate) fn factory(&self) -> DriverFactory {
        let agent = self.clone();
        Arc::new(move |_spec: &AgentLaunchSpec| Ok(Arc::new(agent.clone()) as Arc<dyn AgentDriver>))
    }
}

#[async_trait]
impl AgentDriver for MockAgent {
    fn driver_kind(&self) -> &'static str {
        "mock"
    }

    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession> {
        // ネイティブ復元対応が有効で、エージェント側IDがある場合のみ復元成功とする
        let can_restore = req.resume.as_ref().is_some_and(|resume| {
            resume.agent_session_id.is_some() && self.inner.resume_supported.load(Ordering::SeqCst)
        });
        // ネイティブ限定 (`allow_fresh = false`) で復元できない場合は実ドライバと
        // 同様に `NativeResumeUnavailable` を返す (セッションは作成しない)
        if req
            .resume
            .as_ref()
            .is_some_and(|resume| !resume.allow_fresh)
            && !can_restore
        {
            return Err(
                fxg_acp::NativeResumeUnavailable("mock resume unsupported".to_owned()).into(),
            );
        }
        self.inner.events.lock().expect("events").push(event_tx);
        self.inner.starts.lock().expect("starts").push(MockStart {
            session_id: req.session_id,
            cwd: req.cwd,
            resume: req.resume,
        });
        Ok(StartedSession {
            handle: Box::new(self.clone()),
            context_restored: can_restore,
        })
    }
}

#[async_trait]
impl ActiveSessionHandle for MockAgent {
    async fn send_prompt(&self, text: String) -> anyhow::Result<()> {
        self.inner.prompts.lock().expect("prompts").push(text);
        Ok(())
    }

    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()> {
        self.inner
            .permissions
            .lock()
            .expect("permissions")
            .push((request_id, selected_option_id));
        Ok(())
    }

    async fn respond_elicitation(
        &self,
        elicitation_id: String,
        action: fxg_protocol::common::ElicitationAction,
        content: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.inner.elicitations.lock().expect("elicitations").push((
            elicitation_id,
            action,
            content,
        ));
        Ok(())
    }

    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()> {
        self.inner.modes.lock().expect("modes").push(mode_id);
        Ok(())
    }

    async fn set_config(&self, _key: String, _value: serde_json::Value) -> anyhow::Result<()> {
        Ok(())
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn revert_context(&self, keep_turns: u64) -> anyhow::Result<()> {
        self.inner
            .reverted
            .lock()
            .expect("reverted")
            .push(keep_turns);
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
