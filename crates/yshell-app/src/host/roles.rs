//! Process-role entrypoints (`--host`, `--shell`, `--session-worker`, `--mcp-daemon`).

use std::io::{self, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use yshell_mcp::IpcResponse;

use crate::error::{AppError, AppResult};
use crate::mcp_server::{start_mcp_server, McpInbox, McpJob};
use crate::runtime::AppRuntime;

use super::api::ProcessRole;
use super::run_state::{current_pid, process_is_alive, RunState};

/// CLI payload for `--session-worker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWorkerArgs {
    pub config_dir: PathBuf,
    pub session_id: String,
    pub generation: u64,
    pub parent_pid: u32,
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

/// Headless Host loop (Phase 4 scaffolding).
///
/// Creates run-state + job, then waits for stdin `quit` / EOF before shutting down.
/// Full Host control-socket IPC lands with Shell attach.
pub fn run_host_headless(config_dir: PathBuf) -> AppResult<()> {
    let runtime = AppRuntime::new(config_dir)?;
    let host = super::service::HostService::start(runtime)?;
    writeln!(
        io::stdout(),
        "yshell host generation={} pid={} role={}",
        host.borrow().generation(),
        current_pid(),
        ProcessRole::Host.as_str()
    )
    .ok();
    let stdin = io::stdin();
    let mut line = String::new();
    loop {
        line.clear();
        match stdin.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line.trim().eq_ignore_ascii_case("quit") => break,
            Ok(_) => {}
            Err(error) => return Err(AppError::from_error(error)),
        }
    }
    host.borrow_mut().begin_shutdown()?;
    Ok(())
}

/// Session worker process (Phase 2 scaffolding).
///
/// Watches parent Host liveness and run-state generation; exits when either fails.
/// Terminal/SSH I/O over IPC is layered onto this boundary next.
pub fn run_session_worker(args: SessionWorkerArgs) -> AppResult<()> {
    writeln!(
        io::stdout(),
        "yshell session-worker session={} generation={} pid={}",
        args.session_id,
        args.generation,
        current_pid()
    )
    .ok();

    let heartbeat = Duration::from_millis(500);
    loop {
        if args.parent_pid != 0 && !process_is_alive(args.parent_pid) {
            tracing::info!(
                session_id = %args.session_id,
                parent_pid = args.parent_pid,
                "parent host gone; session worker exiting"
            );
            break;
        }
        match RunState::load(&args.config_dir)? {
            Some(state) if state.generation != args.generation => {
                tracing::info!(
                    session_id = %args.session_id,
                    expected = args.generation,
                    actual = state.generation,
                    "run-state generation changed; session worker exiting"
                );
                break;
            }
            None => break,
            Some(_) => {}
        }
        thread::sleep(heartbeat);
    }
    Ok(())
}

/// Standalone MCP TCP listener (Phase 3 scaffolding).
///
/// Keeps a single listener process alive; tool calls fail fast until Host bridge IPC
/// is attached. Host GUI still runs the in-process listener today; `--mcp-daemon` is
/// the spawn target for the out-of-process path.
pub fn run_mcp_daemon(args: McpDaemonArgs) -> AppResult<()> {
    if args.parent_pid != 0 && !process_is_alive(args.parent_pid) {
        return Err(AppError::new("mcp-daemon parent host is not alive"));
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
        "yshell mcp-daemon ready port={} generation={} pid={}",
        handle.port,
        args.generation,
        current_pid()
    )
    .ok();

    let config_dir = args.config_dir.clone();
    let generation = args.generation;
    let parent_pid = args.parent_pid;
    loop {
        if parent_pid != 0 && !process_is_alive(parent_pid) {
            break;
        }
        match RunState::load(&config_dir).ok().flatten() {
            Some(state) if state.generation != generation => break,
            None if generation != 0 => break,
            _ => {}
        }
        while let Some(job) = inbox.try_pop() {
            reject_mcp_job(job);
        }
        thread::sleep(Duration::from_millis(50));
    }
    drop(handle);
    Ok(())
}

fn reject_mcp_job(job: McpJob) {
    match job {
        McpJob::Handle { request, reply } => {
            let _ = reply.send(IpcResponse::err(
                request.id,
                "host_bridge_pending",
                "MCP daemon waiting for Host bridge (D38 Phase 3)",
            ));
        }
        McpJob::BeginControl { reply, .. } => {
            let _ = reply.send(Err(
                "MCP daemon waiting for Host bridge (D38 Phase 3)".to_owned()
            ));
        }
        McpJob::ReleaseClient { .. } => {}
    }
}
