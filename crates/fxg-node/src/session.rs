//! Immediate delivery and persistence (compaction) of session events.
//!
//! Design: `docs/01-architecture-and-sync.md` §2.3.
//!
//! - **Separation of delivery and persistence**: token chunks mid-stream are
//!   delivered immediately to the in-memory broadcast channel via
//!   [`SessionEventBus::publish_delta`] and are not written to SQLite.
//! - **Persistence on turn completion**: completed events (`is_complete = true` etc.)
//!   are assigned a `node_seq` by [`SessionEventBus::record`], appended to `node.db`,
//!   and then delivered as finalized events.
//! - **Ephemeral events**: discrete events that are not persisted, such as
//!   keystrokes (`TerminalInput`), are only delivered via
//!   [`SessionEventBus::publish_ephemeral_event`] (they do not consume a `node_seq`).
//!
//! Clients upsert received events by `event_id` / `(session_id, node_seq)`, merge
//! and display `StreamDeltaPayload` by `message_id` / `thought_id`, and replace
//! them with the final rendering when the completed event arrives.

use std::sync::Arc;

use fxg_db::Db;
use fxg_protocol::common::StreamDeltaPayload;
use fxg_protocol::events::{SessionEventEnvelope, UnifiedEventPayload};
use fxg_protocol::util::{now_ms, uuid_v7};
use tokio::sync::broadcast;

use crate::error::NodeError;

/// Buffer size of the broadcast channel
/// (generous because token chunks arrive quickly. Lagging subscribers detect
/// `Lagged` and re-fetch the history from `node.db`).
const BROADCAST_CAPACITY: usize = 2048;

/// Per-session message delivered to subscribers.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionBroadcast {
    /// A persisted event (`node_seq` / `cursor` finalized)
    Persisted {
        /// The event itself
        event: SessionEventEnvelope,
        /// `session_events.cursor` in `node.db` (incremental resume cursor)
        cursor: u64,
    },
    /// An ephemeral event that is not persisted (keystrokes etc.).
    /// No `node_seq` is assigned, so `0` is stored.
    Ephemeral(SessionEventEnvelope),
    /// A partial delta chunk mid-stream (not persisted)
    StreamDelta {
        /// Target session ID
        session_id: fxg_protocol::SessionId,
        /// The delta itself
        delta: StreamDeltaPayload,
    },
}

impl SessionBroadcast {
    /// The target session ID.
    pub fn session_id(&self) -> &fxg_protocol::SessionId {
        match self {
            Self::Persisted { event, .. } | Self::Ephemeral(event) => &event.session_id,
            Self::StreamDelta { session_id, .. } => session_id,
        }
    }

    /// Returns `(envelope, cursor)` if this is a persisted event.
    pub fn persisted(&self) -> Option<(&SessionEventEnvelope, u64)> {
        match self {
            Self::Persisted { event, cursor } => Some((event, *cursor)),
            _ => None,
        }
    }
}

/// Session event bus (immediate delivery + idempotent persistence).
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
    /// Creates a new bus from a DB handle.
    pub fn new(db: Db) -> Self {
        let (events, _receiver) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            inner: Arc::new(Inner { db, events }),
        }
    }

    /// The DB handle for persistence.
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// Subscribes to the event stream (for IPC / local WS / the outbox worker).
    pub fn subscribe(&self) -> broadcast::Receiver<SessionBroadcast> {
        self.inner.events.subscribe()
    }

    /// The current number of subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.inner.events.receiver_count()
    }

    /// Creates a new session (persists `SessionCreated` as `node_seq = 1`).
    ///
    /// # Errors
    ///
    /// - `payload` is not [`UnifiedEventPayload::SessionCreated`]
    /// - The same `session_id` already exists
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
            event_id: uuid_v7().into(),
            session_id: session_id.to_owned().into(),
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

    /// Persists a completed event with an allocated `node_seq` and delivers it as a finalized event.
    ///
    /// Events that are not persistable ([`UnifiedEventPayload::is_persistable`] is `false`)
    /// are rejected with [`NodeError::NonPersistableEvent`]
    /// (use [`Self::publish_delta`] for chunks mid-stream).
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

    /// Delivers a partial delta chunk mid-stream immediately (does not write to the DB).
    pub fn publish_delta(
        &self,
        session_id: impl Into<fxg_protocol::SessionId>,
        delta: StreamDeltaPayload,
    ) {
        self.broadcast(SessionBroadcast::StreamDelta {
            session_id: session_id.into(),
            delta,
        });
    }

    /// Delivers a discrete event that is not persisted (keystrokes etc.).
    ///
    /// No `node_seq` is assigned and `0` is set (so no gaps appear in the event log sequence).
    pub fn publish_ephemeral_event(
        &self,
        session_id: impl Into<fxg_protocol::SessionId>,
        payload: UnifiedEventPayload,
    ) -> SessionEventEnvelope {
        let envelope = SessionEventEnvelope {
            event_id: uuid_v7().into(),
            session_id: session_id.into(),
            node_seq: 0,
            created_at: now_ms(),
            payload,
        };
        self.broadcast(SessionBroadcast::Ephemeral(envelope.clone()));
        envelope
    }

    /// Sends to subscribers (ignores failures when there are no subscribers).
    fn broadcast(&self, message: SessionBroadcast) {
        fxg_protocol::discard_with!(
            self.inner.events.send(message),
            "send a message to a channel that may be closed"
        );
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
            project_id: PROJECT_ID.into(),
            name: "flexagent".to_owned(),
            canonical_git_url: None,
            identity: fxg_protocol::settings::ProjectIdentity::local(),
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
            node_id: NODE_ID.into(),
            project_id: PROJECT_ID.into(),
            project_name: "flexagent".to_owned(),
            local_path: "/tmp/repo".to_owned(),
            git_branch: Some("main".to_owned()),
            is_worktree: false,
            agent_id: "opencode".to_owned(),
            parent_session_id: None,
            title: "New Session".to_owned(),
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
            project_id: PROJECT_ID.into(),
            name: "flexagent".to_owned(),
            canonical_git_url: None,
            identity: fxg_protocol::settings::ProjectIdentity::local(),
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

        // Duplicate creation is rejected
        let err = bus
            .create_session(&session_id, session_created_payload())
            .await
            .expect_err("duplicate");
        assert!(matches!(err, NodeError::InvalidSession(_)));

        // Payloads other than SessionCreated are also rejected
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

        // It is persisted (retrievable via the history API)
        let batch = bus
            .db()
            .session_events_after(&session_id, 0, 100)
            .await
            .expect("events");
        assert_eq!(batch.events.len(), 3);
        assert_eq!(batch.events[2].node_seq, 3);

        // Delivered as a finalized event
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

        // Mid-stream (is_complete = false) is not persisted
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
            NodeError::NonPersistableEvent(fxg_protocol::common::EventType::AgentMessage)
        ));

        // Keystrokes are not persisted either
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
            NodeError::NonPersistableEvent(fxg_protocol::common::EventType::TerminalInput)
        ));

        // Completed events can be persisted
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
                    assert_eq!(id.as_str(), session_id);
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

        // Nothing is written to the DB (only SessionCreated)
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
        assert_eq!(envelope.node_seq, 0, "ephemeral events have no node_seq");
        assert!(matches!(
            receiver.try_recv().expect("broadcast"),
            SessionBroadcast::Ephemeral(_)
        ));

        // Since no sequence was allocated, the next persisted event is node_seq = 2
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
