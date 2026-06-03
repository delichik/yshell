//! Placeholder command dispatcher for UI-to-core commands.

use std::{error::Error, fmt};

use crate::SessionId;

/// Commands accepted by the core layer from UI/application code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCommand {
    /// Open a saved or quick-connect session.
    OpenSession { session_id: SessionId },
    /// Close a session/tab.
    CloseSession { session_id: SessionId },
    /// Send terminal input bytes to a connected session.
    SendTerminalInput {
        session_id: SessionId,
        bytes: Vec<u8>,
    },
    /// Resize the remote PTY.
    ResizeTerminal {
        session_id: SessionId,
        columns: u16,
        rows: u16,
    },
}

/// Error returned when a command cannot be dispatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandDispatchError {
    message: String,
}

impl CommandDispatchError {
    /// Creates a dispatch error with a human-readable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CommandDispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for CommandDispatchError {}

/// Minimal dispatcher trait; concrete async implementations come later.
pub trait CommandDispatcher {
    /// Dispatches one session command.
    fn dispatch(&mut self, command: SessionCommand) -> Result<(), CommandDispatchError>;
}
