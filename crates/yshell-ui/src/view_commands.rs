//! Commands emitted by the UI and translated by the app shell.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewCommand {
    OpenQuickConnect(QuickConnectRequest),
    OpenSession { session_id: String },
    CloseTab { tab_id: String },
    ReconnectSession { session_id: String },
    OpenSftp { session_id: String },
    ToggleTunnelPanel,
    ToggleQuickCommands,
    UpdateWorkspaceSplit { axis: SplitAxis },
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
