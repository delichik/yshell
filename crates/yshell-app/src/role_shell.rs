//! `yshell-shell` entry: links the GUI Shell (Slint) path.

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
        CliCommand::RunShell | CliCommand::Shell => {
            let config_dir = cli::resolve_config_dir(invocation.config_dir.clone())?;
            if host::try_notify_existing_instance(&config_dir)? {
                return Ok(());
            }
            if host::inprocess_host_requested() {
                let app = crate::bootstrap::bootstrap_app(Some(config_dir.clone()))?;
                return app.run();
            }
            host::ensure_host_process(&config_dir)?;
            crate::remote_shell::run_remote_shell(config_dir)
        }
        other => Err(AppError::new(format!("yshell-shell cannot run: {other:?}"))),
    }
}
