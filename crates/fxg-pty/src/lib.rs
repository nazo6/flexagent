//! FlexAgent PTY 制御クレート (`fxg-pty`)
//!
//! ConPTY (Windows) / Unix PTY の双方向ストリーム管理、Windows Job Object による
//! プロセスツリー確実終了、`which` (`PATHEXT` 対応) / `dunce` による
//! コマンド・パス解決を提供する。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §5。
//!
//! # モジュール構成
//!
//! - `proc`: コマンド解決 (`resolve_command`)・パス正規化 (`canonicalize`)・
//!   プロセスツリーガード (`ProcessTreeGuard` / Windows `WinJobGuard`)
//! - `pty`: ConPTY / Unix PTY の双方向セッション管理 ([`PtySessionManager`])

#![warn(missing_docs)]

pub mod error;
pub mod proc;
pub mod pty;

pub use error::PtyError;
pub use proc::{ProcessTreeGuard, TreeShutdownOutcome, canonicalize, resolve_command};
pub use pty::{PtyEvent, PtySessionInfo, PtySessionManager, PtySpawnRequest};

#[cfg(windows)]
pub use proc::WinJobGuard;
