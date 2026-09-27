//! Terminal screen grid and scrollback.

use std::collections::VecDeque;

use unicode_width::UnicodeWidthChar;

use crate::cell::{CellStyle, TerminalCell};

/// Default scrollback line limit used by [`TerminalGrid::new`].
pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
/// Default scrollback cell limit used by [`TerminalGrid::new`].
pub const DEFAULT_SCROLLBACK_MAX_CELLS: usize = 2_000_000;

/// A run of columns that share one style inside a [`ScrollbackLine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StyleSpan {
    start: u16,
    len: u16,
    style: CellStyle,
}

/// One scrollback line in compressed form.
///
/// Glyphs are stored in column order inside `text`. `glyph_lens` holds each
/// glyph's UTF-8 byte length and `glyph_widths` the columns it consumes:
/// `0` marks an orphan wide continuation cell, `1` a normal cell and `2` a
/// wide glyph whose second column is re-created as a continuation cell.
/// Trailing blank cells never enter `text`; their styles stay in `spans` so
/// background colors survive the round trip.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScrollbackLine {
    columns: u16,
    text: String,
    glyph_lens: Vec<u8>,
    glyph_widths: Vec<u8>,
    spans: Vec<StyleSpan>,
}

impl ScrollbackLine {
    fn compress(cells: &[TerminalCell]) -> Self {
        let mut line = Self {
            columns: u16::try_from(cells.len()).unwrap_or(u16::MAX),
            text: String::new(),
            glyph_lens: Vec::new(),
            glyph_widths: Vec::new(),
            spans: compress_spans(cells),
        };
        // Blank cells at the end never enter `text`; only their styles (kept
        // by `spans`) matter, so stop before they grow the arrays.
        let mut end = cells.len();
        while end > 0 {
            let cell = &cells[end - 1];
            if cell.wide_continuation || cell.grapheme != " " {
                break;
            }
            end -= 1;
        }
        let mut column = 0;
        while column < end {
            let cell = &cells[column];
            let (grapheme, width) = if cell.wide_continuation {
                // Orphan continuation: keep it so the round trip stays lossless.
                (cell.grapheme.as_str(), 0u8)
            } else if glyph_is_wide(cell)
                && cells
                    .get(column + 1)
                    .is_some_and(|next| next.wide_continuation)
            {
                (cell.grapheme.as_str(), 2u8)
            } else {
                (cell.grapheme.as_str(), 1u8)
            };
            line.push_glyph(grapheme, width);
            column += if width == 2 { 2 } else { 1 };
        }
        line
    }

    fn materialize(&self) -> Vec<TerminalCell> {
        let columns = usize::from(self.columns);
        let mut cells = vec![TerminalCell::default(); columns];
        for span in &self.spans {
            let start = usize::from(span.start).min(columns);
            let end = start.saturating_add(usize::from(span.len)).min(columns);
            let blank = TerminalCell::blank_with_style(&span.style);
            for cell in &mut cells[start..end] {
                *cell = blank.clone();
            }
        }

        let mut column = 0usize;
        let mut offset = 0usize;
        for (index, glyph_len) in self.glyph_lens.iter().enumerate() {
            let Some(&width) = self.glyph_widths.get(index) else {
                break;
            };
            let byte_len = usize::from(*glyph_len);
            let Some(grapheme) = self.text.get(offset..offset + byte_len) else {
                break;
            };
            offset += byte_len;
            if column >= cells.len() {
                break;
            }
            match width {
                0 => {
                    let mut cell = cells[column].clone();
                    cell.grapheme = grapheme.to_owned();
                    cell.wide_continuation = true;
                    cells[column] = cell;
                    column += 1;
                }
                2 => {
                    let mut cell = cells[column].clone();
                    cell.grapheme = grapheme.to_owned();
                    cells[column] = cell;
                    if column + 1 < cells.len() {
                        let mut continuation = cells[column + 1].clone();
                        continuation.grapheme = " ".to_owned();
                        continuation.wide_continuation = true;
                        cells[column + 1] = continuation;
                    }
                    column += 2;
                }
                _ => {
                    let mut cell = cells[column].clone();
                    cell.grapheme = grapheme.to_owned();
                    cells[column] = cell;
                    column += 1;
                }
            }
        }
        cells
    }

    fn push_glyph(&mut self, grapheme: &str, width: u8) {
        self.text.push_str(grapheme);
        self.glyph_lens
            .push(u8::try_from(grapheme.len()).unwrap_or(u8::MAX));
        self.glyph_widths.push(width);
    }

    fn heap_bytes(&self) -> usize {
        self.text.capacity()
            + self.glyph_lens.capacity()
            + self.glyph_widths.capacity()
            + self.spans.capacity() * std::mem::size_of::<StyleSpan>()
    }
}

fn compress_spans(cells: &[TerminalCell]) -> Vec<StyleSpan> {
    let default = CellStyle::default();
    let mut spans: Vec<StyleSpan> = Vec::new();
    let mut column = 0usize;
    while column < cells.len() {
        let style = cell_style(&cells[column]);
        let start = column;
        while column < cells.len() && cell_style(&cells[column]) == style {
            column += 1;
        }
        if style != default {
            spans.push(StyleSpan {
                start: u16::try_from(start).unwrap_or(u16::MAX),
                len: u16::try_from(column - start).unwrap_or(u16::MAX),
                style,
            });
        }
    }
    spans
}

fn cell_style(cell: &TerminalCell) -> CellStyle {
    CellStyle {
        foreground: cell.foreground,
        background: cell.background,
        bold: cell.bold,
        italic: cell.italic,
        underline: cell.underline,
    }
}

fn glyph_is_wide(cell: &TerminalCell) -> bool {
    cell.grapheme
        .chars()
        .next()
        .map(|character| UnicodeWidthChar::width(character).unwrap_or(1) >= 2)
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalGrid {
    pub columns: u16,
    pub rows: u16,
    pub cells: Vec<TerminalCell>,
    pub cursor_column: u16,
    pub cursor_row: u16,
    scrollback: VecDeque<ScrollbackLine>,
    scrollback_cells: usize,
    scrollback_line_limit: usize,
    scrollback_cell_limit: usize,
}

impl TerminalGrid {
    #[must_use]
    pub fn new(columns: u16, rows: u16) -> Self {
        Self::with_limits(
            columns,
            rows,
            DEFAULT_SCROLLBACK_LINES,
            DEFAULT_SCROLLBACK_MAX_CELLS,
        )
    }

    /// Compatibility constructor: only the line limit is capped.
    #[must_use]
    pub fn with_scrollback_limit(columns: u16, rows: u16, scrollback_limit: usize) -> Self {
        Self::with_limits(columns, rows, scrollback_limit, usize::MAX)
    }

    /// Build a grid with both a line and a total-cell scrollback cap.
    #[must_use]
    pub fn with_limits(columns: u16, rows: u16, line_limit: usize, cell_limit: usize) -> Self {
        let len = usize::from(columns) * usize::from(rows);
        Self {
            columns,
            rows,
            cells: vec![TerminalCell::default(); len],
            cursor_column: 0,
            cursor_row: 0,
            scrollback: VecDeque::new(),
            scrollback_cells: 0,
            scrollback_line_limit: line_limit,
            scrollback_cell_limit: cell_limit,
        }
    }

    /// Replace both scrollback caps and evict overflow immediately.
    pub fn set_scrollback_limits(&mut self, line_limit: usize, cell_limit: usize) {
        self.scrollback_line_limit = line_limit;
        self.scrollback_cell_limit = cell_limit;
        self.evict_scrollback();
    }

    /// Current `(line_limit, cell_limit)` scrollback caps.
    #[must_use]
    pub const fn scrollback_limits(&self) -> (usize, usize) {
        (self.scrollback_line_limit, self.scrollback_cell_limit)
    }

    #[must_use]
    pub fn cell(&self, column: u16, row: u16) -> Option<&TerminalCell> {
        self.index(column, row)
            .and_then(|index| self.cells.get(index))
    }

    pub fn put_cell(&mut self, column: u16, row: u16, cell: TerminalCell) {
        debug_assert_eq!(
            self.cells.len(),
            usize::from(self.rows) * usize::from(self.columns),
            "grid cells must stay exactly rows * columns"
        );
        if let Some(index) = self.index(column, row) {
            self.cells[index] = cell;
        }
    }

    pub fn clear_screen(&mut self, style: &CellStyle) {
        self.cells.fill(TerminalCell::blank_with_style(style));
        self.cursor_column = 0;
        self.cursor_row = 0;
        self.debug_assert_shape();
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

    /// Number of lines currently held in the scrollback buffer.
    #[must_use]
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Total addressable lines: scrollback plus the visible screen.
    #[must_use]
    pub fn total_lines(&self) -> usize {
        self.scrollback.len() + usize::from(self.rows)
    }

    /// Read a line by absolute index (`0` = oldest scrollback line).
    ///
    /// Scrollback lines are expanded from their compressed form, so the
    /// returned slice is owned.
    #[must_use]
    pub fn line_cells(&self, absolute_line: usize) -> Option<Vec<TerminalCell>> {
        if absolute_line < self.scrollback.len() {
            return self
                .scrollback
                .get(absolute_line)
                .map(ScrollbackLine::materialize);
        }
        let row = u16::try_from(absolute_line - self.scrollback.len()).ok()?;
        self.visible_cells(row).map(<[TerminalCell]>::to_vec)
    }

    /// Zero-copy access to a visible screen row.
    #[must_use]
    pub fn visible_cells(&self, row: u16) -> Option<&[TerminalCell]> {
        if row >= self.rows || self.columns == 0 {
            return None;
        }
        let columns = usize::from(self.columns);
        let start = usize::from(row) * columns;
        self.cells.get(start..start + columns)
    }

    /// Text of a line by absolute index, trimmed like [`Self::line_text`].
    #[must_use]
    pub fn line_text_absolute(&self, absolute_line: usize) -> Option<String> {
        if let Some(line) = self.scrollback.get(absolute_line) {
            return Some(line.text.trim_end().to_owned());
        }
        let row = absolute_line - self.scrollback.len();
        if row >= usize::from(self.rows) || self.columns == 0 {
            return None;
        }
        Some(self.line_text(u16::try_from(row).unwrap_or(u16::MAX)))
    }

    /// Estimated heap footprint of the compressed scrollback buffer.
    ///
    /// Counts the `VecDeque` slots plus each line's allocated text, glyph
    /// metadata and style spans.
    #[must_use]
    pub fn scrollback_bytes_estimate(&self) -> usize {
        self.scrollback.len() * std::mem::size_of::<ScrollbackLine>()
            + self
                .scrollback
                .iter()
                .map(ScrollbackLine::heap_bytes)
                .sum::<usize>()
    }

    /// Resize the screen while keeping the visible content and scrollback.
    ///
    /// Visible lines reflow to the new column count. Scrollback lines keep
    /// their captured width. Growing pulls lines back from the scrollback tail
    /// to fill the taller screen; shrinking pushes rows above the cursor into
    /// the scrollback (compressed) and drops remaining rows from the bottom.
    pub fn resize(&mut self, columns: u16, rows: u16) {
        if columns == self.columns && rows == self.rows {
            return;
        }

        let old_rows = usize::from(self.rows);
        let new_rows = usize::from(rows);
        let new_columns = usize::from(columns);
        let cursor_row = usize::from(self.cursor_row).min(old_rows.saturating_sub(1));
        let mut cursor_row_after = cursor_row;

        let mut visible = self.visible_line_buffers();
        visible.resize_with(old_rows, Vec::new);

        if new_rows >= old_rows {
            let growth = new_rows - old_rows;
            let pulled = growth.min(self.scrollback.len());
            if pulled > 0 {
                let mut restored = Vec::with_capacity(pulled + visible.len());
                for _ in 0..pulled {
                    let Some(line) = self.scrollback.pop_back() else {
                        break;
                    };
                    self.scrollback_cells = self
                        .scrollback_cells
                        .saturating_sub(usize::from(line.columns));
                    restored.push(line.materialize());
                }
                // `pop_back` yields the newest line first; the screen is ordered
                // oldest-first, so the pulled block has to be flipped back.
                restored.reverse();
                restored.extend(visible);
                visible = restored;
            }
            cursor_row_after = cursor_row + pulled;
        } else {
            let remove = old_rows - new_rows;
            let remove_top = remove.min(cursor_row);
            for _ in 0..remove_top {
                if visible.is_empty() {
                    break;
                }
                let line = visible.remove(0);
                self.push_scrollback_line(line);
                cursor_row_after = cursor_row_after.saturating_sub(1);
            }
            visible.truncate(new_rows);
        }

        // Pad/truncate the screen to exactly `new_rows` *before* flattening, and
        // give every padded row a full-width blank buffer. Padding with empty
        // buffers used to leave `cells.len() == old_rows * new_columns` while
        // `rows` already reported the new height, so the first write into one of
        // the new rows panicked with `index out of bounds` (see
        // `resize_growth_with_empty_scrollback_keeps_shape_invariant`).
        visible.resize_with(new_rows, || vec![TerminalCell::default(); new_columns]);
        for line in &mut visible {
            line.resize(new_columns, TerminalCell::default());
            line.truncate(new_columns);
        }

        self.columns = columns;
        self.rows = rows;
        self.cells = visible.into_iter().flatten().collect();
        self.cursor_row = u16::try_from(cursor_row_after)
            .unwrap_or(u16::MAX)
            .min(rows.saturating_sub(1));
        self.cursor_column = self.cursor_column.min(columns.saturating_sub(1));
        self.debug_assert_shape();
    }

    /// Debug-only guard for the structural invariant: the cell buffer must hold
    /// exactly `rows * columns` cells, so any row in `0..rows` is a full buffer.
    fn debug_assert_shape(&self) {
        debug_assert_eq!(
            self.cells.len(),
            usize::from(self.rows) * usize::from(self.columns),
            "grid cells must stay exactly rows * columns"
        );
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
        self.push_scrollback_line(first_line);
        self.cells
            .extend(vec![TerminalCell::blank_with_style(style); columns]);
        self.debug_assert_shape();
    }

    fn visible_line_buffers(&self) -> Vec<Vec<TerminalCell>> {
        if self.columns == 0 {
            return Vec::new();
        }
        self.cells
            .chunks(usize::from(self.columns))
            .map(<[TerminalCell]>::to_vec)
            .collect()
    }

    fn push_scrollback_line(&mut self, cells: Vec<TerminalCell>) {
        if cells.is_empty() {
            return;
        }
        let line = ScrollbackLine::compress(&cells);
        self.scrollback_cells = self
            .scrollback_cells
            .saturating_add(usize::from(line.columns));
        self.scrollback.push_back(line);
        self.evict_scrollback();
    }

    fn evict_scrollback(&mut self) {
        while self.scrollback.len() > self.scrollback_line_limit
            || self.scrollback_cells > self.scrollback_cell_limit
        {
            let Some(line) = self.scrollback.pop_front() else {
                break;
            };
            self.scrollback_cells = self
                .scrollback_cells
                .saturating_sub(usize::from(line.columns));
        }
    }

    fn index(&self, column: u16, row: u16) -> Option<usize> {
        if column < self.columns && row < self.rows {
            Some(usize::from(row) * usize::from(self.columns) + usize::from(column))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{color::TerminalColor, parser::TerminalParser};

    fn put_line(grid: &mut TerminalGrid, row: u16, text: &str) {
        for (column, character) in text.chars().enumerate() {
            let mut cell = TerminalCell::blank_with_style(&CellStyle::default());
            cell.grapheme = character.to_string();
            grid.put_cell(column as u16, row, cell);
        }
    }

    fn cells_text(cells: &[TerminalCell]) -> String {
        cells
            .iter()
            .filter(|cell| !cell.wide_continuation)
            .map(|cell| cell.grapheme.as_str())
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    fn styled_cell(grapheme: &str, style: &CellStyle) -> TerminalCell {
        let mut cell = TerminalCell::blank_with_style(style);
        cell.grapheme = grapheme.to_owned();
        cell
    }

    #[test]
    fn absolute_line_helpers_cover_scrollback_and_screen() {
        let mut grid = TerminalGrid::with_scrollback_limit(4, 2, 8);
        put_line(&mut grid, 0, "one");
        grid.newline(&CellStyle::default());
        put_line(&mut grid, 1, "two");
        grid.newline(&CellStyle::default());
        put_line(&mut grid, 1, "thr");

        assert_eq!(grid.scrollback_len(), 1);
        assert_eq!(grid.total_lines(), 3);
        assert_eq!(
            grid.line_cells(0).map(|cells| cells_text(&cells)),
            Some("one".to_owned())
        );
        assert_eq!(
            grid.line_cells(1).map(|cells| cells_text(&cells)),
            Some("two".to_owned())
        );
        assert_eq!(
            grid.line_cells(2).map(|cells| cells_text(&cells)),
            Some("thr".to_owned())
        );
        assert!(grid.line_cells(3).is_none());
    }

    #[test]
    fn parser_written_scrollback_stays_addressable_by_absolute_line() {
        let mut grid = TerminalGrid::with_scrollback_limit(10, 2, 8);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"one\ntwo\nthree\nfour");

        let lines = (0..grid.total_lines())
            .map(|line| {
                grid.line_cells(line)
                    .map(|cells| cells_text(&cells))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        assert_eq!(lines, vec!["one", "two", "three", "four"]);
    }

    #[test]
    fn line_text_absolute_matches_scrollback_and_screen() {
        let mut grid = TerminalGrid::with_scrollback_limit(8, 2, 8);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"one\ntwo\nthree\nfour");

        assert_eq!(grid.line_text_absolute(0).as_deref(), Some("one"));
        assert_eq!(grid.line_text_absolute(1).as_deref(), Some("two"));
        assert_eq!(grid.line_text_absolute(2).as_deref(), Some("three"));
        assert_eq!(grid.line_text_absolute(3).as_deref(), Some("four"));
        assert_eq!(grid.line_text_absolute(4), None);
    }

    #[test]
    fn compressed_lines_round_trip_losslessly() {
        let styled_background = CellStyle {
            background: TerminalColor::BLUE,
            ..CellStyle::default()
        };
        let bold = CellStyle {
            bold: true,
            foreground: TerminalColor::RED,
            ..CellStyle::default()
        };
        let underlined = CellStyle {
            underline: true,
            foreground: TerminalColor::GREEN,
            ..CellStyle::default()
        };

        let mut cells = vec![TerminalCell::default(); 12];
        cells[0] = styled_cell("H", &CellStyle::default());
        cells[1] = styled_cell("i", &bold);
        cells[2] = styled_cell(" ", &CellStyle::default());
        cells[3] = styled_cell("\u{597d}", &underlined);
        cells[4] = styled_cell(" ", &underlined);
        cells[4].wide_continuation = true;
        for cell in &mut cells[5..] {
            *cell = styled_cell(" ", &styled_background);
        }

        let line = ScrollbackLine::compress(&cells);

        assert_eq!(line.columns, 12);
        // Trailing blanks are represented by spans only.
        assert_eq!(line.text, "Hi \u{597d}");
        assert_eq!(line.glyph_lens.len(), 4);
        assert_eq!(line.glyph_widths, vec![1, 1, 1, 2]);
        assert_eq!(line.materialize(), cells);
        assert!(line.heap_bytes() > 0);
    }

    #[test]
    fn compressed_lines_round_trip_orphan_continuations() {
        let mut cells = vec![TerminalCell::default(); 4];
        let mut wide = styled_cell("\u{597d}", &CellStyle::default());
        wide.foreground = TerminalColor::MAGENTA;
        cells[0] = wide;
        cells[1] = styled_cell(" ", &CellStyle::default());
        cells[1].wide_continuation = true;
        cells[2] = styled_cell("x", &CellStyle::default());
        // Overwrite the wide glyph but leave its continuation cell orphaned.
        cells[0] = styled_cell("a", &CellStyle::default());
        cells[1].foreground = TerminalColor::CYAN;

        let line = ScrollbackLine::compress(&cells);

        assert_eq!(line.text, "a x");
        assert_eq!(line.glyph_widths, vec![1, 0, 1]);
        assert_eq!(line.materialize(), cells);
    }

    #[test]
    fn scroll_up_compresses_lines_losslessly() {
        let mut grid = TerminalGrid::with_scrollback_limit(4, 1, 8);
        let bold = CellStyle {
            bold: true,
            ..CellStyle::default()
        };

        for column in 0..4u16 {
            grid.put_cell(column, 0, styled_cell(" ", &bold));
        }
        let original = (0..4)
            .map(|column| grid.cell(column, 0).expect("cell").clone())
            .collect::<Vec<_>>();
        grid.newline(&CellStyle::default());

        assert_eq!(grid.scrollback_len(), 1);
        assert_eq!(grid.line_cells(0), Some(original));
    }

    #[test]
    fn scrollback_limits_evict_oldest_lines_and_cells() {
        let mut by_lines = TerminalGrid::with_limits(4, 1, 2, usize::MAX);
        for line in 0..3 {
            put_line(&mut by_lines, 0, &format!("l{line}"));
            by_lines.newline(&CellStyle::default());
        }
        assert_eq!(by_lines.scrollback_len(), 2);
        assert_eq!(
            by_lines.line_text_absolute(0).as_deref(),
            Some("l1"),
            "oldest line is evicted first"
        );

        let mut by_cells = TerminalGrid::with_limits(4, 1, 100, 8);
        for line in 0..3 {
            put_line(&mut by_cells, 0, &format!("c{line}"));
            by_cells.newline(&CellStyle::default());
        }
        assert_eq!(by_cells.scrollback_len(), 2);
        assert_eq!(by_cells.line_text_absolute(0).as_deref(), Some("c1"));

        by_cells.set_scrollback_limits(1, usize::MAX);
        assert_eq!(by_cells.scrollback_len(), 1);
        assert_eq!(by_cells.line_text_absolute(0).as_deref(), Some("c2"));

        by_cells.set_scrollback_limits(1, 0);
        assert_eq!(by_cells.scrollback_len(), 0);

        let mut unlimited = TerminalGrid::with_limits(4, 1, 8, usize::MAX);
        unlimited.newline(&CellStyle::default());
        assert_eq!(unlimited.scrollback_len(), 1);
        assert_eq!(unlimited.scrollback_limits(), (8, usize::MAX));
    }

    #[test]
    fn scrollback_memory_estimate_stays_below_four_megabytes() {
        let mut grid = TerminalGrid::with_limits(120, 32, 10_000, 2_000_000);
        let mut parser = TerminalParser::new();
        for index in 0..12_000 {
            let line = format!("line {index:>5} scrollback sample output with a few words\n");
            parser.advance(&mut grid, line.as_bytes());
        }

        assert_eq!(grid.scrollback_len(), 10_000);
        let estimate = grid.scrollback_bytes_estimate();
        assert!(
            estimate < 4 * 1024 * 1024,
            "scrollback estimate {estimate} bytes exceeds 4 MiB"
        );
        let uncompressed = grid.scrollback_len() * 120 * std::mem::size_of::<TerminalCell>();
        assert!(
            estimate * 4 < uncompressed,
            "compressed estimate {estimate} should stay well below {uncompressed}"
        );
    }

    #[test]
    fn resize_growth_keeps_content_and_cursor_position() {
        let mut grid = TerminalGrid::new(4, 2);
        put_line(&mut grid, 0, "one");
        put_line(&mut grid, 1, "two");
        grid.cursor_row = 1;

        grid.resize(4, 4);

        assert_eq!(grid.rows, 4);
        assert_eq!(grid.line_text(0), "one");
        assert_eq!(grid.line_text(1), "two");
        assert_eq!(grid.line_text(3), "");
        assert_eq!(grid.cursor_row, 1);
        assert_eq!(grid.total_lines(), 4);
    }

    #[test]
    fn resize_keeps_scrollback_capture_width_and_reflows_visible_screen() {
        let mut grid = TerminalGrid::with_scrollback_limit(6, 1, 4);
        put_line(&mut grid, 0, "scroll");
        grid.newline(&CellStyle::default());
        put_line(&mut grid, 0, "screen");

        assert_eq!(grid.scrollback_len(), 1);
        grid.resize(3, 1);

        assert_eq!(grid.line_text(0), "scr");
        assert_eq!(
            grid.line_cells(0).map(|cells| cells_text(&cells)),
            Some("scroll".to_owned())
        );
        assert_eq!(grid.line_cells(0).map(|cells| cells.len()), Some(6));

        grid.resize(5, 1);

        assert_eq!(grid.line_text(0), "scr");
        assert_eq!(grid.line_cells(0).map(|cells| cells.len()), Some(6));
        assert_eq!(grid.columns, 5);
    }

    #[test]
    fn resize_shrink_keeps_cursor_line_and_scrollback() {
        let mut grid = TerminalGrid::with_scrollback_limit(4, 3, 8);
        put_line(&mut grid, 0, "one");
        put_line(&mut grid, 1, "two");
        put_line(&mut grid, 2, "tri");
        grid.cursor_row = 2;
        grid.newline(&CellStyle::default());

        assert_eq!(grid.scrollback_len(), 1);
        grid.cursor_row = 0;
        grid.resize(4, 2);

        assert_eq!(grid.rows, 2);
        assert_eq!(grid.scrollback_len(), 1);
        assert_eq!(grid.line_text(0), "two");
        assert_eq!(grid.line_text(1), "tri");
        assert_eq!(grid.cursor_row, 0);
    }

    #[test]
    fn resize_moves_lines_between_scrollback_and_screen() {
        let mut grid = TerminalGrid::with_scrollback_limit(4, 3, 8);
        put_line(&mut grid, 0, "one");
        put_line(&mut grid, 1, "two");
        put_line(&mut grid, 2, "tri");
        grid.cursor_row = 2;
        grid.newline(&CellStyle::default());

        // Shrinking pushes the row above the cursor into scrollback.
        grid.resize(4, 2);
        assert_eq!(grid.scrollback_len(), 2);
        assert_eq!(grid.line_text_absolute(0).as_deref(), Some("one"));
        assert_eq!(grid.line_text_absolute(1).as_deref(), Some("two"));
        assert_eq!(grid.line_text(0), "tri");
        assert_eq!(grid.cursor_row, 1);

        // Growing pulls it back out of the scrollback tail.
        grid.resize(4, 3);
        assert_eq!(grid.scrollback_len(), 1);
        assert_eq!(grid.line_text_absolute(0).as_deref(), Some("one"));
        assert_eq!(grid.line_text(0), "two");
        assert_eq!(grid.line_text(1), "tri");
        assert_eq!(grid.cursor_row, 2);
    }

    #[test]
    fn resize_after_scrollback_keeps_absolute_access() {
        let mut grid = TerminalGrid::with_scrollback_limit(10, 3, 8);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"one\ntwo\nthree\nfour\nfive");
        assert_eq!(grid.scrollback_len(), 2);

        grid.resize(6, 2);

        assert_eq!(grid.scrollback_len(), 3, "shrunk row moves into scrollback");
        assert_eq!(grid.line_text_absolute(0).as_deref(), Some("one"));
        assert_eq!(grid.line_text_absolute(1).as_deref(), Some("two"));
        assert_eq!(grid.line_text_absolute(2).as_deref(), Some("three"));
        assert_eq!(grid.line_text(0), "four");
        assert_eq!(grid.line_text(1), "five");

        grid.resize(4, 4);

        // Growing pulls the two most recent scrollback lines back on screen, in
        // chronological order above the rows that were already visible.
        assert_eq!(grid.scrollback_len(), 1);
        assert_eq!(grid.line_text_absolute(0).as_deref(), Some("one"));
        assert_eq!(grid.line_text(0), "two");
        assert_eq!(
            grid.line_text(1),
            "thre",
            "visible lines reflow to 4 columns"
        );
        assert_eq!(grid.line_text(2), "four");
        assert_eq!(grid.line_text(3), "five");
        assert_eq!(
            grid.visible_cells(0).map(cells_text),
            Some("two".to_owned())
        );
        assert!(grid.visible_cells(4).is_none());
        // Scrollback keeps the width it was captured with.
        assert_eq!(grid.line_cells(0).map(|cells| cells.len()), Some(10));
    }

    /// Structural invariant every resize path must keep:
    /// `cells.len() == rows * columns`, so every visible row is a full buffer.
    fn assert_shape(grid: &TerminalGrid) {
        assert_eq!(
            grid.cells.len(),
            usize::from(grid.rows) * usize::from(grid.columns),
            "cells must be exactly rows * columns"
        );
        if grid.columns == 0 {
            return;
        }
        for row in 0..grid.rows {
            assert_eq!(
                grid.visible_cells(row).map(<[TerminalCell]>::len),
                Some(usize::from(grid.columns)),
                "visible row {row} must be a full-width buffer"
            );
        }
    }

    /// Regression for the app crash: growing the screen with an empty scrollback
    /// used to pad the extra rows with *empty* line buffers, so `cells.len()`
    /// stayed at `old_rows * new_columns` while `rows` already reported the new
    /// height. The next write into one of the padded rows panicked with
    /// `index out of bounds: the len is 2496 but the index is 2496`.
    #[test]
    fn resize_growth_with_empty_scrollback_keeps_shape_invariant() {
        let mut grid = TerminalGrid::new(78, 32);
        put_line(&mut grid, 0, "prompt");
        assert_eq!(grid.scrollback_len(), 0);

        grid.resize(78, 36);

        // Write into the row the growth added *before* checking anything else:
        // on the buggy implementation this is where the app crashed with
        // `index out of bounds: the len is 2496 but the index is 2496`.
        put_line(&mut grid, 35, "tail");
        assert_eq!(grid.line_text(35), "tail");

        assert_shape(&grid);
        assert_eq!(grid.scrollback_len(), 0);
        assert_eq!(grid.cells.len(), 36 * 78);
        assert_eq!(grid.line_text(0), "prompt");
        assert_eq!(grid.line_text(34), "");

        grid.cursor_row = 35;
        grid.cursor_column = 4;
        grid.write_grapheme("!".to_owned(), 1, &CellStyle::default());
        assert_eq!(grid.line_text(35), "tail!");

        // The parser write path that crashed in the app must be safe as well.
        let mut parser = TerminalParser::new();
        grid.cursor_column = 0;
        parser.advance(&mut grid, b"more");
        assert_eq!(
            grid.line_text(35),
            "more!",
            "parser overwrote the first cells"
        );
        assert_shape(&grid);
    }

    /// The app sequence that reproduced the crash: banner parsed into an 80x24
    /// grid, viewport grown to 78x36 while the scrollback is still empty, then
    /// the shell keeps printing.
    #[test]
    fn parser_output_after_growing_the_screen_does_not_panic() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = TerminalParser::new();
        parser.advance(
            &mut grid,
            b"Connecting to example.com:2200 as alice\nFake shell established\n$ ",
        );

        grid.resize(78, 36);

        for index in 0..80 {
            parser.advance(
                &mut grid,
                format!("fake-shell received input: {index}\n").as_bytes(),
            );
        }

        assert_shape(&grid);
        assert!(grid.scrollback_len() > 0, "long output must scroll off");
        assert!(
            grid.visible_lines()
                .iter()
                .any(|line| line == "fake-shell received input: 79"),
            "the newest echoed line must be on screen"
        );
    }

    /// Column growth extends every row with blank cells; column shrinking drops
    /// the tail, matching the reflow semantics the scrollback tests pin down.
    #[test]
    fn resize_column_changes_keep_shape_invariant() {
        let mut grid = TerminalGrid::new(4, 2);
        put_line(&mut grid, 0, "ab");
        put_line(&mut grid, 1, "cd");

        grid.resize(8, 2);

        assert_shape(&grid);
        assert_eq!(grid.line_text(0), "ab");
        assert_eq!(grid.line_text(1), "cd");
        assert_eq!(
            grid.cell(7, 0).map(|cell| cell.grapheme.clone()),
            Some(" ".to_owned()),
            "new cells are blank"
        );

        grid.resize(3, 2);

        assert_shape(&grid);
        assert_eq!(grid.line_text(0), "ab");
        assert_eq!(grid.line_text(1), "cd");

        let mut long = TerminalGrid::new(6, 1);
        put_line(&mut long, 0, "abcdef");
        long.resize(3, 1);
        assert_shape(&long);
        assert_eq!(long.line_text(0), "abc", "shrink truncates the line tail");
    }

    /// Shrinking then growing again (with and without column changes) must keep
    /// the shape invariant at every step.
    #[test]
    fn resize_shrink_then_grow_keeps_shape_invariant() {
        let mut grid = TerminalGrid::new(10, 3);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"one\ntwo\nthree\nfour\nfive");
        assert_shape(&grid);

        for (columns, rows) in [
            (10, 2),
            (10, 6),
            (4, 6),
            (7, 1),
            (1, 1),
            (12, 4),
            (0, 5),
            (3, 0),
            (0, 0),
        ] {
            grid.resize(columns, rows);
            assert_shape(&grid);
            assert_eq!(grid.columns, columns);
            assert_eq!(grid.rows, rows);
        }
    }

    /// Growing with a non-empty scrollback pulls the newest lines back on screen
    /// and keeps the invariant (the order they land in is covered by the
    /// scrollback resize tests).
    #[test]
    fn resize_growth_with_scrollback_pulls_lines_back_and_keeps_shape() {
        let mut grid = TerminalGrid::with_scrollback_limit(10, 3, 8);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"one\ntwo\nthree\nfour\nfive");
        assert_eq!(grid.scrollback_len(), 2);

        grid.resize(10, 5);

        assert_shape(&grid);
        assert_eq!(grid.scrollback_len(), 0, "growth pulls the scrollback back");
        assert_eq!(grid.total_lines(), 5);
        let visible = grid.visible_lines();
        for line in ["one", "two", "three", "four", "five"] {
            assert!(
                visible.iter().any(|visible| visible.as_str() == line),
                "{line} is missing from the grown screen"
            );
        }
    }
}
