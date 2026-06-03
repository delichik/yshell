//! PTY request placeholders.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtySize {
    pub columns: u16,
    pub rows: u16,
}
