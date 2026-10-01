//! セッションイベントの即時配信と永続化 (Compaction)。
//!
//! 設計: `docs/01-architecture-and-sync.md` §2.3。
//!
//! - **配信と永続化の分離**: ストリーミング途中のトークンチャンクは
//!   [`SessionEventBus::publish_delta`] でメモリ上のブロードキャストチャネルへ
//!   即時配信し、SQLite へは書き込まない。
//! - **ターン完了時の永続化**: 完成したイベント (`is_complete = true` 等) は
//!   [`SessionEventBus::record`] が `node_seq` を採番して `node.db` へ追記し、
//!   その後で確定イベントとして配信する。
//! - **エフェメラルイベント**: キーストローク (`TerminalInput`) のように
//!   永続化しない離散イベントは [`SessionEventBus::publish_ephemeral_event`] で
//!   配信のみ行う (`node_seq` を消費しない)。
//!
//! クライアントは受信イベントを `event_id` / `(session_id, node_seq)` で upsert し、
//! `StreamDeltaPayload` は `message_id` / `thought_id` でマージ表示したうえで、
//! 完成イベント到着時に確定表示へ置き換える。

use std::sync::Arc;

use fxg_db::Db;
use fxg_protocol::common::StreamDeltaPayload;
use fxg_protocol::events::{SessionEventEnvelope, UnifiedEventPayload};
use fxg_protocol::util::{now_ms, uuid_v7};
use tokio::sync::broadcast;

use crate::error::NodeError;

/// ブロードキャストチャネルのバッファサイズ
/// (トークンチャンクの流入が速いため広めに取る。遅延した購読者は `Lagged` を検知し
/// `node.db` から履歴を取り直す)。
const BROADCAST_CAPACITY: usize = 2048;

/// 購読者へ配信されるセッション単位のメッセージ。
#[derive(Debug, Clone, PartialEq)]
pub enum SessionBroadcast {
    /// 永続化済みイベント (`node_seq` / `cursor` 確定)
    Persisted {
        /// イベント本体
        event: SessionEventEnvelope,
        /// `node.db` の `session_events.cursor` (差分再開カーソル)
        cursor: u64,
    },
    /// 永続化しないエフェメラルイベント (キーストローク等)。
    /// `node_seq` は採番されないため `0` が入る。
    Ephemeral(SessionEventEnvelope),
    /// ストリーミング途中の差分チャンク (永続化されない)
    StreamDelta {
        /// 対象セッションID
        session_id: String,
        /// 差分本体
        delta: StreamDeltaPayload,
    },
}

impl SessionBroadcast {
    /// 対象セッションID。
    pub fn session_id(&self) -> &str {
        match self {
            Self::Persisted { event, .. } | Self::Ephemeral(event) => &event.session_id,
            Self::StreamDelta { session_id, .. } => session_id,
        }
    }

    /// 永続化済みイベントなら `(envelope, cursor)` を返す。
    pub fn persisted(&self) -> Option<(&SessionEventEnvelope, u64)> {
        match self {
            Self::Persisted { event, cursor } => Some((event, *cursor)),
            _ => None,
        }
    }
}

/// セッションイベントのバス (即時配信 + 冪等永続化)。
#[derive(Clone)]
pub struct SessionEventBus {
    inner: Arc<Inner>,
}

struct Inner {
    db: Db,
    events: broadcast::Sender<SessionBroadcast>,
}

impl std::fmt::Debug for SessionEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionEventBus")
            .field("subscribers", &self.inner.events.receiver_count())
            .finish()
    }
}

impl SessionEventBus {
    /// DB ハンドルから新しいバスを作成する。
    pub fn new(db: Db) -> Self {
        let (events, _receiver) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            inner: Arc::new(Inner { db, events }),
        }
    }

    /// 永続化先の DB ハンドル。
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// イベントストリームを購読する (IPC / ローカルWS / Outbox ワーカー用)。
    pub fn subscribe(&self) -> broadcast::Receiver<SessionBroadcast> {
        self.inner.events.subscribe()
    }

    /// 現在の購読者数。
    pub fn subscriber_count(&self) -> usize {
        self.inner.events.receiver_count()
    }

    /// 新規セッションを作成する (`SessionCreated` を `node_seq = 1` として永続化)。
    ///
    /// # Errors
    ///
    /// - `payload` が [`UnifiedEventPayload::SessionCreated`] でない
    /// - 同一 `session_id` が既に存在する
    pub async fn create_session(
        &self,
        session_id: &str,
        payload: UnifiedEventPayload,
    ) -> Result<SessionEventEnvelope, NodeError> {
        if !matches!(payload, UnifiedEventPayload::SessionCreated { .. }) {
            return Err(NodeError::InvalidSession(format!(
                "create_session requires SessionCreated payload, got {}",
                payload.event_type()
            )));
        }
        if self.inner.db.get_session(session_id).await?.is_some() {
            return Err(NodeError::InvalidSession(format!(
                "session already exists: {session_id}"
            )));
        }

        let envelope = SessionEventEnvelope {
            event_id: uuid_v7(),
            session_id: session_id.to_owned(),
            node_seq: 1,
            created_at: now_ms(),
            payload,
        };
        let outcomes = self
            .db()
            .append_events(std::slice::from_ref(&envelope))
            .await?;
        let cursor = outcomes.first().map(|outcome| outcome.cursor).unwrap_or(0);
        self.broadcast(SessionBroadcast::Persisted {
            event: envelope.clone(),
            cursor,
        });
        Ok(envelope)
    }

    /// 完成イベントを `node_seq` 採番のうえ永続化し、確定イベントとして配信する。
    ///
    /// 永続化対象外のイベント ([`UnifiedEventPayload::is_persistable`] が `false`) は
    /// [`NodeError::NonPersistableEvent`] で拒否する
    /// (ストリーミング途中のチャンクは [`Self::publish_delta`] を使う)。
    pub async fn record(
        &self,
        session_id: &str,
        payload: UnifiedEventPayload,
    ) -> Result<SessionEventEnvelope, NodeError> {
        if !payload.is_persistable() {
            return Err(NodeError::NonPersistableEvent(payload.event_type()));
        }
        let appended = self.db().append_next_event(session_id, payload).await?;
        self.broadcast(SessionBroadcast::Persisted {
            event: appended.envelope.clone(),
            cursor: appended.cursor,
        });
        Ok(appended.envelope)
    }

    /// ストリーミング途中の差分チャンクを即時配信する (DB へは書き込まない)。
    pub fn publish_delta(&self, session_id: impl Into<String>, delta: StreamDeltaPayload) {
        self.broadcast(SessionBroadcast::StreamDelta {
            session_id: session_id.into(),
            delta,
        });
    }

    /// 永続化しない離散イベント (キーストローク等) を配信する。
    ///
    /// `node_seq` は採番せず `0` を設定する (イベントログの連番に欠番を作らない)。
    pub fn publish_ephemeral_event(
        &self,
        session_id: impl Into<String>,
        payload: UnifiedEventPayload,
    ) -> SessionEventEnvelope {
        let envelope = SessionEventEnvelope {
            event_id: uuid_v7(),
            session_id: session_id.into(),
            node_seq: 0,
            created_at: now_ms(),
            payload,
        };
        self.broadcast(SessionBroadcast::Ephemeral(envelope.clone()));
        envelope
    }

    /// 購読者へ送信する (購読者がいない場合の失敗は無視する)。
    fn broadcast(&self, message: SessionBroadcast) {
        let _ = self.inner.events.send(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_db::DbRole;
    use fxg_protocol::common::{SessionStatus, StreamDeltaPayload};
    use fxg_protocol::events::UnifiedEventPayload;

    const NODE_ID: &str = "test-node";
    const PROJECT_ID: &str = "github.com/nazo6/flexagent";

    async fn setup() -> (SessionEventBus, String) {
        let db = Db::open_in_memory(DbRole::Node).await.expect("db");
        db.upsert_node(&fxg_db::NodeRecord::new(
            NODE_ID,
            "Test Node",
            "linux",
            "aarch64",
            "0.1.0",
        ))
        .await
        .expect("node");
        db.upsert_project(&fxg_db::ProjectRecord {
            project_id: PROJECT_ID.to_owned(),
            name: "flexagent".to_owned(),
            canonical_git_url: None,
        })
        .await
        .expect("project");
        let bus = SessionEventBus::new(db);
        let session_id = uuid_v7();
        bus.create_session(&session_id, session_created_payload())
            .await
            .expect("create session");
        (bus, session_id)
    }

    fn session_created_payload() -> UnifiedEventPayload {
        UnifiedEventPayload::SessionCreated {
            node_id: NODE_ID.to_owned(),
            project_id: PROJECT_ID.to_owned(),
            project_name: "flexagent".to_owned(),
            local_path: "/tmp/repo".to_owned(),
            git_branch: Some("main".to_owned()),
            is_worktree: false,
            agent_id: "opencode2".to_owned(),
            parent_session_id: None,
            fork_from_node_seq: None,
            title: "New Session".to_owned(),
            opencode_mode: None,
        }
    }

    #[tokio::test]
    async fn create_session_starts_at_node_seq_one() {
        let db = Db::open_in_memory(DbRole::Node).await.expect("db");
        db.upsert_node(&fxg_db::NodeRecord::new(
            NODE_ID,
            "Test Node",
            "linux",
            "aarch64",
            "0.1.0",
        ))
        .await
        .expect("node");
        db.upsert_project(&fxg_db::ProjectRecord {
            project_id: PROJECT_ID.to_owned(),
            name: "flexagent".to_owned(),
            canonical_git_url: None,
        })
        .await
        .expect("project");
        let bus = SessionEventBus::new(db);
        let session_id = uuid_v7();
        let mut receiver = bus.subscribe();

        let envelope = bus
            .create_session(&session_id, session_created_payload())
            .await
            .expect("create");
        assert_eq!(envelope.node_seq, 1);
        match receiver.try_recv().expect("broadcast") {
            SessionBroadcast::Persisted { event, cursor } => {
                assert_eq!(event.node_seq, 1);
                assert!(cursor > 0, "cursor must be assigned");
            }
            other => panic!("unexpected broadcast: {other:?}"),
        }

        let summary = bus
            .db()
            .get_session(&session_id)
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(summary.last_node_seq, 1);

        // 二重作成は拒否される
        let err = bus
            .create_session(&session_id, session_created_payload())
            .await
            .expect_err("duplicate");
        assert!(matches!(err, NodeError::InvalidSession(_)));

        // SessionCreated 以外の payload も拒否される
        let other = uuid_v7();
        let err = bus
            .create_session(
                &other,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Idle,
                    error_message: None,
                },
            )
            .await
            .expect_err("wrong payload");
        assert!(matches!(err, NodeError::InvalidSession(_)));
    }

    #[tokio::test]
    async fn record_allocates_sequential_node_seq_and_broadcasts() {
        let (bus, session_id) = setup().await;
        let mut receiver = bus.subscribe();

        let first = bus
            .record(
                &session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Running,
                    error_message: None,
                },
            )
            .await
            .expect("record 1");
        assert_eq!(first.node_seq, 2);
        let second = bus
            .record(
                &session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Idle,
                    error_message: None,
                },
            )
            .await
            .expect("record 2");
        assert_eq!(second.node_seq, 3);

        // 永続化されている (履歴 API で取得できる)
        let batch = bus
            .db()
            .session_events_after(&session_id, 0, 100)
            .await
            .expect("events");
        assert_eq!(batch.events.len(), 3);
        assert_eq!(batch.events[2].node_seq, 3);

        // 確定イベントとして配信される
        let mut broadcasted = Vec::new();
        while let Ok(message) = receiver.try_recv() {
            broadcasted.push(message);
        }
        assert_eq!(broadcasted.len(), 2);
        match &broadcasted[0] {
            SessionBroadcast::Persisted { event, cursor } => {
                assert_eq!(event.node_seq, 2);
                assert!(*cursor > 0);
            }
            other => panic!("unexpected broadcast: {other:?}"),
        }
    }

    #[tokio::test]
    async fn record_rejects_non_persistable_payloads() {
        let (bus, session_id) = setup().await;

        // ストリーミング途中 (is_complete = false) は永続化されない
        let err = bus
            .record(
                &session_id,
                UnifiedEventPayload::AgentMessage {
                    message_id: "m1".to_owned(),
                    text: "partial".to_owned(),
                    is_complete: false,
                },
            )
            .await
            .expect_err("non persistable");
        assert!(matches!(
            err,
            NodeError::NonPersistableEvent("agent_message")
        ));

        // キーストロークも永続化されない
        let err = bus
            .record(
                &session_id,
                UnifiedEventPayload::TerminalInput {
                    terminal_id: "t1".to_owned(),
                    data_b64: "aGk=".to_owned(),
                },
            )
            .await
            .expect_err("terminal input");
        assert!(matches!(
            err,
            NodeError::NonPersistableEvent("terminal_input")
        ));

        // 完成イベントは永続化できる
        let envelope = bus
            .record(
                &session_id,
                UnifiedEventPayload::AgentMessage {
                    message_id: "m1".to_owned(),
                    text: "complete".to_owned(),
                    is_complete: true,
                },
            )
            .await
            .expect("complete message");
        assert_eq!(envelope.node_seq, 2);
    }

    #[tokio::test]
    async fn deltas_are_broadcast_without_persisting() {
        let (bus, session_id) = setup().await;
        let mut receiver = bus.subscribe();

        bus.publish_delta(
            session_id.clone(),
            StreamDeltaPayload::AgentMessageDelta {
                message_id: "m1".to_owned(),
                text_delta: "Hel".to_owned(),
            },
        );
        bus.publish_delta(
            session_id.clone(),
            StreamDeltaPayload::AgentMessageDelta {
                message_id: "m1".to_owned(),
                text_delta: "lo".to_owned(),
            },
        );

        let mut deltas = 0;
        while let Ok(message) = receiver.try_recv() {
            match message {
                SessionBroadcast::StreamDelta {
                    session_id: id,
                    delta,
                } => {
                    assert_eq!(id, session_id);
                    assert!(matches!(
                        delta,
                        StreamDeltaPayload::AgentMessageDelta { .. }
                    ));
                    deltas += 1;
                }
                other => panic!("unexpected broadcast: {other:?}"),
            }
        }
        assert_eq!(deltas, 2);

        // DB には何も書かれていない (SessionCreated のみ)
        let batch = bus
            .db()
            .session_events_after(&session_id, 0, 100)
            .await
            .expect("events");
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].node_seq, 1);
    }

    #[tokio::test]
    async fn ephemeral_events_do_not_consume_node_seq() {
        let (bus, session_id) = setup().await;
        let mut receiver = bus.subscribe();

        let envelope = bus.publish_ephemeral_event(
            session_id.clone(),
            UnifiedEventPayload::TerminalInput {
                terminal_id: "term-1".to_owned(),
                data_b64: "bHM=".to_owned(),
            },
        );
        assert_eq!(envelope.node_seq, 0, "エフェメラルは node_seq を持たない");
        assert!(matches!(
            receiver.try_recv().expect("broadcast"),
            SessionBroadcast::Ephemeral(_)
        ));

        // 採番されていないため、次の永続イベントは node_seq = 2
        let next = bus
            .record(
                &session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Running,
                    error_message: None,
                },
            )
            .await
            .expect("record");
        assert_eq!(next.node_seq, 2);
    }

    #[tokio::test]
    async fn record_on_unknown_session_fails() {
        let (bus, _session_id) = setup().await;
        let err = bus
            .record(
                &uuid_v7(),
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Idle,
                    error_message: None,
                },
            )
            .await
            .expect_err("unknown session");
        assert!(matches!(
            err,
            NodeError::Db(fxg_db::DbError::SessionNotFound(_))
        ));
    }
}
