//! Terminal screen grid and scrollback.

use crate::cell::{CellStyle, TerminalCell};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalGrid {
    pub columns: u16,
    pub rows: u16,
    pub cells: Vec<TerminalCell>,
    pub cursor_column: u16,
    pub cursor_row: u16,
    scrollback: Vec<Vec<TerminalCell>>,
    scrollback_limit: usize,
}

impl TerminalGrid {
    #[must_use]
    pub fn new(columns: u16, rows: u16) -> Self {
        Self::with_scrollback_limit(columns, rows, 1_000)
    }

    #[must_use]
    pub fn with_scrollback_limit(columns: u16, rows: u16, scrollback_limit: usize) -> Self {
        let len = usize::from(columns) * usize::from(rows);
        Self {
            columns,
            rows,
            cells: vec![TerminalCell::default(); len],
            cursor_column: 0,
            cursor_row: 0,
            scrollback: Vec::new(),
            scrollback_limit,
        }
    }

    #[must_use]
    pub fn cell(&self, column: u16, row: u16) -> Option<&TerminalCell> {
        self.index(column, row)
            .and_then(|index| self.cells.get(index))
    }

    pub fn put_cell(&mut self, column: u16, row: u16, cell: TerminalCell) {
        if let Some(index) = self.index(column, row) {
            self.cells[index] = cell;
        }
    }

    pub fn clear_screen(&mut self, style: &CellStyle) {
        self.cells.fill(TerminalCell::blank_with_style(style));
        self.cursor_column = 0;
        self.cursor_row = 0;
    }

    pub fn newline(&mut self, style: &CellStyle) {
        self.cursor_column = 0;
        if self.cursor_row + 1 >= self.rows {
            self.scroll_up(style);
        } else {
            self.cursor_row += 1;
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor_column = 0;
    }

    pub fn backspace(&mut self) {
        self.cursor_column = self.cursor_column.saturating_sub(1);
    }

    pub fn write_grapheme(&mut self, grapheme: String, width: usize, style: &CellStyle) {
        let width = width.clamp(1, 2);
        if self.cursor_column >= self.columns
            || usize::from(self.cursor_column) + width > usize::from(self.columns)
        {
            self.newline(style);
        }

        let mut cell = TerminalCell::blank_with_style(style);
        cell.grapheme = grapheme;
        self.put_cell(self.cursor_column, self.cursor_row, cell);

        if width == 2 && self.cursor_column + 1 < self.columns {
            let mut continuation = TerminalCell::blank_with_style(style);
            continuation.wide_continuation = true;
            self.put_cell(self.cursor_column + 1, self.cursor_row, continuation);
        }

        self.cursor_column =
            (self.cursor_column + u16::try_from(width).unwrap_or(1)).min(self.columns);
    }

    #[must_use]
    pub fn scrollback_rows(&self) -> &[Vec<TerminalCell>] {
        &self.scrollback
    }

    #[must_use]
    pub fn line_text(&self, row: u16) -> String {
        (0..self.columns)
            .filter_map(|column| self.cell(column, row))
            .filter(|cell| !cell.wide_continuation)
            .map(|cell| cell.grapheme.as_str())
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[must_use]
    pub fn visible_lines(&self) -> Vec<String> {
        (0..self.rows).map(|row| self.line_text(row)).collect()
    }

    fn scroll_up(&mut self, style: &CellStyle) {
        if self.rows == 0 || self.columns == 0 {
            return;
        }
        let columns = usize::from(self.columns);
        let first_line = self.cells.drain(0..columns).collect::<Vec<_>>();
        self.scrollback.push(first_line);
        if self.scrollback.len() > self.scrollback_limit {
            let overflow = self.scrollback.len() - self.scrollback_limit;
            self.scrollback.drain(0..overflow);
        }
        self.cells
            .extend(vec![TerminalCell::blank_with_style(style); columns]);
    }

    fn index(&self, column: u16, row: u16) -> Option<usize> {
        if column < self.columns && row < self.rows {
            Some(usize::from(row) * usize::from(self.columns) + usize::from(column))
        } else {
            None
        }
    }
}
