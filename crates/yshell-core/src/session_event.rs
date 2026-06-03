//! Events emitted by core services toward the UI layer.

use std::fmt;

/// Stable identifier for a logical shell/SFTP session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(String);

impl SessionId {
    /// Creates a session identifier from a caller-provided value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string representation of this identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Session lifecycle and IO events consumed by UI/view-model crates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// A session is beginning connection setup.
    Connecting { session_id: SessionId },
    /// A session is ready for terminal/SFTP work.
    Connected { session_id: SessionId },
    /// A session disconnected with a human-readable reason.
    Disconnected {
        session_id: SessionId,
        reason: String,
    },
    /// A session failed with a human-readable error.
    Error {
        session_id: SessionId,
        error: String,
    },
    /// Raw terminal bytes arrived from the remote shell.
    TerminalOutput {
        session_id: SessionId,
        bytes: Vec<u8>,
    },
}
