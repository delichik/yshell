//! `yshell-mcp-daemon` entry: links only the MCP listener path.

use crate::cli::{self, CliCommand};
use crate::error::{AppError, AppResult};
use crate::host;

pub fn run() -> AppResult<()> {
    let invocation = cli::parse_for_role(host::ProcessRole::McpDaemon)?;
    cli::attach_parent_console();
    if let Some(result) =
        cli::handle_metadata_command(&invocation.command, invocation.config_dir.clone())
    {
        return result;
    }
    match invocation.command {
        CliCommand::McpDaemon {
            bind_host,
            port,
            token,
            generation,
            parent_pid,
        } => {
            let config_dir = cli::resolve_config_dir(invocation.config_dir)?;
            host::run_mcp_daemon(host::McpDaemonArgs {
                config_dir,
                bind_host,
                port,
                token,
                generation,
                parent_pid,
            })
        }
        other => Err(AppError::new(format!(
            "yshell-mcp-daemon cannot run: {other:?}"
        ))),
    }
}
