//! `yshell-host` entry: links only the headless Host path (no GUI/Slint code).

use crate::cli::{self, CliCommand};
use crate::error::{AppError, AppResult};
use crate::host;

pub fn run() -> AppResult<()> {
    let invocation = cli::parse_for_role(host::ProcessRole::Host)?;
    cli::attach_parent_console();
    if let Some(result) =
        cli::handle_metadata_command(&invocation.command, invocation.config_dir.clone())
    {
        return result;
    }
    match invocation.command {
        CliCommand::Host => {
            let config_dir = cli::resolve_config_dir(invocation.config_dir)?;
            host::run_host_headless(config_dir)
        }
        other => Err(AppError::new(format!("yshell-host cannot run: {other:?}"))),
    }
}
