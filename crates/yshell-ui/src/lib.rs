//! UI view-model and event binding definitions for YShell.
//!
//! This crate intentionally keeps UI state separate from SSH, SFTP, terminal,
//! configuration, and secret-handling implementation details.

pub mod models;
pub mod view_commands;
pub mod view_events;

pub use models::{
    LoggingModel, QuickCommandModel, SearchModel, SessionTreeModel, SftpModel, StatusModel,
    TabModel, TunnelModel,
};
pub use view_commands::{QuickConnectRequest, SearchScopeCommand, SplitAxis, ViewCommand};
pub use view_events::{ConnectionState, ViewEvent};

/// Top-level state projected into the Slint shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppViewModel {
    pub sessions: SessionTreeModel,
    pub tabs: TabModel,
    pub sftp: SftpModel,
    pub tunnels: TunnelModel,
    pub quick_commands: QuickCommandModel,
    pub logging: LoggingModel,
    pub search: SearchModel,
    pub status: StatusModel,
    pub status_message: String,
}

impl Default for AppViewModel {
    fn default() -> Self {
        Self {
            sessions: SessionTreeModel::placeholder(),
            tabs: TabModel::placeholder(),
            sftp: SftpModel::placeholder(),
            tunnels: TunnelModel::placeholder(),
            quick_commands: QuickCommandModel::placeholder(),
            logging: LoggingModel::default(),
            search: SearchModel::default(),
            status: StatusModel::default(),
            status_message: "Ready".to_owned(),
        }
    }
}
