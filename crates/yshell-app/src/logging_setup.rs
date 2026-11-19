//! Tracing/logging initialisation shared by the GUI Shell, the headless Host and
//! the Host-spawned role children.
//!
//! This lives outside `bootstrap.rs` so the Host/role binaries (which must stay
//! Slint-free) can initialise logging without linking the UI module.
//!
//! Every role process installs the same subscriber: an `yshell=debug,warn`
//! filter (overridable with `RUST_LOG`), stderr disabled (role children run
//! without a console), and one file writer under `{config_dir}/logs`. The writer
//! follows the Host-published control file (`yshell_logging::set_debug_log_control`),
//! so enabling debug logging in Settings applies to the Host, the GUI Shell and
//! every already-running SessionWorker / MCP daemon instead of only the process
//! that handled the toggle.

use std::path::Path;

use crate::error::{AppError, AppResult};
use crate::host::ProcessRole;
use crate::runtime::AppRuntime;

/// Debug log file of the GUI Shell process.
pub const SHELL_DEBUG_LOG_FILE_NAME: &str = "yshell-shell-debug.log";
/// Debug log file of SessionWorker processes.
///
/// Every worker appends to this one file (a worker writes its pid / session id
/// in the startup record before it connects), so a session's SSH details are
/// all in the SessionWorker log instead of mixed with the Host log.
pub const SESSION_WORKER_DEBUG_LOG_FILE_NAME: &str = "yshell-session-worker-debug.log";
/// Debug log file of the standalone MCP daemon.
pub const MCP_DAEMON_DEBUG_LOG_FILE_NAME: &str = "yshell-mcp-daemon-debug.log";

/// Debug log file name for `role` under `{config_dir}/logs`.
#[must_use]
pub fn debug_log_file_name(role: ProcessRole) -> &'static str {
    match role {
        // The Host keeps the historical file name so existing diagnostics /
        // support instructions still find it.
        ProcessRole::Host => yshell_logging::DEBUG_LOG_FILE_NAME,
        ProcessRole::Shell => SHELL_DEBUG_LOG_FILE_NAME,
        ProcessRole::SessionWorker => SESSION_WORKER_DEBUG_LOG_FILE_NAME,
        ProcessRole::McpDaemon => MCP_DAEMON_DEBUG_LOG_FILE_NAME,
    }
}

/// Tracing configuration for a role process.
fn logging_config(config_dir: &Path, role: ProcessRole) -> yshell_logging::LoggingConfig {
    let env_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "yshell=debug,warn".to_owned());
    yshell_logging::LoggingConfig::with_env_filter(env_filter)
        .with_ansi(false)
        .with_stderr(false)
        .with_file_name(debug_log_file_name(role))
        .with_file_directory(yshell_logging::debug_log_dir(config_dir))
        .with_debug_control_file(yshell_logging::debug_log_control_path(config_dir))
}

/// Installs the subscriber without emitting a startup event.
fn init_role_subscriber(config_dir: &Path, role: ProcessRole) -> AppResult<()> {
    match yshell_logging::init_tracing(&logging_config(config_dir, role)) {
        Ok(()) | Err(yshell_logging::InitLoggingError::AlreadyInitialized(_)) => Ok(()),
        Err(error) => Err(AppError::from_error(error)),
    }
}

/// Emits the per-process startup line that identifies the writer.
///
/// The event is formatted even while debug logging is disabled and then dropped
/// by the writer, so an enabled process always has one record naming its role,
/// pid and log file.
fn log_role_startup(config_dir: &Path, role: ProcessRole) {
    tracing::info!(
        role = role.as_str(),
        pid = std::process::id(),
        log_file = %yshell_logging::debug_log_path_named(config_dir, debug_log_file_name(role))
            .display(),
        "process logging initialized"
    );
}

/// Initialise file-backed debug logging for a role process that does **not**
/// own the configuration (Shell / SessionWorker / MCP daemon).
///
/// The Host has already published the control file before spawning these
/// children, and the writer keeps following it at runtime.
pub fn init_logging_for_role(config_dir: &Path, role: ProcessRole) -> AppResult<()> {
    init_role_subscriber(config_dir, role)?;
    log_role_startup(config_dir, role);
    Ok(())
}

/// Initialise logging for a process that owns an [`AppRuntime`] (the Host).
///
/// The Host is the only process allowed to publish the debug-logging control
/// file; doing it here also clears a stale file left by a previous Host run.
pub fn init_logging_for_runtime(runtime: &AppRuntime) -> AppResult<()> {
    init_role_subscriber(&runtime.config_dir, ProcessRole::Host)?;
    let enabled = runtime.debug_logging_enabled();
    if let Err(error) = yshell_logging::set_debug_log_control(&runtime.config_dir, enabled) {
        // Without the control file the other role processes cannot follow the
        // toggle, so surface the failure even if debug logging is off (stderr
        // of the detached Host is redirected to host-stderr.log).
        eprintln!("yshell: could not publish the debug logging control file: {error}");
        tracing::warn!(%error, "could not publish the debug logging control file");
    }
    yshell_logging::set_debug_logging_enabled(enabled);
    log_role_startup(&runtime.config_dir, ProcessRole::Host);
    if enabled {
        tracing::info!(
            path = %yshell_logging::debug_log_path(&runtime.config_dir).display(),
            "debug logging enabled"
        );
    }
    Ok(())
}
