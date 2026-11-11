//! In-process Host facade: run-state, job, window counting, shutdown.
//!
//! Shell / WindowManager talk to Host through this type instead of owning
//! session cleanup themselves (D38).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::rc::Rc;

use crate::error::{AppError, AppResult};
use crate::runtime::AppRuntime;

use super::api::{HostCommand, HostEvent, ProcessRole};
use super::job::JobObject;
use super::run_state::{current_pid, RunState};
use super::spawn::{spawn_mcp_daemon, spawn_session_worker};

/// Shared Host service (one per application generation).
pub struct HostService {
    config_dir: PathBuf,
    runtime: Rc<RefCell<AppRuntime>>,
    run_state: RunState,
    job: Option<JobObject>,
    window_count: u32,
    shutdown_requested: bool,
    pending_events: Vec<HostEvent>,
    /// Owned child processes (SessionWorker / McpDaemon). Dropping Host waits are
    /// best-effort via Job Object kill-on-close; we keep handles to avoid zombies.
    children: Vec<Child>,
}

impl HostService {
    /// Starts (or reclaims into) a Host generation wrapping `runtime`.
    pub fn start(runtime: AppRuntime) -> AppResult<Rc<RefCell<Self>>> {
        let config_dir = runtime.config_dir.clone();
        let self_pid = current_pid();
        let generation = match RunState::load(&config_dir)? {
            Some(stale) => {
                if stale.host_pid != self_pid {
                    let killed = RunState::reclaim_orphans(&stale, self_pid);
                    if killed > 0 {
                        tracing::warn!(
                            killed,
                            generation = stale.generation,
                            "reclaimed orphaned yshell processes from prior run-state"
                        );
                    }
                }
                stale.generation.saturating_add(1).max(1)
            }
            None => 1,
        };
        let _ = RunState::clear(&config_dir);
        let run_state = RunState::new(generation, self_pid);
        run_state.save(&config_dir)?;

        let job = match JobObject::create() {
            Ok(job) => Some(job),
            Err(error) => {
                tracing::warn!(error = %error, "failed to create Host job object");
                None
            }
        };

        Ok(Rc::new(RefCell::new(Self {
            config_dir,
            runtime: Rc::new(RefCell::new(runtime)),
            run_state,
            job,
            window_count: 0,
            shutdown_requested: false,
            pending_events: Vec::new(),
            children: Vec::new(),
        })))
    }

    /// Spawn a SessionWorker child and register it in run-state / job.
    #[allow(dead_code)] // D38 Phase 2+ out-of-process path
    pub fn start_session_worker_process(&mut self, session_id: &str) -> AppResult<u32> {
        let child = spawn_session_worker(&self.config_dir, session_id, self.run_state.generation)?;
        let pid = child.id();
        self.dispatch(HostCommand::WorkerStarted {
            session_id: session_id.to_owned(),
            pid,
        })?;
        self.children.push(child);
        Ok(pid)
    }

    /// Spawn an MCP daemon child (out-of-process listener path).
    #[allow(dead_code)] // D38 Phase 3+ out-of-process path
    pub fn start_mcp_daemon_process(
        &mut self,
        bind_host: &str,
        port: u16,
        token: &str,
    ) -> AppResult<u32> {
        let child = spawn_mcp_daemon(
            &self.config_dir,
            bind_host,
            port,
            token,
            self.run_state.generation,
        )?;
        let pid = child.id();
        self.dispatch(HostCommand::McpStarted { pid })?;
        self.children.push(child);
        Ok(pid)
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.run_state.generation
    }

    #[must_use]
    pub fn runtime(&self) -> Rc<RefCell<AppRuntime>> {
        Rc::clone(&self.runtime)
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn window_count(&self) -> u32 {
        self.window_count
    }

    #[must_use]
    #[allow(dead_code)] // used by HostService unit tests
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    /// Assign a child pid to the Host job (best-effort).
    pub fn track_child_pid(&self, pid: u32) -> AppResult<()> {
        if let Some(job) = self.job.as_ref() {
            job.assign_pid(pid)?;
        }
        Ok(())
    }

    pub fn dispatch(&mut self, command: HostCommand) -> AppResult<()> {
        match command {
            HostCommand::SetWindowCount { count } => {
                self.window_count = count;
                if count == 0 {
                    self.begin_shutdown()?;
                }
            }
            HostCommand::RequestShutdown => {
                self.begin_shutdown()?;
            }
            HostCommand::WorkerStarted { session_id, pid } => {
                // Never assign the Host process itself into its own job.
                if pid != current_pid() {
                    let _ = self.track_child_pid(pid);
                }
                self.run_state.upsert_worker(session_id, pid);
                self.persist_run_state()?;
            }
            HostCommand::WorkerExited { session_id } => {
                self.run_state.remove_worker(&session_id);
                self.persist_run_state()?;
                self.pending_events
                    .push(HostEvent::WorkerFaulted { session_id });
            }
            HostCommand::McpStarted { pid } => {
                if pid != current_pid() {
                    let _ = self.track_child_pid(pid);
                }
                self.run_state.mcp_pid = Some(pid);
                self.persist_run_state()?;
                self.pending_events.push(HostEvent::McpStatus {
                    ready: true,
                    detail: format!("mcp pid {pid}"),
                });
            }
            HostCommand::McpExited => {
                self.run_state.mcp_pid = None;
                self.persist_run_state()?;
                self.pending_events.push(HostEvent::McpStatus {
                    ready: false,
                    detail: "mcp exited".to_owned(),
                });
            }
            HostCommand::Heartbeat {
                generation,
                role,
                pid,
            } => {
                if generation != self.run_state.generation {
                    return Err(AppError::new(format!(
                        "heartbeat generation mismatch from {role} pid {pid}: got {generation}, host {}",
                        self.run_state.generation
                    )));
                }
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn take_events(&mut self) -> Vec<HostEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Host shutdown sequence: clear run-state and signal listeners.
    pub fn begin_shutdown(&mut self) -> AppResult<()> {
        if self.shutdown_requested {
            return Ok(());
        }
        self.shutdown_requested = true;
        self.pending_events.push(HostEvent::ShutdownStarted);
        // Child processes die with the Job Object when Host exits. Clear the
        // run-state so a crash mid-exit does not leave a false live map.
        RunState::clear(&self.config_dir)?;
        tracing::info!(
            generation = self.run_state.generation,
            role = ProcessRole::Host.as_str(),
            "host shutdown sequence started"
        );
        Ok(())
    }

    fn persist_run_state(&mut self) -> AppResult<()> {
        if self.shutdown_requested {
            return Ok(());
        }
        self.run_state.save(&self.config_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn runtime_in(temp: &TempDir) -> AppRuntime {
        AppRuntime::new(temp.path().to_path_buf()).expect("runtime")
    }

    #[test]
    fn zero_windows_requests_shutdown_and_clears_run_state() {
        let temp = TempDir::new().expect("temp");
        let host = HostService::start(runtime_in(&temp)).expect("host");
        assert!(RunState::path(temp.path()).exists());
        {
            let mut host = host.borrow_mut();
            host.dispatch(HostCommand::SetWindowCount { count: 1 })
                .expect("set");
            assert!(!host.shutdown_requested());
            host.dispatch(HostCommand::SetWindowCount { count: 0 })
                .expect("zero");
            assert!(host.shutdown_requested());
            let events = host.take_events();
            assert!(events
                .iter()
                .any(|event| matches!(event, HostEvent::ShutdownStarted)));
        }
        assert!(!RunState::path(temp.path()).exists());
    }

    #[test]
    fn worker_registration_persists() {
        let temp = TempDir::new().expect("temp");
        let host = HostService::start(runtime_in(&temp)).expect("host");
        // Use a non-self pid so we do not AssignProcessToJobObject(self).
        host.borrow_mut()
            .dispatch(HostCommand::WorkerStarted {
                session_id: "ses_1".to_owned(),
                pid: 1,
            })
            .expect("worker");
        let loaded = RunState::load(temp.path()).expect("load").expect("some");
        assert_eq!(loaded.workers.len(), 1);
        assert_eq!(loaded.workers[0].session_id, "ses_1");
        assert_eq!(loaded.workers[0].pid, 1);
    }
}
