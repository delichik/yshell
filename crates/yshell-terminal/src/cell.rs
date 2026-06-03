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
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self {
            grapheme: " ".to_owned(),
            foreground: TerminalColor::WHITE,
            background: TerminalColor::BLACK,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}
