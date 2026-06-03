//! Command dispatcher for UI-to-core session/workspace commands.

use std::{error::Error, fmt};

use crate::{PaneId, SessionEvent, SessionId, SessionManager, SessionState, SplitDirection, TabId};

/// Commands accepted by the core layer from UI/application code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCommand {
    OpenSession {
        tab_id: TabId,
        session_id: SessionId,
    },
    CloseSession {
        tab_id: TabId,
    },
    SetSessionState {
        session_id: SessionId,
        state: SessionState,
    },
    SendTerminalInput {
        session_id: SessionId,
        bytes: Vec<u8>,
    },
    ResizeTerminal {
        session_id: SessionId,
        columns: u16,
        rows: u16,
    },
    SplitPane {
        tab_id: TabId,
        pane_id: PaneId,
        direction: SplitDirection,
    },
    ClosePane {
        tab_id: TabId,
        pane_id: PaneId,
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

/// Minimal dispatcher trait; concrete async implementations can wrap this later.
pub trait CommandDispatcher {
    /// Dispatches one session command and returns synchronously emitted events.
    fn dispatch(
        &mut self,
        command: SessionCommand,
    ) -> Result<Vec<SessionEvent>, CommandDispatchError>;
}

/// In-memory dispatcher that updates `SessionManager` and records passthrough commands.
#[derive(Debug, Default)]
pub struct CoreCommandDispatcher {
    manager: SessionManager,
    terminal_inputs: Vec<(SessionId, Vec<u8>)>,
    terminal_resizes: Vec<(SessionId, u16, u16)>,
}

impl CoreCommandDispatcher {
    /// Creates an empty dispatcher.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the underlying session manager.
    #[must_use]
    pub const fn manager(&self) -> &SessionManager {
        &self.manager
    }

    /// Returns mutable access to the underlying session manager.
    pub fn manager_mut(&mut self) -> &mut SessionManager {
        &mut self.manager
    }

    /// Returns captured terminal input commands.
    #[must_use]
    pub fn terminal_inputs(&self) -> &[(SessionId, Vec<u8>)] {
        &self.terminal_inputs
    }

    /// Returns captured terminal resize commands.
    #[must_use]
    pub fn terminal_resizes(&self) -> &[(SessionId, u16, u16)] {
        &self.terminal_resizes
    }
}

impl CommandDispatcher for CoreCommandDispatcher {
    fn dispatch(
        &mut self,
        command: SessionCommand,
    ) -> Result<Vec<SessionEvent>, CommandDispatchError> {
        match command {
            SessionCommand::OpenSession { tab_id, session_id } => {
                Ok(vec![self.manager.open_tab(tab_id, session_id)])
            }
            SessionCommand::CloseSession { tab_id } => self
                .manager
                .close_tab(&tab_id)
                .map(|event| vec![event])
                .ok_or_else(|| CommandDispatchError::new(format!("tab {tab_id} is not open"))),
            SessionCommand::SetSessionState { session_id, state } => self
                .manager
                .set_state(&session_id, state)
                .map(|event| vec![event])
                .ok_or_else(|| {
                    CommandDispatchError::new(format!("session {session_id} is not open"))
                }),
            SessionCommand::SendTerminalInput { session_id, bytes } => {
                if self.manager.get(&session_id).is_none() {
                    return Err(CommandDispatchError::new(format!(
                        "session {session_id} is not open"
                    )));
                }
                self.terminal_inputs.push((session_id, bytes));
                Ok(Vec::new())
            }
            SessionCommand::ResizeTerminal {
                session_id,
                columns,
                rows,
            } => {
                if self.manager.get(&session_id).is_none() {
                    return Err(CommandDispatchError::new(format!(
                        "session {session_id} is not open"
                    )));
                }
                self.terminal_resizes.push((session_id, columns, rows));
                Ok(Vec::new())
            }
            SessionCommand::SplitPane {
                tab_id,
                pane_id,
                direction,
            } => {
                let tab = self.manager.tab_mut(&tab_id).ok_or_else(|| {
                    CommandDispatchError::new(format!("tab {tab_id} is not open"))
                })?;
                let session_id = tab.session_id().clone();
                tab.layout_mut()
                    .split(pane_id, direction, 0.5, Some(session_id))
                    .map_err(|error| CommandDispatchError::new(error.to_string()))?;
                Ok(Vec::new())
            }
            SessionCommand::ClosePane { tab_id, pane_id } => {
                let tab = self.manager.tab_mut(&tab_id).ok_or_else(|| {
                    CommandDispatchError::new(format!("tab {tab_id} is not open"))
                })?;
                tab.layout_mut()
                    .close(pane_id)
                    .map_err(|error| CommandDispatchError::new(error.to_string()))?;
                Ok(Vec::new())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_session_and_captures_terminal_input() {
        let mut dispatcher = CoreCommandDispatcher::new();
        let tab_id = TabId::new("tab-1");
        let session_id = SessionId::new("session-1");

        let events = dispatcher
            .dispatch(SessionCommand::OpenSession {
                tab_id,
                session_id: session_id.clone(),
            })
            .expect("open");
        assert_eq!(events.len(), 1);

        dispatcher
            .dispatch(SessionCommand::SendTerminalInput {
                session_id: session_id.clone(),
                bytes: b"pwd\n".to_vec(),
            })
            .expect("input");

        assert_eq!(
            dispatcher.terminal_inputs(),
            &[(session_id, b"pwd\n".to_vec())]
        );
    }
}
