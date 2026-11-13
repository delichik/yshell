//! Process-role entrypoints (`--host`, `--shell`, `--session-worker`, `--mcp-daemon`).

use std::io::{self, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use yshell_mcp::IpcResponse;

use crate::error::{AppError, AppResult};
use crate::mcp_server::{start_mcp_server, McpInbox, McpJob};
use crate::runtime::AppRuntime;

use super::api::ProcessRole;
use super::bridge::HostBridgeClient;
use super::liveness::{current_pid, process_is_alive};

/// CLI payload for `--session-worker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWorkerArgs {
    pub config_dir: PathBuf,
    pub session_id: String,
    pub generation: u64,
    pub parent_pid: u32,
    pub ipc_port: u16,
    pub ipc_token: String,
}

/// CLI payload for `--mcp-daemon`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDaemonArgs {
    pub config_dir: PathBuf,
    pub bind_host: String,
    pub port: u16,
    pub token: String,
    pub generation: u64,
    pub parent_pid: u32,
}

/// File name of the executable implementing `role` (without `.exe`).
///
/// `Host` maps to the user-facing `yshell` entry itself: the Host is the entry
/// process now, not a separate `yshell-host` binary.
#[must_use]
pub fn role_exe_name(role: ProcessRole) -> &'static str {
    match role {
        ProcessRole::Host => "yshell",
        ProcessRole::Shell => "yshell-shell",
        ProcessRole::SessionWorker => "yshell-session-worker",
        ProcessRole::McpDaemon => "yshell-mcp-daemon",
    }
}

/// Executable that should run `role`.
///
/// Release bundles ship one thin executable per role next to the user-facing
/// `yshell` entry, so a sibling lookup keeps every role in the same directory
/// (fonts / themes are resolved relative to the executable directory). When the
/// sibling is missing — unit tests, `cargo run`, partial builds — fall back to
/// the current executable, which still understands every role argv flag.
#[must_use]
pub fn role_exe(role: ProcessRole) -> PathBuf {
    let current = std::env::current_exe().ok();
    if let Some(current) = &current {
        if let Some(dir) = current.parent() {
            // Mirror the current executable's extension (`.exe` on Windows).
            let file_name = match current.extension() {
                Some(extension) => {
                    format!("{}.{}", role_exe_name(role), extension.to_string_lossy())
                }
                None => role_exe_name(role).to_owned(),
            };
            let sibling = dir.join(file_name);
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    // Last resort: the current executable (or, if even that is unavailable,
    // the role name so spawning fails with the OS error for the right file).
    current.unwrap_or_else(|| PathBuf::from(role_exe_name(role)))
}

/// Headless Host supervisor loop — owns AppRuntime (D38 Phase 4).
///
/// `spawn_shell` is true for the default `yshell` entry: the Host spawns one GUI
/// Shell child, waits for it to attach and shuts down when the last window closes
/// (or when the Shell dies). `--host` runs with `spawn_shell = false`.
pub fn run_host_headless(config_dir: PathBuf, spawn_shell: bool) -> AppResult<()> {
    let runtime = AppRuntime::new(config_dir)?;
    // The Host is the only process that owns configuration state; give it a
    // file-backed log so startup/shutdown problems are diagnosable even when it
    // was spawned detached (stdlib stderr is /dev/null in that case).
    if let Err(error) = crate::logging_setup::init_logging_for_runtime(&runtime) {
        tracing::debug!(%error, "host logging initialisation failed");
    }
    let host = super::service::HostService::start(runtime)?;
    tracing::info!(
        generation = host.borrow().generation(),
        pid = current_pid(),
        role = ProcessRole::Host.as_str(),
        spawn_shell,
        "host supervisor started"
    );
    writeln!(
        io::stdout(),
        "yshell host generation={} pid={} role={}",
        host.borrow().generation(),
        current_pid(),
        ProcessRole::Host.as_str()
    )
    .ok();

    // The Host entry owns the GUI Shell: spawn it after the bridge is published
    // and supervise it. The Job Object (Win) / PDEATHSIG (Linux) / parent-pid
    // watchdog (macOS) guarantee that the Shell dies with the Host.
    if spawn_shell {
        host.borrow_mut().spawn_shell_process()?;
    }
    let shell_deadline = if spawn_shell {
        Some(Instant::now() + Duration::from_secs(30))
    } else {
        None
    };

    // Wait until a Shell attaches or we are asked to quit. After the first
    // Shell has attached and later detached to zero windows, supervisor_tick
    // requests shutdown.
    let mut saw_shell = false;
    loop {
        let shutdown = match host.borrow_mut().supervisor_tick() {
            Ok(shutdown) => shutdown,
            Err(error) => {
                tracing::error!(%error, "host supervisor tick failed");
                return Err(error);
            }
        };
        if host.borrow().shell_registry_shell_count() > 0 {
            saw_shell = true;
        }
        // A Shell that dies before attaching (startup failure) or never attaches
        // must not leave a headless Host lingering.
        if !saw_shell {
            if host.borrow().shell_spawn_failed() {
                tracing::error!("GUI Shell exited before attaching to the Host");
                let _ = host.borrow_mut().begin_shutdown();
                return Err(AppError::new(
                    "GUI Shell exited before attaching to the Host",
                ));
            }
            if shell_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                tracing::error!("GUI Shell did not attach within 30s");
                let _ = host.borrow_mut().begin_shutdown();
                return Err(AppError::new(
                    "timed out waiting for the GUI Shell to attach",
                ));
            }
        }
        if shutdown {
            break;
        }
        // If we already had a shell and now have none with zero windows, exit.
        if saw_shell
            && host.borrow().shell_registry_shell_count() == 0
            && host.borrow().window_count() == 0
        {
            host.borrow_mut().begin_shutdown()?;
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    tracing::info!(saw_shell, "host supervisor loop exited");
    Ok(())
}

/// Session worker process — owns SSH/PTY for one session (D38 Phase 2).
pub fn run_session_worker(args: SessionWorkerArgs) -> AppResult<()> {
    if args.ipc_port == 0 || args.ipc_token.is_empty() {
        return Err(AppError::new(
            "session-worker requires --ipc-port and --ipc-token (Host-spawned only)",
        ));
    }

    super::session_ipc::run_session_worker_ipc(
        args.config_dir,
        args.session_id,
        args.generation,
        args.parent_pid,
        args.ipc_port,
        args.ipc_token,
    )
}

/// Standalone MCP TCP listener — real OS process.
pub fn run_mcp_daemon(args: McpDaemonArgs) -> AppResult<()> {
    if args.parent_pid != 0 && !process_is_alive(args.parent_pid) {
        return Err(AppError::new("mcp-daemon parent host is not alive"));
    }

    let bridge = HostBridgeClient::connect(&args.config_dir)?;
    if bridge.info().generation != 0 && bridge.info().generation != args.generation {
        return Err(AppError::new(format!(
            "host bridge generation {} != daemon generation {}",
            bridge.info().generation,
            args.generation
        )));
    }

    let inbox = McpInbox::new();
    let handle = start_mcp_server(
        args.config_dir.clone(),
        &args.bind_host,
        args.port,
        "127.0.0.1",
        args.token.clone(),
        inbox.clone(),
    )?;

    writeln!(
        io::stdout(),
        "yshell mcp-daemon ready port={} generation={} pid={} bridge={}:{}",
        handle.port,
        args.generation,
        current_pid(),
        bridge.info().host,
        bridge.info().port
    )
    .ok();

    let parent_pid = args.parent_pid;
    loop {
        // The bridge/daemon connection is the liveness signal; the parent pid is
        // a cheap secondary check for the standalone (Host-spawned) daemon.
        if parent_pid != 0 && !process_is_alive(parent_pid) {
            break;
        }
        while let Some(job) = inbox.try_pop() {
            forward_mcp_job(&bridge, job);
        }
        thread::sleep(Duration::from_millis(5));
    }
    drop(handle);
    Ok(())
}

fn forward_mcp_job(bridge: &HostBridgeClient, job: McpJob) {
    match job {
        McpJob::Handle { request, reply } => {
            let response = match bridge.mcp_handle(request.clone()) {
                Ok(response) => response,
                Err(error) => IpcResponse::err(request.id, "host_bridge", error.to_string()),
            };
            let _ = reply.send(response);
        }
        McpJob::BeginControl {
            client_id,
            endpoint_token,
            reply,
        } => {
            let result = bridge
                .mcp_begin_control(client_id, endpoint_token)
                .map_err(|error| error.to_string());
            let _ = reply.send(result);
        }
        McpJob::ReleaseClient { client_id } => {
            let _ = bridge.mcp_release_client(client_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_exe_falls_back_to_the_current_executable_in_dev() {
        // `cargo test` runs the harness from `target/<profile>/deps`, where the
        // role binaries are not siblings: the fallback must still resolve to a
        // real, runnable executable (the current process).
        let executable = role_exe(ProcessRole::Host);
        assert!(
            executable.is_file(),
            "role_exe should resolve to an existing file, got {}",
            executable.display()
        );
        let current = std::env::current_exe().expect("current executable");
        let is_sibling = executable
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(role_exe_name(ProcessRole::Host)));
        assert!(
            executable == current || is_sibling,
            "expected the current executable fallback or a role sibling, got {}",
            executable.display()
        );
    }
}
