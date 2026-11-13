//! Host process facade (D38).
//!
//! See [`docs/prototypes/process-architecture.md`].

pub mod api;
pub mod bridge;
mod dialog_dispatch;
pub mod ensure;
pub mod job;
pub mod lock;
pub mod roles;
pub mod run_state;
pub mod runtime_client;
pub mod runtime_dispatch;
pub mod service;
pub mod session_ipc;
pub mod shell_inbox;
pub mod shell_ipc;
pub mod shell_jobs;
pub mod spawn;

#[allow(unused_imports)]
pub use api::{HostCommand, HostEvent, ProcessRole};
pub use bridge::try_notify_existing_instance;
#[allow(unused_imports)] // used by ensure_host_process; re-export for tests / tooling
pub use ensure::spawn_host;
pub use ensure::{connect_shell_client, ensure_host_process, inprocess_host_requested};
pub use roles::{
    role_exe, role_exe_name, run_host_headless, run_mcp_daemon, run_session_worker, McpDaemonArgs,
    SessionWorkerArgs,
};
#[allow(unused_imports)] // re-exported for callers / future IPC wiring
pub use run_state::{current_pid, process_is_alive, RunState, RUN_STATE_FILE_NAME};
pub use runtime_client::AppRuntimeClient;
pub use service::HostService;
pub use shell_inbox::{ShellCommand, ShellInbox};
#[allow(unused_imports)]
pub use spawn::spawn_mcp_daemon;
