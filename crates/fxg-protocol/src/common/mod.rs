//! Shared helper types and error codes.
//!
//! Defines the types referenced by `UnifiedEventPayload`, by each protocol
//! message and by REST responses. All of them are handled by clients (UI / CLI),
//! so they carry `#[derive(specta::Type)]` + `#[specta(collect)]`
//! (design: `docs/03-protocol-and-api.md` §1.1).

mod acp;
mod agent;
mod audit;
mod error;
#[cfg(any(test, feature = "testing"))]
mod fixture;
mod project;
mod session;

pub use acp::*;
pub use agent::*;
pub use audit::*;
pub use error::*;
pub use project::*;
pub use session::*;

use serde::{Deserialize, Serialize};
use specta::Type;

/// Intentionally opaque JSON carried across the protocol.
///
/// Use this only at a declared boundary (tool arguments, error details, audit
/// details, ...). Domain data must be parsed into a typed struct or enum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(transparent)]
#[specta(transparent)]
pub struct RawJson(pub serde_json::Value);

impl RawJson {
    /// Wraps a JSON value.
    pub fn new(value: serde_json::Value) -> Self {
        Self(value)
    }

    /// Returns the wrapped JSON value.
    pub fn as_value(&self) -> &serde_json::Value {
        &self.0
    }

    /// Unwraps the JSON value.
    pub fn into_inner(self) -> serde_json::Value {
        self.0
    }
}

impl Default for RawJson {
    fn default() -> Self {
        Self(serde_json::Value::Object(serde_json::Map::new()))
    }
}

impl From<serde_json::Value> for RawJson {
    fn from(value: serde_json::Value) -> Self {
        Self(value)
    }
}

impl From<RawJson> for serde_json::Value {
    fn from(value: RawJson) -> Self {
        value.0
    }
}

/// Error returned when parsing the string representation of an enum fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} value: {value:?}")]
pub struct EnumParseError {
    /// Enum kind name (used in the error message)
    pub kind: &'static str,
    /// The string that failed to parse
    pub value: String,
}

impl EnumParseError {
    /// Creates a new parse error.
    pub fn new(kind: &'static str, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
        }
    }
}

// Closed enums stored in TEXT columns (`sessions.status` etc.). Unlike the open
// enums above, an unknown DB value is data corruption rather than a future peer
// value, so decoding fails instead of preserving it.
#[cfg(feature = "sqlx")]
crate::impl_sqlx_text!(
    SessionStatus,
    NodeLifecycleStatus,
    PermissionRequestStatus,
    ElicitationRequestStatus,
);
