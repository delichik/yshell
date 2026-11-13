//! `yshell-session-worker` entry: links only the SSH/PTY worker path.

use crate::cli::{self, CliCommand};
use crate::error::{AppError, AppResult};
use crate::host;

pub fn run() -> AppResult<()> {
    let invocation = cli::parse_for_role(host::ProcessRole::SessionWorker)?;
    cli::attach_parent_console();
    if let Some(result) =
        cli::handle_metadata_command(&invocation.command, invocation.config_dir.clone())
    {
        return result;
    }
    match invocation.command {
        CliCommand::SessionWorker {
            session_id,
            generation,
            parent_pid,
            ipc_port,
            ipc_token,
        } => {
            let config_dir = cli::resolve_config_dir(invocation.config_dir)?;
            host::run_session_worker(host::SessionWorkerArgs {
                config_dir,
                session_id,
                generation,
                parent_pid,
                ipc_port,
                ipc_token,
            })
        }
        other => Err(AppError::new(format!(
            "yshell-session-worker cannot run: {other:?}"
        ))),
    }
}
