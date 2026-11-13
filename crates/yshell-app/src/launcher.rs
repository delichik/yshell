//! Host-entry launcher for the user-facing `yshell` binary.
//!
//! `yshell` is the **Host entry** and is intentionally Slint-free: the default
//! command becomes the globally unique Host for the configuration directory and
//! spawns the GUI Shell child (`yshell-shell --attach`). A second launch only
//! asks the running Host to open a new window. This module must not reference
//! `bootstrap`/`remote_shell`, otherwise the Host binary would link the whole GUI.

use std::env;

use crate::cli::{self, CliCommand, CliInvocation};
use crate::error::{AppError, AppResult};
use crate::host;
use crate::runtime::AppRuntime;

/// Run the product launcher / multi-role CLI (`yshell` binary).
pub fn run() -> AppResult<()> {
    dispatch(CliInvocation::parse(env::args().skip(1))?)
}

fn dispatch(invocation: CliInvocation) -> AppResult<()> {
    if !matches!(
        invocation.command,
        CliCommand::HostEntry | CliCommand::Shell
    ) {
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
        // Default launch and `--shell` both mean "make sure a window exists":
        // Host entry owns the singleton Host and opens a window (or asks the
        // running Host for another one).
        CliCommand::HostEntry | CliCommand::Shell => host_entry(invocation.config_dir),
        CliCommand::Host => {
            let config_dir = cli::resolve_config_dir(invocation.config_dir)?;
            host::run_host_headless(config_dir, false)
        }
        CliCommand::AttachShell => Err(AppError::new(
            "--attach is internal to yshell-shell; run `yshell` to open a window",
        )),
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
    }
}

/// Default launch: become the unique Host (or wake the running one) and make
/// sure one GUI Shell window exists. Never starts a second Host.
fn host_entry(config_dir: Option<std::path::PathBuf>) -> AppResult<()> {
    let config_dir = cli::resolve_config_dir(config_dir)?;
    if host::inprocess_host_requested() {
        // Debug / test escape hatch: the full in-process GUI lives in the Shell
        // binary; the Host entry only spawns it (Slint must not be linked here).
        return spawn_shell_child_and_wait(&config_dir);
    }
    if host::try_notify_existing_instance(&config_dir)? {
        return Ok(());
    }
    match host::run_host_headless(config_dir.clone(), true) {
        Ok(()) => Ok(()),
        Err(error) => {
            // Lost the single-instance race (another launch became the Host in
            // between): behave like a second launch and ask it for a window.
            if error.to_string().contains("another yshell Host") {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                while std::time::Instant::now() < deadline {
                    if host::try_notify_existing_instance(&config_dir)? {
                        return Ok(());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                return Ok(());
            }
            Err(error)
        }
    }
}

/// Spawn `yshell-shell --attach` and wait for it (used by the in-process debug
/// escape hatch, where the Shell itself hosts the in-process Host facade).
fn spawn_shell_child_and_wait(config_dir: &std::path::Path) -> AppResult<()> {
    let mut child = host::spawn_shell(config_dir)?;
    child.wait().map_err(AppError::from_error)?;
    Ok(())
}
