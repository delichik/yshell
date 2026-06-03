//! Core domain coordination primitives for `YShell`.
//!
//! This crate owns UI-independent session/tab state, workspace split models,
//! command dispatching, and task supervision boundaries.

pub mod command_dispatcher;
pub mod session_event;
pub mod session_handle;
pub mod session_manager;
pub mod task_supervisor;
pub mod workspace_layout;

pub use command_dispatcher::{
    CommandDispatchError, CommandDispatcher, CoreCommandDispatcher, SessionCommand,
};
pub use session_event::{SessionEvent, SessionId, TabId};
pub use session_handle::{SessionHandle, SessionState};
pub use session_manager::{ManagedTab, SessionManager};
pub use task_supervisor::{TaskId, TaskSupervisor};
pub use workspace_layout::{
    PaneId, SplitDirection, WorkspaceLayout, WorkspaceLayoutError, WorkspaceNode,
};
