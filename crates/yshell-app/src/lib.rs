//! YShell application library shared by role binaries.
//!
//! Role processes (`yshell`, `yshell-shell`, `yshell-session-worker`,
//! `yshell-mcp-daemon`) are thin `src/bin/*` stubs that call into this crate.
//! `yshell` is the Host entry; each role has its own small entry module so the
//! linker can drop the GUI/Slint code from the Host and worker binaries
//! (see `docs/prototypes/process-architecture.md`).

// The split-process scaffold still exposes internal modules so role stubs and
// integration tests can reach them; narrowing this surface is tracked as
// follow-up work rather than blocking the process-architecture delivery.
#![allow(private_interfaces)]

pub mod app_state;
pub mod bootstrap;
pub mod cli;
pub mod connect_jobs;
pub mod dialog_events;
pub mod dialog_windows;
pub mod error;
pub mod host;
pub mod launcher;
pub mod local_fs;
pub mod logging_setup;
pub mod mcp_server;
pub mod remote_dialog_wire;
pub mod remote_dialogs;
pub mod remote_shell;
pub mod role_mcp;
pub mod role_shell;
pub mod role_worker;
pub mod runtime;
pub mod session_runtime;
pub mod sftp_jobs;
pub mod sftp_view;
pub mod win_chrome;
pub mod windows;

pub use error::{AppError, AppResult};
pub use launcher::run;
pub use role_mcp::run as run_mcp_daemon_role;
pub use role_shell::run as run_shell_role;
pub use role_worker::run as run_session_worker_role;
