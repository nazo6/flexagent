//! Native OpenCode v2 driver (Tier 3).
//!
//! OpenCode exposes a full session API over `opencode serve` (HTTP + SSE).
//! ACP only exposes a subset (no revert, no message-boundary fork, no
//! server-side steering, no provider/MCP/skill management), so this driver
//! talks to the native API directly.
//!
//! The driver reuses the OpenCode service when one is running
//! (`<state>/opencode/service.json`) and otherwise starts a private
//! `opencode serve` process owned by the node. Sessions live in OpenCode's own
//! store, so a session created here can be continued by the TUI or another
//! OpenCode client.
//!
//! Scope (Phase 1-3 of the native-driver plan):
//! - create / bind / resume / fork sessions
//! - streaming text, reasoning, tools, permissions, forms, usage, compaction
//! - model / agent selection, slash commands, attachments
//! - revert, message-boundary fork, steering, manual compaction
//! - subagent child sessions
//! - history import
//! - provider / MCP / skill / command management primitives (used by the node API)
//!
//! Design: `docs/04-agent-drivers-and-windows.md` and
//! `docs/proposals/2026-11-14-opencode-native-driver.md`.

#![allow(
    clippy::disallowed_types,
    reason = "OpenCode v2 HTTP/SSE payloads are parsed at this external boundary"
)]

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use futures_util::StreamExt;
use fxg_protocol::client_api::AgentCapabilitiesPreview;
use fxg_protocol::common::{
    ClientSource, CommandInfo, ConfigOptionCategory, ConfigOptionChoice, ConfigOptionInfo,
    ConfigOptionValue, ElicitationAction, ElicitationArrayItems, ElicitationContent,
    ElicitationOption, ElicitationProperty, ElicitationPropertyKind, ElicitationSchema,
    ElicitationSchemaType, ElicitationValue, ForkScope, InitialConfig, ModeInfo,
    NativeSessionFeature, PermissionOption, PermissionOptionKind, PromptInput, RawJson,
    SessionStatus, StreamDeltaPayload, ToolCallKind, ToolCallStatus, UsageCost,
};
use fxg_protocol::events::{SubagentStatus, UnifiedEventPayload};
use fxg_protocol::util::uuid_v7;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

use crate::driver::{
    ActiveSessionHandle, AgentDriver, AgentLaunchSpec, AgentSessionInfo, AgentSessionList,
    DriverEvent, DriverKind, DriverSessionCapabilities, HistoryCaptureUnavailable,
    NativeForkUnavailable, NativeResumeUnavailable, NativeRevertUnavailable, StartSessionRequest,
    StartedSession,
};
use crate::management::AgentManagement;

const AUTH_USER: &str = "opencode";
const OPENCODE_MAJOR: u64 = 2;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const SSE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const SSE_RECONNECT_DELAY: Duration = Duration::from_millis(750);
const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(45);
const SERVER_HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(200);
const CATALOG_READY_TIMEOUT: Duration = Duration::from_secs(5);
const CATALOG_POLL_INTERVAL: Duration = Duration::from_millis(150);
const MAX_SEEN_EVENT_IDS: usize = 4096;
const MAX_STDERR_LINES: usize = 200;
const COMPACT_COMMAND: &str = "compact";
/// `after` value used only to read the current durable-log watermark without
/// fetching the historical events.
const LOG_WATERMARK_PROBE: u64 = 1_000_000_000_000;

/// Discards a failed optional read (used for OpenCode endpoints that are
/// unavailable on older versions).
#[allow(
    clippy::manual_ok_err,
    reason = "Result::ok is disallowed by the workspace lint"
)]
fn optional<T, E: std::fmt::Display>(result: Result<T, E>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(err) => {
            debug!("optional OpenCode request failed: {err}");
            None
        }
    }
}

/// Reads an environment variable without treating a non-Unicode value as an error.
#[allow(
    clippy::manual_ok_err,
    reason = "Result::ok is disallowed by the workspace lint"
)]
fn env_var(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) => Some(value),
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

/// A typed-ish client for the OpenCode v2 HTTP API.
///
/// Responses are parsed at this boundary; the driver converts them to fxg's
/// normalized types immediately.
mod client;
mod driver;
mod mapping;
mod server;

pub use client::OpenCodeClient;
pub use server::OpenCodeNativeDriver;

use driver::*;
use mapping::*;
use server::*;
