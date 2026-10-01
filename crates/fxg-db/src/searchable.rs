//! `searchable_text` 生成ロジック。
//!
//! イベントを永続化する側 (ノード / ハブ双方の受信ハンドラ) が
//! [`classify_payload`] を用いて `session_events.event_type` と
//! `searchable_text` (FTS5 `tokenize='trigram'` の検索対象) を生成する。
//!
//! `TerminalOutput` (ANSI エスケープを含む生バイト列) や `TerminalInput`
//! (キーストローク)、`PermissionResolved` / `SessionAgentBound` などの
//! バイナリ系・識別子系は FTS 対象外とする (設計: `docs/02-database-schema.md` §2)。

use fxg_protocol::events::UnifiedEventPayload;

/// FTS5 へ投入するテキストの上限 (文字数)。
///
/// 巨大なツール出力でインデックスが肥大化するのを防ぐ。
pub const MAX_SEARCHABLE_CHARS: usize = 8 * 1024;

/// payload から (`event_type`, `searchable_text`) を生成する。
///
/// `event_type` は `session_events.event_type` に保存する語彙
/// ([`UnifiedEventPayload::event_type`]) と同一。
pub fn classify_payload(payload: &UnifiedEventPayload) -> (&'static str, Option<String>) {
    let event_type = payload.event_type();
    let text = match payload {
        UnifiedEventPayload::SessionTitleChanged { title } => join_parts(vec![title.clone()]),
        UnifiedEventPayload::UserMessage {
            text, attachments, ..
        } => {
            let mut parts = vec![text.clone()];
            parts.extend(attachments.iter().map(|a| a.file_name.clone()));
            join_parts(parts)
        }
        UnifiedEventPayload::AgentMessage { text, .. } => join_parts(vec![text.clone()]),
        UnifiedEventPayload::AgentThought { text, .. } => join_parts(vec![text.clone()]),
        UnifiedEventPayload::ToolCall {
            title,
            locations,
            raw_output,
            ..
        } => {
            let mut parts = vec![title.clone()];
            parts.extend(locations.iter().cloned());
            if let Some(output) = raw_output {
                parts.push(output.clone());
            }
            join_parts(parts)
        }
        UnifiedEventPayload::PlanUpdate { entries } => join_parts(
            entries
                .iter()
                .map(|entry| format!("{} {}", entry.title, entry.status))
                .collect(),
        ),
        UnifiedEventPayload::PermissionRequest {
            tool_name, summary, ..
        } => join_parts(vec![tool_name.clone(), summary.clone()]),
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode,
            available_modes,
            available_commands,
            ..
        } => {
            let mut parts = Vec::new();
            if let Some(mode) = current_mode {
                parts.push(mode.clone());
            }
            parts.extend(available_modes.iter().map(|m| m.mode_id.clone()));
            parts.extend(
                available_commands
                    .iter()
                    .map(|c| format!("{} {}", c.name, c.description)),
            );
            join_parts(parts)
        }
        UnifiedEventPayload::BootstrapLog { line } => join_parts(vec![line.clone()]),
        UnifiedEventPayload::StatusChanged {
            status,
            error_message,
        } => {
            let mut parts = vec![status.to_string()];
            if let Some(message) = error_message {
                parts.push(message.clone());
            }
            join_parts(parts)
        }
        // FTS 対象外 (バイナリ系・識別子系・投影の更新源のみのイベント)
        UnifiedEventPayload::SessionCreated { .. }
        | UnifiedEventPayload::SessionAgentBound { .. }
        | UnifiedEventPayload::PermissionResolved { .. }
        | UnifiedEventPayload::SessionReverted { .. }
        | UnifiedEventPayload::SessionArchived { .. }
        | UnifiedEventPayload::SessionDeleted {}
        | UnifiedEventPayload::TerminalOutput { .. }
        | UnifiedEventPayload::TerminalInput { .. } => None,
    };

    (
        event_type,
        text.map(|t| truncate_chars(&t, MAX_SEARCHABLE_CHARS)),
    )
}

/// 空要素を除いて改行で連結する。全要素が空なら `None`。
fn join_parts(parts: Vec<String>) -> Option<String> {
    let joined = parts
        .into_iter()
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

/// 文字境界を壊さずに `max_chars` 文字へ切り詰める。
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(max_chars).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::common::{AttachmentMeta, FileDiff, PermissionOption, PlanEntry};

    #[test]
    fn user_message_includes_text_and_attachment_names() {
        let payload = UnifiedEventPayload::UserMessage {
            text: "認証エラーを修正して".to_owned(),
            attachments: vec![AttachmentMeta {
                file_name: "screenshot.png".to_owned(),
                mime_type: "image/png".to_owned(),
                size_bytes: 10,
                local_path: None,
                data_b64: None,
            }],
            client_source: "cli".to_owned(),
            snapshot_tree_hash: None,
        };
        let (event_type, text) = classify_payload(&payload);
        assert_eq!(event_type, "user_message");
        let text = text.unwrap();
        assert!(text.contains("認証エラーを修正して"));
        assert!(text.contains("screenshot.png"));
    }

    #[test]
    fn tool_call_includes_title_locations_and_output() {
        let payload = UnifiedEventPayload::ToolCall {
            tool_call_id: "t1".to_owned(),
            title: "cargo test --workspace".to_owned(),
            kind: "execute".to_owned(),
            status: "completed".to_owned(),
            locations: vec!["crates/fxg-db/src/lib.rs".to_owned()],
            diff: Some(FileDiff {
                path: "a.rs".to_owned(),
                old_text: None,
                new_text: None,
                unified_diff: "diff".to_owned(),
                additions: 1,
                deletions: 0,
            }),
            raw_output: Some("test result: FAILED".to_owned()),
        };
        let (event_type, text) = classify_payload(&payload);
        assert_eq!(event_type, "tool_call");
        let text = text.unwrap();
        assert!(text.contains("cargo test --workspace"));
        assert!(text.contains("crates/fxg-db/src/lib.rs"));
        assert!(text.contains("test result: FAILED"));
    }

    #[test]
    fn terminal_output_and_input_are_excluded_from_fts() {
        let output = UnifiedEventPayload::TerminalOutput {
            terminal_id: "t1".to_owned(),
            command: "ls".to_owned(),
            data_b64: "GVsbG8=".to_owned(),
            exit_code: None,
        };
        assert_eq!(classify_payload(&output).1, None);

        let input = UnifiedEventPayload::TerminalInput {
            terminal_id: "t1".to_owned(),
            data_b64: "aGk=".to_owned(),
        };
        assert_eq!(classify_payload(&input).1, None);
    }

    #[test]
    fn permission_request_is_searchable() {
        let payload = UnifiedEventPayload::PermissionRequest {
            request_id: "r1".to_owned(),
            tool_name: "terminal/create".to_owned(),
            summary: "Run command: cargo test --workspace".to_owned(),
            options: vec![PermissionOption {
                option_id: "allow_once".to_owned(),
                name: "Allow".to_owned(),
                kind: "allow_once".to_owned(),
            }],
            details: serde_json::json!({}),
        };
        let (event_type, text) = classify_payload(&payload);
        assert_eq!(event_type, "permission_request");
        assert!(text.unwrap().contains("cargo test --workspace"));
    }

    #[test]
    fn plan_and_status_are_searchable() {
        let plan = UnifiedEventPayload::PlanUpdate {
            entries: vec![PlanEntry {
                id: "1".to_owned(),
                title: "テストを追加".to_owned(),
                status: "in_progress".to_owned(),
            }],
        };
        assert_eq!(classify_payload(&plan).0, "plan");
        assert!(classify_payload(&plan).1.unwrap().contains("テストを追加"));

        let status = UnifiedEventPayload::StatusChanged {
            status: fxg_protocol::common::SessionStatus::Error,
            error_message: Some("認証トークンが無効です".to_owned()),
        };
        assert_eq!(classify_payload(&status).0, "status_change");
        assert!(
            classify_payload(&status)
                .1
                .unwrap()
                .contains("認証トークンが無効です")
        );
    }

    #[test]
    fn session_created_has_no_searchable_text() {
        let payload = UnifiedEventPayload::SessionCreated {
            node_id: "n".to_owned(),
            project_id: "github.com/nazo6/flexagent".to_owned(),
            project_name: "flexagent".to_owned(),
            local_path: "/tmp".to_owned(),
            git_branch: None,
            is_worktree: false,
            agent_id: "opencode2".to_owned(),
            parent_session_id: None,
            fork_from_node_seq: None,
            title: "New Session".to_owned(),
            opencode_mode: None,
        };
        assert_eq!(classify_payload(&payload).1, None);
    }

    #[test]
    fn long_text_is_truncated_on_char_boundary() {
        let long = "あ".repeat(MAX_SEARCHABLE_CHARS + 100);
        let payload = UnifiedEventPayload::AgentMessage {
            message_id: "m".to_owned(),
            text: long,
            is_complete: true,
        };
        let text = classify_payload(&payload).1.unwrap();
        assert_eq!(text.chars().count(), MAX_SEARCHABLE_CHARS + 1); // + 省略記号
        assert!(text.ends_with('…'));
    }

    #[test]
    fn empty_texts_collapse_to_none() {
        let payload = UnifiedEventPayload::AgentMessage {
            message_id: "m".to_owned(),
            text: "   ".to_owned(),
            is_complete: true,
        };
        assert_eq!(classify_payload(&payload).1, None);
    }
}
