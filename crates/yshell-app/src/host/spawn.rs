//! Spawn helpers for out-of-process Host children.
//!
//! SessionWorkers are spawned by the connect path
//! ([`crate::host::session_ipc::connect_via_session_worker`]) because it owns the
//! loopback listener they dial back into; the MCP daemon is spawned here by the
//! Host supervisor.

use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::error::{AppError, AppResult};

use super::api::ProcessRole;
use super::liveness::current_pid;
use super::roles::role_exe;

/// Common configuration for Host-spawned role children.
///
/// - Windows: `CREATE_NO_WINDOW` so role processes do not flash a console.
/// - Unix: own process group so a group kill reaches the whole child tree.
/// - Linux: `PR_SET_PDEATHSIG(SIGKILL)` so the kernel kills the child if the
///   Host thread that spawned it dies. Only valid from long-lived Host threads
///   (the Host supervisor loop and the connect-job worker), which is where the
///   role children are spawned.
pub fn configure_child_spawn(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // prctl(2) + pre_exec are libc/unsafe APIs; the workspace denies unsafe
        // by default.
        #[allow(unsafe_code)]
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

/// Spawn the MCP daemon role executable (`yshell-mcp-daemon --mcp-daemon …`)
/// as a child of the current Host.
pub fn spawn_mcp_daemon(
    config_dir: &Path,
    bind_host: &str,
    port: u16,
    token: &str,
    generation: u64,
) -> AppResult<Child> {
    let exe = role_exe(ProcessRole::McpDaemon);
    let mut cmd = Command::new(&exe);
    cmd.arg("--config-dir")
        .arg(config_dir)
        .arg("--mcp-daemon")
        .arg("--mcp-bind")
        .arg(bind_host)
        .arg("--mcp-port")
        .arg(port.to_string())
        .arg("--mcp-token")
        .arg(token)
        .arg("--generation")
        .arg(generation.to_string())
        .arg("--parent-pid")
        .arg(current_pid().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_child_spawn(&mut cmd);
    cmd.spawn().map_err(|error| {
        AppError::new(format!(
            "failed to spawn MCP daemon at {}: {error}",
            exe.display()
        ))
    })
}
