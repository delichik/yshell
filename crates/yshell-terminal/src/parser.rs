//! Small ANSI/VTE parser for the terminal model.
//!
//! Supports printable text, LF/CR/backspace/tab, SGR colors/styles, cursor
//! addressing (`CSI H`/`f`/`A`–`D`), and erase (`CSI J`/`K`) — enough for TUI
//! apps such as `htop` to paint without drifting.

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
    /// OSC/DCS/PM/APC 字符串序列：消费到 BEL (`0x07`) 或 ST (`ESC \`) 为止。
    /// D3：窗口标题等序列不得作为明文回显到网格。
    StringSequence,
    /// 字符串序列里读到 `ESC`，等待 `\`（ST）或重新回到序列内容。
    StringSequenceEscape,
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
                let (line_limit, cell_limit) = grid.scrollback_limits();
                *grid = TerminalGrid::with_limits(*columns, *rows, line_limit, cell_limit)
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
                } else if matches!(character, ']' | 'P' | '^' | '_') {
                    // OSC / DCS / PM / APC：整段消费，不写入网格。
                    self.state = ParserState::StringSequence;
                } else {
                    self.state = ParserState::Ground;
                }
            }
            ParserState::StringSequence => match character {
                '\x07' => self.state = ParserState::Ground,
                '\x1b' => self.state = ParserState::StringSequenceEscape,
                _ => {}
            },
            ParserState::StringSequenceEscape => match character {
                '\\' | '\x07' => self.state = ParserState::Ground,
                '\x1b' => {}
                _ => self.state = ParserState::StringSequence,
            },
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
        // Private-mode CSI (`?…h`/`l`) is consumed without effect for now.
        if params.starts_with('?') {
            return;
        }
        let numbers = parse_csi_params(params);
        match final_byte {
            'm' => self.apply_sgr(params),
            // CUP / HVP: row;col (1-based). Empty → home.
            'H' | 'f' => {
                let row = numbers.first().copied().unwrap_or(1).max(1);
                let column = numbers.get(1).copied().unwrap_or(1).max(1);
                grid.set_cursor(column.saturating_sub(1), row.saturating_sub(1));
            }
            'A' => grid.move_cursor(0, -(csi_count(&numbers) as i32)),
            'B' => grid.move_cursor(0, csi_count(&numbers) as i32),
            'C' => grid.move_cursor(csi_count(&numbers) as i32, 0),
            'D' => grid.move_cursor(-(csi_count(&numbers) as i32), 0),
            'G' => {
                // CHA: cursor horizontal absolute (1-based column).
                let column = numbers.first().copied().unwrap_or(1).max(1);
                grid.set_cursor(column.saturating_sub(1), grid.cursor_row);
            }
            'd' => {
                // VPA: vertical position absolute (1-based row).
                let row = numbers.first().copied().unwrap_or(1).max(1);
                grid.set_cursor(grid.cursor_column, row.saturating_sub(1));
            }
            'J' => {
                let mode = numbers.first().copied().unwrap_or(0);
                if mode == 2 {
                    // Legacy callers / tests: full clear also homes the cursor.
                    grid.clear_screen(&self.style);
                } else {
                    grid.erase_in_display(mode, &self.style);
                }
            }
            'K' => {
                let mode = numbers.first().copied().unwrap_or(0);
                grid.erase_in_line(mode, &self.style);
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
                38 | 48 if index + 2 < codes.len() && codes[index + 1] == 5 => {
                    // 256-color indexed (`38;5;n` / `48;5;n`) — htop / TUIs use this heavily.
                    let color = TerminalColor::indexed(codes[index + 2].min(255) as u8);
                    if codes[index] == 38 {
                        self.style.foreground = color;
                    } else {
                        self.style.background = color;
                    }
                    index += 2;
                }
                _ => {}
            }
            index += 1;
        }
    }
}

/// CSI numeric parameters (`"1;2"` → `[1, 2]`; empty → `[]`).
fn parse_csi_params(params: &str) -> Vec<u16> {
    if params.is_empty() {
        return Vec::new();
    }
    params
        .split(';')
        .map(|part| {
            if part.is_empty() {
                0
            } else {
                part.parse::<u16>().unwrap_or(0)
            }
        })
        .collect()
}

/// Movement count for CSI A/B/C/D: missing or 0 means 1.
fn csi_count(numbers: &[u16]) -> u16 {
    match numbers.first().copied().unwrap_or(1) {
        0 => 1,
        n => n,
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

        assert_eq!(grid.scrollback_len(), 1);
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
    fn consumes_osc_window_title_without_printing_it() {
        let mut grid = TerminalGrid::new(40, 2);
        let mut parser = TerminalParser::new();

        // D3：OSC 0 标题（BEL 与 ST 两种终止符）不得回显为明文。
        parser.advance(
            &mut grid,
            b"\x1b]0;tester@host: ~\x07prompt$ \x1b]0;second title\x1b\\done",
        );

        assert_eq!(grid.line_text(0), "prompt$ done");
    }

    #[test]
    fn consumes_dcs_and_apc_sequences() {
        let mut grid = TerminalGrid::new(40, 2);
        let mut parser = TerminalParser::new();

        parser.advance(&mut grid, b"a\x1bP1$r0m\x1b\\b\x1b_apc payload\x1b\\c");

        assert_eq!(grid.line_text(0), "abc");
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

    #[test]
    fn cup_and_erase_in_line_paint_like_htop() {
        let mut grid = TerminalGrid::new(8, 3);
        let mut parser = TerminalParser::new();

        // Fill three rows, then overwrite the middle of row 2 and erase to EOL.
        parser.advance(&mut grid, b"AAAAAAAA\nBBBBBBBB\nCCCCCCCC");
        parser.advance(&mut grid, b"\x1b[2;3HXY\x1b[K");

        assert_eq!(grid.cursor_column, 4);
        assert_eq!(grid.cursor_row, 1);
        assert_eq!(grid.line_text(0), "AAAAAAAA");
        assert_eq!(grid.line_text(1), "BBXY");
        assert_eq!(grid.line_text(2), "CCCCCCCC");
    }

    #[test]
    fn relative_cursor_moves_and_cha() {
        let mut grid = TerminalGrid::new(10, 4);
        let mut parser = TerminalParser::new();

        parser.advance(&mut grid, b"\x1b[3;5H\x1b[2A\x1b[3C\x1b[1D");
        assert_eq!((grid.cursor_column, grid.cursor_row), (6, 0));

        parser.advance(&mut grid, b"\x1b[4G");
        assert_eq!(grid.cursor_column, 3);
    }

    #[test]
    fn indexed_256_color_sgr() {
        let mut grid = TerminalGrid::new(4, 1);
        let mut parser = TerminalParser::new();

        parser.advance(&mut grid, b"\x1b[38;5;196mX");
        let cell = grid.cell(0, 0).expect("cell");
        assert_eq!(cell.grapheme, "X");
        assert_eq!(cell.foreground, TerminalColor::indexed(196));
    }
}
