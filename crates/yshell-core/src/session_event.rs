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

/// Stable identifier for an application tab.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(String);

impl TabId {
    /// Creates a tab identifier from a caller-provided value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string representation of this identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Session lifecycle, tab, workspace, and IO events consumed by UI/view-model crates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    TabOpened {
        tab_id: TabId,
        session_id: SessionId,
    },
    TabClosed {
        tab_id: TabId,
    },
    StateChanged {
        session_id: SessionId,
        state: crate::SessionState,
    },
    Connecting {
        session_id: SessionId,
    },
    Connected {
        session_id: SessionId,
    },
    Disconnected {
        session_id: SessionId,
        reason: String,
    },
    Error {
        session_id: SessionId,
        error: String,
    },
    TerminalOutput {
        session_id: SessionId,
        bytes: Vec<u8>,
    },
}
