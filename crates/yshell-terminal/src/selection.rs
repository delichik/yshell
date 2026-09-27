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

    /// True when the normalized range covers the cell.
    #[must_use]
    pub fn contains(self, column: u16, row: u16) -> bool {
        let range = self.normalized();
        if row < range.start.row || row > range.end.row {
            return false;
        }
        let start = if row == range.start.row {
            range.start.column
        } else {
            0
        };
        let end = if row == range.end.row {
            range.end.column
        } else {
            u16::MAX
        };
        column >= start && column <= end
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

    /// Copy a range whose rows are absolute lines (scrollback aware).
    #[must_use]
    pub fn copy_text_absolute(self, grid: &TerminalGrid) -> String {
        let range = self.normalized();
        let mut lines = Vec::new();
        for row in range.start.row..=range.end.row {
            let Some(cells) = grid.line_cells(usize::from(row)) else {
                break;
            };
            let start_column = if row == range.start.row {
                usize::from(range.start.column)
            } else {
                0
            };
            let end_column = if row == range.end.row {
                usize::from(range.end.column)
            } else {
                cells.len().saturating_sub(1)
            };
            let line = cells
                .iter()
                .enumerate()
                .filter(|(column, cell)| {
                    *column >= start_column && *column <= end_column && !cell.wide_continuation
                })
                .map(|(_, cell)| cell.grapheme.as_str())
                .collect::<String>()
                .trim_end()
                .to_owned();
            lines.push(line);
        }
        lines.join("\n")
    }

    /// A selection covering every addressable line.
    #[must_use]
    pub fn select_all(grid: &TerminalGrid) -> Option<Self> {
        if grid.columns == 0 || grid.total_lines() == 0 {
            return None;
        }
        Some(Self {
            start: GridPoint { column: 0, row: 0 },
            end: GridPoint {
                column: grid.columns.saturating_sub(1),
                row: u16::try_from(grid.total_lines() - 1).unwrap_or(u16::MAX),
            },
        })
    }

    /// Expand a point into the word around it on its absolute line.
    ///
    /// Word characters are anything that is neither whitespace nor ASCII
    /// punctuation; returns `None` when the point is not on a word.
    #[must_use]
    pub fn word_at(grid: &TerminalGrid, point: GridPoint) -> Option<Self> {
        let cells = grid.line_cells(usize::from(point.row))?;
        let characters = cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| !cell.wide_continuation)
            .map(|(column, cell)| (column as u16, cell.grapheme.chars().next().unwrap_or(' ')))
            .collect::<Vec<_>>();
        let index = characters
            .iter()
            .enumerate()
            .filter(|(_, (column, _))| *column <= point.column)
            .map(|(index, _)| index)
            .next_back()?;
        if !is_word_character(characters[index].1) {
            return None;
        }
        let mut start = index;
        while start > 0 && is_word_character(characters[start - 1].1) {
            start -= 1;
        }
        let mut end = index;
        while end + 1 < characters.len() && is_word_character(characters[end + 1].1) {
            end += 1;
        }
        Some(Self {
            start: GridPoint {
                column: characters[start].0,
                row: point.row,
            },
            end: GridPoint {
                column: characters[end].0,
                row: point.row,
            },
        })
    }
}

fn is_word_character(character: char) -> bool {
    !character.is_whitespace() && !character.is_ascii_punctuation()
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

    #[test]
    fn normalizes_reverse_selections() {
        let selection = SelectionRange {
            start: GridPoint { column: 4, row: 3 },
            end: GridPoint { column: 1, row: 1 },
        }
        .normalized();

        assert_eq!(selection.start, GridPoint { column: 1, row: 1 });
        assert_eq!(selection.end, GridPoint { column: 4, row: 3 });
        assert!(selection.contains(2, 2));
        assert!(!selection.contains(5, 3));
    }

    #[test]
    fn copies_absolute_range_across_scrollback() {
        let mut grid = TerminalGrid::with_scrollback_limit(10, 2, 8);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"one\ntwo\nthree\nfour");

        assert_eq!(grid.scrollback_len(), 2);
        let selection = SelectionRange {
            start: GridPoint { column: 1, row: 0 },
            end: GridPoint { column: 2, row: 3 },
        };

        assert_eq!(selection.copy_text_absolute(&grid), "ne\ntwo\nthree\nfou");
    }

    #[test]
    fn select_all_covers_scrollback_and_screen() {
        let mut grid = TerminalGrid::with_scrollback_limit(5, 2, 8);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"aa\nbb\ncc");

        let selection = SelectionRange::select_all(&grid).expect("selection");
        assert_eq!(selection.start, GridPoint { column: 0, row: 0 });
        assert_eq!(selection.end, GridPoint { column: 4, row: 2 });
        assert_eq!(selection.copy_text_absolute(&grid), "aa\nbb\ncc");

        let empty = TerminalGrid::new(0, 0);
        assert!(SelectionRange::select_all(&empty).is_none());
    }

    #[test]
    fn word_at_expands_on_whitespace_and_punctuation_boundaries() {
        let mut grid = TerminalGrid::new(32, 1);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"git commit -m message.txt");

        let word = SelectionRange::word_at(&grid, GridPoint { column: 1, row: 0 })
            .expect("word under cursor");
        assert_eq!(word.start, GridPoint { column: 0, row: 0 });
        assert_eq!(word.end, GridPoint { column: 2, row: 0 });
        assert_eq!(word.copy_text_absolute(&grid), "git");

        let flag = SelectionRange::word_at(&grid, GridPoint { column: 5, row: 0 })
            .expect("word under cursor");
        assert_eq!(flag.copy_text_absolute(&grid), "commit");

        let path = SelectionRange::word_at(&grid, GridPoint { column: 14, row: 0 })
            .expect("word under cursor");
        assert_eq!(path.copy_text_absolute(&grid), "message");

        assert!(SelectionRange::word_at(&grid, GridPoint { column: 3, row: 0 }).is_none());
        assert!(SelectionRange::word_at(&grid, GridPoint { column: 1, row: 5 }).is_none());
    }

    #[test]
    fn word_at_reads_compressed_scrollback_lines() {
        let mut grid = TerminalGrid::with_scrollback_limit(16, 2, 8);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"git commit -m message\nsecond\nthird");

        assert_eq!(grid.scrollback_len(), 2);
        let word = SelectionRange::word_at(&grid, GridPoint { column: 6, row: 0 })
            .expect("word on a scrollback line");
        assert_eq!(word.start, GridPoint { column: 4, row: 0 });
        assert_eq!(word.end, GridPoint { column: 9, row: 0 });
        assert_eq!(word.copy_text_absolute(&grid), "commit");
    }
}
