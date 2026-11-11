//! Windows Job Object wrapper so Host death cascades to Worker / MCP children.
//!
//! Non-Windows builds expose a no-op handle; orphans are still covered by heartbeat
//! and `run-state` reclaim (see `docs/prototypes/process-architecture.md`).

use crate::error::{AppError, AppResult};

/// Process job that kills assigned children when the Host process exits.
#[derive(Debug)]
pub struct JobObject {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

impl JobObject {
    /// Creates a job with `KILL_ON_JOB_CLOSE`.
    pub fn create() -> AppResult<Self> {
        #[cfg(windows)]
        {
            windows_create_job()
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }

    /// Assigns an existing process id to this job.
    pub fn assign_pid(&self, pid: u32) -> AppResult<()> {
        if pid == 0 {
            return Err(AppError::new("cannot assign pid 0 to job"));
        }
        #[cfg(windows)]
        {
            windows_assign_pid(self.handle, pid)
        }
        #[cfg(not(windows))]
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
