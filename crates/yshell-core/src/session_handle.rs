//! Lightweight session handle and state model.

use crate::SessionId;

/// High-level lifecycle state for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

/// Public handle for a managed session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHandle {
    id: SessionId,
    state: SessionState,
    active_tab: Option<crate::TabId>,
}

impl SessionHandle {
    /// Creates an idle session handle.
    #[must_use]
    pub const fn new(id: SessionId) -> Self {
        Self {
            id,
            state: SessionState::Idle,
            active_tab: None,
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

    /// Returns the active tab id, if any.
    #[must_use]
    pub const fn active_tab(&self) -> Option<&crate::TabId> {
        self.active_tab.as_ref()
    }

    /// Updates the lifecycle state.
    pub const fn set_state(&mut self, state: SessionState) {
        self.state = state;
    }

    /// Associates the session with a tab.
    pub fn set_active_tab(&mut self, tab_id: Option<crate::TabId>) {
        self.active_tab = tab_id;
    }
}
