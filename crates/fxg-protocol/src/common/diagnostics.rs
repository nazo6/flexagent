//! Diagnostics types: recorded incidents, component health and drop counters.
//!
//! These types are shared by the local node and the hub so the Web UI
//! and CLI can inspect the same diagnostics regardless of the connection target
//! (design: `docs/03-protocol-and-api.md` §3.1).

use serde::{Deserialize, Serialize};
use specta::Type;

use super::error::ErrorCode;
use super::{RawJson, SessionSummary};
use crate::{NodeId, SessionId};

crate::open_string_enum! {
    /// Severity of a recorded incident.
    pub enum IncidentLevel {
        /// Informational event worth keeping for later diagnosis
        Info => "info",
        /// Recoverable degradation
        Warning => "warning",
        /// Failure that affected the user or the data
        Error => "error",
    }
}

crate::open_string_enum! {
    /// Component that produced an incident.
    pub enum IncidentSource {
        /// REST / Client WS boundary
        Api => "api",
        /// Agent process / ACP protocol
        Agent => "agent",
        /// Outbox synchronization / WebSocket relay
        Sync => "sync",
        /// Git operations (worktree, snapshot, diff)
        Git => "git",
        /// SQLite / migration
        Db => "db",
        /// PTY / child process
        Pty => "pty",
        /// ACP registry / agent catalog
        Registry => "registry",
        /// Provisioner / temporary VM
        Provisioner => "provisioner",
        /// Web Push delivery
        Push => "push",
        /// Desktop integration (tray / service)
        Service => "service",
    }
}

crate::open_string_enum! {
    /// Health component of a node or the hub.
    pub enum ComponentName {
        /// Database connectivity
        Db => "db",
        /// Migration state
        Migrations => "migrations",
        /// HTTP / WebSocket listener
        Http => "http",
        /// Hub synchronization
        Sync => "sync",
        /// ACP registry / agent catalog
        Registry => "registry",
        /// Agent process spawning
        AgentSpawn => "agent_spawn",
        /// Git operations
        Git => "git",
        /// PTY / child processes
        Pty => "pty",
        /// Provisioner / temporary VM
        Provisioner => "provisioner",
        /// Desktop integration (tray / service)
        Service => "service",
        /// Disk capacity of the data directory
        Disk => "disk",
        /// Web Push delivery
        Push => "push",
        /// Hub node hub
        Hub => "hub",
    }
}

crate::open_string_enum! {
    /// Health state of one component.
    pub enum ComponentState {
        /// Operating normally
        Ok => "ok",
        /// Working but degraded (backlog, recent recoverable failures)
        Degraded => "degraded",
        /// Not working
        Error => "error",
        /// No measurement available yet
        NotMeasured => "unknown",
    }
}

/// Health of one component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ComponentHealth {
    /// Component name
    pub component: ComponentName,
    /// Current state
    pub state: ComponentState,
    /// Human-readable reason for the current state (English fallback)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Last time the component was observed healthy (Unix epoch ms)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_at: Option<i64>,
    /// Last time the component failed (Unix epoch ms)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_at: Option<i64>,
}

impl ComponentHealth {
    /// Builds a healthy component entry observed at `now`.
    pub fn ok(component: ComponentName, now: i64) -> Self {
        Self {
            component,
            state: ComponentState::Ok,
            message: None,
            last_ok_at: Some(now),
            last_error_at: None,
        }
    }

    /// Builds a failing / degraded component entry observed at `now`.
    pub fn failed(
        component: ComponentName,
        state: ComponentState,
        message: impl Into<String>,
        now: i64,
    ) -> Self {
        Self {
            component,
            state,
            message: Some(message.into()),
            last_ok_at: None,
            last_error_at: Some(now),
        }
    }
}

/// Dropped / retried delivery counters (reset when the process restarts).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct DropCounters {
    /// Events dropped because a subscriber lagged
    pub dropped_events: u64,
    /// Broadcast receivers that observed `Lagged`
    pub lagged_broadcasts: u64,
    /// Client WS replays that failed
    pub replay_failures: u64,
    /// Outbox connection / send retries
    pub outbox_retries: u64,
    /// Web Push deliveries that failed
    pub push_failures: u64,
}

/// Overall health snapshot returned by diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SystemStatus {
    /// Measurement time (Unix epoch ms)
    pub checked_at: i64,
    /// Per-component health
    pub components: Vec<ComponentHealth>,
    /// Delivery drop / retry counters
    pub counters: DropCounters,
}

/// A persisted diagnostic incident.
///
/// Repeated occurrences of the same cause are grouped into one entry with an
/// increasing `occurrence_count` instead of filling the table with duplicates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct IncidentEntry {
    /// Row ID
    pub id: i64,
    /// First occurrence (Unix epoch ms)
    pub created_at: i64,
    /// Most recent occurrence (Unix epoch ms)
    pub last_seen_at: i64,
    /// Severity
    pub level: IncidentLevel,
    /// Structured error code
    pub code: ErrorCode,
    /// English human-readable summary
    pub summary: String,
    /// Technical details (paths, request context, ...)
    #[serde(default)]
    pub details: RawJson,
    /// Related session
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Related node
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<NodeId>,
    /// Request correlation ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Component that produced the incident
    pub source: IncidentSource,
    /// Number of occurrences since the first one
    pub occurrence_count: u64,
    /// Time the incident was marked resolved (Unix epoch ms)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
}

/// `GET /api/v1/diagnostics/incidents` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct IncidentListResponse {
    /// Newest first
    pub incidents: Vec<IncidentEntry>,
    /// Retention period of the ring buffer (days)
    pub retention_days: u32,
    /// Maximum number of retained rows
    pub max_entries: u32,
}

/// Summary of one session in a support bundle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SupportBundleSession {
    /// Session summary
    pub session: SessionSummary,
    /// Persisted event count
    pub event_count: u64,
}
