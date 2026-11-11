//! Host process facade (D38).
//!
//! See [`docs/prototypes/process-architecture.md`].

pub mod api;
pub mod job;
pub mod roles;
pub mod run_state;
pub mod service;
pub mod spawn;

#[allow(unused_imports)]
pub use api::{HostCommand, HostEvent, ProcessRole};
pub use roles::{
    run_host_headless, run_mcp_daemon, run_session_worker, McpDaemonArgs, SessionWorkerArgs,
};
#[allow(unused_imports)] // re-exported for callers / future IPC wiring
pub use run_state::{current_pid, process_is_alive, RunState, RUN_STATE_FILE_NAME};
pub use service::HostService;
#[allow(unused_imports)]
pub use spawn::{spawn_mcp_daemon, spawn_session_worker};
