//! `yshell-shell` entry: the GUI Shell process spawned by the Host (`--attach`).
//!
//! It attaches to the already-running Host; it never starts a Host by itself.
//! `--shell` is still accepted for compatibility (ensure a Host, then attach).

use crate::cli::{self, CliCommand};
use crate::error::{AppError, AppResult};
use crate::host;

pub fn run() -> AppResult<()> {
    let invocation = cli::parse_for_role(host::ProcessRole::Shell)?;
    if let Some(result) =
        cli::handle_metadata_command(&invocation.command, invocation.config_dir.clone())
    {
        return result;
    }
    match invocation.command {
        CliCommand::AttachShell => {
            let config_dir = cli::resolve_config_dir(invocation.config_dir)?;
            if host::inprocess_host_requested() {
                // Debug / test escape hatch: full in-process Host facade + GUI.
                let app = crate::bootstrap::bootstrap_app(Some(config_dir))?;
                return app.run();
            }
            crate::remote_shell::run_remote_shell(config_dir)
        }
        CliCommand::Shell => {
            let config_dir = cli::resolve_config_dir(invocation.config_dir.clone())?;
            if host::inprocess_host_requested() {
                let app = crate::bootstrap::bootstrap_app(Some(config_dir))?;
                return app.run();
            }
            if host::try_notify_existing_instance(&config_dir)? {
                return Ok(());
            }
            host::ensure_host_process(&config_dir)?;
            crate::remote_shell::run_remote_shell(config_dir)
        }
        other => Err(AppError::new(format!("yshell-shell cannot run: {other:?}"))),
    }
}
