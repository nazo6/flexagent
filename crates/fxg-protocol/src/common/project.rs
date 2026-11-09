//! Logical projects, node bindings, worktrees and diffs.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use specta::Type;

use super::EnumParseError;
use crate::{NodeId, ProjectId};

/// Lifecycle status of a node (same values as `nodes.lifecycle_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum NodeLifecycleStatus {
    /// Provisioning (ephemeral VM)
    Provisioning,
    /// Bootstrapping (ephemeral VM)
    Bootstrapping,
    /// Ready to serve
    Ready,
    /// Shutting down (draining)
    Draining,
    /// Terminated
    Terminated,
    /// Error
    Error,
}

impl NodeLifecycleStatus {
    /// String representation used in the DB / JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Bootstrapping => "bootstrapping",
            Self::Ready => "ready",
            Self::Draining => "draining",
            Self::Terminated => "terminated",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for NodeLifecycleStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for NodeLifecycleStatus {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "provisioning" => Ok(Self::Provisioning),
            "bootstrapping" => Ok(Self::Bootstrapping),
            "ready" => Ok(Self::Ready),
            "draining" => Ok(Self::Draining),
            "terminated" => Ok(Self::Terminated),
            "error" => Ok(Self::Error),
            other => Err(EnumParseError::new("node lifecycle status", other)),
        }
    }
}

/// Project binding information reported in `NodeHello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct NodeProjectReport {
    /// Logical project ID
    pub project_id: ProjectId,
    /// Display name
    pub name: String,
    /// Git URL the canonical form was derived from
    pub canonical_git_url: Option<String>,
    /// How the project identity was derived
    pub identity: crate::settings::ProjectIdentity,
    /// Local path bindings on this node (including worktrees)
    pub bindings: Vec<ProjectNodeBinding>,
}

/// Local path binding between a project and a node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectNodeBinding {
    /// Local path on the node
    pub local_path: String,
    /// Whether this is a git worktree (false means the main repository)
    pub is_worktree: bool,
    /// Whether the path exists on the node at report time (directory check)
    pub path_exists: bool,
    /// Git branch as of the last check
    pub git_branch: Option<String>,
    /// Last used time (Unix epoch ms)
    pub last_used_at: i64,
}

/// Unified diff information of a file change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct FileDiff {
    /// Path of the affected file
    pub path: String,
    /// Text before the change
    pub old_text: Option<String>,
    /// Text after the change
    pub new_text: Option<String>,
    /// Unified diff body
    pub unified_diff: String,
    /// Number of added lines
    pub additions: u32,
    /// Number of deleted lines
    pub deletions: u32,
}

/// Worktree operation action (`ManageWorktree` command).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "action", rename_all = "snake_case")]
#[specta(collect)]
pub enum WorktreeAction {
    /// Create a new worktree (`git worktree add -b <branch> <path> <base>`)
    Add {
        /// Name of the branch to create
        branch: String,
        /// Base branch (defaults to the current HEAD)
        base_branch: Option<String>,
    },
    /// Remove a worktree (`git worktree remove`)
    Remove {
        /// Path to remove
        path: String,
        /// Whether to remove even if there are uncommitted changes
        force: bool,
    },
    /// Clean up the administrative data of deleted worktrees (`git worktree prune`)
    Prune,
}

/// Scope of a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum DiffScope {
    /// Uncommitted diff of the working tree (`git diff HEAD`)
    Uncommitted,
    /// Cumulative diff against the base branch (`git diff <base>...HEAD`)
    BranchBase,
}

/// Result of fetching the diff of a git working tree (`GET /api/v1/sessions/:id/diff`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorkspaceDiffResponse {
    /// Scope of the fetched diff
    pub scope: DiffScope,
    /// Base branch used for the comparison (set for `BranchBase`)
    pub base_branch: Option<String>,
    /// Current HEAD commit
    pub head_commit: String,
    /// Diff per changed file
    pub files: Vec<FileDiff>,
}

/// Project projection exposed to clients (`GET /api/v1/projects`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectSummary {
    /// Logical project ID
    pub project_id: ProjectId,
    /// Display name
    pub name: String,
    /// Git URL the canonical form was derived from
    pub canonical_git_url: Option<String>,
    /// How the project identity was derived
    pub identity: crate::settings::ProjectIdentity,
    /// Creation time (Unix epoch ms)
    pub created_at: i64,
    /// Last update time (Unix epoch ms)
    pub updated_at: i64,
    /// Local path bindings for each participating node
    pub bindings: Vec<ProjectBindingSummary>,
}

/// Project-to-node binding exposed to clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectBindingSummary {
    /// Node ID
    pub node_id: NodeId,
    /// Local path on the node
    pub local_path: String,
    /// Whether this is a git worktree
    pub is_worktree: bool,
    /// Whether the local path actually exists on the node
    ///
    /// On the central server this is the value from the last `NodeHello` report;
    /// on the local node it is checked live when fetching.
    pub path_exists: bool,
    /// Git branch as of the last check
    pub git_branch: Option<String>,
    /// Last used time (Unix epoch ms)
    pub last_used_at: i64,
}

/// Node information exposed to clients (`GET /api/v1/nodes` / `GET /api/v1/system/info`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct NodeSummary {
    /// Node ID
    pub node_id: NodeId,
    /// Display name
    pub name: String,
    /// OS (`windows` / `linux` / `macos`)
    pub os: String,
    /// Architecture (`x86_64` / `aarch64`)
    pub arch: String,
    /// fxg binary version
    pub version: String,
    /// Whether this is an ephemeral VM / container node
    pub is_ephemeral: bool,
    /// Provisioner identifier of an ephemeral node
    pub provisioner: Option<String>,
    /// Lifecycle status
    pub lifecycle_status: NodeLifecycleStatus,
    /// Whether it is currently connected
    pub is_online: bool,
    /// Last successful contact time (Unix epoch ms)
    pub last_seen_at: i64,
    /// Agent IDs available on this node
    pub installed_agents: Vec<String>,
}
