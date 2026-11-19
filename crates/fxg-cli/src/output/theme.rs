//! Semantic roles for CLI styling.
//!
//! Call sites name a [`Role`] (`Error`, `Muted`, `Success`, ...) instead of an
//! ANSI color, so the binary keeps a single visual language and a palette
//! change touches only this file. [`Theme::plain`] resolves every role to an
//! empty [`Style`], which is how non-color output is produced: no escape ever
//! has to be stripped afterwards.

use std::sync::atomic::{AtomicBool, Ordering};

use anstyle::{AnsiColor, Effects, Style};

/// What a piece of text means, independent of how it is painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    /// Ordinary text; no styling.
    #[default]
    Plain,
    /// A banner or section title.
    Heading,
    /// Field labels and table headers.
    Key,
    /// Secondary text: hints, footers, timestamps.
    Muted,
    /// The accent that identifies the node (also used for links).
    Accent,
    /// A second accent used to tell surfaces apart (the hub).
    AccentAlt,
    /// A check or operation that succeeded.
    Success,
    /// Something that needs attention but is not a failure.
    Warning,
    /// A failure or a destructive action.
    Error,
}

static DISABLED: AtomicBool = AtomicBool::new(false);

/// Record the process-wide color decision for the current command.
///
/// `--no-color` and `log.no_color` are the explicit switches; `NO_COLOR` is
/// honored as described by <https://no-color.org/>. TTY detection is left to
/// `anstream`, which strips escapes per stream when output is redirected.
pub fn init(no_color: bool) {
    let env_disabled = std::env::var_os("NO_COLOR").is_some();
    DISABLED.store(no_color || env_disabled, Ordering::Relaxed);
}

/// The theme for the current process.
pub fn current() -> Theme {
    Theme::new(!DISABLED.load(Ordering::Relaxed))
}

/// Maps [`Role`]s to concrete [`Style`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    enabled: bool,
}

impl Theme {
    /// A theme that paints (`false` resolves every role to no styling).
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// A theme that always paints (tests and snapshots).
    #[cfg(test)]
    pub const fn styled() -> Self {
        Self::new(true)
    }

    /// A theme whose roles all resolve to an empty style (tests and snapshots).
    #[cfg(test)]
    pub const fn plain() -> Self {
        Self::new(false)
    }

    /// Resolve a [`Role`] to a style.
    pub fn style(self, role: Role) -> Style {
        if !self.enabled {
            return Style::new();
        }
        match role {
            Role::Plain => Style::new(),
            Role::Heading => AnsiColor::Cyan.on_default().effects(Effects::BOLD),
            Role::Key => Style::new().bold(),
            Role::Muted => Style::new().dimmed(),
            Role::Accent => AnsiColor::Cyan.on_default(),
            Role::AccentAlt => AnsiColor::Magenta.on_default(),
            Role::Success => AnsiColor::Green.on_default(),
            Role::Warning => AnsiColor::Yellow.on_default(),
            Role::Error => AnsiColor::Red.on_default().bold(),
        }
    }
}
