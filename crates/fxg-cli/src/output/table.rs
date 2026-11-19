//! Table rendering shared by every list command.
//!
//! `comfy-table` owns the layout (Unicode widths, wrapping to the terminal
//! width) so call sites only supply cells and their [`Role`]s.

use comfy_table::{Cell as ComfyCell, ContentArrangement, Table as ComfyTable, presets};

use super::{Role, Theme, paint};

/// One table cell together with the role its text is painted in.
#[derive(Debug, Clone)]
pub struct Cell {
    text: String,
    role: Role,
}

impl Cell {
    /// A plain cell.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: Role::Plain,
        }
    }

    /// A cell painted with `role`.
    pub fn role(text: impl Into<String>, role: Role) -> Self {
        Self {
            text: text.into(),
            role,
        }
    }
}

impl From<&str> for Cell {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for Cell {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

/// A themed table with a styled header row.
#[derive(Debug)]
pub struct Table {
    inner: ComfyTable,
    theme: Theme,
}

impl Table {
    /// Create a table with `headers` and no rows yet.
    pub fn new(theme: Theme, headers: &[&str]) -> Self {
        let mut inner = ComfyTable::new();
        inner
            .load_style(presets::UTF8_HORIZONTAL_ONLY)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header(
                headers
                    .iter()
                    .map(|header| ComfyCell::new(paint(theme.style(Role::Key), header)))
                    .collect::<Vec<_>>(),
            );
        Self { inner, theme }
    }

    /// Append a row.
    pub fn push_row(mut self, cells: impl IntoIterator<Item = Cell>) -> Self {
        self.inner.add_row(
            cells
                .into_iter()
                .map(|cell| ComfyCell::new(paint(self.theme.style(cell.role), &cell.text)))
                .collect::<Vec<_>>(),
        );
        self
    }

    /// Render the table (without a trailing newline).
    ///
    /// Cells are padded on the right for alignment, so trailing spaces are
    /// trimmed to keep copied lines clean.
    pub fn render(&self) -> String {
        self.inner
            .to_string()
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }
}
