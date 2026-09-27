//! Terminal color palettes and the built-in color schemes.
//!
//! The renderer used to hardcode the foreground, background and selection
//! colors plus the 16 ANSI entries as `TerminalColor` constants. A
//! [`TerminalPalette`] is the per-session replacement for those constants:
//! [`TerminalPalette::default`] reproduces the legacy values exactly, and
//! [`TerminalRenderer::apply_appearance`](crate::TerminalRenderer::apply_appearance)
//! installs a selected palette on a renderer.
//!
//! [`COLOR_SCHEMES`] is the list the settings UI offers. Ids are stable and
//! are what a `TerminalProfile::color_scheme` stores.

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
    /// Selection highlight, blended over the cell background at 50%.
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

/// One built-in color scheme: a stable [`id`](Self::id), a display
/// [`name`](Self::name) and its [`palette`](Self::palette).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalColorScheme {
    /// Stable identifier stored in `TerminalProfile::color_scheme`.
    pub id: &'static str,
    /// Display name for the settings UI.
    pub name: &'static str,
    /// The scheme's colors.
    pub palette: TerminalPalette,
}

/// The built-in color schemes offered by the settings UI, first version.
pub const COLOR_SCHEMES: &[TerminalColorScheme] = &[
    TerminalColorScheme {
        id: "yshell-default",
        name: "YShell Default",
        palette: TerminalPalette::DEFAULT,
    },
    TerminalColorScheme {
        id: "dracula",
        name: "Dracula",
        palette: TerminalPalette {
            foreground: c(0xf8, 0xf8, 0xf2),
            background: c(0x28, 0x2a, 0x36),
            cursor: c(0xf8, 0xf8, 0xf2),
            selection: c(0x44, 0x47, 0x5a),
            ansi: [
                c(0x21, 0x22, 0x2c),
                c(0xff, 0x55, 0x55),
                c(0x50, 0xfa, 0x7b),
                c(0xf1, 0xfa, 0x8c),
                c(0xbd, 0x93, 0xf9),
                c(0xff, 0x79, 0xc6),
                c(0x8b, 0xe9, 0xfd),
                c(0xf8, 0xf8, 0xf2),
                c(0x62, 0x72, 0xa4),
                c(0xff, 0x6e, 0x6e),
                c(0x69, 0xff, 0x94),
                c(0xff, 0xff, 0xa5),
                c(0xd6, 0xac, 0xff),
                c(0xff, 0x92, 0xdf),
                c(0xa4, 0xff, 0xff),
                c(0xff, 0xff, 0xff),
            ],
        },
    },
    TerminalColorScheme {
        id: "nord",
        name: "Nord",
        palette: TerminalPalette {
            foreground: c(0xd8, 0xde, 0xe9),
            background: c(0x2e, 0x34, 0x40),
            cursor: c(0xd8, 0xde, 0xe9),
            selection: c(0x43, 0x4c, 0x5e),
            ansi: [
                c(0x3b, 0x42, 0x52),
                c(0xbf, 0x61, 0x6a),
                c(0xa3, 0xbe, 0x8c),
                c(0xeb, 0xcb, 0x8b),
                c(0x81, 0xa1, 0xc1),
                c(0xb4, 0x8e, 0xad),
                c(0x88, 0xc0, 0xd0),
                c(0xe5, 0xe9, 0xf0),
                c(0x4c, 0x56, 0x6a),
                c(0xbf, 0x61, 0x6a),
                c(0xa3, 0xbe, 0x8c),
                c(0xeb, 0xcb, 0x8b),
                c(0x81, 0xa1, 0xc1),
                c(0xb4, 0x8e, 0xad),
                c(0x8f, 0xbc, 0xbb),
                c(0xec, 0xef, 0xf4),
            ],
        },
    },
    TerminalColorScheme {
        id: "gruvbox-dark",
        name: "Gruvbox Dark",
        palette: TerminalPalette {
            foreground: c(0xeb, 0xdb, 0xb2),
            background: c(0x28, 0x28, 0x28),
            cursor: c(0xeb, 0xdb, 0xb2),
            selection: c(0x50, 0x49, 0x45),
            ansi: [
                c(0x28, 0x28, 0x28),
                c(0xcc, 0x24, 0x1d),
                c(0x98, 0x97, 0x1a),
                c(0xd7, 0x99, 0x21),
                c(0x45, 0x85, 0x88),
                c(0xb1, 0x62, 0x86),
                c(0x68, 0x9d, 0x6a),
                c(0xa8, 0x99, 0x84),
                c(0x92, 0x83, 0x74),
                c(0xfb, 0x49, 0x34),
                c(0xb8, 0xbb, 0x26),
                c(0xfa, 0xbd, 0x2f),
                c(0x83, 0xa5, 0x98),
                c(0xd3, 0x86, 0x9b),
                c(0x8e, 0xc0, 0x7c),
                c(0xeb, 0xdb, 0xb2),
            ],
        },
    },
    TerminalColorScheme {
        id: "solarized-dark",
        name: "Solarized Dark",
        palette: TerminalPalette {
            foreground: c(0x83, 0x94, 0x96),
            background: c(0x00, 0x2b, 0x36),
            cursor: c(0x93, 0xa1, 0xa1),
            selection: c(0x07, 0x36, 0x42),
            ansi: [
                c(0x07, 0x36, 0x42),
                c(0xdc, 0x32, 0x2f),
                c(0x85, 0x99, 0x00),
                c(0xb5, 0x89, 0x00),
                c(0x26, 0x8b, 0xd2),
                c(0xd3, 0x36, 0x82),
                c(0x2a, 0xa1, 0x98),
                c(0xee, 0xe8, 0xd5),
                c(0x00, 0x2b, 0x36),
                c(0xcb, 0x4b, 0x16),
                c(0x58, 0x6e, 0x75),
                c(0x65, 0x7b, 0x83),
                c(0x83, 0x94, 0x96),
                c(0x6c, 0x71, 0xc4),
                c(0x93, 0xa1, 0xa1),
                c(0xfd, 0xf6, 0xe3),
            ],
        },
    },
    TerminalColorScheme {
        id: "solarized-light",
        name: "Solarized Light",
        palette: TerminalPalette {
            foreground: c(0x65, 0x7b, 0x83),
            background: c(0xfd, 0xf6, 0xe3),
            cursor: c(0x58, 0x6e, 0x75),
            selection: c(0xee, 0xe8, 0xd5),
            ansi: [
                c(0x07, 0x36, 0x42),
                c(0xdc, 0x32, 0x2f),
                c(0x85, 0x99, 0x00),
                c(0xb5, 0x89, 0x00),
                c(0x26, 0x8b, 0xd2),
                c(0xd3, 0x36, 0x82),
                c(0x2a, 0xa1, 0x98),
                c(0xee, 0xe8, 0xd5),
                c(0x00, 0x2b, 0x36),
                c(0xcb, 0x4b, 0x16),
                c(0x58, 0x6e, 0x75),
                c(0x65, 0x7b, 0x83),
                c(0x83, 0x94, 0x96),
                c(0x6c, 0x71, 0xc4),
                c(0x93, 0xa1, 0xa1),
                c(0xfd, 0xf6, 0xe3),
            ],
        },
    },
    TerminalColorScheme {
        id: "one-dark",
        name: "One Dark",
        palette: TerminalPalette {
            foreground: c(0xab, 0xb2, 0xbf),
            background: c(0x28, 0x2c, 0x34),
            cursor: c(0x52, 0x8b, 0xff),
            selection: c(0x3e, 0x44, 0x51),
            ansi: [
                c(0x28, 0x2c, 0x34),
                c(0xe0, 0x6c, 0x75),
                c(0x98, 0xc3, 0x79),
                c(0xe5, 0xc0, 0x7b),
                c(0x61, 0xaf, 0xef),
                c(0xc6, 0x78, 0xdd),
                c(0x56, 0xb6, 0xc2),
                c(0xab, 0xb2, 0xbf),
                c(0x5c, 0x63, 0x70),
                c(0xe0, 0x6c, 0x75),
                c(0x98, 0xc3, 0x79),
                c(0xe5, 0xc0, 0x7b),
                c(0x61, 0xaf, 0xef),
                c(0xc6, 0x78, 0xdd),
                c(0x56, 0xb6, 0xc2),
                c(0xff, 0xff, 0xff),
            ],
        },
    },
    TerminalColorScheme {
        id: "tokyo-night",
        name: "Tokyo Night",
        palette: TerminalPalette {
            foreground: c(0xc0, 0xca, 0xf5),
            background: c(0x1a, 0x1b, 0x26),
            cursor: c(0xc0, 0xca, 0xf5),
            selection: c(0x33, 0x46, 0x7c),
            ansi: [
                c(0x15, 0x16, 0x1e),
                c(0xf7, 0x76, 0x8e),
                c(0x9e, 0xce, 0x6a),
                c(0xe0, 0xaf, 0x68),
                c(0x7a, 0xa2, 0xf7),
                c(0xbb, 0x9a, 0xf7),
                c(0x7d, 0xcf, 0xff),
                c(0xa9, 0xb1, 0xd6),
                c(0x41, 0x48, 0x68),
                c(0xf7, 0x76, 0x8e),
                c(0x9e, 0xce, 0x6a),
                c(0xe0, 0xaf, 0x68),
                c(0x7a, 0xa2, 0xf7),
                c(0xbb, 0x9a, 0xf7),
                c(0x7d, 0xcf, 0xff),
                c(0xc0, 0xca, 0xf5),
            ],
        },
    },
];

/// Looks up a built-in scheme by its stable [`TerminalColorScheme::id`].
#[must_use]
pub fn color_scheme(id: &str) -> Option<&'static TerminalColorScheme> {
    COLOR_SCHEMES.iter().find(|scheme| scheme.id == id)
}

/// Short alias for `TerminalColor::rgb` in the table above.
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
    fn color_scheme_catalog_is_complete_and_unique() {
        assert!(
            (6..=8).contains(&COLOR_SCHEMES.len()),
            "expected 6-8 built-in schemes, got {}",
            COLOR_SCHEMES.len()
        );
        for (index, scheme) in COLOR_SCHEMES.iter().enumerate() {
            assert!(!scheme.id.is_empty());
            assert!(!scheme.name.is_empty());
            assert!(
                COLOR_SCHEMES[..index]
                    .iter()
                    .all(|earlier| earlier.id != scheme.id),
                "duplicate scheme id {}",
                scheme.id
            );
            // A usable scheme must not paint text in the background color.
            assert_ne!(
                scheme.palette.foreground, scheme.palette.background,
                "scheme {} has an invisible foreground",
                scheme.id
            );
        }

        let default = color_scheme("yshell-default").expect("default scheme is listed");
        assert_eq!(default.name, "YShell Default");
        assert_eq!(default.palette, TerminalPalette::default());
        assert_eq!(color_scheme("dracula").map(|s| s.name), Some("Dracula"));
        assert!(color_scheme("not-a-scheme").is_none());
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
