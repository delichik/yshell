//! Single-instance lock for the Host process (D38).
//!
//! Only one Host may own a configuration directory. Windows uses a named
//! mutex in the local session namespace (the kernel releases it if the process
//! dies); Unix uses a non-blocking `flock` on `<config_dir>/instance.lock`
//! (also released by the kernel on process exit).
//!
//! The lock is intentionally *not* a run-state pid file: stale pid files need
//! reclaim heuristics, while an OS lock can never outlive the Host.

use std::path::Path;

use crate::error::{AppError, AppResult};

/// Held for the lifetime of the Host process; dropping it releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    _file: std::fs::File,
}

impl InstanceLock {
    /// Acquires the per-config-directory Host lock.
    ///
    /// Returns a clear error when another Host is already alive; callers
    /// (`--host`) exit and the GUI's `ensure_host_process` attaches to the
    /// running Host instead.
    pub fn acquire(config_dir: &Path) -> AppResult<Self> {
        acquire_impl(config_dir)
    }
}

#[cfg(windows)]
fn acquire_impl(config_dir: &Path) -> AppResult<InstanceLock> {
    use std::hash::{Hash, Hasher};

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    config_dir.hash(&mut hasher);
    let name = format!("Local\\yshell-host-{:016x}", hasher.finish());
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

    // CreateMutexW is Win32 FFI; the workspace denies unsafe by default.
    #[allow(unsafe_code)]
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
    if handle.is_null() {
        return Err(AppError::new(
            "failed to create the Host single-instance mutex",
        ));
    }
    // `GetLastError` is only meaningful immediately after CreateMutexW.
    #[allow(unsafe_code)]
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_running {
        #[allow(unsafe_code)]
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Err(AppError::new(format!(
            "another yshell Host already owns {}",
            config_dir.display()
        )));
    }
    Ok(InstanceLock { handle })
}

#[cfg(unix)]
fn acquire_impl(config_dir: &Path) -> AppResult<InstanceLock> {
    use std::os::unix::io::AsRawFd;

    std::fs::create_dir_all(config_dir).map_err(AppError::from_error)?;
    let path = config_dir.join("instance.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(AppError::from_error)?;
    // flock(2) is a libc FFI; the workspace denies unsafe by default.
    #[allow(unsafe_code)]
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(AppError::new(format!(
            "another yshell Host already owns {}",
            config_dir.display()
        )));
    }
    Ok(InstanceLock { _file: file })
}

#[cfg(not(any(windows, unix)))]
fn acquire_impl(_config_dir: &Path) -> AppResult<InstanceLock> {
    Ok(InstanceLock {})
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::CloseHandle;
            #[allow(unsafe_code)]
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn second_host_lock_fails_while_first_is_held() {
        let temp = TempDir::new().expect("tempdir");
        let first = InstanceLock::acquire(temp.path()).expect("first Host lock");
        let second = InstanceLock::acquire(temp.path());
        assert!(second.is_err(), "a second Host must not take the lock");
        drop(first);
        InstanceLock::acquire(temp.path()).expect("lock is released on drop");
    }
}
