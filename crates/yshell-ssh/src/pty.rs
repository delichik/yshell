//! PTY request configuration.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyConfig {
    pub term: String,
    pub size: PtySize,
    pub modes: Vec<(String, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub columns: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            term: "xterm-256color".to_owned(),
            size: PtySize::default(),
            modes: Vec::new(),
        }
    }
}

impl Default for PtySize {
    fn default() -> Self {
        Self {
            columns: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}
