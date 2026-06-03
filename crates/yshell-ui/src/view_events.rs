//! Events projected from app/core services into the UI.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewEvent {
    SessionStateChanged {
        session_id: String,
        state: ConnectionState,
    },
    TerminalOutputAvailable {
        session_id: String,
    },
    StatusMessageChanged(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Error,
}
