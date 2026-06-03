//! Terminal selection primitives.

use crate::grid::TerminalGrid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GridPoint {
    pub column: u16,
    pub row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: GridPoint,
    pub end: GridPoint,
}

impl SelectionRange {
    #[must_use]
    pub fn normalized(self) -> Self {
        if self.start <= self.end {
            self
        } else {
            Self {
                start: self.end,
                end: self.start,
            }
        }
    }

    #[must_use]
    pub fn copy_text(self, grid: &TerminalGrid) -> String {
        let range = self.normalized();
        let mut lines = Vec::new();
        for row in range.start.row..=range.end.row {
            let start_column = if row == range.start.row {
                range.start.column
            } else {
                0
            };
            let end_column = if row == range.end.row {
                range.end.column
            } else {
                grid.columns.saturating_sub(1)
            };
            let line = (start_column..=end_column.min(grid.columns.saturating_sub(1)))
                .filter_map(|column| grid.cell(column, row))
                .filter(|cell| !cell.wide_continuation)
                .map(|cell| cell.grapheme.as_str())
                .collect::<String>()
                .trim_end()
                .to_owned();
            lines.push(line);
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use crate::{parser::TerminalParser, TerminalGrid};

    use super::*;

    #[test]
    fn copies_multiline_selection() {
        let mut grid = TerminalGrid::new(5, 2);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"hello\nworld");

        let selection = SelectionRange {
            start: GridPoint { column: 1, row: 0 },
            end: GridPoint { column: 2, row: 1 },
        };

        assert_eq!(selection.copy_text(&grid), "ello\nwor");
    }
}
