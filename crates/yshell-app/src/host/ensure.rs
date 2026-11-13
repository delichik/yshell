//! Ensure a Host OS process is running, then attach Shell.

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{AppError, AppResult};

use super::api::ProcessRole;
use super::bridge::{load_host_bridge, HostBridgeInfo};
use super::roles::role_exe;
use super::run_state::{current_pid, process_is_alive, RunState};
use super::shell_ipc::HostShellClient;

/// When set to a truthy value, GUI keeps an in-process Host facade (tests / debugging).
pub fn inprocess_host_requested() -> bool {
    env_flag("YSHELL_TEST_INPROCESS_HOST")
}

/// Reads a debug/test environment flag (`1` / `true` / `yes` / `on`).
///
/// Only test / diagnostic switches go through here; product configuration must
/// come from the config directory (enforced by `tests/env_config_guard.rs`).
pub(crate) fn env_flag(name: &str) -> bool {
    std::env::var_os(name)
        .and_then(|value| value.to_str().map(str::to_owned))
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

/// Spawn the Host role executable (`yshell --host …`) detached.
///
/// The Host's stderr is appended to `{config_dir}/host-stderr.log`: detached
/// role processes have no console, so a panic or startup error would otherwise
/// be invisible. The file is small (errors only) and helps support diagnose a
/// Host that refuses to stay alive.
pub fn spawn_host(config_dir: &Path) -> AppResult<u32> {
    let exe = role_exe(ProcessRole::Host);
    let stderr = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config_dir.join("host-stderr.log"))
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null());
    let mut cmd = Command::new(&exe);
    cmd.arg("--config-dir")
        .arg(config_dir)
        .arg("--host")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        // Put the Host in its own process group so a Ctrl-C / SIGHUP aimed at the
        // launching terminal only reaches the Shell, not the Host and sessions.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn().map_err(|error| {
        AppError::new(format!(
            "failed to spawn Host process at {}: {error}",
            exe.display()
        ))
    })?;
    Ok(child.id())
}

/// Ensure a live Host owns `config_dir`, waiting until the bridge answers.
pub fn ensure_host_process(config_dir: &Path) -> AppResult<HostBridgeInfo> {
    if let Some(info) = live_host_bridge(config_dir) {
        return Ok(info);
    }
    // Stale run-state without a live host: reclaim happens inside Host::start.
    let _ = spawn_host(config_dir)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some(info) = live_host_bridge(config_dir) {
            return Ok(info);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(AppError::new(
        "timed out waiting for yshell Host process to publish host-bridge.json",
    ))
}

fn live_host_bridge(config_dir: &Path) -> Option<HostBridgeInfo> {
    let state = RunState::load(config_dir).ok().flatten()?;
    if state.host_pid == current_pid() {
        // Same process — not an out-of-process Host.
        return None;
    }
    if !process_is_alive(state.host_pid) {
        return None;
    }
    let info = load_host_bridge(config_dir).ok()?;
    // Probe TCP.
    if std::net::TcpStream::connect((info.host.as_str(), info.port)).is_ok() {
        Some(info)
    } else {
        None
    }
}

/// Connect a Shell client to the ensured Host.
pub fn connect_shell_client(config_dir: &Path) -> AppResult<HostShellClient> {
    let _ = ensure_host_process(config_dir)?;
    HostShellClient::connect(config_dir)
}

/// Spawn the GUI Shell child (`yshell-shell --attach …`) for the Host entry.
///
/// The Host writes `host-bridge.json` before spawning, so the child can attach
/// immediately. Its stderr is appended to `{config_dir}/shell-stderr.log` for
/// diagnostics (children run without a console).
pub fn spawn_shell(config_dir: &Path) -> AppResult<std::process::Child> {
    let exe = role_exe(ProcessRole::Shell);
    // Never fall back to the Host entry for the Shell role: that would run the
    // GUI inside the Host process. Dev builds must build the role binaries
    // (`cargo xtask build`) or ship them next to the entry.
    if std::env::current_exe()
        .ok()
        .is_some_and(|current| current == exe)
    {
        return Err(AppError::new(
            "GUI Shell binary `yshell-shell` is missing next to the Host entry; \
             build the workspace binaries (`cargo xtask build`) or reinstall",
        ));
    }
    let stderr = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config_dir.join("shell-stderr.log"))
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null());
    let mut cmd = Command::new(&exe);
    cmd.arg("--config-dir")
        .arg(config_dir)
        .arg("--attach")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    super::spawn::configure_child_spawn(&mut cmd);
    cmd.spawn().map_err(|error| {
        AppError::new(format!(
            "failed to spawn GUI Shell process at {}: {error}",
            exe.display()
        ))
    })
}

/// Attach-only Shell client: used by a Host-spawned Shell. It never starts a
/// Host by itself; a missing bridge is an error (the Host will reap this child).
pub fn connect_shell_client_attached(config_dir: &Path) -> AppResult<HostShellClient> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match HostShellClient::connect(config_dir) {
            Ok(client) => return Ok(client),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(AppError::new(format!(
                        "cannot attach the Shell to the Host ({error}); is the Host running?"
                    )));
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}
