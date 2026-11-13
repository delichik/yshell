//! Process liveness helpers (no persisted state).
//!
//! The Host is discovered through `host-bridge.json` + the IPC handshake and
//! children detect Host death through their IPC connection EOF / parent
//! liveness. There is deliberately **no run-state file** anymore: nothing on
//! disk records pids, generations or worker maps.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch (used as an in-memory Host generation).
#[must_use]
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[must_use]
pub fn current_pid() -> u32 {
    std::process::id()
}

#[must_use]
pub fn process_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        windows_process_is_alive(pid)
    }
    #[cfg(unix)]
    {
        unix_process_is_alive(pid)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        false
    }
}

/// Best-effort terminate (used when stopping the MCP daemon).
#[must_use]
pub fn terminate_pid(pid: u32) -> bool {
    terminate_process(pid)
}

/// Force-kill a process (used after the graceful shutdown grace period).
#[must_use]
pub fn force_terminate_pid(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        windows_terminate_process(pid)
    }
    #[cfg(unix)]
    {
        unix_signal(pid, libc::SIGKILL)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        false
    }
}

fn terminate_process(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        windows_terminate_process(pid)
    }
    #[cfg(unix)]
    {
        unix_signal(pid, libc::SIGTERM)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        false
    }
}

#[cfg(unix)]
fn unix_process_is_alive(pid: u32) -> bool {
    // kill(2) is a libc FFI; the workspace denies unsafe by default.
    #[allow(unsafe_code)]
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    // EPERM means the process exists but is owned by another user.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn unix_signal(pid: u32, signal: libc::c_int) -> bool {
    #[allow(unsafe_code)]
    {
        unsafe { libc::kill(pid as libc::pid_t, signal) == 0 }
    }
}

#[cfg(windows)]
const INVALID_HANDLE: windows_sys::Win32::Foundation::HANDLE = -1isize as _;

#[cfg(windows)]
fn windows_process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };

    #[allow(unsafe_code)]
    unsafe {
        let handle = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        );
        if handle.is_null() || handle == INVALID_HANDLE {
            return false;
        }
        let alive = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
        let _ = CloseHandle(handle);
        alive
    }
}

#[cfg(windows)]
fn windows_terminate_process(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    #[allow(unsafe_code)]
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() || handle == INVALID_HANDLE {
            return false;
        }
        let ok = TerminateProcess(handle, 1) != 0;
        let _ = CloseHandle(handle);
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive() {
        assert!(process_is_alive(current_pid()));
    }

    #[test]
    fn dead_pid_is_not_alive() {
        // pid 0 is invalid on every supported platform.
        assert!(!process_is_alive(0));
    }
}
