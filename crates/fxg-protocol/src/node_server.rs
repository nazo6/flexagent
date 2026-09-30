//! Node ⇔ Central Server 間プロトコル (WebSocket & Stdio Pipe 共通)。
//!
//! 常駐ノード (Outbound WebSocket) と一時VMノード (`fxg daemon --stdio` の
//! JSON Lines) は、トランスポート層が異なるだけで **100%同一のメッセージ型**を
//! 共有する (設計: `docs/03-protocol-and-api.md` §2)。
//!
//! これらはサーバー側実装の内部プロトコルであり UI には公開しないため
//! `ts-rs` の export 対象外とする。

use serde::{Deserialize, Serialize};

use crate::common::{ErrorCode, NodeProjectReport, SessionControlAction, WorkspaceDiffResponse};
use crate::events::SessionEventEnvelope;

/// ノードが保持するセッションの同期状態 (`NodeHello` で報告)。
///
/// ハブは自 DB の `last_node_seq` と比較し、欠落・遅延があれば
/// [`ServerToNodeMsg::ResyncRequest`] を返す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSyncState {
    /// 対象セッションID
    pub session_id: String,
    /// ノード側に永続化済みの最新 `node_seq`
    pub last_node_seq: u64,
}

/// ハブ側で欠落・遅延しているセッションの再送指定 (`ResyncRequest`)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResyncTarget {
    /// 対象セッションID
    pub session_id: String,
    /// この値以降 (`>=`) のイベントを水位に関係なく再送する
    /// (`1` は `SessionCreated` を含む全量再送を意味する)
    pub from_node_seq: u64,
}

/// Node → Server への送信メッセージ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum NodeToServerMsg {
    /// 接続直後のハンドシェイク
    /// (ノード情報・プロジェクト紐付け・セッション同期状態一覧を通知)。
    NodeHello {
        /// ノードID (サーバー側でトークン発行対象と一致することを検証)
        node_id: String,
        /// 表示名
        name: String,
        /// OS (`windows` / `linux` / `macos`)
        os: String,
        /// アーキテクチャ (`x86_64` / `aarch64`)
        arch: String,
        /// fxg バイナリバージョン
        version: String,
        /// 一時VM / コンテナノードか
        is_ephemeral: bool,
        /// 利用可能なエージェントID一覧
        installed_agents: Vec<String>,
        /// プロジェクト紐付け報告
        projects: Vec<NodeProjectReport>,
        /// セッション同期状態一覧
        sessions: Vec<SessionSyncState>,
    },
    /// 永続化イベントのバッチ送信 (Store-and-Forward Outbox)。
    ///
    /// セッションのメタデータ (`SessionCreated` 等) も `session_events` 上の
    /// イベントとして送信される。通常は `synced_up_to_node_seq` より後の
    /// イベントを `node_seq` 昇順で送る。
    EventBatchPush {
        /// 送信イベント (バッチはセッション単位に分割される)
        events: Vec<SessionEventEnvelope>,
    },
    /// リアルタイム・エフェメラルチャンク (DBフラッシュ前の高速表示用)。
    LiveStreamDelta {
        /// 対象セッションID
        session_id: String,
        /// 差分本体
        delta: crate::common::StreamDeltaPayload,
    },
    /// サーバーからのコマンド実行結果応答 (要求元クライアントへ相関返却される)。
    CommandResult {
        /// 相関ID
        command_id: String,
        /// 成功したか
        success: bool,
        /// 失敗時の構造化エラーコード
        code: Option<ErrorCode>,
        /// 人間向けメッセージ
        error: Option<String>,
        /// 対象セッションID (任意)
        session_id: Option<String>,
    },
    /// PTY 出力データチャンク (Web Terminal → クライアント)。
    PtyOutput {
        /// PTY ID
        pty_id: String,
        /// 出力バイト列 (Base64)
        data_b64: String,
    },
    /// PTY プロセス終了通知。
    PtyExit {
        /// PTY ID
        pty_id: String,
        /// 終了コード
        exit_code: Option<i32>,
    },
    /// ノードのGit Diff取得結果応答。
    GitDiffResult {
        /// 要求時の相関ID
        request_id: String,
        /// 取得成功時の Diff
        diff: Option<WorkspaceDiffResponse>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// Worktree 操作 (`ManageWorktree`) の結果応答。
    WorktreeResult {
        /// 要求時の相関ID
        command_id: String,
        /// 成功時の Worktree 情報 (削除時は `None`)
        worktree: Option<crate::client_api::WorktreeInfo>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// PTY 起動・操作の失敗通知 (エラーコードは文字列。例: `PTY_DISABLED`)。
    ///
    /// ノード側設定 `allow_remote_pty = false` の場合、リモートからの
    /// `PtySpawn` に対して `PTY_DISABLED` を返す (設計: docs/01 §7.4)。
    PtyError {
        /// PTY ID
        pty_id: String,
        /// エラーコード文字列
        code: String,
        /// 人間向けメッセージ
        message: String,
    },
    /// 一時VMからのオンメモリGit認証要求 (`GIT_ASKPASS` プロキシ)。
    GitCredentialRequest {
        /// 相関ID
        request_id: String,
        /// 対象リポジトリURL
        repo_url: String,
        /// 操作種別 (`clone` / `fetch` / `push`)
        operation: String,
    },
    /// 一時VM破棄前の作業ツリー・コミット履歴バンドル退避 (`git bundle create`)。
    ///
    /// bundle は圧縮して Base64 化し、1メッセージ上限 (例: 16 MiB) を
    /// 超える場合は分割転送する。
    WorkspaceBundleUpload {
        /// 対象セッションID
        session_id: String,
        /// 対象ブランチ
        branch: String,
        /// HEAD コミット
        head_commit: String,
        /// bundle データ (圧縮 + Base64)
        bundle_b64: String,
    },
    /// Graceful Drain 完了通知 (受信後に中央サーバーが子プロセス/VMを破棄)。
    DrainComplete {
        /// ノードID
        node_id: String,
    },
}

/// Server → Node への送信メッセージ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ServerToNodeMsg {
    /// [`NodeToServerMsg::EventBatchPush`] に対する永続化完了ACK。
    ///
    /// バッチはセッション単位に分割して送信し、ACK もセッション単位で返却する。
    /// ノードは受信後 `sessions.synced_up_to_node_seq` を水位として更新する。
    EventBatchAck {
        /// 対象セッションID
        session_id: String,
        /// この値以下の `node_seq` がハブに永続化された
        acked_up_to_node_seq: u64,
    },
    /// `NodeHello` への応答: ハブ側で欠落・遅延しているセッションの再送要求。
    ///
    /// ノードは指定 `from_node_seq` 以降のイベントを水位に関係なく再送する。
    ResyncRequest {
        /// 再送対象
        sessions: Vec<ResyncTarget>,
    },
    /// クライアント (Web/Android) からのセッション新規起動要求。
    StartSession {
        /// 相関ID
        command_id: String,
        /// 起動するセッションID (サーバーが採番)
        session_id: String,
        /// 論理プロジェクトID
        project_id: String,
        /// 実行ディレクトリ (Worktree パス含む)
        local_path: String,
        /// エージェントID
        agent_id: String,
        /// 初期プロンプト
        initial_prompt: Option<String>,
        /// 別ノードからの履歴引き継ぎ時 (Replay 注入) の会話履歴
        fork_context_messages: Option<Vec<crate::common::ForkHistoryItem>>,
        /// 一時VMから退避された Git バンドルの復元用
        restore_git_bundle_b64: Option<String>,
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
    /// ワークスペースWebターミナル (PTY) の起動要求。
    PtySpawn {
        /// PTY ID
        pty_id: String,
        /// 対象セッションID
        session_id: String,
        /// 列数
        cols: u16,
        /// 行数
        rows: u16,
        /// シェルコマンド (省略時は既定シェル)
        shell_cmd: Option<String>,
    },
    /// PTY へのユーザー入力送信 (キー入力)。
    PtyInput {
        /// PTY ID
        pty_id: String,
        /// 入力バイト列 (Base64)
        data_b64: String,
    },
    /// PTY ウィンドウリサイズ。
    PtyResize {
        /// PTY ID
        pty_id: String,
        /// 列数
        cols: u16,
        /// 行数
        rows: u16,
    },
    /// PTY プロセス終了。
    PtyKill {
        /// PTY ID
        pty_id: String,
    },
    /// ノードのGit作業ツリー/ブランチDiff取得要求。
    GetGitDiff {
        /// 相関ID
        request_id: String,
        /// 対象セッションID
        session_id: String,
        /// 取得スコープ (未コミット / ベースブランチ累積)
        scope: crate::common::DiffScope,
        /// `BranchBase` 時のベースブランチ
        base_branch: Option<String>,
    },
    /// Worktree 操作要求 (作成・削除)。
    ManageWorktree {
        /// 相関ID
        command_id: String,
        /// 対象論理プロジェクトID
        project_id: String,
        /// 実行する操作
        action: crate::common::WorktreeAction,
    },
    /// `GitCredentialRequest` に対する短命トークン応答。
    GitCredentialResponse {
        /// 相関ID
        request_id: String,
        /// Basic 認証ユーザー名
        username: String,
        /// トークン (オンメモリのみ。VM内ディスクへは保存しない)
        token: Option<String>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// 一時VMの終了前フラッシュ＆Gitバンドル退避要求。
    DrainAndShutdown {
        /// 終了理由 (ログ用)
        reason: String,
        /// `git bundle create` による成果物退避を行うか
        create_git_bundle: bool,
    },
    /// 緊急キルスイッチ: ノード上で稼働中の全セッション・プロセスツリー・PTYを
    /// 即時強制停止する。
    KillAllSessions {
        /// 実行理由 (監査ログ用)
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{DiffScope, SessionControlAction};

    #[test]
    fn event_batch_push_shape() {
        let msg = NodeToServerMsg::EventBatchPush { events: vec![] };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "event_batch_push");
        assert!(json["events"].as_array().unwrap().is_empty());
        let decoded: NodeToServerMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn node_hello_shape() {
        let msg = NodeToServerMsg::NodeHello {
            node_id: "home-win".into(),
            name: "Home Windows PC".into(),
            os: "windows".into(),
            arch: "x86_64".into(),
            version: "0.1.0".into(),
            is_ephemeral: false,
            installed_agents: vec!["opencode2".into()],
            projects: vec![],
            sessions: vec![SessionSyncState {
                session_id: "s1".into(),
                last_node_seq: 3,
            }],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "node_hello");
        assert_eq!(json["sessions"][0]["last_node_seq"], 3);
        let decoded: NodeToServerMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn server_to_node_control_session_shape() {
        let msg = ServerToNodeMsg::ControlSession {
            command_id: "c1".into(),
            session_id: "s1".into(),
            action: SessionControlAction::SetMode {
                mode_id: "plan".into(),
            },
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "control_session");
        assert_eq!(json["action"]["action"], "set_mode");
        assert_eq!(json["action"]["mode_id"], "plan");
        let decoded: ServerToNodeMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn git_diff_request_shape() {
        let msg = ServerToNodeMsg::GetGitDiff {
            request_id: "r1".into(),
            session_id: "s1".into(),
            scope: DiffScope::BranchBase,
            base_branch: Some("main".into()),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "get_git_diff");
        assert_eq!(json["scope"], "branch_base");
        let decoded: ServerToNodeMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }
}
