//! Commands from a second `yshell` launch into the running Shell/Host.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellCommand {
    /// Second-instance launch: open (or focus) a Shell window.
    OpenWindow,
}

/// Thread-safe queue drained on the UI tick.
#[derive(Debug, Clone, Default)]
pub struct ShellInbox {
    inner: Arc<Mutex<VecDeque<ShellCommand>>>,
}

impl ShellInbox {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, command: ShellCommand) {
        if let Ok(mut queue) = self.inner.lock() {
            queue.push_back(command);
        }
    }

    pub fn try_pop(&self) -> Option<ShellCommand> {
        self.inner.lock().ok()?.pop_front()
    }
}
