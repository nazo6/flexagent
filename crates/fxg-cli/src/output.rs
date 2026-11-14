//! Structured CLI output helpers (ANSI styling, key-value lists, startup banners).

use nu_ansi_term::Style;

/// Conditionally apply an ANSI style to `text`.
///
/// Returns `text` unmodified when `use_color` is false.
pub fn paint(use_color: bool, style: Style, text: &str) -> String {
    if use_color {
        style.paint(text).to_string()
    } else {
        text.to_owned()
    }
}

/// A single entry in a [`KvList`].
#[derive(Debug, Clone)]
pub struct KvRow {
    label: Option<String>,
    value: String,
    value_style: Option<Style>,
}

impl KvRow {
    /// A labeled key-value row.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            value: value.into(),
            value_style: None,
        }
    }

    /// A labeled row whose value uses `style` when color output is enabled.
    pub fn styled(label: impl Into<String>, value: impl Into<String>, style: Style) -> Self {
        Self {
            label: Some(label.into()),
            value: value.into(),
            value_style: Some(style),
        }
    }

    /// A continuation line aligned to the value column of the preceding rows.
    pub fn continuation(value: impl Into<String>) -> Self {
        Self {
            label: None,
            value: value.into(),
            value_style: None,
        }
    }
}

/// Aligned key-value list renderer.
///
/// Computes the value column offset from the unstyled label widths so colored
/// and plain output share a single layout path and cannot drift out of sync.
#[derive(Debug, Clone)]
pub struct KvList {
    indent: usize,
    gap: usize,
    label_style: Option<Style>,
    rows: Vec<KvRow>,
}

impl Default for KvList {
    fn default() -> Self {
        Self {
            indent: 0,
            gap: 2,
            label_style: None,
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

    /// Apply `style` to every label when color output is enabled.
    pub fn label_style(mut self, style: Style) -> Self {
        self.label_style = Some(style);
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

    /// Append a key-value row with a styled value.
    pub fn styled_field(
        self,
        label: impl Into<String>,
        value: impl Into<String>,
        style: Style,
    ) -> Self {
        self.push(KvRow::styled(label, value, style))
    }

    /// Render the key-value lines (without a trailing newline).
    pub fn render(&self, use_color: bool) -> String {
        let max_label = self
            .rows
            .iter()
            .filter_map(|row| row.label.as_deref())
            .map(|label| label.chars().count())
            .max()
            .unwrap_or(0);
        let value_col = max_label + self.gap;
        let indent_pad = " ".repeat(self.indent);

        let mut out = String::new();
        for (idx, row) in self.rows.iter().enumerate() {
            if idx > 0 {
                out.push('\n');
            }
            let value = match row.value_style {
                Some(style) => paint(use_color, style, &row.value),
                None => row.value.clone(),
            };
            match row.label.as_deref() {
                Some(label) => {
                    let padding = value_col.saturating_sub(label.chars().count());
                    let styled_label = match self.label_style {
                        Some(style) => paint(use_color, style, label),
                        None => label.to_owned(),
                    };
                    out.push_str(&format!(
                        "{indent_pad}{styled_label}{}{value}",
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
    title_style: Style,
    title_suffix: String,
    fields: KvList,
    footer: Option<String>,
}

impl Banner {
    /// Construct a banner from a highlighted title prefix, suffix, and aligned fields.
    pub fn new(
        title_highlight: impl Into<String>,
        title_style: Style,
        title_suffix: impl Into<String>,
        fields: KvList,
    ) -> Self {
        Self {
            title_highlight: title_highlight.into(),
            title_style,
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
    pub fn render(&self, use_color: bool) -> String {
        let mut out = String::new();
        let highlight = paint(use_color, self.title_style, &self.title_highlight);
        out.push_str(&format!("{highlight}{}\n", self.title_suffix));
        let body = self.fields.render(use_color);
        if !body.is_empty() {
            out.push_str(&body);
            out.push('\n');
        }
        if let Some(footer) = self.footer.as_deref() {
            let styled_footer = paint(use_color, Style::new().dimmed(), footer);
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
    use nu_ansi_term::Color;

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
            .label_style(Style::new().bold())
            .field("HTTP/WS API:", "http://127.0.0.1:7860")
            .styled_field("Auth Token:", "secret", Color::Yellow.normal())
            .field("Hub:", "standalone (local only)")
            .push(KvRow::continuation("extra line"));

        let plain = list.render(false);
        let colored = list.render(true);

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
}
