//! Terminal model independent from Slint rendering.

pub mod cell;
pub mod color;
pub mod grid;
pub mod input;
pub mod keys;
pub mod palette;
pub mod parser;
#[cfg(test)]
mod png;
pub mod render;
pub mod search;
pub mod selection;

pub use cell::{CellStyle, TerminalCell};
pub use color::TerminalColor;
pub use grid::{TerminalGrid, DEFAULT_SCROLLBACK_LINES, DEFAULT_SCROLLBACK_MAX_CELLS};
pub use input::{ControlKey, TerminalInputEvent};
pub use keys::encode_key;
pub use palette::{color_scheme, TerminalColorScheme, TerminalPalette, COLOR_SCHEMES};
pub use parser::TerminalParser;
pub use render::{
    FontFace, FontFaceError, FontFaceStyle, FontLineMetrics, FontSelection, GlyphSource,
    GlyphStyle, PrimaryFont, ResolvedFace, TerminalFontSet, TerminalFontStack, TerminalFrame,
    TerminalRenderer, TerminalSnapshot, DEFAULT_BACKGROUND, DEFAULT_CURSOR, DEFAULT_FONT_SIZE,
    DEFAULT_FOREGROUND, DEFAULT_SCALE_FACTOR,
};
pub use search::{SearchMatch, SearchQuery};
pub use selection::{GridPoint, SelectionRange};
