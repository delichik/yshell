//! In-memory session registry used by early tests and future services.

use std::collections::HashMap;

use crate::{SessionHandle, SessionId, SessionState};

/// Tracks active session handles.
#[derive(Debug, Default)]
pub struct SessionManager {
    sessions: HashMap<SessionId, SessionHandle>,
}

impl SessionManager {
    /// Creates an empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an idle session if it does not already exist.
    pub fn create_session(&mut self, session_id: SessionId) -> SessionHandle {
        self.sessions
            .entry(session_id.clone())
            .or_insert_with(|| SessionHandle::new(session_id))
            .clone()
    }

    /// Returns a session handle by id.
    #[must_use]
    pub fn get(&self, session_id: &SessionId) -> Option<&SessionHandle> {
        self.sessions.get(session_id)
    }

    /// Updates a registered session state and returns whether it existed.
    pub fn set_state(&mut self, session_id: &SessionId, state: SessionState) -> bool {
        self.sessions
            .get_mut(session_id)
            .map(|handle| handle.set_state(state))
            .is_some()
    }

    /// Returns the number of tracked sessions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Returns true when no sessions are tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_updates_session() {
        let mut manager = SessionManager::new();
        let session_id = SessionId::new("session-1");

        let handle = manager.create_session(session_id.clone());

        assert_eq!(handle.state(), SessionState::Idle);
        assert_eq!(manager.len(), 1);
        assert!(manager.set_state(&session_id, SessionState::Connected));
        assert_eq!(
            manager.get(&session_id).map(SessionHandle::state),
            Some(SessionState::Connected)
        );
    }
}
