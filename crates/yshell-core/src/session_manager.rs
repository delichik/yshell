//! In-memory session/tab registry used by core services.

use std::collections::HashMap;

use crate::{SessionEvent, SessionHandle, SessionId, SessionState, TabId, WorkspaceLayout};

/// Tracks active session handles and tab layouts.
#[derive(Debug, Default)]
pub struct SessionManager {
    sessions: HashMap<SessionId, SessionHandle>,
    tabs: HashMap<TabId, ManagedTab>,
    active_tab: Option<TabId>,
}

/// Runtime state for one tab.
#[derive(Debug, Clone, PartialEq)]
pub struct ManagedTab {
    id: TabId,
    session_id: SessionId,
    layout: WorkspaceLayout,
}

impl ManagedTab {
    /// Returns the tab id.
    #[must_use]
    pub const fn id(&self) -> &TabId {
        &self.id
    }

    /// Returns the primary session id for this tab.
    #[must_use]
    pub const fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Returns the workspace layout for this tab.
    #[must_use]
    pub const fn layout(&self) -> &WorkspaceLayout {
        &self.layout
    }

    /// Returns a mutable workspace layout for this tab.
    pub fn layout_mut(&mut self) -> &mut WorkspaceLayout {
        &mut self.layout
    }
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

    /// Opens a tab for a session and returns the emitted event.
    pub fn open_tab(&mut self, tab_id: TabId, session_id: SessionId) -> SessionEvent {
        self.create_session(session_id.clone());
        if let Some(handle) = self.sessions.get_mut(&session_id) {
            handle.set_active_tab(Some(tab_id.clone()));
        }
        self.tabs
            .entry(tab_id.clone())
            .or_insert_with(|| ManagedTab {
                id: tab_id.clone(),
                session_id: session_id.clone(),
                layout: WorkspaceLayout::new(),
            });
        self.active_tab = Some(tab_id.clone());
        SessionEvent::TabOpened { tab_id, session_id }
    }

    /// Closes a tab and disconnects the associated session state.
    pub fn close_tab(&mut self, tab_id: &TabId) -> Option<SessionEvent> {
        let tab = self.tabs.remove(tab_id)?;
        if let Some(handle) = self.sessions.get_mut(&tab.session_id) {
            handle.set_active_tab(None);
            handle.set_state(SessionState::Disconnected);
        }
        if self.active_tab.as_ref() == Some(tab_id) {
            self.active_tab = self.tabs.keys().next().cloned();
        }
        Some(SessionEvent::TabClosed { tab_id: tab.id })
    }

    /// Returns a session handle by id.
    #[must_use]
    pub fn get(&self, session_id: &SessionId) -> Option<&SessionHandle> {
        self.sessions.get(session_id)
    }

    /// Returns a tab by id.
    #[must_use]
    pub fn tab(&self, tab_id: &TabId) -> Option<&ManagedTab> {
        self.tabs.get(tab_id)
    }

    /// Returns a mutable tab by id.
    pub fn tab_mut(&mut self, tab_id: &TabId) -> Option<&mut ManagedTab> {
        self.tabs.get_mut(tab_id)
    }

    /// Returns active tab id.
    #[must_use]
    pub fn active_tab(&self) -> Option<&TabId> {
        self.active_tab.as_ref()
    }

    /// Updates a registered session state and returns the emitted event if it existed.
    pub fn set_state(
        &mut self,
        session_id: &SessionId,
        state: SessionState,
    ) -> Option<SessionEvent> {
        self.sessions.get_mut(session_id).map(|handle| {
            handle.set_state(state);
            SessionEvent::StateChanged {
                session_id: session_id.clone(),
                state,
            }
        })
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

    /// Returns the number of open tabs.
    #[must_use]
    pub fn tab_len(&self) -> usize {
        self.tabs.len()
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
        assert_eq!(
            manager.set_state(&session_id, SessionState::Connected),
            Some(SessionEvent::StateChanged {
                session_id: session_id.clone(),
                state: SessionState::Connected,
            })
        );
        assert_eq!(
            manager.get(&session_id).map(SessionHandle::state),
            Some(SessionState::Connected)
        );
    }

    #[test]
    fn opens_and_closes_tab() {
        let mut manager = SessionManager::new();
        let tab_id = TabId::new("tab-1");
        let session_id = SessionId::new("session-1");

        assert_eq!(
            manager.open_tab(tab_id.clone(), session_id.clone()),
            SessionEvent::TabOpened {
                tab_id: tab_id.clone(),
                session_id: session_id.clone(),
            }
        );

        assert_eq!(manager.active_tab(), Some(&tab_id));
        assert_eq!(manager.tab_len(), 1);
        assert_eq!(
            manager.close_tab(&tab_id),
            Some(SessionEvent::TabClosed { tab_id })
        );
        assert_eq!(
            manager.get(&session_id).map(SessionHandle::state),
            Some(SessionState::Disconnected)
        );
    }
}
