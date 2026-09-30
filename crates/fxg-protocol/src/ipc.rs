//! CLI (`fxg`) ⇔ Local Daemon (`fxg daemon`) 間 ローカルIPC メッセージ。
//!
//! - **トランスポート**: Windows は Named Pipe (`\\.\pipe\fxg-daemon-<username>`)、
//!   Linux / macOS / WSL は Unix Domain Socket
//!   (`$XDG_RUNTIME_DIR/fxg/daemon.sock` または `/tmp/fxg-<uid>/daemon.sock`)
//! - **フレーミング**: Length-prefixed JSON
//!   ([`IPC_LENGTH_PREFIX_BYTES`] バイトのリトルエンディアン長さヘッダ + JSON)
//!
//! サーバー実装の内部プロトコルであり UI には公開しないため `ts-rs` の
//! export 対象外とする (設計: `docs/03-protocol-and-api.md` §4)。

use serde::{Deserialize, Serialize};

use crate::common::{ErrorCode, SessionControlAction, SessionSummary, StreamDeltaPayload};
use crate::events::SessionEventEnvelope;

/// Length-prefixed JSON の長さヘッダ長 (リトルエンディアン u32)。
pub const IPC_LENGTH_PREFIX_BYTES: usize = 4;

/// `EnsureSession` の応答で返されるアタッチモード。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AttachMode {
    /// `fxg` CLI 自身の内蔵TUI (`ratatui`) でIPCストリームを描画するモード。
    AcpTui,
    /// デーモンが管理する `opencode2 serve` に対して CLI が
    /// `opencode2 run --attach <server_url> --session <id>` を子プロセス実行し、
    /// 純正TUIを直接表示するモード。
    NativeOpenCodeAttach {
        /// `opencode2 serve` のローカルURL
        server_url: String,
        /// アタッチ対象のセッションID
        session_id: String,
    },
}

/// ローカルノードの稼働状態 (`GetLocalStatus` 応答)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalStatus {
    /// ローカルで稼働中・最近のセッション一覧
    pub sessions: Vec<SessionSummary>,
    /// 中央サーバーへの WebSocket 接続状態
    pub central_connected: bool,
    /// 未送信 Outbox イベント件数
    pub unsynced_event_count: u64,
    /// 最終同期日時 (Unix epoch ms)
    pub last_synced_at: Option<i64>,
}

/// CLI → Daemon の要求メッセージ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum IpcClientMessage {
    /// `fxg run <agent>` 実行時に呼ばれ、デーモン側でセッションを開始する。
    EnsureSession {
        /// 相関ID
        command_id: String,
        /// 実行ディレクトリ (カレントディレクトリ / Worktree)
        cwd: String,
        /// エージェントID
        agent_id: String,
        /// エージェントプロセスへのパススルー引数
        extra_args: Vec<String>,
    },
    /// 既存セッションのイベントストリーム購読 + 双方向操作。
    ///
    /// `after_node_seq` 指定時はその連番以降の履歴をリプレイしてから
    /// ライブストリームへ接続する (途中切断からの再接続用)。
    AttachSession {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// リプレイ開始位置 (この連番以降を送信)
        after_node_seq: Option<u64>,
    },
    /// 稼働中セッション一覧・中央サーバー接続状態・未送信 Outbox 件数を返却する。
    GetLocalStatus {
        /// 相関ID
        command_id: String,
    },
    /// プロンプト送信 (スラッシュコマンド含む)。
    SendPrompt {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// プロンプト本文
        text: String,
        /// 送信元 (`cli` / `web` / `android`)
        client_source: String,
    },
    /// 承認リクエストへの回答。
    RespondPermission {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// ACP request id
        request_id: String,
        /// 選択された `option_id`
        selected_option_id: String,
        /// 解決主体 (`cli` / `web` / `android_push`)
        resolved_by: String,
    },
    /// モード切替 / 設定変更 / キャンセル / Kill。
    ControlSession {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// 実行する操作
        action: SessionControlAction,
    },
    /// キープアライブ
    Ping,
}

/// Daemon → CLI の応答本体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum IpcResult {
    /// `EnsureSession` の結果
    EnsureSession {
        /// 開始したセッションID
        session_id: String,
        /// アタッチモード (内蔵TUI / OpenCode2 純正TUI)
        attach_mode: AttachMode,
    },
    /// `AttachSession` の受理 (以降は [`IpcServerMessage::EventBatch`] で配信)
    AttachSession {
        /// 対象セッションID
        session_id: String,
    },
    /// `GetLocalStatus` の結果
    LocalStatus {
        /// 稼働状態
        status: LocalStatus,
    },
    /// 非同期コマンド (`SendPrompt` / `RespondPermission` / `ControlSession`)
    /// の受理
    CommandAccepted {
        /// 対象セッションID (任意)
        session_id: Option<String>,
    },
}

/// Daemon → CLI のメッセージ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcServerMessage {
    /// 要求への応答
    Result {
        /// 相関ID
        command_id: String,
        /// 応答本体
        result: IpcResult,
    },
    /// エラー応答
    Error {
        /// 相関ID (接続確立前等で不明な場合は `None`)
        command_id: Option<String>,
        /// 構造化エラーコード
        code: ErrorCode,
        /// 人間向けメッセージ
        message: String,
    },
    /// 永続化イベントの配信 (接続直後のリプレイおよび以降のリアルタイム配信)
    EventBatch {
        /// 対象セッションID
        session_id: String,
        /// イベント列 (`node_seq` 昇順)
        events: Vec<SessionEventEnvelope>,
        /// ノードDBのカーソル
        cursor: u64,
    },
    /// ストリーミング途中のエフェメラルチャンク (永続化されない)
    LiveStreamDelta {
        /// 対象セッションID
        session_id: String,
        /// 差分本体
        delta: StreamDeltaPayload,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_session_roundtrip() {
        let msg = IpcClientMessage::EnsureSession {
            command_id: "c1".into(),
            cwd: "/Users/nazo/src/flexagent".into(),
            agent_id: "opencode2".into(),
            extra_args: vec!["--model".into(), "sonnet".into()],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["method"], "ensure_session");
        assert_eq!(json["extra_args"][1], "sonnet");
        let decoded: IpcClientMessage = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn attach_mode_native_opencode_shape() {
        let result = IpcResult::EnsureSession {
            session_id: "s1".into(),
            attach_mode: AttachMode::NativeOpenCodeAttach {
                server_url: "http://127.0.0.1:4096".into(),
                session_id: "s1".into(),
            },
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["result"], "ensure_session");
        assert_eq!(json["attach_mode"]["mode"], "native_open_code_attach");
        let decoded: IpcResult = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, result);
    }

    #[test]
    fn acp_tui_attach_mode_is_unit_variant() {
        let mode = AttachMode::AcpTui;
        let json = serde_json::to_value(&mode).unwrap();
        assert_eq!(json["mode"], "acp_tui");
        assert_eq!(serde_json::from_value::<AttachMode>(json).unwrap(), mode);
    }

    #[test]
    fn server_message_tags() {
        let msg = IpcServerMessage::Error {
            command_id: None,
            code: ErrorCode::NotFound,
            message: "no such session".into(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["code"], "NOT_FOUND");
    }
}
