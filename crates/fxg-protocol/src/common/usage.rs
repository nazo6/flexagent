//! Agent-level usage limits (plan quotas, budgets).
//!
//! These types describe what a meter measures, not how it is displayed: the
//! source identifies the contract (for example `OpenCode Go`), the group the
//! covered scope (a model group or the whole allowance), and the window the
//! period (`5h` / weekly / monthly).
//!
//! Design: `docs/03-protocol-and-api.md` §3.7,
//! `docs/04-agent-drivers-and-windows.md` §1.2.

use serde::{Deserialize, Serialize};
use specta::Type;

/// Usage limits reported by an agent integration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentUsageLimits {
    /// Where the limits come from (contract, plan, account).
    pub source: UsageSource,
    /// Scopes covered by the limits (one entry per model group / allowance).
    pub groups: Vec<UsageGroup>,
    /// When the snapshot was fetched (Unix epoch ms).
    pub fetched_at: i64,
}

/// A contract or account that owns a set of usage limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UsageSource {
    /// Stable source ID (`opencode-go`, `opencode-console`, `antigravity`).
    pub id: String,
    /// Display name shown next to every meter (`OpenCode Go`, `Antigravity`).
    pub label: String,
    /// Plan name when the agent reports it (`Go Plus`, `Pro`). Never guessed.
    pub plan: Option<String>,
    /// Account / workspace identifier when the agent reports it.
    pub account: Option<String>,
    /// Icon URL or built-in asset key.
    pub icon: Option<String>,
}

/// One scope of a usage snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UsageGroup {
    /// Stable group ID within the source.
    pub id: String,
    /// Display name from the agent (`Go allowance`, `Gemini Models`).
    pub label: String,
    /// Optional explanation (for example the models covered by the group).
    pub description: Option<String>,
    /// Time windows measured for this group.
    pub windows: Vec<UsageWindow>,
}

/// One time window of a usage group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UsageWindow {
    /// Period kind (`rolling_5h`, `weekly`, `monthly`, `budget`).
    pub kind: UsageWindowKind,
    /// Agent-provided label when the kind is not enough (`Weekly Limit Remaining`).
    pub label: Option<String>,
    /// Fraction already used (`0.0` = nothing used, `1.0` = exhausted).
    pub used_fraction: f64,
    /// When the window resets (Unix epoch ms), when the agent reports it.
    pub resets_at: Option<i64>,
    /// Whether the window is currently blocking requests.
    pub limited: bool,
}

crate::open_string_enum! {
    /// Period kind of a [`UsageWindow`].
    ///
    /// Open so a new agent window does not break older clients; the UI falls
    /// back to [`UsageWindow::label`] for unknown values.
    pub enum UsageWindowKind {
        /// Rolling 5-hour window (OpenCode Go).
        Rolling5h => "rolling_5h",
        /// Fixed 5-hour window (Antigravity).
        FiveHour => "5h",
        /// Weekly window.
        Weekly => "weekly",
        /// Monthly window.
        Monthly => "monthly",
        /// Dollar budget window (OpenCode Console).
        Budget => "budget",
    }
}

impl UsageWindowKind {
    /// Whether this window is one of the windows the UI knows how to label.
    #[must_use]
    pub fn is_standard(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}
