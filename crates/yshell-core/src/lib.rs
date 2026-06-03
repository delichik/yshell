//! Core domain coordination primitives for `YShell`.
//!
//! Milestone 0 intentionally keeps these types small. They establish stable
//! module boundaries for later SSH, SFTP, terminal, and UI integration work.

pub mod command_dispatcher;
pub mod session_event;
pub mod session_handle;
pub mod session_manager;
pub mod task_supervisor;

pub use command_dispatcher::{CommandDispatchError, CommandDispatcher, SessionCommand};
pub use session_event::{SessionEvent, SessionId};
pub use session_handle::{SessionHandle, SessionState};
pub use session_manager::SessionManager;
pub use task_supervisor::{TaskId, TaskSupervisor};
