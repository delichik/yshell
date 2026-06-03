//! Commands emitted by the UI and translated by the app shell.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewCommand {
    OpenQuickConnect(QuickConnectRequest),
    OpenSession {
        session_id: String,
    },
    CloseTab {
        tab_id: String,
    },
    ReconnectSession {
        session_id: String,
    },
    OpenSftp {
        session_id: String,
    },
    RefreshSftp {
        session_id: String,
        path: String,
    },
    SftpTransfer {
        session_id: String,
        transfer_id: String,
    },
    SftpCancelTransfer {
        transfer_id: String,
    },
    SftpRetryTransfer {
        transfer_id: String,
    },
    ToggleTunnelPanel,
    StartTunnel {
        tunnel_id: String,
    },
    StopTunnel {
        tunnel_id: String,
    },
    RunQuickCommand {
        command_id: String,
    },
    OpenSearch {
        scope: SearchScopeCommand,
    },
    UpdateSearch {
        query: String,
    },
    ToggleLoggingPanel,
    ToggleQuickCommands,
    UpdateWorkspaceSplit {
        axis: SplitAxis,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickConnectRequest {
    pub raw_input: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScopeCommand {
    ActiveTerminal,
    AllTabs,
    SftpListing,
}
