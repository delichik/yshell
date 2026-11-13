//! YShell application library shared by role binaries.
//!
//! Role processes (`yshell`, `yshell-host`, `yshell-shell`,
//! `yshell-session-worker`, `yshell-mcp-daemon`) are thin `src/bin/*` stubs
//! that call into this crate. See `docs/prototypes/process-architecture.md`.

// The split-process scaffold still exposes internal modules so role stubs and
// integration tests can reach them; narrowing this surface is tracked as
// follow-up work rather than blocking the process-architecture delivery.
#![allow(private_interfaces)]

pub mod app_state;
pub mod bootstrap;
pub mod cli;
pub mod connect_jobs;
pub mod dialog_windows;
pub mod error;
pub mod host;
pub mod local_fs;
pub mod mcp_server;
pub mod remote_dialog_wire;
pub mod remote_dialogs;
pub mod remote_shell;
pub mod runtime;
pub mod session_runtime;
pub mod sftp_jobs;
pub mod sftp_view;
pub mod win_chrome;
pub mod windows;

pub use cli::{run, run_with_role};
pub use error::{AppError, AppResult};
