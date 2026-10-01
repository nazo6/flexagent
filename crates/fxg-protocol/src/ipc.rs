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

use crate::client_api::WorktreeInfo;
use crate::common::{
    ErrorCode, HookLogEntry, PermissionRequestEntry, ProjectResolutionSource, ProjectSummary,
    SessionControlAction, SessionSummary, StreamDeltaPayload,
};
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
    /// `opencode2 run --server <server_url> --session <session_id>` を子プロセス実行し、
    /// 純正TUIを直接表示するモード。
    NativeOpenCodeAttach {
        /// `opencode2 serve` のローカルURL
        server_url: String,
        /// アタッチ対象のエージェント側セッションID (`ses_...`)
        session_id: String,
        /// 純正 CLI へ注入する環境変数 (例: `OPENCODE_PASSWORD`)。
        /// ループバック限定サーバーの一時クレデンシャルを含むため、
        /// トークン認証済みのローカルIPC 以外へは出力しないこと。
        env: Vec<(String, String)>,
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

/// 論理プロジェクト解決の詳細 (`fxg project info` の結果)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// 論理プロジェクトID (`project_key`)
    pub project_id: String,
    /// 表示名
    pub name: String,
    /// 正規化元の Git URL
    pub canonical_git_url: Option<String>,
    /// Git ルート (非 Git の場合は `None`)
    pub git_root: Option<String>,
    /// Git ルートからの相対サブパス (モノレポ)
    pub relative_subpath: Option<String>,
    /// 解決対象ディレクトリ
    pub local_path: String,
    /// Git リポジトリか
    pub is_git_repo: bool,
    /// 解決元
    pub source: ProjectResolutionSource,
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
        /// 初期モード (`--mode` 指定時。ACP の `session/set_mode` で適用)
        initial_mode: Option<String>,
        /// `opencode2` を標準ACPモード (`opencode2 acp`) で起動する (`--acp`)
        acp: bool,
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
    /// 指定ターン時点へのファイル復元 (`fxg session revert`)。
    ///
    /// 会話イベントは削除せず、Revert 操作を `session_reverted` イベントとして
    /// 追記する (`docs/04-agent-drivers-and-windows.md` §4.1)。
    SessionRevert {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// Revert 基準にする `UserMessage` の `node_seq`
        /// (省略時は直近ターン)
        target_node_seq: Option<u64>,
    },
    /// 指定ターンからの会話分岐 (`fxg session fork`)。
    ///
    /// 新しい `session_id` を作成し、指定 `node_seq` までの履歴を
    /// 初期コンテキストとして新エージェントセッションへ注入する。
    SessionFork {
        /// 相関ID
        command_id: String,
        /// 分岐元セッションID
        session_id: String,
        /// 分岐元の `node_seq` (省略時は最新イベント)
        from_node_seq: Option<u64>,
        /// 分岐先で使うエージェントID (省略時は同一エージェント)
        agent_id: Option<String>,
        /// 分岐先の作業ディレクトリ (省略時は分岐元と同じ。`-w/--worktree` 用)
        cwd: Option<String>,
    },
    /// 停止済みセッションの再開 (`fxg session resume`)。
    ///
    /// 同一 `session_id` のままエージェント側コンテキストを復元して継続する
    /// (`session/resume` → `session/load` → 履歴 Replay の順にフォールバック)。
    SessionResume {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// ネイティブ復元を試みず履歴 Replay で継続する (`--replay`)
        force_replay: bool,
    },
    /// セッションのアーカイブ/復元 (`fxg session archive` / `unarchive`)。
    ///
    /// アーカイブは一覧からの非表示/復元のみで、イベントログは保持される。
    SessionArchive {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// `true` = アーカイブ、`false` = 復元
        archived: bool,
    },
    /// セッションの削除 (`fxg session delete`)。
    ///
    /// 稼働中セッションは停止してから、イベント本文をパージする
    /// (復元不能。同期用 tombstone のみ残る)。
    SessionDelete {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
    },
    /// セッション詳細の取得 (`fxg session show`)。
    SessionShow {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// 末尾から表示するイベント数
        recent_events: u32,
    },
    /// 承認待ちリクエスト一覧 (`fxg inbox list`)。
    InboxList {
        /// 相関ID
        command_id: String,
    },
    /// 指定ディレクトリの論理プロジェクトを解決する (`fxg project info`)。
    ProjectInfo {
        /// 相関ID
        command_id: String,
        /// 解決対象ディレクトリ
        cwd: String,
    },
    /// 登録済み論理プロジェクト一覧 (`fxg project list`)。
    ProjectList {
        /// 相関ID
        command_id: String,
    },
    /// カレントディレクトリを指定の論理プロジェクトIDへ手動紐付け
    /// (`.fxg.toml` へ保存。`fxg project link`)
    ProjectLink {
        /// 相関ID
        command_id: String,
        /// 対象ディレクトリ
        cwd: String,
        /// 紐付け先の論理プロジェクトID
        project_id: String,
    },
    /// 指定ディレクトリ配下のGitリポジトリを一括スキャン (`fxg project scan`)。
    ProjectScan {
        /// 相関ID
        command_id: String,
        /// スキャン対象ディレクトリ (省略時は `project_scan_dirs` を走査)
        dir: Option<String>,
    },
    /// Worktree 一覧 (`fxg worktree list`)。
    WorktreeList {
        /// 相関ID
        command_id: String,
        /// 対象リポジトリのディレクトリ (省略時は論理プロジェクトから解決)
        cwd: Option<String>,
        /// 論理プロジェクトIDで絞り込む
        project_id: Option<String>,
    },
    /// Worktree 作成 (`fxg worktree add`)。
    WorktreeAdd {
        /// 相関ID
        command_id: String,
        /// メインリポジトリのディレクトリ
        cwd: String,
        /// 論理プロジェクトID (省略時は `cwd` から解決)
        project_id: Option<String>,
        /// 作成するブランチ名
        branch: String,
        /// 起点ブランチ (省略時は現在の HEAD)
        base_branch: Option<String>,
        /// 配置先パスの明示指定
        path: Option<String>,
    },
    /// Worktree 削除 (`fxg worktree remove`)。
    WorktreeRemove {
        /// 相関ID
        command_id: String,
        /// メインリポジトリのディレクトリ
        cwd: String,
        /// 削除対象のブランチ名またはパス
        target: String,
        /// 未コミット変更があっても強制削除するか
        force: bool,
    },
    /// 削除済み Worktree 管理情報のクリーンアップ (`fxg worktree prune`)。
    WorktreePrune {
        /// 相関ID
        command_id: String,
        /// メインリポジトリのディレクトリ
        cwd: String,
    },
    /// セッション一覧 (`fxg ps` / `fxg session list`)。
    ListSessions {
        /// 相関ID
        command_id: String,
        /// 停止済みセッションも含めるか (`-a/--all`)
        include_stopped: bool,
        /// アーカイブ済みセッションも含めるか (`--archived`)
        include_archived: bool,
    },
    /// ローカルノード上の全セッション・プロセスツリー・PTYを強制終了 (`fxg kill-all`)。
    KillAll {
        /// 相関ID
        command_id: String,
    },
    /// 認証トークンの取得 / 再生成 (`fxg auth token` / `fxg auth rotate-token`)。
    AuthToken {
        /// 相関ID
        command_id: String,
        /// 再生成するか
        rotate: bool,
    },
    /// Git 資格情報の解決 (`fxg git-askpass`。`GIT_ASKPASS` ヘルパーとして使う)。
    ///
    /// 一時VM (`fxg daemon --stdio`) 上の Git 操作は、このメソッド経由で
    /// デーモン → 中央サーバー (`GitCredentialRequest`) へ中継され、
    /// VM内ディスクにトークンを残さずに認証する (設計: docs/01 §6.4)。
    GitCredential {
        /// 相関ID
        command_id: String,
        /// Git (`GIT_ASKPASS`) が渡すプロンプト文字列
        prompt: String,
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
        /// アタッチモード (内蔵TUI / OpenCode2 純正TUI)。
        /// 稼働中でない (停止済み) セッションは [`AttachMode::AcpTui`]
        /// (イベント履歴の閲覧のみ)。
        attach_mode: AttachMode,
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
    /// `SessionRevert` の結果
    SessionReverted {
        /// 基準にした `UserMessage` の `node_seq`
        target_node_seq: u64,
        /// 復元先の Tree Hash
        restored_tree_hash: String,
        /// 復元直前を退避したバックアップ Tree Hash
        backup_tree_hash: Option<String>,
        /// 復元したファイル数
        restored_files: u64,
        /// 削除したファイル数
        removed_files: u64,
    },
    /// `SessionFork` の結果
    SessionForked {
        /// 新しいセッションID
        session_id: String,
        /// アタッチモード (内蔵TUI / OpenCode2 純正TUI)
        attach_mode: AttachMode,
    },
    /// `SessionResume` の結果
    SessionResumed {
        /// 再開したセッションID (リクエスト対象と同一)
        session_id: String,
        /// アタッチモード (内蔵TUI / OpenCode2 純正TUI)
        attach_mode: AttachMode,
        /// ネイティブ復元できたか (`false` = 履歴 Replay で継続)
        context_restored: bool,
    },
    /// `SessionArchive` の結果
    SessionArchived {
        /// 対象セッションID
        session_id: String,
        /// アーカイブ日時 (Unix epoch ms)。復元時は `None`
        archived_at: Option<i64>,
    },
    /// `SessionDelete` の結果
    SessionDeleted {
        /// 削除したセッションID
        session_id: String,
    },
    /// `SessionShow` の結果
    SessionDetail(Box<SessionDetail>),
    /// `InboxList` の結果
    Inbox {
        /// 承認待ちリクエスト一覧 (全セッション横断)
        requests: Vec<PermissionRequestEntry>,
    },
    /// `ProjectInfo` の結果
    ProjectInfo {
        /// 解決された論理プロジェクト
        project: ProjectInfo,
    },
    /// `ProjectList` の結果
    Projects {
        /// 登録済み論理プロジェクト一覧
        projects: Vec<ProjectSummary>,
    },
    /// `ProjectScan` の結果
    ProjectScan {
        /// スキャンで解決された論理プロジェクト一覧
        projects: Vec<ProjectSummary>,
        /// 実際に走査したディレクトリ
        scanned_dirs: Vec<String>,
    },
    /// `WorktreeList` の結果
    Worktrees {
        /// Worktree 一覧
        worktrees: Vec<WorktreeInfo>,
    },
    /// `WorktreeAdd` の結果
    WorktreeAdded {
        /// Worktree のパス
        path: String,
        /// ブランチ名
        branch: String,
        /// 新規作成されたか (`false` は既存 Worktree の再利用)
        created: bool,
        /// `post_create` フックの実行ログ
        hook_logs: Vec<HookLogEntry>,
    },
    /// `ListSessions` の結果
    Sessions {
        /// セッション一覧 (更新日時降順)
        sessions: Vec<SessionSummary>,
    },
    /// `KillAll` の結果
    KillAll {
        /// 終了させたセッションID
        killed_sessions: Vec<String>,
        /// 終了させた PTY ID
        killed_ptys: Vec<String>,
    },
    /// `AuthToken` の結果
    AuthToken {
        /// クライアント認証トークン
        token: String,
    },
    /// `GitCredential` の結果 (オンメモリのみ。ディスクへは保存しない)
    GitCredential {
        /// Basic 認証ユーザー名
        username: String,
        /// トークン (解決失敗時は `None`)
        token: Option<String>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// 単純な成功応答
    Ack {
        /// 補足メッセージ
        message: Option<String>,
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

/// `fxg session show` の応答本体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionDetail {
    /// セッション集約
    pub session: SessionSummary,
    /// 永続化済みイベント総数
    pub event_count: u64,
    /// 直近イベント (古い順)
    pub recent_events: Vec<SessionEventEnvelope>,
    /// このセッションの承認待ちリクエスト
    pub pending_permissions: Vec<PermissionRequestEntry>,
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
            initial_mode: Some("plan".into()),
            acp: false,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["method"], "ensure_session");
        assert_eq!(json["extra_args"][1], "sonnet");
        assert_eq!(json["initial_mode"], "plan");
        assert_eq!(json["acp"], false);
        let decoded: IpcClientMessage = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn attach_mode_native_opencode_shape() {
        let result = IpcResult::EnsureSession {
            session_id: "s1".into(),
            attach_mode: AttachMode::NativeOpenCodeAttach {
                server_url: "http://127.0.0.1:4096".into(),
                session_id: "ses_1".into(),
                env: vec![("OPENCODE_PASSWORD".into(), "secret".into())],
            },
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["result"], "ensure_session");
        assert_eq!(json["attach_mode"]["mode"], "native_open_code_attach");
        assert_eq!(json["attach_mode"]["env"][0][0], "OPENCODE_PASSWORD");
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

    #[test]
    fn project_and_worktree_methods_roundtrip() {
        let cases = vec![
            IpcClientMessage::ProjectInfo {
                command_id: "c1".into(),
                cwd: "/home/nazo/src/flexagent".into(),
            },
            IpcClientMessage::ProjectList {
                command_id: "c2".into(),
            },
            IpcClientMessage::ProjectLink {
                command_id: "c3".into(),
                cwd: "/home/nazo/src/flexagent".into(),
                project_id: "internal/tools".into(),
            },
            IpcClientMessage::ProjectScan {
                command_id: "c4".into(),
                dir: Some("/home/nazo/src".into()),
            },
            IpcClientMessage::WorktreeList {
                command_id: "c5".into(),
                cwd: Some("/home/nazo/src/flexagent".into()),
                project_id: None,
            },
            IpcClientMessage::WorktreeAdd {
                command_id: "c6".into(),
                cwd: "/home/nazo/src/flexagent".into(),
                project_id: None,
                branch: "feat/auth".into(),
                base_branch: Some("main".into()),
                path: None,
            },
            IpcClientMessage::WorktreeRemove {
                command_id: "c7".into(),
                cwd: "/home/nazo/src/flexagent".into(),
                target: "feat/auth".into(),
                force: true,
            },
            IpcClientMessage::WorktreePrune {
                command_id: "c8".into(),
                cwd: "/home/nazo/src/flexagent".into(),
            },
            IpcClientMessage::ListSessions {
                command_id: "c9".into(),
                include_stopped: true,
                include_archived: true,
            },
            IpcClientMessage::SessionArchive {
                command_id: "c12".into(),
                session_id: "3f6b6f3e".into(),
                archived: true,
            },
            IpcClientMessage::SessionDelete {
                command_id: "c13".into(),
                session_id: "3f6b6f3e".into(),
            },
            IpcClientMessage::KillAll {
                command_id: "c10".into(),
            },
            IpcClientMessage::AuthToken {
                command_id: "c11".into(),
                rotate: false,
            },
        ];
        for message in cases {
            let json = serde_json::to_value(&message).unwrap();
            assert!(json["method"].is_string(), "method tag missing: {json}");
            let decoded: IpcClientMessage = serde_json::from_value(json).unwrap();
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn session_revert_and_fork_roundtrip() {
        for message in [
            IpcClientMessage::SessionRevert {
                command_id: "c1".into(),
                session_id: "s1".into(),
                target_node_seq: Some(4),
            },
            IpcClientMessage::SessionFork {
                command_id: "c2".into(),
                session_id: "s1".into(),
                from_node_seq: None,
                agent_id: Some("antigravity-acp".into()),
                cwd: None,
            },
        ] {
            let json = serde_json::to_value(&message).unwrap();
            assert!(json["method"].is_string(), "method tag missing: {json}");
            let decoded: IpcClientMessage = serde_json::from_value(json).unwrap();
            assert_eq!(decoded, message);
        }

        let reverted = IpcResult::SessionReverted {
            target_node_seq: 4,
            restored_tree_hash: "a".repeat(40),
            backup_tree_hash: Some("b".repeat(40)),
            restored_files: 3,
            removed_files: 1,
        };
        let json = serde_json::to_value(&reverted).unwrap();
        assert_eq!(json["result"], "session_reverted");
        assert_eq!(serde_json::from_value::<IpcResult>(json).unwrap(), reverted);

        let forked = IpcResult::SessionForked {
            session_id: "child".into(),
            attach_mode: AttachMode::AcpTui,
        };
        let json = serde_json::to_value(&forked).unwrap();
        assert_eq!(json["result"], "session_forked");
        assert_eq!(serde_json::from_value::<IpcResult>(json).unwrap(), forked);
    }

    #[test]
    fn session_resume_roundtrip() {
        let resume = IpcClientMessage::SessionResume {
            command_id: "c1".into(),
            session_id: "s1".into(),
            force_replay: true,
        };
        let json = serde_json::to_value(&resume).unwrap();
        assert_eq!(json["method"], "session_resume");
        assert_eq!(json["force_replay"], true);
        assert_eq!(
            serde_json::from_value::<IpcClientMessage>(json).unwrap(),
            resume
        );

        let resumed = IpcResult::SessionResumed {
            session_id: "s1".into(),
            attach_mode: AttachMode::AcpTui,
            context_restored: false,
        };
        let json = serde_json::to_value(&resumed).unwrap();
        assert_eq!(json["result"], "session_resumed");
        assert_eq!(json["context_restored"], false);
        assert_eq!(serde_json::from_value::<IpcResult>(json).unwrap(), resumed);
    }

    #[test]
    fn extended_results_serialize() {
        let result = IpcResult::ProjectInfo {
            project: ProjectInfo {
                project_id: "github.com/nazo6/flexagent".into(),
                name: "flexagent".into(),
                canonical_git_url: Some("git@github.com:nazo6/flexagent.git".into()),
                git_root: Some("/home/nazo/src/flexagent".into()),
                relative_subpath: None,
                local_path: "/home/nazo/src/flexagent".into(),
                is_git_repo: true,
                source: ProjectResolutionSource::GitRemote,
            },
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["result"], "project_info");
        assert_eq!(json["project"]["source"], "git_remote");
        let decoded: IpcResult = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, result);

        let added = IpcResult::WorktreeAdded {
            path: "/home/nazo/.flexagent/worktrees/github.com-nazo6-flexagent/feat-auth".into(),
            branch: "feat/auth".into(),
            created: true,
            hook_logs: vec![HookLogEntry {
                command: "pnpm install".into(),
                success: true,
                output: "done".into(),
            }],
        };
        let json = serde_json::to_value(&added).unwrap();
        assert_eq!(json["result"], "worktree_added");
        assert_eq!(json["hook_logs"][0]["success"], true);
        let decoded: IpcResult = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, added);
    }
}
