//! Terminal search helpers.

use regex::{Regex, RegexBuilder};

use crate::{grid::TerminalGrid, selection::GridPoint};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub case_sensitive: bool,
    pub regex: bool,
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
            regex: false,
        }
    }

    #[must_use]
    pub const fn case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    #[must_use]
    pub const fn regex(mut self, regex: bool) -> Self {
        self.regex = regex;
        self
    }

    /// Search visible lines. Invalid regex returns `Err` with a short reason.
    pub fn try_find_in_grid(&self, grid: &TerminalGrid) -> Result<Vec<SearchMatch>, String> {
        if self.text.is_empty() {
            return Ok(Vec::new());
        }

        if self.regex {
            return self.find_regex_in_grid(grid);
        }

        Ok(self.find_literal_in_grid(grid))
    }

    #[must_use]
    pub fn find_in_grid(&self, grid: &TerminalGrid) -> Vec<SearchMatch> {
        self.try_find_in_grid(grid).unwrap_or_default()
    }

    fn find_literal_in_grid(&self, grid: &TerminalGrid) -> Vec<SearchMatch> {
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

    fn find_regex_in_grid(&self, grid: &TerminalGrid) -> Result<Vec<SearchMatch>, String> {
        let pattern = RegexBuilder::new(&self.text)
            .case_insensitive(!self.case_sensitive)
            .build()
            .map_err(|error| format!("Invalid regex: {error}"))?;

        Ok(grid
            .visible_lines()
            .into_iter()
            .enumerate()
            .flat_map(|(row, line)| regex_matches_on_line(&pattern, &line, row as u16))
            .collect())
    }
}

fn regex_matches_on_line(pattern: &Regex, line: &str, row: u16) -> Vec<SearchMatch> {
    pattern
        .find_iter(line)
        .filter_map(|found| {
            let start_byte = found.start();
            let end_byte = found.end();
            if start_byte >= end_byte {
                return None;
            }
            let start_col = line[..start_byte].chars().count() as u16;
            let end_col = line[..end_byte].chars().count().saturating_sub(1) as u16;
            Some(SearchMatch {
                start: GridPoint {
                    column: start_col,
                    row,
                },
                end: GridPoint {
                    column: end_col,
                    row,
                },
            })
        })
        .collect()
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

    #[test]
    fn searches_visible_lines_with_regex() {
        let mut grid = TerminalGrid::new(24, 1);
        let mut parser = TerminalParser::new();
        parser.advance(&mut grid, b"id=42 name=yshell");

        let matches = SearchQuery::new(r"id=\d+")
            .regex(true)
            .try_find_in_grid(&grid)
            .expect("valid regex");

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start, GridPoint { column: 0, row: 0 });
        assert_eq!(matches[0].end, GridPoint { column: 4, row: 0 });
    }

    #[test]
    fn invalid_regex_returns_error() {
        let grid = TerminalGrid::new(8, 1);
        let error = SearchQuery::new("[")
            .regex(true)
            .try_find_in_grid(&grid)
            .expect_err("invalid regex");
        assert!(error.contains("Invalid regex"));
    }
}
