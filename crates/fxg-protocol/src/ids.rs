//! Typed entity identifiers.
//!
//! IDs stay plain strings on the wire and in the database. The newtypes exist so
//! that IDs of the same shape cannot be swapped at protocol / cross-crate
//! boundaries (for example `node_id` and `command_id` in hub routing).
//!
//! The types deliberately do not implement `Deref<Target = str>`: silent
//! coercion back to `&str` would undo the protection. Use [`as_str`] at the
//! places that need a string (SQL binds, paths, logs).

use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use specta::Type;

/// Defines an opaque string ID newtype.
macro_rules! string_id {
    (
        $(#[$meta:meta])*
        $name:ident
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
        #[serde(transparent)]
        #[specta(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps an existing identifier string.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Returns the identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = Infallible;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Ok(Self(value.to_owned()))
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        // Used by DB-backed IDs (SessionId / NodeId next). `CommandId` itself is
        // not persisted, but sharing the macro avoids a second implementation.
        $crate::maybe_impl_sqlx_text!($name);
    };
}

string_id! {
    /// Correlation ID of a command.
    ///
    /// Attached to `ServerToNodeMsg` / IPC / client WS requests and echoed back in
    /// `CommandResult`. It is not an entity ID and is not persisted.
    CommandId
}

string_id! {
    /// ID of a node (resident daemon or ephemeral VM).
    NodeId
}

string_id! {
    /// ID of a session (UUID v7 assigned by the running node / server).
    SessionId
}

string_id! {
    /// ID of a persisted session event (UUID v7).
    EventId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_id_is_transparent_on_the_wire() {
        let id = CommandId::new("c1");
        assert_eq!(serde_json::to_value(&id).expect("serialize"), "c1");
        assert_eq!(
            serde_json::from_value::<CommandId>("c1".into()).expect("deserialize"),
            id
        );
        assert_eq!(id.as_str(), "c1");
        assert_eq!(id.to_string(), "c1");
    }

    #[test]
    fn command_id_accepts_any_existing_string() {
        // IDs are opaque: legacy / future formats must roundtrip unchanged.
        let id: CommandId = "not-a-uuid".into();
        assert_eq!(id.as_str(), "not-a-uuid");
        assert_eq!(
            serde_json::to_value(&id).expect("serialize"),
            serde_json::json!("not-a-uuid")
        );
    }
}
