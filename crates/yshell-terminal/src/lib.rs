//! Terminal model independent from Slint rendering.

pub mod cell;
pub mod color;
pub mod grid;
pub mod input;
pub mod parser;
pub mod search;
pub mod selection;

pub use cell::{CellStyle, TerminalCell};
pub use color::TerminalColor;
pub use grid::TerminalGrid;
pub use input::{ControlKey, TerminalInputEvent};
pub use parser::TerminalParser;
pub use search::{SearchMatch, SearchQuery};
pub use selection::{GridPoint, SelectionRange};
