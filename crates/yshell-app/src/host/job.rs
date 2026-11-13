//! Process-supervision fallback for Host children.
//!
//! Windows: a Job Object with `KILL_ON_JOB_CLOSE` so Host death cascades to
//! Worker / MCP children.
//!
//! Unix: there is no kernel-level job object; children are spawned in their own
//! process groups (see the spawn helpers) and this handle records their pids so
//! it can `killpg` the recorded groups when the Host exits normally. When the
//! Host is killed abruptly, the heartbeat + `run-state` reclaim path covers the
//! orphans instead (see `docs/prototypes/process-architecture.md` §6.1).

use crate::error::{AppError, AppResult};

/// Process job that tears down Host children when the Host exits.
#[derive(Debug)]
pub struct JobObject {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    pids: std::sync::Mutex<Vec<u32>>,
}

impl JobObject {
    /// Creates a job with `KILL_ON_JOB_CLOSE` (Windows) or an empty pid set (Unix).
    pub fn create() -> AppResult<Self> {
        #[cfg(windows)]
        {
            windows_create_job()
        }
        #[cfg(unix)]
        {
            Ok(Self {
                pids: std::sync::Mutex::new(Vec::new()),
            })
        }
        #[cfg(not(any(windows, unix)))]
        {
            Ok(Self {})
        }
    }

    /// Assigns a child process to this job.
    pub fn assign_pid(&self, pid: u32) -> AppResult<()> {
        if pid == 0 {
            return Err(AppError::new("cannot assign pid 0 to job"));
        }
        #[cfg(windows)]
        {
            windows_assign_pid(self.handle, pid)
        }
        #[cfg(unix)]
        {
            let mut pids = self
                .pids
                .lock()
                .map_err(|_| AppError::new("job pid lock poisoned"))?;
            if !pids.contains(&pid) {
                pids.push(pid);
            }
            Ok(())
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = self;
            let _ = pid;
            Ok(())
        }
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::CloseHandle;
            #[allow(unsafe_code)]
            unsafe {
                if !self.handle.is_null() && self.handle != INVALID_HANDLE {
                    let _ = CloseHandle(self.handle);
                }
                self.handle = std::ptr::null_mut();
            }
        }
        #[cfg(unix)]
        {
            let pids = self
                .pids
                .get_mut()
                .map(|pids| std::mem::take(&mut *pids))
                .unwrap_or_default();
            for pid in pids {
                if pid == 0 {
                    continue;
                }
                // Children run in their own process groups; killing the group
                // also reaps local shells spawned by a worker.
                #[allow(unsafe_code)]
                unsafe {
                    let _ = libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                    let _ = libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
        }
    }
}

#[cfg(windows)]
const INVALID_HANDLE: windows_sys::Win32::Foundation::HANDLE = -1isize as _;

#[cfg(windows)]
fn windows_create_job() -> AppResult<JobObject> {
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    #[allow(unsafe_code)]
    unsafe {
        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if handle.is_null() || handle == INVALID_HANDLE {
            return Err(AppError::new("CreateJobObjectW failed"));
        }

        let mut info = std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            std::ptr::from_mut(&mut info).cast(),
            u32::try_from(std::mem::size_of_val(&info)).unwrap_or(u32::MAX),
        );
        if ok == 0 {
            let _ = windows_sys::Win32::Foundation::CloseHandle(handle);
            return Err(AppError::new("SetInformationJobObject failed"));
        }

        Ok(JobObject { handle })
    }
}

#[cfg(windows)]
fn windows_assign_pid(job: windows_sys::Win32::Foundation::HANDLE, pid: u32) -> AppResult<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    #[allow(unsafe_code)]
    unsafe {
        let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
        if process.is_null() || process == INVALID_HANDLE {
            return Err(AppError::new(format!(
                "OpenProcess({pid}) for job assign failed"
            )));
        }
        let ok = AssignProcessToJobObject(job, process);
        let _ = CloseHandle(process);
        if ok == 0 {
            return Err(AppError::new(format!(
                "AssignProcessToJobObject({pid}) failed"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_job_succeeds() {
        let job = JobObject::create().expect("job");
        drop(job);
    }
}
