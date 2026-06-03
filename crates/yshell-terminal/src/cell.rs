//! Terminal screen cell model.

use crate::color::TerminalColor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCell {
    pub grapheme: String,
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub wide_continuation: bool,
}

impl TerminalCell {
    #[must_use]
    pub fn blank_with_style(style: &CellStyle) -> Self {
        Self {
            grapheme: " ".to_owned(),
            foreground: style.foreground,
            background: style.background,
            bold: style.bold,
            italic: style.italic,
            underline: style.underline,
            wide_continuation: false,
        }
    }
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self::blank_with_style(&CellStyle::default())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellStyle {
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            foreground: TerminalColor::WHITE,
            background: TerminalColor::BLACK,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}
