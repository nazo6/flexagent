//! 正規化セッションイベント型 (`UnifiedEventPayload`)。
//!
//! 設計: `docs/03-protocol-and-api.md` §1、
//! イベントソーシング/投影規則: `docs/02-database-schema.md` §0.3。
//!
//! # 永続化ライフサイクル (docs/01 §2.3)
//!
//! - 離散イベント (`UserMessage` / `ToolCall` / `PermissionRequest` /
//!   `StatusChanged` 等) は発生時に即時永続化する。
//! - ストリーミング系イベント (`AgentMessage` / `AgentThought`) は
//!   ターン完了時に `is_complete = true` の完成イベントのみを永続化する
//!   (途中経過は [`crate::common::StreamDeltaPayload`] として
//!   メモリ上でのみ配信され、DB には書かれない)。
//! - セッションのメタデータ (作成・タイトル・エージェント確定・状態) も
//!   イベントログ上のイベントとして記録し、ハブ側 `sessions` はその投影とする
//!   (別系統のメタデータ同期は行わない)。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{
    AttachmentMeta, CommandInfo, ConfigOptionInfo, ElicitationAction, FileDiff, ModeInfo,
    PermissionOption, PlanEntry, SessionStatus,
};

/// セッションイベントの封筒 (永続化・同期の単位)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionEventEnvelope {
    /// イベントID (**UUID v7**)。冪等適用・重複排除に使用する。
    pub event_id: String,
    /// セッションID (**UUID v7**)。
    pub session_id: String,
    /// セッション内連番 (`1, 2, 3...`)。**実行ノードのみが採番**し欠番は生じない。
    /// セッション内の論理順序は常にこの値を正とする。
    pub node_seq: u64,
    /// ノード側のローカル時計による作成日時 (Unix epoch ms)。
    pub created_at: i64,
    /// イベント本体。
    pub payload: UnifiedEventPayload,
}

/// カーソルを伴うイベントバッチ (Client WS 配信 / 履歴 API の共通型)。
///
/// `cursor` は「**配信元ストア** (`node.db` / `server.db`) への取り込み順」。
/// クライアントは最後に受信したバッチの `cursor` を保存し、再接続時に
/// `Subscribe { since_cursor }` へ渡す。接続先ストア以外の `cursor` は意味を持たない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionEventBatch {
    /// イベント列 (`node_seq` 昇順)
    pub events: Vec<SessionEventEnvelope>,
    /// このバッチ時点の配信元ストアのカーソル
    pub cursor: u64,
}

impl SessionEventBatch {
    /// 空のバッチ (初回購読でイベントが無い場合など)。
    pub const fn empty(cursor: u64) -> Self {
        Self {
            events: Vec::new(),
            cursor,
        }
    }
}

/// 正規化セッションイベント本体。
///
/// ACP (`agent-client-protocol-schema`) のイベントモデルをベースに、
/// ターミナル出力や承認解決イベントを統合した型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[ts(export)]
pub enum UnifiedEventPayload {
    /// セッション生成直後の最初のイベント (`node_seq = 1`)。
    ///
    /// ハブ側 `sessions` 投影の生成源。別系統のメタデータ同期は行わない。
    SessionCreated {
        /// 実行ノードID。実行ノードが自分自身の ID を埋めることで、
        /// イベントログのみからハブ側投影を完全再構築できるようにする。
        node_id: String,
        /// 論理プロジェクトID
        project_id: String,
        /// プロジェクト表示名 (ハブ側の未知プロジェクト作成用)
        project_name: String,
        /// 実行ディレクトリ (Worktree パス含む)
        local_path: String,
        /// 起動時点のGitブランチ (スナップショット)
        git_branch: Option<String>,
        /// Worktree 内での実行か
        is_worktree: bool,
        /// エージェントID
        agent_id: String,
        /// Fork元のセッションID
        parent_session_id: Option<String>,
        /// Fork元のどのイベント (`node_seq`) 時点から分岐したか
        fork_from_node_seq: Option<u64>,
        /// 初期タイトル
        title: String,
        /// OpenCode2 の起動モード (`"bridge"` / `"acp"`。opencode2 以外は `None`)。
        ///
        /// セッション再開 (resume) 時に起動モードを復元するために永続化する
        /// (旧イベントには存在しないため `#[serde(default)]`)。
        #[serde(default)]
        opencode_mode: Option<String>,
    },
    /// セッションタイトル変更 (UI/CLI からのリネーム。
    /// コマンドとして実行ノードに到達してから発行される)。
    SessionTitleChanged {
        /// 新しいタイトル
        title: String,
    },
    /// ACP セッション確立などによる `agent_session_id` の確定。
    SessionAgentBound {
        /// エージェント内部のセッションID
        agent_session_id: String,
    },
    /// ユーザーが送信したプロンプト (スラッシュコマンド含む)。
    UserMessage {
        /// プロンプト本文
        text: String,
        /// 添付ファイル
        attachments: Vec<AttachmentMeta>,
        /// 送信元 (`cli` / `web` / `android`)
        client_source: String,
        /// ターン開始直前の Shadow Git Tree Hash (Revert 用)。
        /// `snapshot_enabled = false` 時は `None`。この payload が唯一の正であり、
        /// 専用DBカラムへの複製は行わない。
        snapshot_tree_hash: Option<String>,
    },
    /// エージェントの返答メッセージ。
    ///
    /// ストリーミング途中は `LiveStreamDelta` として配信され、
    /// ターン完了時に `is_complete = true` の完成イベントのみ永続化する。
    AgentMessage {
        /// メッセージID (`StreamDeltaPayload::AgentMessageDelta` と対応)
        message_id: String,
        /// 本文
        text: String,
        /// 完成したか (永続化されるのは `true` のみ)
        is_complete: bool,
    },
    /// エージェントの思考プロセス (Thinking)。
    ///
    /// `AgentMessage` と同様、`is_complete = true` の完成イベントのみ永続化する。
    AgentThought {
        /// 思考ID (`StreamDeltaPayload::AgentThoughtDelta` と対応)
        thought_id: String,
        /// 本文
        text: String,
        /// 完成したか
        is_complete: bool,
    },
    /// ツール呼び出しとファイルDiff等の状態。
    ///
    /// 同一 `tool_call_id` のイベント追記によって状態更新を表現する
    /// (`tool_update` イベントは廃止)。
    ToolCall {
        /// ツール呼び出しID
        tool_call_id: String,
        /// 表示タイトル
        title: String,
        /// 種別 (`read` / `edit` / `execute` / `search` / `other`)
        kind: String,
        /// ステータス (`pending` / `in_progress` / `completed` / `failed`)
        status: String,
        /// 対象ファイルパス
        locations: Vec<String>,
        /// ファイル変更時の Unified Diff / Before-After
        diff: Option<FileDiff>,
        /// 生出力 (任意)
        raw_output: Option<String>,
    },
    /// エージェントの実行計画 (ACP Plan)。
    PlanUpdate {
        /// 計画エントリ一覧
        entries: Vec<PlanEntry>,
    },
    /// エージェントからの権限承認リクエスト
    /// (ACP `session/request_permission`)。
    PermissionRequest {
        /// ACP request id
        request_id: String,
        /// ツール名 (例: `terminal/create`)
        tool_name: String,
        /// 要約 (例: `Run command: cargo test`)
        summary: String,
        /// 選択肢 (例: `allow_once` / `allow_always` / `reject`)
        options: Vec<PermissionOption>,
        /// 引数やDiff詳細
        details: serde_json::Value,
    },
    /// 承認リクエストの解決結果。
    ///
    /// `fxg daemon` はエージェントへ応答を返すと同時に本イベントを発行し、
    /// 他クライアントの承認ダイアログを自動的に閉じる。2番目以降の応答は
    /// `request_id` 単位で破棄され `ALREADY_RESOLVED` が返却される。
    PermissionResolved {
        /// ACP request id
        request_id: String,
        /// 選択された `option_id`
        selected_option_id: String,
        /// 解決主体 (`cli` / `web` / `android_push`)
        resolved_by: String,
    },
    /// エージェントからの構造化入力リクエスト (ACP `elicitation/create`)。
    ///
    /// エージェントの「質問」ツールがこのイベントとして届き、UI / CLI は
    /// `requested_schema` から回答フォームを生成する。回答は
    /// [`UnifiedEventPayload::ElicitationResolved`] として追記される
    /// (Phase 1 は form モードのみ対応)。
    ElicitationRequest {
        /// ACP elicitation id
        elicitation_id: String,
        /// ユーザーへ提示するメッセージ
        message: String,
        /// 要求モード (`form` / `url`。Phase 1 は `form` のみ)
        mode: String,
        /// form モードの要求 JSON Schema (`requestedSchema`)
        requested_schema: serde_json::Value,
        /// 関連するツール呼び出しID (任意)
        tool_call_id: Option<String>,
    },
    /// 構造化入力リクエストの解決結果 (accept / decline / cancel)。
    ElicitationResolved {
        /// ACP elicitation id
        elicitation_id: String,
        /// ユーザーの応答アクション
        action: ElicitationAction,
        /// `accept` 時の回答内容 (decline / cancel では `Value::Null`)
        content: serde_json::Value,
        /// 解決主体 (`cli` / `web` / `android_push`)
        resolved_by: String,
    },
    /// ワークスペースのファイルを指定ターンの `snapshot_tree_hash` 時点へ
    /// 復元した (Revert)。
    ///
    /// イベントログは追記専用のため会話イベントは削除せず、Revert 操作の
    /// 事実のみを追記する (`docs/04-agent-drivers-and-windows.md` §4.1)。
    /// 復元直前の状態は Shadow Git Tree 上に `backup_tree_hash` として
    /// 退避されており、もう一度 Revert すれば元に戻せる。
    SessionReverted {
        /// Revert 基準にした `UserMessage` の `node_seq`
        target_node_seq: u64,
        /// 復元先の Tree Hash (`UserMessage.snapshot_tree_hash`)
        restored_tree_hash: String,
        /// 復元直前の状態を退避したバックアップ Tree Hash
        backup_tree_hash: Option<String>,
        /// 復元したファイル数
        restored_files: u64,
        /// 削除したファイル数
        removed_files: u64,
    },
    /// セッションのアーカイブ状態変更 (一覧からの非表示/復元)。
    ///
    /// アーカイブは可逆な可視性フラグであり、イベントログ・会話内容は保持される
    /// (完全な消去は [`UnifiedEventPayload::SessionDeleted`])。
    SessionArchived {
        /// `true` = アーカイブ、`false` = 復元
        archived: bool,
    },
    /// セッションの削除 (tombstone)。
    ///
    /// 適用時に本イベントより前のイベント本文をパージする (復元不能)。
    /// 本イベント自体は Outbox / Resync で削除を伝播し、Fork 元参照の整合を
    /// 保つため tombstone として残す。
    SessionDeleted {},
    /// ACP `terminal/*` または PTY の出力チャンク (永続化対象)。
    ///
    /// ただし FTS5 の `searchable_text` からは除外する (バイナリ系)。
    TerminalOutput {
        /// ターミナルID
        terminal_id: String,
        /// 実行コマンド
        command: String,
        /// ANSIエスケープを含む生バイト列 (Base64)
        data_b64: String,
        /// 終了コード (実行完了時)
        exit_code: Option<i32>,
    },
    /// 対話型コマンドへの標準入力送信 (Web UI → エージェントPTY)。
    ///
    /// 入力キーストロークはエフェメラル扱いで、イベントログには永続化しない
    /// (PTY WS 経由の生配信も可)。[`UnifiedEventPayload::is_persistable`] を参照。
    TerminalInput {
        /// ターミナルID
        terminal_id: String,
        /// ユーザー入力キーストローク (Base64)
        data_b64: String,
    },
    /// モード・スラッシュコマンド・設定の更新通知 (永続化対象)。
    ///
    /// ハブ側 `sessions.current_mode` / `available_modes_json` /
    /// `available_commands_json` / `config_options_json` 投影の更新源。
    CapabilitiesUpdated {
        /// 現在のモード (`None` の場合は既存値を維持する)
        current_mode: Option<String>,
        /// 選択可能なモード一覧
        available_modes: Vec<ModeInfo>,
        /// 利用可能なスラッシュコマンド一覧
        available_commands: Vec<CommandInfo>,
        /// 設定項目一覧 (モデル選択等)
        config_options: Vec<ConfigOptionInfo>,
    },
    /// 一時VMプロビジョニング・自動ツール構築 (`stderr`) の進捗ログ行。
    BootstrapLog {
        /// ログ1行
        line: String,
    },
    /// セッション状態の変化。ハブ側 `sessions.status` 投影の更新源。
    StatusChanged {
        /// 新しい状態
        status: SessionStatus,
        /// エラー時のメッセージ
        error_message: Option<String>,
    },
}

impl UnifiedEventPayload {
    /// `session_events.event_type` に保存する種別文字列を返す。
    ///
    /// 値は `docs/02-database-schema.md` §1 の `event_type` 定義と一致する。
    pub const fn event_type(&self) -> &'static str {
        match self {
            Self::SessionCreated { .. } => "session_created",
            Self::SessionTitleChanged { .. } => "session_title_changed",
            Self::SessionAgentBound { .. } => "session_agent_bound",
            Self::UserMessage { .. } => "user_message",
            Self::AgentMessage { .. } => "agent_message",
            Self::AgentThought { .. } => "agent_thought",
            Self::ToolCall { .. } => "tool_call",
            Self::PlanUpdate { .. } => "plan",
            Self::PermissionRequest { .. } => "permission_request",
            Self::PermissionResolved { .. } => "permission_resolved",
            Self::ElicitationRequest { .. } => "elicitation_request",
            Self::ElicitationResolved { .. } => "elicitation_resolved",
            Self::SessionReverted { .. } => "session_reverted",
            Self::SessionArchived { .. } => "session_archived",
            Self::SessionDeleted {} => "session_deleted",
            Self::TerminalOutput { .. } => "terminal_output",
            Self::TerminalInput { .. } => "terminal_input",
            Self::CapabilitiesUpdated { .. } => "capabilities_updated",
            Self::BootstrapLog { .. } => "bootstrap_log",
            Self::StatusChanged { .. } => "status_change",
        }
    }

    /// イベントログへ永続化してよい payload か。
    ///
    /// `TerminalInput` (キーストローク) はエフェメラル扱いのため `false` を返す。
    /// ストリーミング途中の `AgentMessage` / `AgentThought`
    /// (`is_complete = false`) も永続化しない。
    pub const fn is_persistable(&self) -> bool {
        match self {
            Self::TerminalInput { .. } => false,
            Self::AgentMessage { is_complete, .. } | Self::AgentThought { is_complete, .. } => {
                *is_complete
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{CommandInfo, ConfigOptionInfo, ModeInfo};

    fn sample_envelope(payload: UnifiedEventPayload) -> SessionEventEnvelope {
        SessionEventEnvelope {
            event_id: crate::util::uuid_v7(),
            session_id: crate::util::uuid_v7(),
            node_seq: 1,
            created_at: crate::util::now_ms(),
            payload,
        }
    }

    #[test]
    fn payload_serialization_uses_type_and_data_tags() {
        let payload = UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Running,
            error_message: None,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["type"], "status_changed");
        assert_eq!(json["data"]["status"], "running");
        assert!(json["data"]["error_message"].is_null());

        let decoded: UnifiedEventPayload = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn envelope_roundtrips_through_json() {
        let envelope = sample_envelope(UnifiedEventPayload::UserMessage {
            text: "テストを修正して".to_owned(),
            attachments: vec![],
            client_source: "cli".to_owned(),
            snapshot_tree_hash: Some("a".repeat(40)),
        });
        let json = serde_json::to_string(&envelope).unwrap();
        let decoded: SessionEventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn capabilities_updated_roundtrips() {
        let payload = UnifiedEventPayload::CapabilitiesUpdated {
            current_mode: Some("code".to_owned()),
            available_modes: vec![ModeInfo {
                mode_id: "plan".to_owned(),
                name: "Plan".to_owned(),
                description: None,
            }],
            available_commands: vec![CommandInfo {
                name: "/review".to_owned(),
                description: "レビュー".to_owned(),
                input_hint: None,
            }],
            config_options: vec![ConfigOptionInfo {
                key: "model".to_owned(),
                name: "Model".to_owned(),
                current_value: serde_json::json!("sonnet"),
                options: vec![serde_json::json!("sonnet"), serde_json::json!("haiku")],
            }],
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["type"], "capabilities_updated");
        assert_eq!(json["data"]["current_mode"], "code");

        let decoded: UnifiedEventPayload = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn elicitation_events_roundtrip() {
        let payload = UnifiedEventPayload::ElicitationRequest {
            elicitation_id: "elic-1".to_owned(),
            message: "どの戦略で進めますか?".to_owned(),
            mode: "form".to_owned(),
            requested_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "strategy": { "type": "string", "enum": ["a", "b"] }
                },
                "required": ["strategy"]
            }),
            tool_call_id: Some("tool-1".to_owned()),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["type"], "elicitation_request");
        assert_eq!(json["data"]["mode"], "form");
        assert_eq!(
            serde_json::from_value::<UnifiedEventPayload>(json).unwrap(),
            payload
        );
        assert!(payload.is_persistable());

        let resolved = UnifiedEventPayload::ElicitationResolved {
            elicitation_id: "elic-1".to_owned(),
            action: ElicitationAction::Accept,
            content: serde_json::json!({ "strategy": "a" }),
            resolved_by: "cli".to_owned(),
        };
        let json = serde_json::to_value(&resolved).unwrap();
        assert_eq!(json["type"], "elicitation_resolved");
        assert_eq!(json["data"]["action"], "accept");
        assert_eq!(
            serde_json::from_value::<UnifiedEventPayload>(json).unwrap(),
            resolved
        );
    }

    #[test]
    fn event_type_matches_db_vocabulary() {
        assert_eq!(
            UnifiedEventPayload::SessionCreated {
                node_id: "n".into(),
                project_id: "p".into(),
                project_name: "P".into(),
                local_path: "/tmp".into(),
                git_branch: None,
                is_worktree: false,
                agent_id: "opencode2".into(),
                parent_session_id: None,
                fork_from_node_seq: None,
                title: "t".into(),
                opencode_mode: None,
            }
            .event_type(),
            "session_created"
        );
        assert_eq!(
            UnifiedEventPayload::PlanUpdate { entries: vec![] }.event_type(),
            "plan"
        );
        assert_eq!(
            UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Idle,
                error_message: None,
            }
            .event_type(),
            "status_change"
        );
    }

    #[test]
    fn session_created_opencode_mode_roundtrip_and_backward_compat() {
        // 新形式: opencode_mode が保存・復元される
        let payload = UnifiedEventPayload::SessionCreated {
            node_id: "n".into(),
            project_id: "p".into(),
            project_name: "P".into(),
            local_path: "/tmp".into(),
            git_branch: None,
            is_worktree: false,
            agent_id: "opencode2".into(),
            parent_session_id: None,
            fork_from_node_seq: None,
            title: "t".into(),
            opencode_mode: Some("acp".into()),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["data"]["opencode_mode"], "acp");
        assert_eq!(
            serde_json::from_value::<UnifiedEventPayload>(json).unwrap(),
            payload
        );

        // 旧形式: opencode_mode が無くても None として読める (後方互換)
        let legacy = serde_json::json!({
            "type": "session_created",
            "data": {
                "node_id": "n",
                "project_id": "p",
                "project_name": "P",
                "local_path": "/tmp",
                "git_branch": null,
                "is_worktree": false,
                "agent_id": "opencode2",
                "parent_session_id": null,
                "fork_from_node_seq": null,
                "title": "t",
            }
        });
        let decoded: UnifiedEventPayload = serde_json::from_value(legacy).unwrap();
        let UnifiedEventPayload::SessionCreated { opencode_mode, .. } = decoded else {
            panic!("expected session_created");
        };
        assert_eq!(opencode_mode, None);
    }

    #[test]
    fn persistability_rules() {
        assert!(
            !UnifiedEventPayload::TerminalInput {
                terminal_id: "t".into(),
                data_b64: "aGk=".into(),
            }
            .is_persistable()
        );
        assert!(
            !UnifiedEventPayload::AgentMessage {
                message_id: "m".into(),
                text: "途中".into(),
                is_complete: false,
            }
            .is_persistable()
        );
        assert!(
            UnifiedEventPayload::AgentMessage {
                message_id: "m".into(),
                text: "完成".into(),
                is_complete: true,
            }
            .is_persistable()
        );
        assert!(
            UnifiedEventPayload::TerminalOutput {
                terminal_id: "t".into(),
                command: "cargo test".into(),
                data_b64: "".into(),
                exit_code: Some(0),
            }
            .is_persistable()
        );
    }

    #[test]
    fn batch_cursor_helper() {
        let batch = SessionEventBatch::empty(42);
        assert!(batch.events.is_empty());
        assert_eq!(batch.cursor, 42);
    }
}
