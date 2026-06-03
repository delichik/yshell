//! Terminal screen grid and scrollback skeleton.

use crate::cell::TerminalCell;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalGrid {
    pub columns: u16,
    pub rows: u16,
    pub cells: Vec<TerminalCell>,
}

impl TerminalGrid {
    pub fn new(columns: u16, rows: u16) -> Self {
        let len = usize::from(columns) * usize::from(rows);
        Self {
            columns,
            rows,
            cells: vec![TerminalCell::default(); len],
        }
    }
}
