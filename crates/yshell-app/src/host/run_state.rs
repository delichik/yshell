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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRecord {
    pub session_id: String,
    pub pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunState {
    /// Incremented every time a Host process starts cleanly.
    pub generation: u64,
    pub host_pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_pid: Option<u32>,
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
            mcp_pid: None,
            workers: Vec::new(),
            updated_unix_ms: now_unix_ms(),
        }
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
        self.workers.push(WorkerRecord { session_id, pid });
    }

    pub fn remove_worker(&mut self, session_id: &str) {
        self.workers
            .retain(|worker| worker.session_id != session_id);
    }

    /// Kill processes recorded in `stale` that are still alive and are not `self_pid`.
    ///
    /// Returns the number of processes signaled.
    pub fn reclaim_orphans(stale: &Self, self_pid: u32) -> usize {
        let mut signaled = 0usize;
        if stale.host_pid != self_pid
            && process_is_alive(stale.host_pid)
            && terminate_process(stale.host_pid)
        {
            signaled += 1;
        }
        if let Some(mcp_pid) = stale.mcp_pid {
            if mcp_pid != self_pid && process_is_alive(mcp_pid) && terminate_process(mcp_pid) {
                signaled += 1;
            }
        }
        for worker in &stale.workers {
            if worker.pid != self_pid
                && process_is_alive(worker.pid)
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

#[must_use]
pub fn process_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        windows_process_is_alive(pid)
    }
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        matches!(
            std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status(),
            Ok(code) if code.success()
        )
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
    #[cfg(not(windows))]
    {
        let status = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
        matches!(status, Ok(code) if code.success())
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
}
