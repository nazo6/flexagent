//! エージェントドライバ抽象化 (`AgentDriver` / `ActiveSessionHandle`)。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §1。
//!
//! `fxg-node` のセッションマネージャはこのトレイトにのみ依存し、
//! ACP (`AcpDriver`) / OpenCode2 (`OpenCode2Driver`) の差を吸収する。
//! ドライバは正規化済みイベント ([`UnifiedEventPayload`] /
//! [`StreamDeltaPayload`]) を [`DriverEvent`] として送出し、永続化・配信は
//! セッションマネージャ (イベントバス) が一元して行う。

use std::path::PathBuf;

use async_trait::async_trait;
use fxg_protocol::common::{ElicitationAction, StreamDeltaPayload};
use fxg_protocol::events::UnifiedEventPayload;
use tokio::sync::mpsc;

/// エージェントプロセスの起動方法 (ACP Registry / カスタム定義から解決済み)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLaunchSpec {
    /// 解決後のエージェントID (registry id / カスタム名 / `opencode2`)
    pub agent_id: String,
    /// 表示名
    pub display_name: String,
    /// ドライバ種別 ([`AgentDriver::driver_kind`] の値: `"acp"` / `"opencode2"`)
    pub driver_kind: String,
    /// 実行ファイル (絶対パス、または `PATH` 上のコマンド名)。
    /// `PATH` 解決 (`which` / `PATHEXT`) は起動直前にドライバが行う。
    pub program: PathBuf,
    /// コマンド引数 (base。パススルー引数は [`StartSessionRequest::extra_args`])
    pub args: Vec<String>,
    /// プロセスへ追加注入する環境変数 (親の環境を継承した上で上書き)
    pub env: Vec<(String, String)>,
}

/// 既存エージェントセッションからの再開指定 ([`StartSessionRequest::resume`])。
///
/// 設計: `docs/04-agent-drivers-and-windows.md` §4.3。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeRequest {
    /// 記録済みのエージェント内部セッションID (`SessionAgentBound` の値)。
    /// `None` は新規セッション作成 (呼び出し側の履歴 Replay 注入前提)。
    pub agent_session_id: Option<String>,
    /// ネイティブ復元ができない場合に新規セッション作成を許容するか
    /// (`false` の場合は [`NativeResumeUnavailable`] を返す)。
    pub allow_fresh: bool,
}

/// ネイティブ復元 (`session/resume` / `session/load` / opencode2 既存セッション
/// bind) が利用できないため、新規セッションを作成せずに再開を中断した。
///
/// [`ResumeRequest::allow_fresh`] が `false` のときのみ返される。呼び出し側
/// (ノード) はこの型へダウンキャストして判別し、履歴 Replay での明示的な
/// 再開を案内する (`ErrorCode::RESUME_REQUIRED`)。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("native session resume is not available: {0}")]
pub struct NativeResumeUnavailable(pub String);

/// セッション開始要求。
#[derive(Debug, Clone)]
pub struct StartSessionRequest {
    /// fxg 側のセッションID (UUID v7)
    pub session_id: String,
    /// セッション表示名 (`fxg` の `SessionCreated.title` と同一。
    /// エージェント側のセッション名として使えるドライバのみ利用する)。
    pub title: Option<String>,
    /// 作業ディレクトリ (セッションの `local_path` / Worktree パス)
    pub cwd: PathBuf,
    /// 起動スペック (解決済み)
    pub launch: AgentLaunchSpec,
    /// エージェントプロセスへのパススルー引数 (`fxg run <agent> -- ...`)
    pub extra_args: Vec<String>,
    /// 初期モード (`--mode` 指定時。ACP の `session/set_mode` で適用)
    pub initial_mode: Option<String>,
    /// 既存エージェントセッションからの再開指定 (`None` は新規セッション)
    pub resume: Option<ResumeRequest>,
}

/// [`AgentDriver::start_session`] の結果。
pub struct StartedSession {
    /// 起動済みセッションの操作ハンドル
    pub handle: Box<dyn ActiveSessionHandle>,
    /// `true` = エージェント側コンテキストをネイティブ復元した
    /// (`session/resume` / `session/load` / opencode2 既存セッション bind)。
    /// `false` = 新規エージェントセッションを作成した
    /// (呼び出し側は履歴 Replay を注入してコンテキストを引き継ぐ)。
    pub context_restored: bool,
}

/// ドライバからセッションマネージャへ送出されるイベント。
///
/// 設計 (docs/04 §1) の `mpsc::Sender<DriverEvent>` から
/// [`mpsc::UnboundedSender`] へ変更している (ドライバは接続イベントループ上で
/// 動くため、送信でブロックしないことを保証する)。
#[derive(Debug, Clone)]
pub enum DriverEvent {
    /// 永続化対象の完全イベント (`node_seq` 採番・配信はマネージャが行う)
    Event(UnifiedEventPayload),
    /// ストリーミング途中の差分 (メモリ配信のみ。永続化しない)
    Delta(StreamDeltaPayload),
    /// ドライバ側の異常 (セッションを `error` 状態へ遷移させる)
    Failed {
        /// エラーメッセージ
        message: String,
    },
}

/// エージェントドライバ (ACP / OpenCode2 等)。
#[async_trait]
pub trait AgentDriver: Send + Sync {
    /// ドライバ識別子 (`"acp"` / `"opencode2"`)。
    fn driver_kind(&self) -> &'static str;

    /// 新規セッションを起動し、操作ハンドルを返す。
    ///
    /// `req.resume` が指定されている場合、ドライバはネイティブ復元
    /// (`session/resume` / `session/load` / opencode2 既存セッション bind) を
    /// 試み、成否を [`StartedSession::context_restored`] で返す。
    /// ネイティブ復元できない場合の履歴 Replay 注入は呼び出し側の責務である。
    ///
    /// 以降のイベントは `event_tx` へ送出される。
    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession>;

    /// アイドル状態 (セッションが接続していない) のエージェントプロセスを
    /// すべて破棄する。
    ///
    /// `fxg kill-all` / デーモン終了時に呼ばれる。プロセスをセッション終了後も
    /// 温存するドライバは、ここでそれらを確実に終了させる。戻り値は破棄した数。
    /// プロセスを保持しないドライバの既定実装は 0 (何もしない)。
    async fn shutdown_idle(&self) -> usize {
        0
    }
}

/// 起動済みセッションの操作ハンドル。
#[async_trait]
pub trait ActiveSessionHandle: Send + Sync {
    /// プロンプト送信 (スラッシュコマンド含む)。
    async fn send_prompt(&self, text: String) -> anyhow::Result<()>;
    /// 権限リクエストへの応答 (`ALREADY_RESOLVED` 判定はマネージャ側)。
    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()>;
    /// elicitation (構造化入力リクエスト) への応答 (`accept` / `decline` / `cancel`)。
    ///
    /// `accept` の `content` はリクエストの `requested_schema` (form モード) に
    /// 準拠すること。未対応ドライバの既定実装はエラーを返す。
    async fn respond_elicitation(
        &self,
        _elicitation_id: String,
        _action: ElicitationAction,
        _content: serde_json::Value,
    ) -> anyhow::Result<()> {
        anyhow::bail!("respond_elicitation is not supported by this driver")
    }
    /// モード変更 (`plan` / `code` 等)。
    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()>;
    /// 設定変更 (モデル選択等)。
    async fn set_config(&self, key: String, value: serde_json::Value) -> anyhow::Result<()>;
    /// 現在のターンの中断。
    async fn cancel_turn(&self) -> anyhow::Result<()>;
    /// エージェント側の会話コンテキストを、先頭から `keep_turns` ターン分だけ
    /// 残した状態へ巻き戻す。
    ///
    /// `fxg` のファイル復元 (Shadow Git Tree) とは独立に、エージェント内部の
    /// 会話履歴を巻き戻すためのフック。標準ACPには会話を巻き戻す API が無いため
    /// 既定実装は「未対応」を返す (`fxg` はファイル復元のみを行い、この失敗は
    /// 情報ログとして扱う)。ネイティブ API を持つドライバ (OpenCode2 の
    /// `POST /api/session/{id}/revert` 等) が実装する。
    async fn revert_context(&self, _keep_turns: u64) -> anyhow::Result<()> {
        anyhow::bail!("revert_context is not supported by this driver")
    }
    /// エージェント純正 TUI へ Attach するための情報 (OpenCode2 ブリッジのみ)。
    ///
    /// `Some` を返すドライバでは、CLI は `fxg` の内蔵 TUI ではなく
    /// 純正 CLI を `server_url` / `session_id` 付きで子プロセス実行する
    /// ([`crate::driver::NativeAttachInfo`])。
    fn native_attach(&self) -> Option<NativeAttachInfo> {
        None
    }
    /// セッションプロセスの完全終了 (プロセスツリーごと)。
    ///
    /// プロセスをセッション終了後も温存するドライバ (`AcpDriver`) は、
    /// ここで**温存せず破棄**する。`fxg kill-all` / デーモン終了時に使う。
    async fn dispose(&self) -> anyhow::Result<()> {
        self.shutdown().await
    }

    /// セッションの終了。
    ///
    /// プロセスをセッション終了後も温存するドライバでは、セッションを閉じて
    /// プロセスを再利用プールへ返す (完全終了は [`Self::dispose`])。
    async fn shutdown(&self) -> anyhow::Result<()>;
}

/// エージェント純正 TUI へ Attach するための情報。
///
/// CLI は `server_url` / `session_id` を引数に、`env` を環境変数として
/// 純正 CLI (例: `opencode2 run --server <url> --session <id>`) を起動する。
/// `env` にはローカルサーバー用の一時クレデンシャルが含まれ得るため、
/// ローカルIPC (トークン認証済み) の応答としてのみ受け渡すこと。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAttachInfo {
    /// アタッチ先のローカルサーバーURL (例: `http://127.0.0.1:38219`)
    pub server_url: String,
    /// アタッチ対象のエージェント側セッションID (例: `ses_...`)
    pub session_id: String,
    /// 純正 CLI へ注入する環境変数 (例: `OPENCODE_PASSWORD`)
    pub env: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// トレイトがオブジェクト安全で、モック実装から `Box<dyn ...>` を
    /// 返せることを検証する。
    struct MockDriver;

    struct MockHandle;

    #[async_trait]
    impl ActiveSessionHandle for MockHandle {
        async fn send_prompt(&self, _text: String) -> anyhow::Result<()> {
            Ok(())
        }
        async fn respond_permission(
            &self,
            _request_id: String,
            _selected_option_id: String,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn set_mode(&self, _mode_id: String) -> anyhow::Result<()> {
            Ok(())
        }
        async fn set_config(&self, _key: String, _value: serde_json::Value) -> anyhow::Result<()> {
            Ok(())
        }
        async fn cancel_turn(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn shutdown(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl AgentDriver for MockDriver {
        fn driver_kind(&self) -> &'static str {
            "mock"
        }

        async fn start_session(
            &self,
            req: StartSessionRequest,
            event_tx: mpsc::UnboundedSender<DriverEvent>,
        ) -> anyhow::Result<StartedSession> {
            event_tx
                .send(DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                    message_id: "m1".to_owned(),
                    text: "hello".to_owned(),
                    is_complete: true,
                }))
                .expect("event_tx");
            assert_eq!(req.launch.driver_kind, "mock");
            Ok(StartedSession {
                handle: Box::new(MockHandle),
                context_restored: false,
            })
        }
    }

    #[tokio::test]
    async fn driver_trait_is_object_safe_and_emits_events() {
        let driver: Box<dyn AgentDriver> = Box::new(MockDriver);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let launch = AgentLaunchSpec {
            agent_id: "mock".to_owned(),
            display_name: "Mock".to_owned(),
            driver_kind: "mock".to_owned(),
            program: PathBuf::from("mock"),
            args: vec![],
            env: vec![],
        };
        let handle = driver
            .start_session(
                StartSessionRequest {
                    session_id: "s".to_owned(),
                    title: None,
                    cwd: PathBuf::from("."),
                    launch,
                    extra_args: vec![],
                    initial_mode: None,
                    resume: None,
                },
                tx,
            )
            .await
            .expect("start_session");
        assert!(!handle.context_restored);
        handle
            .handle
            .send_prompt("hi".to_owned())
            .await
            .expect("prompt");
        match rx.recv().await.expect("event") {
            DriverEvent::Event(payload) => {
                assert_eq!(payload.event_type(), "agent_message");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
