//! Spawn helpers for out-of-process SessionWorker / McpDaemon children.

use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::error::{AppError, AppResult};

use super::run_state::current_pid;

/// Spawn `yshell --session-worker …` as a child of the current Host.
#[allow(dead_code)] // exercised when Host flips sessions out-of-process (D38 Phase 2+)
pub fn spawn_session_worker(
    config_dir: &Path,
    session_id: &str,
    generation: u64,
) -> AppResult<Child> {
    let exe = std::env::current_exe().map_err(AppError::from_error)?;
    Command::new(exe)
        .arg("--config-dir")
        .arg(config_dir)
        .arg("--session-worker")
        .arg(session_id)
        .arg("--generation")
        .arg(generation.to_string())
        .arg("--parent-pid")
        .arg(current_pid().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(AppError::from_error)
}

/// Spawn `yshell --mcp-daemon …` as a child of the current Host.
#[allow(dead_code)] // exercised when Host flips MCP out-of-process (D38 Phase 3+)
pub fn spawn_mcp_daemon(
    config_dir: &Path,
    bind_host: &str,
    port: u16,
    token: &str,
    generation: u64,
) -> AppResult<Child> {
    let exe = std::env::current_exe().map_err(AppError::from_error)?;
    Command::new(exe)
        .arg("--config-dir")
        .arg(config_dir)
        .arg("--mcp-daemon")
        .arg("--mcp-bind")
        .arg(bind_host)
        .arg("--mcp-port")
        .arg(port.to_string())
        .arg("--mcp-token")
        .arg(token)
        .arg("--generation")
        .arg(generation.to_string())
        .arg("--parent-pid")
        .arg(current_pid().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(AppError::from_error)
}
