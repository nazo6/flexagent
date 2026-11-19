//! Shared presentation layer for the `fxg` CLI.
//!
//! Commands describe *what* a line is — a key/value pair, a table, a banner —
//! and *which [`Role`]* it plays; this module decides how that looks. Styles
//! come from [`Theme`], so a command never names an ANSI color and plain output
//! is produced by rendering with [`Theme::plain`] rather than by stripping
//! escapes.

pub mod table;
pub mod theme;

pub use table::{Cell, Table};
pub use theme::{Role, Theme};

use anstyle::Style;

/// Apply `style` to `text`.
///
/// A plain theme resolves every role to an empty [`Style`], so this returns
/// `text` untouched and no escape ever reaches a pipe or a test.
pub fn paint(style: Style, text: &str) -> String {
    format!("{}{text}{}", style.render(), style.render_reset())
}

/// Record the process-wide color decision (`--no-color`, `NO_COLOR`).
pub fn init(no_color: bool) {
    theme::init(no_color);
}

/// The theme for the current process.
pub fn theme() -> Theme {
    theme::current()
}

/// Write a rendered block to stdout.
///
/// `anstream` strips escapes when stdout is not a terminal and degrades colors
/// the terminal cannot show, so callers never decide about TTYs themselves.
pub fn print_block(text: &str) {
    anstream::println!("{text}");
}

/// Write a rendered block to stderr (banners, progress, diagnostics).
pub fn eprint_block(text: &str) {
    anstream::eprintln!("{text}");
}

/// How a command reports its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Human-readable text on stdout.
    #[default]
    Human,
    /// Machine-readable JSON on stdout.
    Json,
}

impl OutputFormat {
    /// Derive the format from a `--json` flag.
    pub const fn from_json(json: bool) -> Self {
        if json { Self::Json } else { Self::Human }
    }

    /// Whether JSON output was requested.
    pub const fn is_json(self) -> bool {
        matches!(self, Self::Json)
    }
}

/// A single entry in a [`KvList`].
#[derive(Debug, Clone)]
pub struct KvRow {
    label: Option<String>,
    value: String,
    role: Role,
}

impl KvRow {
    /// A labeled key-value row.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            value: value.into(),
            role: Role::Plain,
        }
    }

    /// A labeled row whose value plays `role`.
    pub fn role(label: impl Into<String>, value: impl Into<String>, role: Role) -> Self {
        Self {
            label: Some(label.into()),
            value: value.into(),
            role,
        }
    }

    /// A continuation line aligned to the value column of the preceding rows.
    pub fn continuation(value: impl Into<String>) -> Self {
        Self {
            label: None,
            value: value.into(),
            role: Role::Plain,
        }
    }
}

/// Aligned key-value list renderer.
///
/// The value column is computed from the unstyled label widths, so colored and
/// plain output share a single layout path and cannot drift out of sync.
#[derive(Debug, Clone)]
pub struct KvList {
    indent: usize,
    gap: usize,
    label_role: Role,
    rows: Vec<KvRow>,
}

impl Default for KvList {
    fn default() -> Self {
        Self {
            indent: 0,
            gap: 2,
            label_role: Role::Key,
            rows: Vec::new(),
        }
    }
}

impl KvList {
    /// Create an empty key-value list with default indentation (`0`) and gap (`2`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the leading space count for every line.
    pub fn indent(mut self, indent: usize) -> Self {
        self.indent = indent;
        self
    }

    /// Set the minimum space count between the longest label and the value column.
    pub fn gap(mut self, gap: usize) -> Self {
        self.gap = gap;
        self
    }

    /// Append a [`KvRow`].
    pub fn push(mut self, row: KvRow) -> Self {
        self.rows.push(row);
        self
    }

    /// Append a plain key-value row.
    pub fn field(self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.push(KvRow::new(label, value))
    }

    /// Append a key-value row whose value plays `role`.
    pub fn field_with(
        self,
        label: impl Into<String>,
        value: impl Into<String>,
        role: Role,
    ) -> Self {
        self.push(KvRow::role(label, value, role))
    }

    /// Render the key-value lines (without a trailing newline).
    pub fn render(&self, theme: Theme) -> String {
        let max_label = self
            .rows
            .iter()
            .filter_map(|row| row.label.as_deref())
            .map(|label| label.chars().count())
            .max()
            .unwrap_or(0);
        let value_col = max_label + self.gap;
        let indent_pad = " ".repeat(self.indent);
        let label_style = theme.style(self.label_role);

        let mut out = String::new();
        for (idx, row) in self.rows.iter().enumerate() {
            if idx > 0 {
                out.push('\n');
            }
            let value = paint(theme.style(row.role), &row.value);
            match row.label.as_deref() {
                Some(label) => {
                    let padding = value_col.saturating_sub(label.chars().count());
                    out.push_str(&format!(
                        "{indent_pad}{}{}{value}",
                        paint(label_style, label),
                        " ".repeat(padding)
                    ));
                }
                None => {
                    out.push_str(&format!("{indent_pad}{}{value}", " ".repeat(value_col)));
                }
            }
        }
        out
    }
}

/// Startup banner with a styled title, aligned fields, and an optional footer.
#[derive(Debug, Clone)]
pub struct Banner {
    title_highlight: String,
    title_role: Role,
    title_suffix: String,
    fields: KvList,
    footer: Option<String>,
}

impl Banner {
    /// Construct a banner from a highlighted title prefix, its role, a suffix,
    /// and aligned fields.
    pub fn new(
        title_highlight: impl Into<String>,
        title_role: Role,
        title_suffix: impl Into<String>,
        fields: KvList,
    ) -> Self {
        Self {
            title_highlight: title_highlight.into(),
            title_role,
            title_suffix: title_suffix.into(),
            fields,
            footer: None,
        }
    }

    /// Set an optional dimmed footer line aligned to `fields.indent`.
    pub fn footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(footer.into());
        self
    }

    /// Render the full banner (ending with a newline after the last line).
    pub fn render(&self, theme: Theme) -> String {
        let mut out = String::new();
        let highlight = paint(theme.style(self.title_role), &self.title_highlight);
        out.push_str(&format!("{highlight}{}\n", self.title_suffix));
        let body = self.fields.render(theme);
        if !body.is_empty() {
            out.push_str(&body);
            out.push('\n');
        }
        if let Some(footer) = self.footer.as_deref() {
            let styled_footer = paint(theme.style(Role::Muted), footer);
            out.push_str(&format!(
                "{}{styled_footer}\n",
                " ".repeat(self.fields.indent)
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' && chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn kv_list_aligns_values_consistently_with_and_without_color() {
        let list = KvList::new()
            .indent(2)
            .field("HTTP/WS API:", "http://127.0.0.1:7860")
            .field_with("Auth Token:", "secret", Role::Warning)
            .field("Hub:", "standalone (local only)")
            .push(KvRow::continuation("extra line"));

        let plain = list.render(Theme::plain());
        let colored = list.render(Theme::styled());

        assert!(!plain.contains("\x1b["));
        assert!(colored.contains("\x1b["));
        assert_eq!(strip_ansi(&colored), plain);

        let lines: Vec<&str> = plain.lines().collect();
        assert_eq!(
            lines,
            vec![
                "  HTTP/WS API:  http://127.0.0.1:7860",
                "  Auth Token:   secret",
                "  Hub:          standalone (local only)",
                "                extra line",
            ]
        );
    }

    #[test]
    fn banner_styles_only_the_heading_and_footer() {
        let banner = Banner::new(
            "FlexAgent node",
            Role::Accent,
            " started",
            KvList::new().indent(2).field("Hub:", "standalone"),
        )
        .footer("Press Ctrl+C to stop.");

        let plain = banner.render(Theme::plain());
        let colored = banner.render(Theme::styled());
        assert!(!plain.contains("\x1b["));
        assert!(colored.contains("\x1b["));
        assert_eq!(strip_ansi(&colored), plain);
        assert_eq!(
            plain,
            "FlexAgent node started\n  Hub:  standalone\n  Press Ctrl+C to stop.\n"
        );
    }

    #[test]
    fn table_aligns_cells_and_keeps_color_out_of_width() {
        let plain = Table::new(Theme::plain(), &["ID", "TITLE"])
            .push_row([Cell::new("a1"), Cell::from("first")])
            .push_row([Cell::new("b2"), Cell::role("second", Role::Success)])
            .render();
        let colored = Table::new(Theme::styled(), &["ID", "TITLE"])
            .push_row([Cell::new("a1"), Cell::from("first")])
            .push_row([Cell::new("b2"), Cell::role("second", Role::Success)])
            .render();

        assert!(!plain.contains("\x1b["));
        assert!(colored.contains("\x1b["));
        // Painted cells must not widen the column: stripping color restores the
        // exact same layout.
        assert_eq!(strip_ansi(&colored), plain);
        assert!(plain.contains("first"));
        assert!(plain.contains("second"));
        // Padding must never leave trailing whitespace on a copied line.
        assert!(plain.lines().all(|line| line == line.trim_end()));
    }
}
