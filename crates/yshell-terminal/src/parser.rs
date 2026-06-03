//! Small ANSI/VTE parser for the terminal model.
//!
//! This parser intentionally supports the control sequences needed by the
//! first terminal model milestone: printable text, LF/CR/backspace, SGR
//! colors/styles, and clear-screen commands.

use unicode_width::UnicodeWidthChar;

use crate::{cell::CellStyle, color::TerminalColor, grid::TerminalGrid, input::TerminalInputEvent};

#[derive(Debug, Clone)]
pub struct TerminalParser {
    style: CellStyle,
    state: ParserState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParserState {
    Ground,
    Escape,
    Csi(String),
}

impl Default for TerminalParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            style: CellStyle::default(),
            state: ParserState::Ground,
        }
    }

    #[must_use]
    pub const fn style(&self) -> CellStyle {
        self.style
    }

    pub fn advance(&mut self, grid: &mut TerminalGrid, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        for character in text.chars() {
            self.advance_char(grid, character);
        }
    }

    pub fn apply_input(&mut self, grid: &mut TerminalGrid, event: &TerminalInputEvent) {
        match event {
            TerminalInputEvent::PrintableText { text } => self.advance(grid, text.as_bytes()),
            TerminalInputEvent::ControlKey { key } => match key {
                crate::input::ControlKey::Enter => self.advance(grid, b"\n"),
                crate::input::ControlKey::Backspace => self.advance(grid, b"\x08"),
                crate::input::ControlKey::Tab => self.advance(grid, b"\t"),
                crate::input::ControlKey::Escape => self.advance(grid, b"\x1b"),
                crate::input::ControlKey::Delete => {}
            },
            TerminalInputEvent::Paste { bytes, .. } => self.advance(grid, bytes),
            TerminalInputEvent::Resize { columns, rows } => {
                *grid = TerminalGrid::new(*columns, *rows)
            }
            TerminalInputEvent::NavigationKey { .. }
            | TerminalInputEvent::FunctionKey { .. }
            | TerminalInputEvent::ModifiedKey { .. } => {}
        }
    }

    fn advance_char(&mut self, grid: &mut TerminalGrid, character: char) {
        match &mut self.state {
            ParserState::Ground => match character {
                '\x1b' => self.state = ParserState::Escape,
                '\n' => grid.newline(&self.style),
                '\r' => grid.carriage_return(),
                '\x08' => grid.backspace(),
                '\t' => {
                    let next_tab = ((grid.cursor_column / 8) + 1) * 8;
                    while grid.cursor_column < next_tab.min(grid.columns) {
                        grid.write_grapheme(" ".to_owned(), 1, &self.style);
                    }
                }
                character if !character.is_control() => {
                    let width = UnicodeWidthChar::width(character).unwrap_or(1).max(1);
                    grid.write_grapheme(character.to_string(), width, &self.style);
                }
                _ => {}
            },
            ParserState::Escape => {
                if character == '[' {
                    self.state = ParserState::Csi(String::new());
                } else {
                    self.state = ParserState::Ground;
                }
            }
            ParserState::Csi(buffer) => {
                if character.is_ascii_digit() || matches!(character, ';' | '?' | ':') {
                    buffer.push(character);
                } else {
                    let params = buffer.clone();
                    self.state = ParserState::Ground;
                    self.apply_csi(grid, &params, character);
                }
            }
        }
    }

    fn apply_csi(&mut self, grid: &mut TerminalGrid, params: &str, final_byte: char) {
        match final_byte {
            'm' => self.apply_sgr(params),
            'J' => {
                if params.is_empty() || params == "0" || params == "2" || params == "3" {
                    grid.clear_screen(&self.style);
                }
            }
            _ => {}
        }
    }

    fn apply_sgr(&mut self, params: &str) {
        let codes = if params.is_empty() {
            vec![0]
        } else {
            params
                .split(';')
                .map(|value| value.parse::<u16>().unwrap_or(0))
                .collect::<Vec<_>>()
        };

        let mut index = 0;
        while index < codes.len() {
            match codes[index] {
                0 => self.style = CellStyle::default(),
                1 => self.style.bold = true,
                3 => self.style.italic = true,
                4 => self.style.underline = true,
                22 => self.style.bold = false,
                23 => self.style.italic = false,
                24 => self.style.underline = false,
                30..=37 => self.style.foreground = TerminalColor::ansi((codes[index] - 30) as u8),
                39 => self.style.foreground = CellStyle::default().foreground,
                40..=47 => self.style.background = TerminalColor::ansi((codes[index] - 40) as u8),
                49 => self.style.background = CellStyle::default().background,
                90..=97 => {
                    self.style.foreground = TerminalColor::ansi((codes[index] - 90 + 8) as u8)
                }
                100..=107 => {
                    self.style.background = TerminalColor::ansi((codes[index] - 100 + 8) as u8)
                }
                38 | 48 if index + 4 < codes.len() && codes[index + 1] == 2 => {
                    let color = TerminalColor::rgb(
                        codes[index + 2].min(255) as u8,
                        codes[index + 3].min(255) as u8,
                        codes[index + 4].min(255) as u8,
                    );
                    if codes[index] == 38 {
                        self.style.foreground = color;
                    } else {
                        self.style.background = color;
                    }
                    index += 4;
                }
                _ => {}
            }
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_printable_controls_and_scrollback() {
        let mut grid = TerminalGrid::with_scrollback_limit(4, 2, 8);
        let mut parser = TerminalParser::new();

        parser.advance(&mut grid, b"abcd\nef\rg\x08h\nijkl");

        assert_eq!(grid.scrollback_rows().len(), 1);
        assert_eq!(grid.line_text(0), "hf");
        assert_eq!(grid.line_text(1), "ijkl");
    }

    #[test]
    fn parses_sgr_and_clear_screen() {
        let mut grid = TerminalGrid::new(8, 2);
        let mut parser = TerminalParser::new();

        parser.advance(&mut grid, b"\x1b[1;31;44mA\x1b[0mB");

        let red_bold = grid.cell(0, 0).expect("cell exists");
        assert_eq!(red_bold.grapheme, "A");
        assert_eq!(red_bold.foreground, TerminalColor::RED);
        assert_eq!(red_bold.background, TerminalColor::BLUE);
        assert!(red_bold.bold);
        assert_eq!(
            grid.cell(1, 0).expect("cell exists").foreground,
            TerminalColor::WHITE
        );

        parser.advance(&mut grid, b"\x1b[2J");
        assert_eq!(grid.visible_lines(), vec!["", ""]);
    }

    #[test]
    fn wide_char_occupies_continuation_cell_without_panicking() {
        let mut grid = TerminalGrid::new(4, 1);
        let mut parser = TerminalParser::new();

        parser.advance(&mut grid, "好a".as_bytes());

        assert_eq!(grid.cell(0, 0).expect("wide").grapheme, "好");
        assert!(grid.cell(1, 0).expect("continuation").wide_continuation);
        assert_eq!(grid.cell(2, 0).expect("a").grapheme, "a");
    }
}
