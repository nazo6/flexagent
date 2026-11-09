//! Test-only builders for large protocol structs.
//!
//! Fields added to these structs must only be set here, so a schema change does
//! not require editing every test. Enable the `testing` feature from a
//! `[dev-dependencies]` entry; production builds never include this module.

use crate::settings::ProjectIdentity;
use crate::{NodeId, ProjectId, SessionId};

use super::acp::PermissionPolicy;
use super::project::{NodeLifecycleStatus, NodeSummary, ProjectSummary};
use super::session::{SessionStatus, SessionSummary};

impl SessionSummary {
    /// Builds a fully populated idle session for tests.
    pub fn dummy() -> Self {
        Self {
            session_id: SessionId::from("session-dummy"),
            project_id: ProjectId::from("project-dummy"),
            node_id: NodeId::from("node-dummy"),
            local_path: "/tmp/fxg-dummy".to_owned(),
            git_branch: Some("main".to_owned()),
            is_worktree: false,
            agent_id: "test-agent".to_owned(),
            agent_session_id: None,
            parent_session_id: None,
            title: "dummy session".to_owned(),
            status: SessionStatus::Idle,
            current_mode: None,
            permission_policy: PermissionPolicy::default(),
            usage: None,
            last_node_seq: 0,
            fork_supported: false,
            created_at: 0,
            updated_at: 0,
            archived_at: None,
        }
    }
}

impl ProjectSummary {
    /// Builds a fully populated local project for tests.
    pub fn dummy() -> Self {
        Self {
            project_id: ProjectId::from("project-dummy"),
            name: "dummy project".to_owned(),
            canonical_git_url: None,
            identity: ProjectIdentity::local(),
            created_at: 0,
            updated_at: 0,
            bindings: Vec::new(),
        }
    }
}

impl NodeSummary {
    /// Builds a fully populated online node for tests.
    pub fn dummy() -> Self {
        Self {
            node_id: NodeId::from("node-dummy"),
            name: "dummy node".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            version: "0.0.0-test".to_owned(),
            is_ephemeral: false,
            provisioner: None,
            lifecycle_status: NodeLifecycleStatus::Ready,
            is_online: true,
            last_seen_at: 0,
            installed_agents: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dummy_builders_cover_every_field() {
        let session = SessionSummary::dummy();
        assert_eq!(session.status, SessionStatus::Idle);
        assert_eq!(session.agent_id, "test-agent");

        let project = ProjectSummary::dummy();
        assert_eq!(project.project_id.as_str(), "project-dummy");

        let node = NodeSummary::dummy();
        assert_eq!(node.lifecycle_status, NodeLifecycleStatus::Ready);
        assert!(node.is_online);
    }
}
