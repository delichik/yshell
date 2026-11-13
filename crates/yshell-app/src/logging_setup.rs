//! Tracing/logging initialisation shared by the GUI and the headless Host.
//!
//! This lives outside `bootstrap.rs` so the Host/role binaries (which must stay
//! Slint-free) can initialise logging without linking the UI module.

use std::path::Path;

use crate::error::{AppError, AppResult};
use crate::runtime::AppRuntime;

/// Initialise file-backed logging for a process that owns an [`AppRuntime`].
pub fn init_logging_for_runtime(runtime: &AppRuntime) -> AppResult<()> {
    init_logging_for_shell(&runtime.config_dir, runtime.debug_logging_enabled())?;
    if runtime.debug_logging_enabled() {
        tracing::info!(
            path = %yshell_logging::debug_log_path(&runtime.config_dir).display(),
            "debug logging enabled"
        );
    }
    Ok(())
}

/// Shell-process logging initialisation that does **not** construct an
/// `AppRuntime` (the Host is the only process that loads/migrates/writes the
/// configuration; the Shell only uses the projected debug-logging flag).
pub fn init_logging_for_shell(config_dir: &Path, debug_enabled: bool) -> AppResult<()> {
    let env_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "yshell=debug,warn".to_owned());
    let config = yshell_logging::LoggingConfig::with_env_filter(env_filter)
        .with_ansi(false)
        .with_stderr(false)
        .with_file_directory(yshell_logging::debug_log_dir(config_dir))
        .with_debug_enabled(debug_enabled);
    match yshell_logging::init_tracing(&config) {
        Ok(()) => {
            if debug_enabled {
                tracing::info!(
                    path = %yshell_logging::debug_log_path(config_dir).display(),
                    "debug logging enabled"
                );
            }
            Ok(())
        }
        Err(yshell_logging::InitLoggingError::AlreadyInitialized(_)) => {
            yshell_logging::set_debug_logging_enabled(debug_enabled);
            Ok(())
        }
        Err(error) => Err(AppError::from_error(error)),
    }
}
