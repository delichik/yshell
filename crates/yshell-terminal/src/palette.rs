//! Terminal color palettes.
//!
//! The renderer used to hardcode the foreground, background and selection
//! colors plus the 16 ANSI entries as `TerminalColor` constants. A
//! [`TerminalPalette`] is the per-session replacement for those constants:
//! [`TerminalPalette::default`] reproduces the legacy values exactly, and
//! [`TerminalRenderer::apply_appearance`](crate::TerminalRenderer::apply_appearance)
//! installs a selected palette on a renderer.
//!
//! Named built-in schemes ship as sidecar TOML under `themes/` (see
//! `yshell-config::ThemeCatalog`); this module only owns the palette type and
//! the hard-coded default fallback.

use crate::color::TerminalColor;
use crate::render::{DEFAULT_BACKGROUND, DEFAULT_CURSOR, DEFAULT_FOREGROUND, SELECTION_BACKGROUND};

/// Colors of one terminal session: default foreground/background, cursor,
/// selection highlight and the 16 ANSI entries.
///
/// The ANSI array is indexed by the SGR color index (`30..=37` and `90..=97`
/// map to `0..=15`). Cells still carry the legacy [`TerminalColor`] constants;
/// the renderer remaps them to the palette entry at paint time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPalette {
    /// Default foreground (`SGR 39` and plain text).
    pub foreground: TerminalColor,
    /// Default background (`SGR 49`).
    pub background: TerminalColor,
    /// Cursor block color.
    pub cursor: TerminalColor,
    /// Selection highlight painted as the cell background while selected.
    pub selection: TerminalColor,
    /// The 16 ANSI colors (`0..=7` normal, `8..=15` bright).
    pub ansi: [TerminalColor; 16],
}

impl TerminalPalette {
    /// The built-in YShell Default palette; byte-identical to the colors the
    /// renderer used before palettes existed.
    pub const DEFAULT: Self = Self {
        foreground: DEFAULT_FOREGROUND,
        background: DEFAULT_BACKGROUND,
        cursor: DEFAULT_CURSOR,
        selection: SELECTION_BACKGROUND,
        ansi: [
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
        ],
    };

    /// Palette entry for an ANSI index (`0..=15`; higher bits are masked).
    #[must_use]
    pub const fn ansi(self, index: u8) -> TerminalColor {
        self.ansi[(index & 0x0f) as usize]
    }

    /// Same palette with a different foreground.
    #[must_use]
    pub const fn with_foreground(self, color: TerminalColor) -> Self {
        Self {
            foreground: color,
            ..self
        }
    }

    /// Same palette with a different background.
    #[must_use]
    pub const fn with_background(self, color: TerminalColor) -> Self {
        Self {
            background: color,
            ..self
        }
    }

    /// Same palette with a different cursor color.
    #[must_use]
    pub const fn with_cursor(self, color: TerminalColor) -> Self {
        Self {
            cursor: color,
            ..self
        }
    }

    /// Same palette with a different selection color.
    #[must_use]
    pub const fn with_selection(self, color: TerminalColor) -> Self {
        Self {
            selection: color,
            ..self
        }
    }

    /// Same palette with a different 16 color ANSI table.
    #[must_use]
    pub const fn with_ansi(self, ansi: [TerminalColor; 16]) -> Self {
        Self { ansi, ..self }
    }

    /// Same palette with one ANSI entry replaced.
    #[must_use]
    pub const fn with_ansi_entry(self, index: u8, color: TerminalColor) -> Self {
        let mut ansi = self.ansi;
        ansi[(index & 0x0f) as usize] = color;
        Self { ansi, ..self }
    }
}

impl Default for TerminalPalette {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Short alias for `TerminalColor::rgb` in builders / tests.
#[cfg(test)]
const fn c(red: u8, green: u8, blue: u8) -> TerminalColor {
    TerminalColor::rgb(red, green, blue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette_matches_the_legacy_renderer_constants() {
        let palette = TerminalPalette::default();
        assert_eq!(palette.foreground, DEFAULT_FOREGROUND);
        assert_eq!(palette.background, DEFAULT_BACKGROUND);
        assert_eq!(palette.cursor, DEFAULT_CURSOR);
        assert_eq!(palette.selection, SELECTION_BACKGROUND);
        for index in 0..16u8 {
            assert_eq!(
                palette.ansi(index),
                TerminalColor::ansi(index),
                "ANSI entry {index} must keep the legacy constant"
            );
        }
        assert_eq!(palette.ansi(16), palette.ansi(0), "index bits are masked");
    }

    #[test]
    fn palette_builders_replace_single_entries() {
        let palette = TerminalPalette::default()
            .with_foreground(c(0x01, 0x02, 0x03))
            .with_background(c(0x04, 0x05, 0x06))
            .with_cursor(c(0x07, 0x08, 0x09))
            .with_selection(c(0x0a, 0x0b, 0x0c))
            .with_ansi_entry(1, c(0x0d, 0x0e, 0x0f));
        assert_eq!(palette.foreground, c(0x01, 0x02, 0x03));
        assert_eq!(palette.background, c(0x04, 0x05, 0x06));
        assert_eq!(palette.cursor, c(0x07, 0x08, 0x09));
        assert_eq!(palette.selection, c(0x0a, 0x0b, 0x0c));
        assert_eq!(palette.ansi(1), c(0x0d, 0x0e, 0x0f));
        assert_eq!(palette.ansi(2), TerminalPalette::default().ansi(2));

        let replaced = TerminalPalette::default().with_ansi([c(0x11, 0x22, 0x33); 16]);
        assert!(replaced
            .ansi
            .iter()
            .all(|entry| *entry == c(0x11, 0x22, 0x33)));
    }
}
