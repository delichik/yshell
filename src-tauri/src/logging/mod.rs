//! Session logging boundary.
//!
//! Runtime output streams should be tee'd into log writers from this module so
//! UI state never owns large terminal buffers.
