//! Child-session routing for ACP subagents.
//!
//! The ACP SDK's session router only delivers notifications addressed to the
//! root session. Child-addressed `session/update` notifications are claimed by
//! per-child dynamic handlers here, and vendor notifications are claimed by a
//! method allowlist. Both feed [`SessionIngress`] so the turn loop can map them
//! with the same streaming accumulator as the parent.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §2.3.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::v1::schema::SessionNotification;
use agent_client_protocol::{Agent, ConnectionTo, Dispatch, HandleDispatchFrom, Handled};
use fxg_protocol::events::SubagentStatus;
use tokio::sync::mpsc;

use super::SessionContexts;
use crate::agent::extension::{AcpChildUpdate, AcpNotificationParser};

/// Work routed from a dynamic handler to the session turn loop.
#[derive(Debug)]
pub(super) enum SessionIngress {
    /// A `session/update` addressed to a child session.
    Child(SessionNotification),
    /// A vendor notification parsed into a generic child update.
    Vendor(AcpChildUpdate),
}

/// Claims vendor-specific notification methods declared by an extension.
#[derive(Debug)]
pub(super) struct VendorNotificationHandler {
    methods: Vec<String>,
    parser: Arc<dyn AcpNotificationParser>,
    tx: mpsc::UnboundedSender<SessionIngress>,
}

impl VendorNotificationHandler {
    pub(super) fn new(
        parser: Arc<dyn AcpNotificationParser>,
        tx: mpsc::UnboundedSender<SessionIngress>,
    ) -> Self {
        Self {
            methods: parser.methods().iter().map(|m| (*m).to_owned()).collect(),
            parser,
            tx,
        }
    }
}

impl HandleDispatchFrom<Agent> for VendorNotificationHandler {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        _cx: ConnectionTo<Agent>,
    ) -> Result<Handled<Dispatch>, agent_client_protocol::Error> {
        let Dispatch::Notification(notification) = &message else {
            return Ok(Handled::No {
                message,
                retry: false,
            });
        };
        if !self
            .methods
            .iter()
            .any(|method| method == notification.method())
        {
            return Ok(Handled::No {
                message,
                retry: false,
            });
        }
        match self.parser.parse(
            notification.method(),
            &fxg_protocol::common::RawJson::new(notification.params().clone()),
        ) {
            Some(update) => {
                fxg_protocol::discard_with!(
                    self.tx.send(SessionIngress::Vendor(update)),
                    "route vendor notification"
                );
            }
            None => {
                tracing::warn!(
                    method = notification.method(),
                    "failed to parse vendor subagent notification"
                );
            }
        }
        Ok(Handled::Yes)
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        format!("VendorNotificationHandler({:?})", self.methods)
    }
}

/// Claims `session/update` notifications addressed to one child session.
#[derive(Debug)]
pub(super) struct ChildNotificationHandler {
    child_session_id: String,
    tx: mpsc::UnboundedSender<SessionIngress>,
}

impl ChildNotificationHandler {
    pub(super) fn new(child_session_id: String, tx: mpsc::UnboundedSender<SessionIngress>) -> Self {
        Self {
            child_session_id,
            tx,
        }
    }
}

impl HandleDispatchFrom<Agent> for ChildNotificationHandler {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        _cx: ConnectionTo<Agent>,
    ) -> Result<Handled<Dispatch>, agent_client_protocol::Error> {
        let Dispatch::Notification(notification) = &message else {
            return Ok(Handled::No {
                message,
                retry: false,
            });
        };
        if notification.method() != "session/update" {
            return Ok(Handled::No {
                message,
                retry: false,
            });
        }
        let Ok(parsed) =
            serde_json::from_value::<SessionNotification>(notification.params().clone())
        else {
            return Ok(Handled::No {
                message,
                retry: false,
            });
        };
        if parsed.session_id.to_string() != self.child_session_id {
            return Ok(Handled::No {
                message,
                retry: false,
            });
        }
        fxg_protocol::discard_with!(
            self.tx.send(SessionIngress::Child(parsed)),
            "route child session notification"
        );
        Ok(Handled::Yes)
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        format!("ChildNotificationHandler({})", self.child_session_id)
    }
}

/// Known child session metadata.
#[derive(Debug, Clone)]
pub(super) struct ChildInfo {
    /// Immediate parent session ID.
    pub(super) parent: String,
    /// Nesting depth (1 = direct child of the root session).
    pub(super) depth: u32,
    /// Working directory of the child.
    pub(super) cwd: PathBuf,
    /// Display title.
    pub(super) title: Option<String>,
    /// Role / purpose.
    pub(super) description: Option<String>,
    /// Current lifecycle state.
    pub(super) status: SubagentStatus,
}

/// Child session state for one ACP session.
pub(super) struct ChildSessions {
    root: String,
    root_cwd: PathBuf,
    by_id: HashMap<String, ChildInfo>,
    registered: HashSet<String>,
    contexts: SessionContexts,
}

impl ChildSessions {
    pub(super) fn new(root: String, root_cwd: PathBuf, contexts: SessionContexts) -> Self {
        Self {
            root,
            root_cwd,
            by_id: HashMap::new(),
            registered: HashSet::new(),
            contexts,
        }
    }

    /// Known child metadata.
    pub(super) fn get(&self, child: &str) -> Option<&ChildInfo> {
        self.by_id.get(child)
    }

    /// Whether the child handler was already registered.
    pub(super) fn mark_registered(&mut self, child: &str) -> bool {
        self.registered.insert(child.to_owned())
    }

    /// Registers a child context for fs / terminal / permission requests.
    pub(super) fn insert_context(
        &self,
        child: &str,
        cwd: PathBuf,
        events: mpsc::UnboundedSender<super::DriverEvent>,
    ) {
        self.contexts
            .lock()
            .expect("session contexts poisoned")
            .insert(
                child.to_owned(),
                super::SessionContext {
                    cwd,
                    events,
                    subagent_id: Some(child.to_owned()),
                },
            );
    }

    /// Upserts child metadata, keeping omitted fields unchanged.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn upsert(
        &mut self,
        child: &str,
        parent: Option<&str>,
        depth: u32,
        title: Option<String>,
        description: Option<String>,
        cwd: Option<PathBuf>,
        status: Option<SubagentStatus>,
    ) -> ChildInfo {
        let parent = parent
            .map(str::to_owned)
            .or_else(|| self.by_id.get(child).map(|info| info.parent.clone()))
            .unwrap_or_else(|| self.root.clone());
        let depth = if depth > 0 {
            depth
        } else {
            self.by_id
                .get(&parent)
                .map_or(1, |info| info.depth.saturating_add(1))
        };
        let inherited_cwd = self
            .by_id
            .get(&parent)
            .map(|info| info.cwd.clone())
            .unwrap_or_else(|| self.root_cwd.clone());
        let existing = self.by_id.get(child);
        let info = ChildInfo {
            parent,
            depth,
            cwd: cwd
                .or_else(|| existing.map(|info| info.cwd.clone()))
                .unwrap_or(inherited_cwd),
            title: title.or_else(|| existing.and_then(|info| info.title.clone())),
            description: description.or_else(|| existing.and_then(|info| info.description.clone())),
            status: status
                .or_else(|| existing.map(|info| info.status.clone()))
                .unwrap_or_else(|| SubagentStatus::Unknown("unknown".to_owned())),
        };
        self.by_id.insert(child.to_owned(), info.clone());
        info
    }
}

impl Drop for ChildSessions {
    fn drop(&mut self) {
        let mut contexts = self.contexts.lock().expect("session contexts poisoned");
        for child in self.by_id.keys() {
            contexts.remove(child);
        }
    }
}

/// Maps an ACP child work state to the normalized status.
#[cfg(feature = "acp-subagents")]
pub(super) fn map_state_update(
    state: &agent_client_protocol::v1::schema::StateUpdate,
) -> SubagentStatus {
    use agent_client_protocol::v1::schema::StateUpdate;

    match state {
        StateUpdate::Running(_) => SubagentStatus::Running,
        StateUpdate::Idle(_) => SubagentStatus::Completed,
        StateUpdate::RequiresAction(_) => SubagentStatus::RequiresAction,
        StateUpdate::Unknown(_) => SubagentStatus::Unknown("unknown".to_owned()),
        StateUpdate::Other(_) => SubagentStatus::Unknown("other".to_owned()),
        _ => SubagentStatus::Unknown("unknown".to_owned()),
    }
}
