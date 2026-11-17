//! Diagnostics types: component health and drop counters.
//!
//! These types are shared by the local node and the hub so the Web UI
//! and CLI can inspect the same diagnostics regardless of the connection target
//! (design: `docs/03-protocol-and-api.md` §3.1).

use serde::{Deserialize, Serialize};
use specta::Type;

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
        /// Disk capacity of the data directory
        Disk => "disk",
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
