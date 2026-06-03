//! Lightweight session handle and state model.

use crate::SessionId;

/// High-level lifecycle state for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Session exists but has not started connecting.
    Idle,
    /// Connection establishment is in progress.
    Connecting,
    /// Session is connected and ready.
    Connected,
    /// Session has disconnected normally or due to an error.
    Disconnected,
}

/// Public handle for a managed session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHandle {
    id: SessionId,
    state: SessionState,
}

impl SessionHandle {
    /// Creates an idle session handle.
    #[must_use]
    pub const fn new(id: SessionId) -> Self {
        Self {
            id,
            state: SessionState::Idle,
        }
    }

    /// Returns the session identifier.
    #[must_use]
    pub const fn id(&self) -> &SessionId {
        &self.id
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> SessionState {
        self.state
    }

    /// Updates the lifecycle state.
    pub const fn set_state(&mut self, state: SessionState) {
        self.state = state;
    }
}
