//! In-process Host facade: run-state, job, window counting, shutdown.
//!
//! Shell / WindowManager talk to Host through this type instead of owning
//! session cleanup themselves (D38).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::error::{AppError, AppResult};
use crate::mcp_server::McpInbox;
use crate::runtime::AppRuntime;

use super::api::{HostCommand, HostEvent, ProcessRole};
use super::bridge::HostBridgeHandle;
use super::job::JobObject;
use super::lock::InstanceLock;
use super::run_state::{current_pid, RunState};
use super::runtime_dispatch;
use super::shell_inbox::ShellInbox;
use super::shell_ipc::{capture_terminal_snapshot, projection_json, ShellRegistry};
use super::shell_jobs::{ShellJob, ShellJobInbox};
use super::spawn::spawn_mcp_daemon;

/// Which role a tracked child process plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChildRole {
    SessionWorker,
    McpDaemon,
}

/// A Host-owned child process plus the run-state identity it belongs to.
struct TrackedChild {
    child: Child,
    role: ChildRole,
    session_id: Option<String>,
}

/// Shared Host service (one per application generation).
pub struct HostService {
    config_dir: PathBuf,
    runtime: Rc<RefCell<AppRuntime>>,
    run_state: RunState,
    job: Option<JobObject>,
    window_count: u32,
    shutdown_requested: bool,
    pending_events: Vec<HostEvent>,
    /// Owned child processes (SessionWorker / McpDaemon), reaped on the
    /// supervisor tick so run-state never keeps stale worker pid records.
    children: Vec<TrackedChild>,
    /// Per-config-directory single-instance lock, held until the Host exits.
    _instance_lock: InstanceLock,
    /// MCP jobs from the out-of-process daemon land here; UI tick drains them.
    mcp_inbox: McpInbox,
    /// Second-instance / Shell attach commands; UI tick drains them.
    shell_inbox: ShellInbox,
    /// Shell IPC jobs (UiCall / tick / attach) drained on the Host loop.
    shell_jobs: ShellJobInbox,
    /// Attached Shell processes and aggregated window counts.
    shell_registry: ShellRegistry,
    /// Loopback bridge the MCP daemon dials into.
    bridge: Option<HostBridgeHandle>,
    /// Fingerprint of the running MCP daemon child (`bind`, port, token).
    mcp_daemon_fingerprint: Option<(String, u16, String)>,
    /// Connect-job receiver when Host owns the worker (out-of-process Host).
    connect_job_rx: Option<std::sync::mpsc::Receiver<crate::connect_jobs::ConnectJobMessage>>,
    /// SFTP transfer worker messages drained on Host (D38 remote Shell).
    sftp_job_rx: Option<std::sync::mpsc::Receiver<crate::sftp_jobs::SftpJobMessage>>,
}

impl HostService {
    /// Starts (or reclaims into) a Host generation wrapping `runtime`.
    pub fn start(runtime: AppRuntime) -> AppResult<Rc<RefCell<Self>>> {
        let config_dir = runtime.config_dir.clone();
        // One Host per config directory. The OS releases this lock if the Host
        // dies, so a stale lock can never block the next start.
        let instance_lock = InstanceLock::acquire(&config_dir)?;
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

        let mcp_inbox = McpInbox::new();
        let shell_inbox = ShellInbox::new();
        let shell_jobs = ShellJobInbox::new();
        let bridge = match super::bridge::start_host_bridge(
            config_dir.clone(),
            run_state.generation,
            mcp_inbox.clone(),
            shell_inbox.clone(),
            shell_jobs.clone(),
        ) {
            Ok(bridge) => Some(bridge),
            Err(error) => {
                tracing::warn!(error = %error, "failed to start host bridge for MCP daemon");
                None
            }
        };

        let mut runtime = runtime;
        runtime.host_generation = run_state.generation;
        let (connect_jobs, connect_job_rx) = crate::connect_jobs::start_connect_job_worker();
        runtime.connect_jobs = Some(connect_jobs);
        let parallel = runtime
            .config_document
            .ui
            .sftp_max_parallel_transfers
            .max(1);
        let (sftp_job_tx, sftp_job_rx, sftp_limiter) =
            crate::sftp_jobs::start_sftp_job_worker_with_limit(parallel);
        // The Host supervisor thread must never block on SFTP: liveness probes
        // and deferred listings run on a dedicated control worker and report
        // back to this tick.
        let sftp_control = crate::sftp_jobs::start_sftp_control_worker();
        runtime.sftp_jobs = Some(crate::sftp_jobs::SftpJobHandle::with_limiter_and_control(
            sftp_job_tx,
            sftp_limiter,
            sftp_control,
        ));
        runtime.set_sftp_parallel_transfer_limit(parallel);

        Ok(Rc::new(RefCell::new(Self {
            config_dir,
            runtime: Rc::new(RefCell::new(runtime)),
            run_state,
            job,
            window_count: 0,
            shutdown_requested: false,
            pending_events: Vec::new(),
            children: Vec::new(),
            _instance_lock: instance_lock,
            mcp_inbox,
            shell_inbox,
            shell_jobs,
            shell_registry: ShellRegistry::new(),
            bridge,
            mcp_daemon_fingerprint: None,
            connect_job_rx: Some(connect_job_rx),
            sftp_job_rx: Some(sftp_job_rx),
        })))
    }

    #[must_use]
    pub fn mcp_inbox(&self) -> McpInbox {
        self.mcp_inbox.clone()
    }

    #[must_use]
    pub fn shell_inbox(&self) -> ShellInbox {
        self.shell_inbox.clone()
    }

    #[must_use]
    #[allow(dead_code)] // bridge owns a clone; exposed for tests / tooling
    pub fn shell_jobs(&self) -> ShellJobInbox {
        self.shell_jobs.clone()
    }

    /// Take the connect-job receiver for in-process WindowManager draining.
    pub fn take_connect_job_rx(
        &mut self,
    ) -> Option<std::sync::mpsc::Receiver<crate::connect_jobs::ConnectJobMessage>> {
        self.connect_job_rx.take()
    }

    /// Take the SFTP job receiver for in-process WindowManager draining.
    pub fn take_sftp_job_rx(
        &mut self,
    ) -> Option<std::sync::mpsc::Receiver<crate::sftp_jobs::SftpJobMessage>> {
        self.sftp_job_rx.take()
    }

    /// One Host supervisor iteration: MCP, connect jobs, shell IPC, worker adopt.
    ///
    /// Returns `true` when shutdown was requested (caller should exit).
    pub fn supervisor_tick(&mut self) -> AppResult<bool> {
        self.poll_children();
        {
            let mut runtime = self.runtime.borrow_mut();
            self.mcp_inbox.drain(&mut runtime);
            if let Some(rx) = self.connect_job_rx.as_ref() {
                while let Ok(message) = rx.try_recv() {
                    let _ = runtime.apply_connect_job_message(message);
                }
            }
            if let Some(rx) = self.sftp_job_rx.as_ref() {
                while let Ok(message) = rx.try_recv() {
                    let _ = runtime.apply_sftp_job_message(message);
                }
            }
            // Probe/listing work is submitted to the off-thread SFTP worker;
            // this tick only drains and applies what came back.
            let _ = runtime.pump_sftp_control();
            let pending = std::mem::take(&mut runtime.pending_session_workers);
            drop(runtime);
            for worker in pending {
                let _ = self.adopt_session_worker(worker);
            }
        }
        self.drain_shell_jobs()?;
        // Reconcile MCP from runtime settings.
        {
            let enabled = self.runtime.borrow().mcp_enabled();
            if !enabled {
                let _ = self.reconcile_mcp_daemon(false, "", 0, "");
            } else if self.runtime.borrow().mcp_validate_listen_config().is_ok() {
                let (bind, port, token) = {
                    let runtime = self.runtime.borrow();
                    (
                        runtime.config_document.ui.mcp_listen.bind_host().to_owned(),
                        runtime.config_document.ui.mcp_port,
                        runtime.mcp_resolve_endpoint_token(),
                    )
                };
                if self
                    .reconcile_mcp_daemon(true, &bind, port, &token)
                    .unwrap_or(false)
                {
                    let mut runtime = self.runtime.borrow_mut();
                    runtime.mcp_endpoint_token = Some(token);
                    runtime.set_mcp_listener_status(
                        crate::runtime::mcp::McpListenerStatus::Ready { port },
                    );
                }
            }
        }
        self.shell_registry
            .prune_stale(std::time::Duration::from_secs(45));
        let windows = self.shell_registry.total_windows();
        if windows != self.window_count {
            self.dispatch(HostCommand::SetWindowCount { count: windows })?;
        }
        // Headless Host with zero shells after at least one attach cycle: if no
        // shells are registered and shutdown was not requested, keep waiting —
        // Shell may still be starting. Zero windows from an attached shell
        // triggers shutdown via SetWindowCount.
        Ok(self.shutdown_requested)
    }

    /// Reap exited SessionWorker / McpDaemon children and update run-state.
    ///
    /// `Child` handles are waited on here; without this a dead worker would stay
    /// in `run-state.json` forever and a later Host could kill a reused pid.
    fn poll_children(&mut self) {
        let mut exited: Vec<(ChildRole, Option<String>, String)> = Vec::new();
        self.children
            .retain_mut(|tracked| match tracked.child.try_wait() {
                Ok(Some(status)) => {
                    exited.push((tracked.role, tracked.session_id.clone(), status.to_string()));
                    false
                }
                Ok(None) => true,
                Err(error) => {
                    exited.push((
                        tracked.role,
                        tracked.session_id.clone(),
                        format!("wait failed: {error}"),
                    ));
                    false
                }
            });
        for (role, session_id, detail) in exited {
            tracing::info!(
                role = ?role,
                session_id = session_id.as_deref().unwrap_or(""),
                detail,
                "host child exited"
            );
            match role {
                ChildRole::SessionWorker => {
                    if let Some(session_id) = session_id {
                        let _ = self.dispatch(HostCommand::WorkerExited { session_id });
                    }
                }
                ChildRole::McpDaemon => {
                    let _ = self.dispatch(HostCommand::McpExited);
                }
            }
        }
    }

    fn drain_shell_jobs(&mut self) -> AppResult<()> {
        while let Some(job) = self.shell_jobs.try_pop() {
            match job {
                ShellJob::Attach {
                    pid,
                    window_count,
                    reply,
                } => {
                    self.shell_registry.attach(pid, window_count);
                    let _ = reply.send(Ok((self.run_state.generation, current_pid())));
                }
                ShellJob::Detach { pid } => {
                    self.shell_registry.detach(pid);
                    if self.shell_registry.shell_count() == 0 {
                        self.dispatch(HostCommand::SetWindowCount { count: 0 })?;
                    }
                }
                ShellJob::SetWindowCount { pid, count } => {
                    self.shell_registry.set_window_count(pid, count);
                }
                ShellJob::Heartbeat { pid, window_count } => {
                    self.shell_registry.heartbeat(pid, window_count);
                }
                ShellJob::UiCall {
                    method,
                    args,
                    reply,
                } => {
                    let mut runtime = self.runtime.borrow_mut();
                    let result = runtime_dispatch::dispatch(&mut runtime, &method, args);
                    if result.is_ok()
                        && !matches!(
                            method.as_str(),
                            "projection"
                                | "projection_for_window"
                                | "take_ui_dirty"
                                | "poll_all_terminal_outputs"
                                | "reduce_motion"
                                | "ensure_debug_log_dir"
                                | "selected_private_key_public_line"
                                | "generated_private_key_public_line"
                                | "active_session_logging_path"
                                | "mcp_enabled"
                                | "ui_language"
                                | "main_window_id"
                                | "window_of_tab"
                                | "auto_copy_terminal_selection"
                        )
                    {
                        runtime.mark_ui_dirty();
                    }
                    let _ = reply.send(result);
                }
                ShellJob::HostTick { reply } => {
                    let dirty = {
                        let mut runtime = self.runtime.borrow_mut();
                        let mut dirty = false;
                        if let Some(rx) = self.connect_job_rx.as_ref() {
                            while let Ok(message) = rx.try_recv() {
                                let _ = runtime.apply_connect_job_message(message);
                                dirty = true;
                            }
                        }
                        if let Some(rx) = self.sftp_job_rx.as_ref() {
                            while let Ok(message) = rx.try_recv() {
                                let _ = runtime.apply_sftp_job_message(message);
                                dirty = true;
                            }
                        }
                        match runtime.poll_all_terminal_outputs() {
                            Ok(Some(_)) => dirty = true,
                            Ok(None) => {}
                            Err(error) => {
                                tracing::warn!(error = %error, "host terminal poll failed");
                            }
                        }
                        // SFTP probes/listings run on the control worker; the
                        // Host tick only queues them and applies their results,
                        // so a slow/hung SFTP operation can no longer freeze
                        // Shell UiCall/HostTick handling.
                        if runtime.pump_sftp_control() {
                            dirty = true;
                        }
                        if runtime.poll_proxy_test_result() {
                            dirty = true;
                        }
                        if runtime.pump_login_script() {
                            dirty = true;
                        }
                        if runtime.take_ui_dirty() {
                            dirty = true;
                        }
                        dirty
                    };
                    let pending = {
                        let mut runtime = self.runtime.borrow_mut();
                        std::mem::take(&mut runtime.pending_session_workers)
                    };
                    for worker in pending {
                        let _ = self.adopt_session_worker(worker);
                    }
                    let _ = reply.send(Ok((
                        dirty,
                        self.shell_registry.total_windows(),
                        self.shutdown_requested,
                    )));
                }
                ShellJob::GetProjection { window_id, reply } => {
                    let runtime = self.runtime.borrow();
                    let value = projection_json(&runtime.projection_for_window(window_id));
                    let _ = reply.send(Ok(value));
                }
                ShellJob::GetTerminalSnapshot {
                    window_id,
                    secondary,
                    reply,
                } => {
                    let runtime = self.runtime.borrow();
                    let snap = capture_terminal_snapshot(&runtime, window_id, secondary);
                    let _ = reply.send(Ok(snap));
                }
            }
        }
        Ok(())
    }

    /// Adopt a SessionWorker already spawned by the connect path.
    pub fn adopt_session_worker(
        &mut self,
        worker: super::session_ipc::SessionWorkerChild,
    ) -> AppResult<()> {
        let session_id = worker.session_id;
        let pid = worker.pid;
        self.dispatch(HostCommand::WorkerStarted {
            session_id: session_id.clone(),
            pid,
        })?;
        self.children.push(TrackedChild {
            child: worker.child,
            role: ChildRole::SessionWorker,
            session_id: Some(session_id),
        });
        Ok(())
    }

    /// Ensure an out-of-process MCP daemon matches Settings bind/port/token.
    ///
    /// Returns `Ok(true)` when the listener status should be refreshed.
    pub fn reconcile_mcp_daemon(
        &mut self,
        enabled: bool,
        bind_host: &str,
        port: u16,
        token: &str,
    ) -> AppResult<bool> {
        if !enabled {
            return Ok(self.stop_mcp_daemon());
        }
        if self.bridge.is_none() {
            return Err(AppError::new(
                "host bridge is not running; cannot start MCP daemon",
            ));
        }
        if self
            .mcp_daemon_fingerprint
            .as_ref()
            .is_some_and(|(h, p, t)| h == bind_host && *p == port && t == token)
            && self
                .run_state
                .mcp_pid
                .is_some_and(super::run_state::process_is_alive)
        {
            return Ok(false);
        }
        let _ = self.stop_mcp_daemon();
        let child = spawn_mcp_daemon(
            &self.config_dir,
            bind_host,
            port,
            token,
            self.run_state.generation,
        )?;
        let pid = child.id();
        if pid != current_pid() {
            let _ = self.track_child_pid(pid);
        }
        self.run_state.set_mcp_pid(pid);
        self.persist_run_state()?;
        self.mcp_daemon_fingerprint = Some((bind_host.to_owned(), port, token.to_owned()));
        self.children.push(TrackedChild {
            child,
            role: ChildRole::McpDaemon,
            session_id: None,
        });
        self.pending_events.push(HostEvent::McpStatus {
            ready: true,
            detail: format!("mcp-daemon pid {pid} on {bind_host}:{port}"),
        });
        Ok(true)
    }

    fn stop_mcp_daemon(&mut self) -> bool {
        let had = self.mcp_daemon_fingerprint.take().is_some() || self.run_state.mcp_pid.is_some();
        if !had {
            // Called from every supervisor tick while MCP is disabled; do not
            // rewrite run-state / clear the endpoint when nothing is running.
            return false;
        }
        if let Some(pid) = self.run_state.mcp_pid {
            let _ = super::run_state::terminate_pid(pid);
            self.children.retain(|tracked| tracked.child.id() != pid);
        }
        self.run_state.clear_mcp_pid();
        let _ = self.persist_run_state();
        let _ = yshell_mcp::clear_endpoint(&self.config_dir);
        had
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
    pub fn window_count(&self) -> u32 {
        self.window_count
    }

    #[must_use]
    pub fn shell_registry_shell_count(&self) -> usize {
        self.shell_registry.shell_count()
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
                // Surface the crash to the Shell through the projection instead
                // of leaving the tab "connected" until a poll notices.
                self.runtime
                    .borrow_mut()
                    .mark_session_worker_exited(&session_id);
                self.pending_events
                    .push(HostEvent::WorkerFaulted { session_id });
            }
            HostCommand::McpStarted { pid } => {
                if pid != current_pid() {
                    let _ = self.track_child_pid(pid);
                }
                self.run_state.set_mcp_pid(pid);
                self.persist_run_state()?;
                self.pending_events.push(HostEvent::McpStatus {
                    ready: true,
                    detail: format!("mcp pid {pid}"),
                });
            }
            HostCommand::McpExited => {
                self.run_state.clear_mcp_pid();
                self.persist_run_state()?;
                self.runtime.borrow_mut().set_mcp_listener_status(
                    crate::runtime::mcp::McpListenerStatus::Failed {
                        message: "MCP daemon exited unexpectedly".to_owned(),
                    },
                );
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

    /// Host shutdown sequence: stop MCP, disconnect workers, force-kill leftovers,
    /// clear run-state and close the bridge.
    ///
    /// Workers are asked to disconnect first (their shell handles are dropped),
    /// then given a short grace period before being terminated. The Job Object
    /// remains the final safety net on Windows.
    pub fn begin_shutdown(&mut self) -> AppResult<()> {
        if self.shutdown_requested {
            return Ok(());
        }
        self.shutdown_requested = true;
        self.pending_events.push(HostEvent::ShutdownStarted);
        // 1. MCP daemon first: it holds no session state.
        let _ = self.stop_mcp_daemon();
        // 2. Ask workers to disconnect and give them a moment to exit.
        self.runtime.borrow_mut().disconnect_all_sessions();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.children.is_empty() && Instant::now() < deadline {
            self.poll_children();
            if !self.children.is_empty() {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        // 3. Force-kill anything that ignored the disconnect.
        for tracked in &mut self.children {
            let _ = super::run_state::force_terminate_pid(tracked.child.id());
        }
        self.children.clear();
        // 4. Close the bridge so second launches stop finding this Host.
        if let Some(bridge) = self.bridge.take() {
            bridge.shutdown();
        }
        // 5. Clear run-state so a crash mid-exit does not leave a false live map.
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
