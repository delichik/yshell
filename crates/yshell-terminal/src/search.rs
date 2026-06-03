//! Terminal search helpers.

use crate::{grid::TerminalGrid, selection::GridPoint};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub case_sensitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    pub start: GridPoint,
    pub end: GridPoint,
}

impl SearchQuery {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            case_sensitive: false,
        }
    }

    #[must_use]
    pub const fn case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    #[must_use]
    pub fn find_in_grid(&self, grid: &TerminalGrid) -> Vec<SearchMatch> {
        if self.text.is_empty() {
            return Vec::new();
        }

        let needle = if self.case_sensitive {
            self.text.clone()
        } else {
            self.text.to_lowercase()
        };

        grid.visible_lines()
            .into_iter()
            .enumerate()
            .flat_map(|(row, line)| {
                let haystack = if self.case_sensitive {
                    line.clone()
                } else {
                    line.to_lowercase()
                };
                let needle = needle.clone();
                let mut offset = 0;
                let mut matches = Vec::new();
                while let Some(found) = haystack[offset..].find(&needle) {
                    let start_byte = offset + found;
                    let end_byte = start_byte + needle.len();
                    let start_col = line[..start_byte].chars().count() as u16;
                    let end_col = line[..end_byte].chars().count().saturating_sub(1) as u16;
                    matches.push(SearchMatch {
                        start: GridPoint {
                            column: start_col,
                            row: row as u16,
                        },
                        end: GridPoint {
                            column: end_col,
                            row: row as u16,
                        },
                    });
                    offset = end_byte;
                }
                matches
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::{parser::TerminalParser, TerminalGrid};

    use super::*;

    #[test]
    fn searches_visible_lines_case_insensitively() {
        let mut grid = TerminalGrid::new(20, 2);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"Prompt> Secret\nsecret again");

        let matches = SearchQuery::new("secret").find_in_grid(&grid);

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].start, GridPoint { column: 8, row: 0 });
        assert_eq!(matches[1].start, GridPoint { column: 0, row: 1 });
    }
}
