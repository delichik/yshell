//! Terminal color primitives.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl TerminalColor {
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const RED: Self = Self::rgb(205, 49, 49);
    pub const GREEN: Self = Self::rgb(13, 188, 121);
    pub const YELLOW: Self = Self::rgb(229, 229, 16);
    pub const BLUE: Self = Self::rgb(36, 114, 200);
    pub const MAGENTA: Self = Self::rgb(188, 63, 188);
    pub const CYAN: Self = Self::rgb(17, 168, 205);
    pub const WHITE: Self = Self::rgb(229, 229, 229);
    pub const BRIGHT_BLACK: Self = Self::rgb(102, 102, 102);
    pub const BRIGHT_RED: Self = Self::rgb(241, 76, 76);
    pub const BRIGHT_GREEN: Self = Self::rgb(35, 209, 139);
    pub const BRIGHT_YELLOW: Self = Self::rgb(245, 245, 67);
    pub const BRIGHT_BLUE: Self = Self::rgb(59, 142, 234);
    pub const BRIGHT_MAGENTA: Self = Self::rgb(214, 112, 214);
    pub const BRIGHT_CYAN: Self = Self::rgb(41, 184, 219);
    pub const BRIGHT_WHITE: Self = Self::rgb(255, 255, 255);

    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// Same color as a lowercase `#rrggbb` string (theme UI, logs, tests).
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.red, self.green, self.blue)
    }

    /// Parses `#rrggbb` / `rrggbb` (case-insensitive, surrounding whitespace
    /// allowed). Returns `None` for anything else, so a settings input can
    /// reject bad hex without panicking.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        let text = text.trim();
        let digits = text.strip_prefix('#').unwrap_or(text);
        if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |start: usize| u8::from_str_radix(&digits[start..start + 2], 16).ok();
        Some(Self::rgb(channel(0)?, channel(2)?, channel(4)?))
    }

    #[must_use]
    pub const fn ansi(index: u8) -> Self {
        match index {
            0 => Self::BLACK,
            1 => Self::RED,
            2 => Self::GREEN,
            3 => Self::YELLOW,
            4 => Self::BLUE,
            5 => Self::MAGENTA,
            6 => Self::CYAN,
            7 => Self::WHITE,
            8 => Self::BRIGHT_BLACK,
            9 => Self::BRIGHT_RED,
            10 => Self::BRIGHT_GREEN,
            11 => Self::BRIGHT_YELLOW,
            12 => Self::BRIGHT_BLUE,
            13 => Self::BRIGHT_MAGENTA,
            14 => Self::BRIGHT_CYAN,
            _ => Self::BRIGHT_WHITE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_rejects_bad_input() {
        let color = TerminalColor::rgb(0x12, 0xab, 0xff);
        assert_eq!(color.to_hex(), "#12abff");
        assert_eq!(TerminalColor::from_hex("#12abff"), Some(color));
        assert_eq!(TerminalColor::from_hex("12ABFF"), Some(color));
        assert_eq!(TerminalColor::from_hex("  #12abff  "), Some(color));

        for bad in [
            "", "#", "12abf", "#12abfff", "12abfg", "0x12abff", "#12 abff",
        ] {
            assert_eq!(TerminalColor::from_hex(bad), None, "accepted {bad:?}");
        }
    }

    #[test]
    fn ansi_table_matches_the_named_constants() {
        let table = [
            TerminalColor::BLACK,
            TerminalColor::RED,
            TerminalColor::GREEN,
            TerminalColor::YELLOW,
            TerminalColor::BLUE,
            TerminalColor::MAGENTA,
            TerminalColor::CYAN,
            TerminalColor::WHITE,
            TerminalColor::BRIGHT_BLACK,
            TerminalColor::BRIGHT_RED,
            TerminalColor::BRIGHT_GREEN,
            TerminalColor::BRIGHT_YELLOW,
            TerminalColor::BRIGHT_BLUE,
            TerminalColor::BRIGHT_MAGENTA,
            TerminalColor::BRIGHT_CYAN,
            TerminalColor::BRIGHT_WHITE,
        ];
        for (index, expected) in table.into_iter().enumerate() {
            assert_eq!(TerminalColor::ansi(index as u8), expected);
        }
        assert_eq!(TerminalColor::ansi(200), TerminalColor::BRIGHT_WHITE);
    }
}
