//! Persistent run-state used to reclaim orphaned Host / Worker / MCP processes.
//!
//! See `docs/prototypes/process-architecture.md` (D38).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// File name under the configuration directory.
pub const RUN_STATE_FILE_NAME: &str = "run-state.json";

/// Identity of a process, used to make pid-based reclaim safe against pid reuse.
///
/// `started_unix_ms == 0` means the platform could not report a start time; the
/// comparison then falls back to the executable path only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    /// Executable path (Linux) or full image path (Windows). On macOS the `ps`
    /// reported command name.
    pub image: String,
    /// Process start time in unix milliseconds (0 when unavailable).
    pub started_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRecord {
    pub session_id: String,
    pub pid: u32,
    /// Captured when the worker was adopted; absent in run-state written by
    /// older builds (those records are never used to kill anything).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<ProcessIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunState {
    /// Incremented every time a Host process starts cleanly.
    pub generation: u64,
    pub host_pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_identity: Option<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_identity: Option<ProcessIdentity>,
    #[serde(default)]
    pub workers: Vec<WorkerRecord>,
    /// Unix millis when this file was last written.
    pub updated_unix_ms: u64,
}

impl RunState {
    #[must_use]
    pub fn new(generation: u64, host_pid: u32) -> Self {
        Self {
            generation,
            host_pid,
            host_identity: process_identity(host_pid),
            mcp_pid: None,
            mcp_identity: None,
            workers: Vec::new(),
            updated_unix_ms: now_unix_ms(),
        }
    }

    /// Record the MCP daemon pid together with its identity.
    pub fn set_mcp_pid(&mut self, pid: u32) {
        self.mcp_pid = Some(pid);
        self.mcp_identity = process_identity(pid);
    }

    /// Forget the MCP daemon pid and identity.
    pub fn clear_mcp_pid(&mut self) {
        self.mcp_pid = None;
        self.mcp_identity = None;
    }

    #[must_use]
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join(RUN_STATE_FILE_NAME)
    }

    pub fn load(config_dir: &Path) -> AppResult<Option<Self>> {
        let path = Self::path(config_dir);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path).map_err(AppError::from_error)?;
        let state: Self = serde_json::from_str(&raw).map_err(|error| {
            AppError::new(format!("failed to parse {}: {error}", path.display()))
        })?;
        Ok(Some(state))
    }

    pub fn save(&self, config_dir: &Path) -> AppResult<()> {
        fs::create_dir_all(config_dir).map_err(AppError::from_error)?;
        let path = Self::path(config_dir);
        let tmp = path.with_extension("json.tmp");
        let mut state = self.clone();
        state.updated_unix_ms = now_unix_ms();
        let body = serde_json::to_string_pretty(&state)
            .map_err(|error| AppError::new(format!("failed to serialize run-state: {error}")))?;
        fs::write(&tmp, body).map_err(AppError::from_error)?;
        fs::rename(&tmp, &path).map_err(AppError::from_error)?;
        Ok(())
    }

    pub fn clear(config_dir: &Path) -> AppResult<()> {
        let path = Self::path(config_dir);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::from_error(error)),
        }
    }

    pub fn upsert_worker(&mut self, session_id: impl Into<String>, pid: u32) {
        let session_id = session_id.into();
        if let Some(existing) = self
            .workers
            .iter_mut()
            .find(|worker| worker.session_id == session_id)
        {
            existing.pid = pid;
            return;
        }
        self.workers.push(WorkerRecord {
            session_id,
            pid,
            identity: process_identity(pid),
        });
    }

    pub fn remove_worker(&mut self, session_id: &str) {
        self.workers
            .retain(|worker| worker.session_id != session_id);
    }

    /// Kill processes recorded in `stale` that are still alive **and verifiably
    /// the same process** (pid + image + start time), so pid reuse cannot make a
    /// new Host kill an unrelated process.
    ///
    /// Returns the number of processes signaled.
    pub fn reclaim_orphans(stale: &Self, self_pid: u32) -> usize {
        let mut signaled = 0usize;
        if stale.host_pid != self_pid
            && process_is_alive(stale.host_pid)
            && identity_matches(stale.host_pid, stale.host_identity.as_ref())
            && terminate_process(stale.host_pid)
        {
            signaled += 1;
        }
        if let Some(mcp_pid) = stale.mcp_pid {
            if mcp_pid != self_pid
                && process_is_alive(mcp_pid)
                && identity_matches(mcp_pid, stale.mcp_identity.as_ref())
                && terminate_process(mcp_pid)
            {
                signaled += 1;
            }
        }
        for worker in &stale.workers {
            if worker.pid != self_pid
                && process_is_alive(worker.pid)
                && identity_matches(worker.pid, worker.identity.as_ref())
                && terminate_process(worker.pid)
            {
                signaled += 1;
            }
        }
        signaled
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[must_use]
pub fn current_pid() -> u32 {
    std::process::id()
}

/// Whether `pid` still refers to the recorded process.
///
/// Records without an identity (written by older builds) are treated as
/// unverifiable and are never killed.
fn identity_matches(pid: u32, recorded: Option<&ProcessIdentity>) -> bool {
    let Some(recorded) = recorded else {
        return false;
    };
    let Some(current) = process_identity(pid) else {
        return false;
    };
    current.image == recorded.image
        && (recorded.started_unix_ms == 0 || current.started_unix_ms == recorded.started_unix_ms)
}

/// Best-effort identity (image path + start time) of a live process.
///
/// Returns `None` when the process is gone or cannot be inspected; callers must
/// treat that as "do not touch it".
#[must_use]
pub fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    if pid == 0 {
        return None;
    }
    #[cfg(windows)]
    {
        windows_process_identity(pid)
    }
    #[cfg(target_os = "linux")]
    {
        linux_process_identity(pid)
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        unix_process_identity(pid)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        None
    }
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

/// Best-effort terminate (used when stopping MCP daemon / reclaim).
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
fn windows_process_identity(pid: u32) -> Option<ProcessIdentity> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    #[allow(unsafe_code)]
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut size = u32::try_from(buffer.len()).ok()?;
        let image_ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size);
        let image = if image_ok != 0 {
            String::from_utf16_lossy(&buffer[..size as usize])
        } else {
            String::new()
        };
        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = creation;
        let mut kernel = creation;
        let mut user = creation;
        let times_ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(handle);
        if image.is_empty() {
            return None;
        }
        let started_unix_ms = if times_ok != 0 {
            let filetime =
                (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
            // FILETIME = 100ns ticks since 1601-01-01.
            (filetime / 10_000).saturating_sub(11_644_473_600_000)
        } else {
            0
        };
        Some(ProcessIdentity {
            image,
            started_unix_ms,
        })
    }
}

#[cfg(target_os = "linux")]
fn linux_process_identity(pid: u32) -> Option<ProcessIdentity> {
    let image = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, tail) = stat.rsplit_once(')')?;
    let fields: Vec<&str> = tail.split_whitespace().collect();
    // `tail` starts at field 3 (state); /proc/<pid>/stat field 22 is starttime.
    let start_ticks: u64 = fields.get(19)?.parse().ok()?;
    let boot_secs: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))
        .and_then(|value| value.trim().parse().ok())?;
    // sysconf(2) is libc FFI; the workspace denies unsafe by default.
    #[allow(unsafe_code)]
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hz = if hz > 0 { hz as u64 } else { 100 };
    let started_unix_ms = boot_secs
        .saturating_mul(1000)
        .saturating_add(start_ticks.saturating_mul(1000) / hz);
    Some(ProcessIdentity {
        image,
        started_unix_ms,
    })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn unix_process_identity(pid: u32) -> Option<ProcessIdentity> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let image = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if image.is_empty() {
        return None;
    }
    // Start time is not portably available here; image-only identity.
    Some(ProcessIdentity {
        image,
        started_unix_ms: 0,
    })
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
    use tempfile::TempDir;

    #[test]
    fn save_load_round_trip() {
        let temp = TempDir::new().expect("temp");
        let mut state = RunState::new(3, 42);
        state.mcp_pid = Some(99);
        state.upsert_worker("ses_a", 7);
        state.save(temp.path()).expect("save");
        let loaded = RunState::load(temp.path()).expect("load").expect("some");
        assert_eq!(loaded.generation, 3);
        assert_eq!(loaded.host_pid, 42);
        assert_eq!(loaded.mcp_pid, Some(99));
        assert_eq!(loaded.workers.len(), 1);
        assert_eq!(loaded.workers[0].session_id, "ses_a");
        RunState::clear(temp.path()).expect("clear");
        assert!(RunState::load(temp.path()).expect("load").is_none());
    }

    #[test]
    fn upsert_and_remove_worker() {
        let mut state = RunState::new(1, 1);
        state.upsert_worker("a", 10);
        state.upsert_worker("a", 11);
        assert_eq!(state.workers.len(), 1);
        assert_eq!(state.workers[0].pid, 11);
        state.remove_worker("a");
        assert!(state.workers.is_empty());
    }

    #[test]
    fn current_process_reports_alive() {
        assert!(process_is_alive(current_pid()));
        assert!(!process_is_alive(0));
    }

    #[test]
    fn current_process_identity_matches_itself() {
        let identity = process_identity(current_pid()).expect("own process identity");
        assert!(!identity.image.is_empty());
        assert!(identity_matches(current_pid(), Some(&identity)));
        // Unverifiable records (older builds) must never be used to kill.
        assert!(!identity_matches(current_pid(), None));
        let mut other = identity;
        other.image = "definitely-not-the-current-executable".to_owned();
        assert!(!identity_matches(current_pid(), Some(&other)));
    }

    #[test]
    fn upsert_worker_captures_identity() {
        let mut state = RunState::new(1, current_pid());
        state.upsert_worker("ses_identity", current_pid());
        let worker = &state.workers[0];
        assert_eq!(worker.identity, process_identity(current_pid()));
    }
}
