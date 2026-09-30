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
use fxg_protocol::common::StreamDeltaPayload;
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

/// セッション開始要求。
#[derive(Debug, Clone)]
pub struct StartSessionRequest {
    /// fxg 側のセッションID (UUID v7)
    pub session_id: String,
    /// 作業ディレクトリ (セッションの `local_path` / Worktree パス)
    pub cwd: PathBuf,
    /// 起動スペック (解決済み)
    pub launch: AgentLaunchSpec,
    /// エージェントプロセスへのパススルー引数 (`fxg run <agent> -- ...`)
    pub extra_args: Vec<String>,
    /// 初期モード (`--mode` 指定時。ACP の `session/set_mode` で適用)
    pub initial_mode: Option<String>,
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
    /// 以降のイベントは `event_tx` へ送出される。
    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<Box<dyn ActiveSessionHandle>>;
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
    /// モード変更 (`plan` / `code` 等)。
    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()>;
    /// 設定変更 (モデル選択等)。
    async fn set_config(&self, key: String, value: serde_json::Value) -> anyhow::Result<()>;
    /// 現在のターンの中断。
    async fn cancel_turn(&self) -> anyhow::Result<()>;
    /// エージェント側の会話コンテキストを指定ターン時点へ巻き戻す。
    ///
    /// 標準ACPには会話を巻き戻す API が無いため、既定実装は「未対応」を返す
    /// (`fxg` はワークスペースのファイル復元のみを行い、この失敗は警告として扱う)。
    /// ネイティブ API を持つドライバ (OpenCode2 の `POST /session/{id}/revert`
    /// 等) が実装する。
    async fn revert_context(&self, _target_node_seq: u64) -> anyhow::Result<()> {
        anyhow::bail!("revert_context is not supported by this driver")
    }
    /// セッションプロセスの完全終了 (プロセスツリーごと)。
    async fn shutdown(&self) -> anyhow::Result<()>;
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
        ) -> anyhow::Result<Box<dyn ActiveSessionHandle>> {
            event_tx
                .send(DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                    message_id: "m1".to_owned(),
                    text: "hello".to_owned(),
                    is_complete: true,
                }))
                .expect("event_tx");
            assert_eq!(req.launch.driver_kind, "mock");
            Ok(Box::new(MockHandle))
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
                    cwd: PathBuf::from("."),
                    launch,
                    extra_args: vec![],
                    initial_mode: None,
                },
                tx,
            )
            .await
            .expect("start_session");
        handle.send_prompt("hi".to_owned()).await.expect("prompt");
        match rx.recv().await.expect("event") {
            DriverEvent::Event(payload) => {
                assert_eq!(payload.event_type(), "agent_message");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
