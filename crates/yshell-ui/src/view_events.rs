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
    SftpListingUpdated {
        session_id: String,
        path: String,
    },
    TransferProgressChanged {
        transfer_id: String,
        bytes_done: u64,
    },
    TunnelStateChanged {
        tunnel_id: String,
        state: String,
    },
    LogEntryAdded {
        level: String,
        message: String,
    },
    SearchResultsChanged {
        match_count: usize,
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
