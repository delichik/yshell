//! Async SSH connect jobs (WP1): run the handshake on a worker thread.
//!
//! Pattern mirrors [`crate::sftp_jobs`]: the UI thread submits a job and drains
//! [`ConnectJobMessage`]s from the app Timer. Stage progress comes from
//! [`yshell_ssh::with_connection_stage_hook`] hooked into `RealConnectionAttempt`.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, Sender},
    Arc,
};

use yshell_ssh::{
    with_connection_stage_hook, RealConnectionStage, ShellClient, ShellSession, SshConnectionConfig,
    SshError, TransportBackend,
};

/// Work item for the connect worker.
#[derive(Debug, Clone)]
pub struct ConnectJobSpec {
    pub session_key: String,
    pub ssh_config: SshConnectionConfig,
    pub transport_backend: TransportBackend,
    /// When true, Finished failure uses the password-retry status kind.
    pub password_retry: bool,
    /// When true, success status uses the reconnect wording.
    pub is_reconnect: bool,
}

/// Messages the worker sends back to the UI thread.
#[derive(Debug)]
pub enum ConnectJobMessage {
    Progress {
        session_key: String,
        /// Overlay stage id: `tcp` / `handshake` / `host-key` / `auth` / `shell`.
        stage: String,
    },
    Finished {
        session_key: String,
        password_retry: bool,
        is_reconnect: bool,
        outcome: ConnectJobOutcome,
    },
}

/// How a connect job ended.
#[derive(Debug)]
pub enum ConnectJobOutcome {
    Succeeded {
        shell: Box<dyn ShellSession>,
    },
    Failed {
        error: SshError,
    },
    Cancelled,
}

/// UI-thread handle: submit / cancel / forget connect jobs.
#[derive(Debug)]
pub struct ConnectJobHandle {
    tx: Sender<ConnectJob>,
    cancels: HashMap<String, Arc<AtomicBool>>,
}

struct ConnectJob {
    spec: ConnectJobSpec,
    cancel: Arc<AtomicBool>,
}

impl ConnectJobHandle {
    fn new(tx: Sender<ConnectJob>) -> Self {
        Self {
            tx,
            cancels: HashMap::new(),
        }
    }

    /// Submits a connect job. Replaces any in-flight cancel flag for the same session.
    pub fn submit(&mut self, spec: ConnectJobSpec) -> bool {
        let session_key = spec.session_key.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = ConnectJob {
            spec,
            cancel: cancel.clone(),
        };
        if self.tx.send(job).is_err() {
            return false;
        }
        self.cancels.insert(session_key, cancel);
        true
    }

    pub fn cancel(&mut self, session_key: &str) -> bool {
        match self.cancels.get(session_key) {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    pub fn forget(&mut self, session_key: &str) {
        self.cancels.remove(session_key);
    }
}

/// Spawns the connect worker and returns the UI handle plus drain receiver.
pub fn start_connect_job_worker() -> (ConnectJobHandle, Receiver<ConnectJobMessage>) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<ConnectJob>();
    let (msg_tx, msg_rx) = std::sync::mpsc::channel::<ConnectJobMessage>();
    let spawned = std::thread::Builder::new()
        .name("yshell-connect-jobs".to_owned())
        .spawn(move || worker_loop(job_rx, msg_tx));
    if spawned.is_err() {
        // Channel still works; submissions fail when the receiver is gone.
    }
    (ConnectJobHandle::new(job_tx), msg_rx)
}

fn worker_loop(job_rx: Receiver<ConnectJob>, msg_tx: Sender<ConnectJobMessage>) {
    while let Ok(job) = job_rx.recv() {
        let session_key = job.spec.session_key.clone();
        let password_retry = job.spec.password_retry;
        let is_reconnect = job.spec.is_reconnect;
        let outcome = run_connect(&job, &msg_tx);
        if msg_tx
            .send(ConnectJobMessage::Finished {
                session_key,
                password_retry,
                is_reconnect,
                outcome,
            })
            .is_err()
        {
            return;
        }
    }
}

fn run_connect(job: &ConnectJob, msg_tx: &Sender<ConnectJobMessage>) -> ConnectJobOutcome {
    if job.cancel.load(Ordering::SeqCst) {
        return ConnectJobOutcome::Cancelled;
    }

    let session_key = job.spec.session_key.clone();
    let _ = msg_tx.send(ConnectJobMessage::Progress {
        session_key: session_key.clone(),
        stage: overlay_stage_id(RealConnectionStage::TcpConnect).to_owned(),
    });

    let progress_tx = msg_tx.clone();
    let progress_key = session_key.clone();
    let result = with_connection_stage_hook(
        move |stage| {
            let _ = progress_tx.send(ConnectJobMessage::Progress {
                session_key: progress_key.clone(),
                stage: overlay_stage_id(stage).to_owned(),
            });
        },
        || open_shell_for_job(&job.spec),
    );

    if job.cancel.load(Ordering::SeqCst) {
        if let Ok(mut shell) = result {
            let _ = shell.disconnect();
        }
        return ConnectJobOutcome::Cancelled;
    }

    match result {
        Ok(shell) => ConnectJobOutcome::Succeeded { shell },
        Err(error) => ConnectJobOutcome::Failed { error },
    }
}

fn open_shell_for_job(
    spec: &ConnectJobSpec,
) -> Result<Box<dyn ShellSession>, SshError> {
    match spec.transport_backend {
        TransportBackend::Fake => ShellClient::with_fake_backend().open_shell_boxed(&spec.ssh_config),
        TransportBackend::Real => ShellClient::with_real_backend().open_shell_boxed(&spec.ssh_config),
    }
}

/// Maps internal stages onto the five overlay stages the UI expects.
pub fn overlay_stage_id(stage: RealConnectionStage) -> &'static str {
    match stage {
        RealConnectionStage::Prepared
        | RealConnectionStage::ProxyNegotiation
        | RealConnectionStage::TcpConnect => "tcp",
        RealConnectionStage::SshHandshake => "handshake",
        RealConnectionStage::HostKeyVerification => "host-key",
        RealConnectionStage::Authentication => "auth",
        RealConnectionStage::PtyRequest | RealConnectionStage::ShellOpen => "shell",
        RealConnectionStage::Connected => "connected",
        RealConnectionStage::Disconnected | RealConnectionStage::Failed => "failed",
    }
}

/// Maps an [`SshError`] stage label (or kind) onto an overlay stage id.
pub fn overlay_stage_from_error(error: &SshError) -> &'static str {
    if let Some(stage) = error.connection_stage {
        return match stage {
            "prepared" | "proxy-negotiation" | "tcp-connect" => "tcp",
            "ssh-handshake" => "handshake",
            "host-key-verification" => "host-key",
            "authentication" => "auth",
            "pty-request" | "shell-open" => "shell",
            "connected" => "connected",
            _ => "failed",
        };
    }
    match error.kind {
        yshell_ssh::SshErrorKind::Dns | yshell_ssh::SshErrorKind::TcpConnect => "tcp",
        yshell_ssh::SshErrorKind::Proxy => "tcp",
        yshell_ssh::SshErrorKind::Handshake => "handshake",
        yshell_ssh::SshErrorKind::HostKeyRejected => "host-key",
        yshell_ssh::SshErrorKind::Authentication => "auth",
        yshell_ssh::SshErrorKind::Channel => "shell",
        _ => "failed",
    }
}
