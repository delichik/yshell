//! Full CLI launcher used by the user-facing `yshell` binary.
//!
//! This module is deliberately separate from [`crate::cli`] (pure parsing):
//! role binaries link their own small `role_*` entry modules, so the Host /
//! worker binaries do not pull the GUI/Shell code (and Slint) through the
//! parser module.

use std::env;

use crate::bootstrap::bootstrap_app;
use crate::cli::{self, CliCommand, CliInvocation};
use crate::error::{AppError, AppResult};
use crate::host;
use crate::remote_shell::run_remote_shell;
use crate::runtime::AppRuntime;

/// Run the product launcher / multi-role CLI (`yshell` binary).
pub fn run() -> AppResult<()> {
    dispatch(CliInvocation::parse(env::args().skip(1))?)
}

fn dispatch(invocation: CliInvocation) -> AppResult<()> {
    if !matches!(invocation.command, CliCommand::RunShell | CliCommand::Shell) {
        cli::attach_parent_console();
    }
    match invocation.command {
        CliCommand::CheckConfig => cli::print_check_config(invocation.config_dir),
        CliCommand::PrintConfigDir => cli::print_config_dir(invocation.config_dir),
        CliCommand::QuickConnect(input) => {
            let config_dir = cli::resolve_config_dir(invocation.config_dir)?;
            let mut runtime = AppRuntime::new(config_dir)?;
            let projection = runtime.handle_quick_connect(&input)?;
            println!(
                "quick connect pipeline initialized: {}",
                projection.status_text
            );
            Ok(())
        }
        CliCommand::Version => cli::print_version(),
        CliCommand::Help => {
            cli::print_help();
            Ok(())
        }
        CliCommand::Mcp => {
            let config_dir = cli::resolve_config_dir(invocation.config_dir)?;
            yshell_mcp::run_stdio_server(&config_dir).map_err(AppError::from_error)
        }
        CliCommand::Host => {
            let config_dir = cli::resolve_config_dir(invocation.config_dir)?;
            host::run_host_headless(config_dir)
        }
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
        CliCommand::RunShell | CliCommand::Shell => {
            let config_dir = cli::resolve_config_dir(invocation.config_dir.clone())?;
            // Single-instance: second launch asks the live Host to open a window.
            if host::try_notify_existing_instance(&config_dir)? {
                return Ok(());
            }
            if host::inprocess_host_requested() {
                // Debug / test: classic in-process Host facade + full GUI.
                let app = bootstrap_app(Some(config_dir.clone()))?;
                return app.run();
            }
            // D38 Phase 4: ensure Host OS process, then attach Shell UI.
            host::ensure_host_process(&config_dir)?;
            run_remote_shell(config_dir)
        }
    }
}
