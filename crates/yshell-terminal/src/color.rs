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
