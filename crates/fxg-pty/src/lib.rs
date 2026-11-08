//! PTY control crate for FlexAgent (`fxg-pty`)
//!
//! Provides bidirectional stream management for ConPTY (Windows) / Unix PTY,
//! guaranteed process tree termination via Windows Job Objects, and command /
//! path resolution through `which` (`PATHEXT` aware) and `dunce`.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §5.
//!
//! # Modules
//!
//! - `proc`: command resolution (`resolve_command`), path normalization
//!   (`canonicalize`), and the process tree guard (`ProcessTreeGuard` / Windows
//!   `WinJobGuard`)
//! - `pty`: bidirectional ConPTY / Unix PTY session management
//!   ([`PtySessionManager`])

#![warn(missing_docs)]
// Migration baseline: remove this crate-level allow as the protocol enums are
// converted to open enums (Phase 1 of the type-safety plan).
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "type-safety migration baseline; see docs/06-type-safety.md"
)]

pub mod error;
pub mod proc;
pub mod pty;

pub use error::PtyError;
pub use proc::{ProcessTreeGuard, TreeShutdownOutcome, canonicalize, resolve_command};
pub use pty::{PtyEvent, PtySessionInfo, PtySessionManager, PtySpawnRequest};

#[cfg(windows)]
pub use proc::WinJobGuard;
