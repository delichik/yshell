//! Terminal model skeleton independent from Slint rendering.

pub mod cell;
pub mod color;
pub mod grid;
pub mod input;
pub mod parser;
pub mod search;
pub mod selection;

pub use cell::TerminalCell;
pub use grid::TerminalGrid;
pub use input::{ControlKey, TerminalInputEvent};
