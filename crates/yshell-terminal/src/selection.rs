//! Terminal selection placeholder.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridPoint {
    pub column: u16,
    pub row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: GridPoint,
    pub end: GridPoint,
}
