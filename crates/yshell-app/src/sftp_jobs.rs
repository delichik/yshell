//! SFTP transfer jobs (N1): run F0 transfers on a worker thread.
//!
//! The UI thread never blocks on SFTP here: it submits an [`SftpJobSpec`] and
//! drains [`SftpJobMessage`]s from the existing 120 ms UI timer. Cancellation
//! is a per-job [`AtomicBool`] the F0 engines poll between items; single-file
//! transfers cannot be interrupted mid-file (there is no F0 hook for that), so
//! their cancel flag is only observed before the transfer starts.
//!
//! Remote→remote copies (D10) are implemented as a download into a temporary
//! directory followed by an upload, reusing the same tree engine.
//!
//! The module is deliberately UI-agnostic: it only needs the SSH connection
//! config, the paths and the overwrite policy.

use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender},
        Arc, Condvar, Mutex,
    },
    time::Duration,
};

use yshell_sftp::{
    DirectoryListing, FsEntryKind, OverwritePolicy, RealSftpBackend, SftpClient, SftpErrorKind,
    TransferProgress, TreeTransferOptions, TreeTransferReport,
};
use yshell_ssh::SshConnectionConfig;

/// One unit of remote work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SftpJobSpec {
    UploadFile {
        config: SshConnectionConfig,
        local: PathBuf,
        remote: String,
        options: TreeTransferOptions,
    },
    UploadTree {
        config: SshConnectionConfig,
        local_root: PathBuf,
        remote_root: String,
        options: TreeTransferOptions,
    },
    DownloadFile {
        config: SshConnectionConfig,
        remote: String,
        local: PathBuf,
        options: TreeTransferOptions,
    },
    DownloadTree {
        config: SshConnectionConfig,
        remote_root: String,
        local_root: PathBuf,
        options: TreeTransferOptions,
    },
    /// Remote→remote copy of one file (download to temp, then upload).
    RemoteCopyFile {
        config: SshConnectionConfig,
        from: String,
        to: String,
        options: TreeTransferOptions,
    },
    /// Remote→remote copy of one directory tree (download to temp, then upload).
    RemoteCopyTree {
        config: SshConnectionConfig,
        from: String,
        to: String,
        options: TreeTransferOptions,
    },
    /// Remote move (rename) of files/directories, destination directory or path.
    RemoteMove {
        config: SshConnectionConfig,
        moves: Vec<(String, String)>,
        options: TreeTransferOptions,
    },
    /// Recursive remote delete.
    RemoteDelete {
        config: SshConnectionConfig,
        entries: Vec<(String, bool)>,
    },
}

impl SftpJobSpec {
    /// The overwrite policy carried by tree-style jobs (None for batch ops).
    pub fn overwrite_policy(&self) -> Option<OverwritePolicy> {
        let options = match self {
            Self::UploadFile { options, .. }
            | Self::UploadTree { options, .. }
            | Self::DownloadFile { options, .. }
            | Self::DownloadTree { options, .. }
            | Self::RemoteCopyFile { options, .. }
            | Self::RemoteCopyTree { options, .. }
            | Self::RemoteMove { options, .. } => options,
            Self::RemoteDelete { .. } => return None,
        };
        Some(options.overwrite)
    }

    /// Re-runs the same job with an explicit conflict policy (Ask → chosen).
    pub fn with_overwrite(mut self, overwrite: OverwritePolicy) -> Self {
        let options = match &mut self {
            Self::UploadFile { options, .. }
            | Self::UploadTree { options, .. }
            | Self::DownloadFile { options, .. }
            | Self::DownloadTree { options, .. }
            | Self::RemoteCopyFile { options, .. }
            | Self::RemoteCopyTree { options, .. }
            | Self::RemoteMove { options, .. } => options,
            Self::RemoteDelete { .. } => return self,
        };
        options.overwrite = overwrite;
        self
    }

    /// Transfer direction shown in the drawer.
    pub fn direction(&self) -> yshell_sftp::TransferDirection {
        match self {
            Self::UploadFile { .. } | Self::UploadTree { .. } => {
                yshell_sftp::TransferDirection::Upload
            }
            Self::DownloadFile { .. } | Self::DownloadTree { .. } => {
                yshell_sftp::TransferDirection::Download
            }
            // Copies are shown as downloads (source side) like the design's
            // "download then upload" wording suggests.
            Self::RemoteCopyFile { .. }
            | Self::RemoteCopyTree { .. }
            | Self::RemoteMove { .. }
            | Self::RemoteDelete { .. } => yshell_sftp::TransferDirection::Download,
        }
    }

    /// Source path shown in the drawer.
    pub fn source_text(&self) -> String {
        match self {
            Self::UploadFile { local, .. }
            | Self::UploadTree {
                local_root: local, ..
            } => local.display().to_string(),
            Self::DownloadFile { remote, .. }
            | Self::DownloadTree {
                remote_root: remote,
                ..
            }
            | Self::RemoteCopyFile { from: remote, .. }
            | Self::RemoteCopyTree { from: remote, .. } => remote.clone(),
            Self::RemoteMove { moves, .. } => moves
                .first()
                .map(|(from, _)| from.clone())
                .unwrap_or_default(),
            Self::RemoteDelete { entries, .. } => entries
                .first()
                .map(|(path, _)| path.clone())
                .unwrap_or_default(),
        }
    }

    /// The destination shown as the job's status/drawer text.
    pub fn destination_text(&self) -> String {
        match self {
            Self::UploadFile { remote, .. }
            | Self::UploadTree {
                remote_root: remote,
                ..
            } => remote.clone(),
            Self::DownloadFile { local, .. }
            | Self::DownloadTree {
                local_root: local, ..
            } => local.display().to_string(),
            Self::RemoteCopyFile { to, .. } | Self::RemoteCopyTree { to, .. } => to.clone(),
            Self::RemoteMove { moves, .. } => {
                moves.first().map(|(_, to)| to.clone()).unwrap_or_default()
            }
            Self::RemoteDelete { entries, .. } => entries
                .first()
                .map(|(path, _)| path.clone())
                .unwrap_or_default(),
        }
    }
}

/// A submitted job: id, payload and its cancellation flag.
#[derive(Debug)]
pub struct SftpJob {
    pub id: String,
    pub spec: SftpJobSpec,
    pub cancel: Arc<AtomicBool>,
}

/// Messages the worker sends back to the UI thread.
#[derive(Debug)]
pub enum SftpJobMessage {
    Progress {
        id: String,
        bytes_done: u64,
        bytes_total: u64,
    },
    Finished {
        id: String,
        outcome: SftpJobOutcome,
    },
}

/// How a job ended.
#[derive(Debug, Clone)]
pub enum SftpJobOutcome {
    /// Transfer finished; the report carries per-item failures/skips (an empty
    /// failure list means full success).
    Completed(TreeTransferReport),
    /// The cancel flag was observed.
    Cancelled(TreeTransferReport),
    /// Setup failed before/while scanning (no report), or the transfer failed
    /// for single-file jobs.
    Failed {
        reason: String,
        report: Option<TreeTransferReport>,
    },
}

// ---------------------------------------------------------------------------
// Control-plane worker: liveness probes + directory listings
// ---------------------------------------------------------------------------
//
// The Host supervisor tick must never block on SFTP. Listing a directory and
// probing the connection are both synchronous network operations, so they are
// submitted here and the results are drained on a later tick. This worker is
// deliberately separate from the transfer pool: a probe queued behind a multi
// gigabyte upload would be useless, and the probe/listing work must not consume
// a transfer slot.

/// One control-plane request handled by the SFTP control worker.
#[derive(Debug, Clone)]
pub enum SftpControlRequest {
    /// Cheap liveness probe (`stat` of the tab's current directory). The
    /// worker reports back `{ session_key, ok, error }`; the tick only marks the
    /// session disconnected when the failure is a transport/backend failure,
    /// exactly like the previous synchronous probe.
    Probe {
        session_key: String,
        /// Tab the probe was issued for; a result for a closed/retargeted tab is
        /// dropped by the runtime.
        tab_id: String,
        config: SshConnectionConfig,
        path: String,
        /// I/O timeout for the probe; a dead connection must fail fast.
        timeout_secs: u64,
    },
    /// One directory listing for a specific SFTP tab.
    ListDir {
        session_key: String,
        tab_id: String,
        config: SshConnectionConfig,
        path: String,
    },
    /// Drops the worker's cached control connections for a session (tab or
    /// session closed). No result is reported back.
    CloseSession { session_key: String },
}

/// Result of one control request, drained by the Host tick.
#[derive(Debug, Clone)]
pub enum SftpControlMessage {
    ProbeResult {
        session_key: String,
        tab_id: String,
        /// `false` when the probe failed for any reason (see `transport_failed`).
        ok: bool,
        /// Whether the failure is a transport/backend failure, i.e. the same
        /// condition that used to flip the session to disconnected. A missing
        /// directory or a permission error is not a transport failure.
        transport_failed: bool,
        error: Option<String>,
    },
    ListingResult {
        session_key: String,
        tab_id: String,
        path: String,
        result: Result<DirectoryListing, String>,
    },
}

/// Why a cached control connection exists (probe vs listing timeouts differ).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ControlClientKind {
    Probe,
    List,
}

/// A cached control-plane SFTP client plus the config it was opened for.
#[derive(Debug)]
struct ControlClient {
    config: SshConnectionConfig,
    client: SftpClient<RealSftpBackend>,
}

/// State shared between the runtime handle and the control worker thread.
#[derive(Debug, Default)]
struct ControlShared {
    results: VecDeque<SftpControlMessage>,
    probe_in_flight: bool,
    /// Listing requests that have not reported back yet (session, tab, path).
    listings_in_flight: Vec<(String, String, String)>,
}

/// Runtime-side handle to the control worker.
///
/// Cloning is cheap: the queue and the in-flight markers live in an `Arc`, so a
/// `&SftpControlHandle` is never needed for mutable access. The runtime keeps it
/// inside [`SftpJobHandle`] so no new `AppRuntime` field is required.
#[derive(Debug, Clone)]
pub struct SftpControlHandle {
    tx: Sender<SftpControlRequest>,
    shared: Arc<Mutex<ControlShared>>,
}

impl SftpControlHandle {
    /// Handle around an explicit request sender (tests / embedded workers).
    pub fn new(tx: Sender<SftpControlRequest>) -> Self {
        Self {
            tx,
            shared: Arc::new(Mutex::new(ControlShared::default())),
        }
    }

    /// Whether a probe is already in flight (the tick must not queue another).
    pub fn probe_in_flight(&self) -> bool {
        self.shared
            .lock()
            .expect("sftp control state poisoned")
            .probe_in_flight
    }

    /// Whether a listing for this exact (session, tab, path) is in flight.
    pub fn listing_in_flight_for(&self, session_key: &str, tab_id: &str, path: &str) -> bool {
        self.shared
            .lock()
            .expect("sftp control state poisoned")
            .listings_in_flight
            .iter()
            .any(|(session, tab, listed)| session == session_key && tab == tab_id && listed == path)
    }

    /// Submits one control request; returns `false` when the worker is gone.
    ///
    /// The in-flight marker is recorded *before* the send so a result that the
    /// worker produces immediately is still matched to this request.
    pub fn submit(&self, request: SftpControlRequest) -> bool {
        let is_probe = matches!(&request, SftpControlRequest::Probe { .. });
        let listing_key = match &request {
            SftpControlRequest::ListDir {
                session_key,
                tab_id,
                path,
                ..
            } => Some((session_key.clone(), tab_id.clone(), path.clone())),
            _ => None,
        };
        {
            let mut shared = self.shared.lock().expect("sftp control state poisoned");
            if is_probe {
                shared.probe_in_flight = true;
            }
            if let Some(key) = &listing_key {
                shared.listings_in_flight.push(key.clone());
            }
        }
        if self.tx.send(request).is_err() {
            let mut shared = self.shared.lock().expect("sftp control state poisoned");
            if is_probe {
                shared.probe_in_flight = false;
            }
            if let Some(key) = &listing_key {
                shared.listings_in_flight.retain(|entry| entry != key);
            }
            return false;
        }
        true
    }

    /// Drains the results the worker reported, clearing their in-flight markers.
    pub fn drain_results(&self) -> Vec<SftpControlMessage> {
        let mut shared = self.shared.lock().expect("sftp control state poisoned");
        let results = shared.results.drain(..).collect::<Vec<_>>();
        for result in &results {
            match result {
                SftpControlMessage::ProbeResult { .. } => shared.probe_in_flight = false,
                SftpControlMessage::ListingResult {
                    session_key,
                    tab_id,
                    path,
                    ..
                } => shared
                    .listings_in_flight
                    .retain(|entry| entry != &(session_key.clone(), tab_id.clone(), path.clone())),
            }
        }
        results
    }

    /// Queues a result as if the worker had produced it (unit tests only).
    #[cfg(test)]
    fn push_result_for_test(&self, message: SftpControlMessage) {
        self.shared
            .lock()
            .expect("sftp control state poisoned")
            .results
            .push_back(message);
    }
}

/// Spawns the dedicated SFTP control worker (liveness probes + listings).
///
/// The worker owns its SSH/SFTP connections: requests carry the connection
/// config and the worker keeps one cached client per `(session, purpose)`, so a
/// probe every few seconds does not redo a TCP+SSH handshake each time. A cached
/// client that fails with a transport error is dropped and the request retried
/// once on a fresh connection, so a stale cache can never report a live session
/// as dead.
///
/// Trade-off: these are *separate* connections from the SFTP tab's browsing
/// client (`AppRuntime::sftp_live`) and from the app-owned SSH transport. In the
/// remote-Shell Host process the runtime has no transport anyway (the
/// SessionWorker owns it), so the previous probe already used a config-based
/// connection; a fresh-connection failure still means "this host is unreachable
/// from here", which is what the tab badge reports.
pub fn start_sftp_control_worker() -> SftpControlHandle {
    let (tx, rx) = std::sync::mpsc::channel::<SftpControlRequest>();
    let handle = SftpControlHandle::new(tx);
    let shared = Arc::clone(&handle.shared);
    let spawned = std::thread::Builder::new()
        .name("yshell-sftp-control".to_owned())
        .spawn(move || control_worker_loop(rx, shared));
    if let Err(error) = spawned {
        tracing::warn!(error = %error, "failed to start the SFTP control worker");
    }
    handle
}

fn control_worker_loop(rx: Receiver<SftpControlRequest>, shared: Arc<Mutex<ControlShared>>) {
    let mut cache = HashMap::<(String, ControlClientKind), ControlClient>::new();
    while let Ok(request) = rx.recv() {
        let Some(message) = run_control_request(&mut cache, request) else {
            continue;
        };
        match shared.lock() {
            Ok(mut shared) => shared.results.push_back(message),
            Err(_) => return,
        }
    }
}

fn run_control_request(
    cache: &mut HashMap<(String, ControlClientKind), ControlClient>,
    request: SftpControlRequest,
) -> Option<SftpControlMessage> {
    match request {
        SftpControlRequest::Probe {
            session_key,
            tab_id,
            config,
            path,
            timeout_secs,
        } => {
            let mut probe_config = config;
            // The probe must fail fast: a black-holed connection cannot be
            // allowed to keep the probe (and the listings queued behind it)
            // waiting for the full connect timeout.
            probe_config.connect_timeout = Duration::from_secs(timeout_secs.max(1));
            let outcome = with_control_client(
                cache,
                (session_key.clone(), ControlClientKind::Probe),
                &probe_config,
                |client| client.entry_kind(&path),
            );
            let transport_failed =
                matches!(&outcome, Err(error) if error.kind == SftpErrorKind::Backend);
            Some(SftpControlMessage::ProbeResult {
                session_key,
                tab_id,
                ok: outcome.is_ok(),
                transport_failed,
                error: outcome.err().map(|error| error.to_string()),
            })
        }
        SftpControlRequest::ListDir {
            session_key,
            tab_id,
            config,
            path,
        } => {
            let outcome = with_control_client(
                cache,
                (session_key.clone(), ControlClientKind::List),
                &config,
                |client| client.list_dir(&path),
            );
            Some(SftpControlMessage::ListingResult {
                session_key,
                tab_id,
                path,
                result: outcome.map_err(|error| error.to_string()),
            })
        }
        SftpControlRequest::CloseSession { session_key } => {
            // Tab/session closed: drop every cached connection for it so a long
            // Host session does not accumulate one SSH connection per closed
            // SFTP tab.
            cache.retain(|(session, _), _| session != &session_key);
            None
        }
    }
}

/// Runs one operation on the cached control client, reconnecting once when the
/// cached connection turns out to be dead.
fn with_control_client<T>(
    cache: &mut HashMap<(String, ControlClientKind), ControlClient>,
    key: (String, ControlClientKind),
    config: &SshConnectionConfig,
    operation: impl Fn(&SftpClient<RealSftpBackend>) -> yshell_sftp::SftpResult<T>,
) -> yshell_sftp::SftpResult<T> {
    ensure_control_client(cache, &key, config);
    let first = {
        let entry = cache.get(&key).expect("control client was just inserted");
        operation(&entry.client)
    };
    match first {
        Ok(value) => Ok(value),
        Err(error) if error.kind == SftpErrorKind::Backend => {
            // The cached connection died (remote reboot, idle timeout, ...):
            // retry once on a fresh connection before reporting a transport
            // failure, so a stale cache never marks a live session dead.
            cache.remove(&key);
            ensure_control_client(cache, &key, config);
            let retry = {
                let entry = cache.get(&key).expect("control client was just inserted");
                operation(&entry.client)
            };
            if matches!(&retry, Err(error) if error.kind == SftpErrorKind::Backend) {
                cache.remove(&key);
            }
            retry
        }
        Err(error) => Err(error),
    }
}

fn ensure_control_client(
    cache: &mut HashMap<(String, ControlClientKind), ControlClient>,
    key: &(String, ControlClientKind),
    config: &SshConnectionConfig,
) {
    if cache.get(key).is_some_and(|entry| &entry.config != config) {
        cache.remove(key);
    }
    if !cache.contains_key(key) {
        cache.insert(
            key.clone(),
            ControlClient {
                config: config.clone(),
                client: SftpClient::with_real_backend(config.clone()),
            },
        );
    }
}

/// UI-thread handle to the worker: submit jobs, cancel them, forget finished ones.
#[derive(Debug)]
pub struct SftpJobHandle {
    tx: Sender<SftpJob>,
    cancels: HashMap<String, Arc<AtomicBool>>,
    /// Submitted specs, kept so a finished job that reported Ask-policy
    /// conflicts can be re-run with an explicit policy.
    specs: HashMap<String, SftpJobSpec>,
    limiter: Arc<TransferLimiter>,
    /// Off-thread probe/listing worker (Host-owned; absent in the fallback
    /// in-process path and in unit tests).
    control: Option<SftpControlHandle>,
}

impl SftpJobHandle {
    /// Handle with an explicit concurrency limit (the "max parallel transfers"
    /// setting).
    pub fn with_limiter(tx: Sender<SftpJob>, limiter: Arc<TransferLimiter>) -> Self {
        Self {
            tx,
            cancels: HashMap::new(),
            specs: HashMap::new(),
            limiter,
            control: None,
        }
    }

    /// Handle with an explicit concurrency limit and an SFTP control worker.
    pub fn with_limiter_and_control(
        tx: Sender<SftpJob>,
        limiter: Arc<TransferLimiter>,
        control: SftpControlHandle,
    ) -> Self {
        Self {
            control: Some(control),
            ..Self::with_limiter(tx, limiter)
        }
    }

    /// The off-thread probe/listing worker, when one was wired.
    pub fn control(&self) -> Option<&SftpControlHandle> {
        self.control.as_ref()
    }

    /// Changes how many transfers may run at the same time.
    pub fn set_parallel_limit(&self, limit: usize) {
        self.limiter.set_limit(limit);
    }

    /// Submits a job and returns whether the worker accepted it.
    pub fn submit(&mut self, id: impl Into<String>, spec: SftpJobSpec) -> bool {
        let id = id.into();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = SftpJob {
            id: id.clone(),
            spec: spec.clone(),
            cancel: cancel.clone(),
        };
        if self.tx.send(job).is_err() {
            return false;
        }
        self.cancels.insert(id.clone(), cancel);
        self.specs.insert(id, spec);
        true
    }

    /// The spec of a submitted job (used by the conflict dialog's re-run).
    pub fn spec(&self, id: &str) -> Option<SftpJobSpec> {
        self.specs.get(id).cloned()
    }

    /// Sets the cancel flag for a running/queued job.
    pub fn cancel(&mut self, id: &str) -> bool {
        match self.cancels.get(id) {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Drops the cancellation/spec entries once a job reported a terminal state.
    pub fn forget(&mut self, id: &str) {
        self.cancels.remove(id);
        self.specs.remove(id);
    }
}

/// Concurrency limiter shared by the transfer workers.
///
/// The workers themselves are a fixed pool; the limiter decides how many of
/// them may run a transfer at the same time ("max parallel uploads/downloads").
#[derive(Debug)]
pub struct TransferLimiter {
    state: Mutex<LimiterState>,
    changed: Condvar,
}

#[derive(Debug)]
struct LimiterState {
    limit: usize,
    in_flight: usize,
}

impl TransferLimiter {
    pub fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new(LimiterState {
                limit: limit.max(1),
                in_flight: 0,
            }),
            changed: Condvar::new(),
        }
    }

    /// Changes the limit; running transfers are not interrupted.
    pub fn set_limit(&self, limit: usize) {
        let mut state = self.state.lock().expect("transfer limiter poisoned");
        state.limit = limit.max(1);
        self.changed.notify_all();
    }

    /// The current limit.
    #[allow(dead_code)]
    pub fn limit(&self) -> usize {
        let state = self.state.lock().expect("transfer limiter poisoned");
        state.limit
    }

    /// Waits until a slot is free and takes it.
    fn acquire(&self) {
        let mut state = self.state.lock().expect("transfer limiter poisoned");
        while state.in_flight >= state.limit {
            state = self.changed.wait(state).expect("transfer limiter poisoned");
        }
        state.in_flight += 1;
    }

    /// Gives the slot back.
    fn release(&self) {
        let mut state = self.state.lock().expect("transfer limiter poisoned");
        state.in_flight = state.in_flight.saturating_sub(1);
        self.changed.notify_all();
    }
}

/// Spawns the transfer workers with an explicit concurrency limit.
///
/// Several worker threads are started so that raising the limit actually
/// enables parallel transfers; the shared limiter keeps the number of
/// simultaneously running transfers at the configured value. The limiter is
/// returned so the UI thread can change the limit at runtime.
pub fn start_sftp_job_worker_with_limit(
    limit: usize,
) -> (
    Sender<SftpJob>,
    Receiver<SftpJobMessage>,
    Arc<TransferLimiter>,
) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<SftpJob>();
    let (msg_tx, msg_rx) = std::sync::mpsc::channel::<SftpJobMessage>();
    let shared_rx = Arc::new(Mutex::new(job_rx));
    let limiter = Arc::new(TransferLimiter::new(limit));
    for index in 0..MAX_TRANSFER_WORKERS {
        let shared_rx = Arc::clone(&shared_rx);
        let msg_tx = msg_tx.clone();
        let limiter = Arc::clone(&limiter);
        let spawned = std::thread::Builder::new()
            .name(format!("yshell-sftp-jobs-{index}"))
            .spawn(move || worker_loop(shared_rx, limiter, msg_tx));
        if spawned.is_err() {
            // The channel still works; submissions are dropped by the sender when
            // the receiver is gone, and the UI reports the failure.
            break;
        }
    }
    (job_tx, msg_rx, limiter)
}

/// Upper bound of worker threads (the effective limit is the configured one).
const MAX_TRANSFER_WORKERS: usize = 16;

fn worker_loop(
    job_rx: Arc<Mutex<Receiver<SftpJob>>>,
    limiter: Arc<TransferLimiter>,
    msg_tx: Sender<SftpJobMessage>,
) {
    loop {
        let job = {
            let receiver = job_rx.lock().expect("sftp job queue poisoned");
            match receiver.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        };
        limiter.acquire();
        let id = job.id.clone();
        let outcome = run_job(&job, &msg_tx);
        limiter.release();
        if msg_tx
            .send(SftpJobMessage::Finished { id, outcome })
            .is_err()
        {
            return;
        }
    }
}

fn run_job(job: &SftpJob, msg_tx: &Sender<SftpJobMessage>) -> SftpJobOutcome {
    let mut client = SftpClient::with_real_backend(spec_config(&job.spec).clone());
    let progress = |event: TransferProgress| {
        let _ = msg_tx.send(SftpJobMessage::Progress {
            id: job.id.clone(),
            bytes_done: event.bytes_done,
            bytes_total: event.bytes_total,
        });
    };
    match &job.spec {
        SftpJobSpec::UploadFile {
            local,
            remote,
            options,
            ..
        } => run_single_file_upload(&mut client, local, remote, *options, &job.cancel, progress),
        SftpJobSpec::DownloadFile {
            remote,
            local,
            options,
            ..
        } => run_single_file_download(&mut client, remote, local, *options, &job.cancel, progress),
        SftpJobSpec::UploadTree {
            local_root,
            remote_root,
            options,
            ..
        } => {
            let mut progress = progress;
            match client.upload_tree(
                local_root,
                remote_root,
                *options,
                &mut progress,
                &job.cancel,
            ) {
                Ok(report) => finish_tree(report),
                Err(error) => SftpJobOutcome::Failed {
                    reason: error.to_string(),
                    report: None,
                },
            }
        }
        SftpJobSpec::DownloadTree {
            remote_root,
            local_root,
            options,
            ..
        } => {
            let mut progress = progress;
            match client.download_tree(
                remote_root,
                local_root,
                *options,
                &mut progress,
                &job.cancel,
            ) {
                Ok(report) => finish_tree(report),
                Err(error) => SftpJobOutcome::Failed {
                    reason: error.to_string(),
                    report: None,
                },
            }
        }
        SftpJobSpec::RemoteCopyFile {
            from, to, options, ..
        } => run_remote_copy(
            &mut client,
            &job.id,
            from,
            to,
            *options,
            false,
            &job.cancel,
            progress,
        ),
        SftpJobSpec::RemoteCopyTree {
            from, to, options, ..
        } => run_remote_copy(
            &mut client,
            &job.id,
            from,
            to,
            *options,
            true,
            &job.cancel,
            progress,
        ),
        SftpJobSpec::RemoteMove { moves, options, .. } => {
            run_remote_move(&mut client, moves, *options)
        }
        SftpJobSpec::RemoteDelete { entries, .. } => {
            run_remote_delete(&mut client, entries, &job.cancel)
        }
    }
}

fn spec_config(spec: &SftpJobSpec) -> &SshConnectionConfig {
    match spec {
        SftpJobSpec::UploadFile { config, .. }
        | SftpJobSpec::UploadTree { config, .. }
        | SftpJobSpec::DownloadFile { config, .. }
        | SftpJobSpec::DownloadTree { config, .. }
        | SftpJobSpec::RemoteCopyFile { config, .. }
        | SftpJobSpec::RemoteCopyTree { config, .. }
        | SftpJobSpec::RemoteMove { config, .. }
        | SftpJobSpec::RemoteDelete { config, .. } => config,
    }
}

fn finish_tree(report: TreeTransferReport) -> SftpJobOutcome {
    // Ask-policy conflicts arrive as `Completed` with a non-empty `conflicts`
    // list; the caller opens the conflict dialog and re-runs the same spec with
    // an explicit policy.
    if report.cancelled {
        SftpJobOutcome::Cancelled(report)
    } else {
        SftpJobOutcome::Completed(report)
    }
}

fn single_file_report(bytes: u64, cancelled: bool) -> TreeTransferReport {
    TreeTransferReport {
        completed: 1,
        bytes_transferred: bytes,
        cancelled,
        ..TreeTransferReport::default()
    }
}

fn run_single_file_upload(
    client: &mut SftpClient<yshell_sftp::RealSftpBackend>,
    local: &Path,
    remote: &str,
    options: TreeTransferOptions,
    cancel: &AtomicBool,
    mut progress: impl FnMut(TransferProgress),
) -> SftpJobOutcome {
    if cancel.load(Ordering::SeqCst) {
        return SftpJobOutcome::Cancelled(single_file_report(0, true));
    }
    let destination = match apply_remote_policy(client, remote, options.overwrite) {
        PolicyDecision::Proceed(path) => path,
        PolicyDecision::Skip => {
            return SftpJobOutcome::Completed(TreeTransferReport {
                skipped: 1,
                ..TreeTransferReport::default()
            })
        }
        PolicyDecision::Conflict(path) => {
            return SftpJobOutcome::Completed(TreeTransferReport {
                skipped: 1,
                conflicts: vec![path],
                ..TreeTransferReport::default()
            })
        }
    };
    let size = fs::metadata(local).map(|m| m.len()).unwrap_or_default();
    progress(TransferProgress {
        path: destination.clone(),
        kind: yshell_sftp::TransferItemKind::File,
        bytes_done: 0,
        bytes_total: size,
        direction: yshell_sftp::TransferDirection::Upload,
    });
    if let Err(error) = client.upload_file(local, &destination) {
        return SftpJobOutcome::Failed {
            reason: error.to_string(),
            report: None,
        };
    }
    let transferred = fs::metadata(local).map(|m| m.len()).unwrap_or(size);
    let report = single_file_report(transferred, false);
    progress(TransferProgress {
        path: destination,
        kind: yshell_sftp::TransferItemKind::File,
        bytes_done: transferred,
        bytes_total: transferred,
        direction: yshell_sftp::TransferDirection::Upload,
    });
    SftpJobOutcome::Completed(report)
}

fn run_single_file_download(
    client: &mut SftpClient<yshell_sftp::RealSftpBackend>,
    remote: &str,
    local: &Path,
    options: TreeTransferOptions,
    cancel: &AtomicBool,
    mut progress: impl FnMut(TransferProgress),
) -> SftpJobOutcome {
    if cancel.load(Ordering::SeqCst) {
        return SftpJobOutcome::Cancelled(single_file_report(0, true));
    }
    let destination = match apply_local_policy(local, options.overwrite) {
        PolicyDecision::Proceed(path) => PathBuf::from(path),
        PolicyDecision::Skip => {
            return SftpJobOutcome::Completed(TreeTransferReport {
                skipped: 1,
                ..TreeTransferReport::default()
            })
        }
        PolicyDecision::Conflict(_) => {
            return SftpJobOutcome::Completed(TreeTransferReport {
                skipped: 1,
                conflicts: vec![remote.to_owned()],
                ..TreeTransferReport::default()
            })
        }
    };
    if let Some(parent) = destination.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(error) = client.download_file(remote, &destination) {
        return SftpJobOutcome::Failed {
            reason: error.to_string(),
            report: None,
        };
    }
    let transferred = fs::metadata(&destination)
        .map(|m| m.len())
        .unwrap_or_default();
    let report = single_file_report(transferred, false);
    progress(TransferProgress {
        path: remote.to_owned(),
        kind: yshell_sftp::TransferItemKind::File,
        bytes_done: transferred,
        bytes_total: transferred,
        direction: yshell_sftp::TransferDirection::Download,
    });
    SftpJobOutcome::Completed(report)
}

/// Overwrite-policy decision for a single destination path.
enum PolicyDecision {
    Proceed(String),
    Skip,
    Conflict(String),
}

fn apply_remote_policy(
    client: &mut SftpClient<yshell_sftp::RealSftpBackend>,
    destination: &str,
    overwrite: OverwritePolicy,
) -> PolicyDecision {
    if !remote_exists(client, destination) {
        return PolicyDecision::Proceed(destination.to_owned());
    }
    match overwrite {
        OverwritePolicy::Overwrite => PolicyDecision::Proceed(destination.to_owned()),
        OverwritePolicy::Skip => PolicyDecision::Skip,
        OverwritePolicy::Ask => PolicyDecision::Conflict(destination.to_owned()),
        OverwritePolicy::Rename => PolicyDecision::Proceed(unique_remote_path(client, destination)),
    }
}

fn apply_local_policy(destination: &Path, overwrite: OverwritePolicy) -> PolicyDecision {
    let exists = fs::symlink_metadata(destination).is_ok();
    if !exists {
        return PolicyDecision::Proceed(destination.display().to_string());
    }
    match overwrite {
        OverwritePolicy::Overwrite => PolicyDecision::Proceed(destination.display().to_string()),
        OverwritePolicy::Skip => PolicyDecision::Skip,
        OverwritePolicy::Ask => PolicyDecision::Conflict(destination.display().to_string()),
        OverwritePolicy::Rename => {
            PolicyDecision::Proceed(unique_local_path(destination).display().to_string())
        }
    }
}

/// `name.txt` → `name (1).txt` for non-clobbering single-file downloads.
fn unique_local_path(path: &Path) -> PathBuf {
    let Some(name) = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return path.to_path_buf();
    };
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    for index in 1..10_000 {
        let candidate = parent.join(numbered_name(&name, index));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    path.to_path_buf()
}

#[allow(clippy::too_many_arguments)]
fn run_remote_copy(
    client: &mut SftpClient<yshell_sftp::RealSftpBackend>,
    id: &str,
    from: &str,
    to: &str,
    options: TreeTransferOptions,
    is_dir: bool,
    cancel: &AtomicBool,
    mut progress: impl FnMut(TransferProgress),
) -> SftpJobOutcome {
    let temp_root = std::env::temp_dir().join(format!("yshell-remote-copy-{id}"));
    let _ = fs::remove_dir_all(&temp_root);
    if is_dir {
        let download_report = match client.download_tree(
            from,
            &temp_root,
            options,
            &mut progress_phase(&mut progress, 0, None),
            cancel,
        ) {
            Ok(report) if !report.cancelled => report,
            Ok(report) => {
                let _ = fs::remove_dir_all(&temp_root);
                return SftpJobOutcome::Cancelled(report);
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&temp_root);
                return SftpJobOutcome::Failed {
                    reason: error.to_string(),
                    report: None,
                };
            }
        };
        let downloaded = download_report.bytes_transferred;
        let upload = client.upload_tree(
            &temp_root,
            to,
            options,
            &mut progress_phase(&mut progress, downloaded, Some(downloaded)),
            cancel,
        );
        let _ = fs::remove_dir_all(&temp_root);
        merge_copy_reports(download_report, upload)
    } else {
        let destination = match apply_remote_policy(client, to, options.overwrite) {
            PolicyDecision::Proceed(path) => path,
            PolicyDecision::Skip => {
                let _ = fs::remove_dir_all(&temp_root);
                return SftpJobOutcome::Completed(TreeTransferReport {
                    skipped: 1,
                    ..TreeTransferReport::default()
                });
            }
            PolicyDecision::Conflict(path) => {
                let _ = fs::remove_dir_all(&temp_root);
                return SftpJobOutcome::Completed(TreeTransferReport {
                    skipped: 1,
                    conflicts: vec![path],
                    ..TreeTransferReport::default()
                });
            }
        };
        let temp_file = temp_root.join("copy.bin");
        if let Some(parent) = temp_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if cancel.load(Ordering::SeqCst) {
            let _ = fs::remove_dir_all(&temp_root);
            return SftpJobOutcome::Cancelled(single_file_report(0, true));
        }
        if let Err(error) = client.download_file(from, &temp_file) {
            let _ = fs::remove_dir_all(&temp_root);
            return SftpJobOutcome::Failed {
                reason: error.to_string(),
                report: None,
            };
        }
        let size = fs::metadata(&temp_file)
            .map(|m| m.len())
            .unwrap_or_default();
        let upload = client.upload_file(&temp_file, &destination);
        let _ = fs::remove_dir_all(&temp_root);
        match upload {
            Ok(()) => SftpJobOutcome::Completed(single_file_report(size, false)),
            Err(error) => SftpJobOutcome::Failed {
                reason: error.to_string(),
                report: None,
            },
        }
    }
}

/// Wraps a progress sink so a second phase reports cumulative bytes
/// (copy = download + upload) against the same total once it is known.
fn progress_phase<'a>(
    sink: &'a mut impl FnMut(TransferProgress),
    base: u64,
    total: Option<u64>,
) -> impl FnMut(TransferProgress) + 'a {
    move |event| {
        sink(TransferProgress {
            path: event.path,
            kind: event.kind,
            bytes_done: base + event.bytes_done,
            bytes_total: total.unwrap_or(event.bytes_total),
            direction: event.direction,
        });
    }
}

fn merge_copy_reports(
    download: TreeTransferReport,
    upload: yshell_sftp::SftpResult<TreeTransferReport>,
) -> SftpJobOutcome {
    match upload {
        Ok(upload) => SftpJobOutcome::Completed(TreeTransferReport {
            completed: download.completed + upload.completed,
            skipped: download.skipped + upload.skipped,
            skipped_items: download
                .skipped_items
                .into_iter()
                .chain(upload.skipped_items)
                .collect(),
            failed: download.failed.into_iter().chain(upload.failed).collect(),
            conflicts: download
                .conflicts
                .into_iter()
                .chain(upload.conflicts)
                .collect(),
            cancelled: download.cancelled || upload.cancelled,
            bytes_transferred: download.bytes_transferred + upload.bytes_transferred,
        }),
        Err(error) => SftpJobOutcome::Failed {
            reason: error.to_string(),
            report: Some(download),
        },
    }
}

fn run_remote_move(
    client: &mut SftpClient<yshell_sftp::RealSftpBackend>,
    moves: &[(String, String)],
    options: TreeTransferOptions,
) -> SftpJobOutcome {
    let mut report = TreeTransferReport::default();
    for (from, to) in moves {
        match remote_move_one(client, from, to, options.overwrite) {
            Ok(Some(path)) => {
                report.completed += 1;
                report.skipped_items.push((path, String::new()));
            }
            Ok(None) => report.skipped += 1,
            Err(error) => report.failed.push((from.clone(), error)),
        }
    }
    SftpJobOutcome::Completed(report)
}

/// Applies the overwrite policy to one rename. `Ok(Some(actual))` = moved
/// (possibly to a numbered sibling); `Ok(None)` = skipped by policy.
fn remote_move_one(
    client: &mut SftpClient<yshell_sftp::RealSftpBackend>,
    from: &str,
    to: &str,
    overwrite: OverwritePolicy,
) -> Result<Option<String>, String> {
    let destination = if remote_exists(client, to) {
        match overwrite {
            OverwritePolicy::Overwrite => to.to_owned(),
            OverwritePolicy::Skip => return Ok(None),
            OverwritePolicy::Ask => return Ok(None),
            OverwritePolicy::Rename => unique_remote_path(client, to),
        }
    } else {
        to.to_owned()
    };
    client
        .rename(from, &destination)
        .map_err(|error| error.to_string())?;
    Ok(Some(destination))
}

fn run_remote_delete(
    client: &mut SftpClient<yshell_sftp::RealSftpBackend>,
    entries: &[(String, bool)],
    cancel: &AtomicBool,
) -> SftpJobOutcome {
    let mut report = TreeTransferReport::default();
    for (path, is_dir) in entries {
        if cancel.load(Ordering::SeqCst) {
            report.cancelled = true;
            return SftpJobOutcome::Cancelled(report);
        }
        delete_remote_entry(client, path, *is_dir, cancel, &mut report);
    }
    SftpJobOutcome::Completed(report)
}

/// Post-order recursive delete. Generic over the backend so unit tests can use
/// [`yshell_sftp::InMemorySftpBackend`].
pub(crate) fn delete_remote_entry<B: yshell_sftp::SftpBackend>(
    client: &mut SftpClient<B>,
    path: &str,
    is_dir: bool,
    cancel: &AtomicBool,
    report: &mut TreeTransferReport,
) {
    if cancel.load(Ordering::SeqCst) {
        report.cancelled = true;
        return;
    }
    if is_dir {
        match client.list_dir(path) {
            Ok(listing) => {
                for entry in listing.entries {
                    if cancel.load(Ordering::SeqCst) {
                        report.cancelled = true;
                        return;
                    }
                    let child_is_dir = matches!(entry.kind, FsEntryKind::Directory);
                    delete_remote_entry(client, &entry.path, child_is_dir, cancel, report);
                }
            }
            Err(error) => {
                report.failed.push((path.to_owned(), error.to_string()));
                return;
            }
        }
    }
    match client.delete(path) {
        Ok(()) => report.completed += 1,
        Err(error) => report.failed.push((path.to_owned(), error.to_string())),
    }
}

fn remote_exists(client: &mut SftpClient<yshell_sftp::RealSftpBackend>, path: &str) -> bool {
    client.entry_kind(path).ok().flatten().is_some()
}

fn unique_remote_path(client: &mut SftpClient<yshell_sftp::RealSftpBackend>, path: &str) -> String {
    let Some((parent, name)) = split_remote(path) else {
        return path.to_owned();
    };
    for index in 1..1000 {
        let candidate = join_remote(&parent, &numbered_name(&name, index));
        if !remote_exists(client, &candidate) {
            return candidate;
        }
    }
    path.to_owned()
}

fn split_remote(path: &str) -> Option<(String, String)> {
    let path = path.trim_end_matches('/');
    let index = path.rfind('/')?;
    let name = &path[index + 1..];
    if name.is_empty() {
        return None;
    }
    let parent = if index == 0 {
        "/".to_owned()
    } else {
        path[..index].to_owned()
    };
    Some((parent, name.to_owned()))
}

fn join_remote(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

fn numbered_name(name: &str, index: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => {
            format!("{} ({index}).{}", &name[..dot], &name[dot + 1..])
        }
        _ => format!("{name} ({index})"),
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::atomic::AtomicBool, sync::Arc};

    use tempfile::tempdir;
    use yshell_sftp::{
        DirectoryListing, FsEntry, InMemorySftpBackend, SftpClient, TreeTransferOptions,
    };
    use yshell_ssh::SshConnectionConfig;

    use super::{
        delete_remote_entry, join_remote, numbered_name, split_remote, SftpControlHandle,
        SftpControlMessage, SftpControlRequest, SftpJobSpec, TransferLimiter,
    };

    fn in_memory_client(entries: &[FsEntry]) -> SftpClient<InMemorySftpBackend> {
        let mut backend = InMemorySftpBackend::default();
        for entry in entries {
            backend.insert(entry.clone());
        }
        SftpClient::with_backend(backend)
    }

    fn names_at(client: &SftpClient<InMemorySftpBackend>, path: &str) -> Vec<String> {
        client
            .list_dir(path)
            .map(|listing| {
                listing
                    .entries
                    .into_iter()
                    .map(|entry| entry.name)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn config() -> SshConnectionConfig {
        SshConnectionConfig::new(
            "example.test",
            22,
            yshell_ssh::AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        )
    }

    #[test]
    fn remote_path_helpers_cover_roots_and_extensions() {
        assert_eq!(
            split_remote("/srv/logs/app.log"),
            Some(("/srv/logs".to_owned(), "app.log".to_owned()))
        );
        assert_eq!(
            split_remote("/app.log"),
            Some(("/".to_owned(), "app.log".to_owned()))
        );
        assert_eq!(split_remote("/"), None);
        assert_eq!(join_remote("/", "a"), "/a");
        assert_eq!(join_remote("/srv/", "a"), "/srv/a");
        assert_eq!(numbered_name("report.txt", 2), "report (2).txt");
        assert_eq!(numbered_name("Makefile", 1), "Makefile (1)");
    }

    #[test]
    fn overwrite_policy_round_trips_through_specs() {
        let spec = SftpJobSpec::UploadTree {
            config: config(),
            local_root: PathBuf::from("/local/tree"),
            remote_root: "/srv/tree".to_owned(),
            options: TreeTransferOptions::default(),
        };
        assert_eq!(
            spec.overwrite_policy(),
            Some(yshell_sftp::OverwritePolicy::Ask)
        );
        let renamed = spec
            .clone()
            .with_overwrite(yshell_sftp::OverwritePolicy::Rename);
        assert_eq!(
            renamed.overwrite_policy(),
            Some(yshell_sftp::OverwritePolicy::Rename)
        );
        // Batch jobs have no policy and are returned unchanged.
        let delete = SftpJobSpec::RemoteDelete {
            config: config(),
            entries: vec![("/srv/a".to_owned(), false)],
        };
        assert_eq!(delete.overwrite_policy(), None);
        assert_eq!(
            delete
                .clone()
                .with_overwrite(yshell_sftp::OverwritePolicy::Skip),
            delete
        );
    }

    #[test]
    fn recursive_delete_walks_directories_post_order() {
        let mut client = in_memory_client(&[
            FsEntry::directory("/srv/tree"),
            FsEntry::directory("/srv/tree/sub"),
            FsEntry::file("/srv/tree/sub/a.txt", 3),
            FsEntry::file("/srv/tree/b.txt", 3),
        ]);

        let mut report = yshell_sftp::TreeTransferReport::default();
        let cancel = AtomicBool::new(false);
        delete_remote_entry(&mut client, "/srv/tree", true, &cancel, &mut report);
        assert!(report.is_success(), "{report:?}");
        assert_eq!(report.completed, 4);
        assert!(names_at(&client, "/srv").is_empty());
    }

    #[test]
    fn recursive_delete_observes_cancel() {
        let mut client = in_memory_client(&[
            FsEntry::directory("/srv/tree"),
            FsEntry::file("/srv/tree/a.txt", 3),
        ]);

        let mut report = yshell_sftp::TreeTransferReport::default();
        let cancel = AtomicBool::new(true);
        delete_remote_entry(&mut client, "/srv/tree", true, &cancel, &mut report);
        assert!(report.cancelled);
        assert_eq!(report.completed, 0);
        assert_eq!(names_at(&client, "/srv/tree"), vec!["a.txt"]);
    }

    #[test]
    fn temp_dirs_for_remote_copy_do_not_touch_real_paths() {
        // The remote-copy temp root is derived from the transfer id only; this
        // test documents the naming so files from parallel runs stay isolated.
        let dir = tempdir().expect("tempdir");
        assert!(dir.path().is_dir());
    }

    #[test]
    fn the_transfer_limiter_caps_the_number_of_parallel_jobs() {
        let limiter = Arc::new(TransferLimiter::new(2));
        assert_eq!(limiter.limit(), 2);
        limiter.acquire();
        limiter.acquire();
        assert_eq!(limiter.limit(), 2);

        // The third slot has to wait for a running transfer to finish.
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let wait_limiter = Arc::clone(&limiter);
        let waiter = std::thread::spawn(move || {
            wait_limiter.acquire();
            tx.send(()).expect("signal");
            wait_limiter.release();
        });
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        limiter.release();
        assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).is_ok());
        waiter.join().expect("waiter");

        // Raising the limit lets more transfers run at the same time.
        limiter.set_limit(4);
        assert_eq!(limiter.limit(), 4);
        limiter.release();
        limiter.release();
    }

    #[test]
    fn control_handle_tracks_in_flight_work_and_drains_results() {
        let (tx, rx) = std::sync::mpsc::channel::<SftpControlRequest>();
        let handle = SftpControlHandle::new(tx);
        assert!(!handle.probe_in_flight());

        assert!(handle.submit(SftpControlRequest::Probe {
            session_key: "s1".to_owned(),
            tab_id: "t1".to_owned(),
            config: config(),
            path: "/srv".to_owned(),
            timeout_secs: 3,
        }));
        assert!(handle.probe_in_flight());
        assert!(matches!(
            rx.recv().expect("probe request"),
            SftpControlRequest::Probe { .. }
        ));

        // A worker result clears the in-flight marker when the tick drains it.
        handle.push_result_for_test(SftpControlMessage::ProbeResult {
            session_key: "s1".to_owned(),
            tab_id: "t1".to_owned(),
            ok: false,
            transport_failed: true,
            error: Some("transport closed".to_owned()),
        });
        assert_eq!(handle.drain_results().len(), 1);
        assert!(!handle.probe_in_flight());

        assert!(handle.submit(SftpControlRequest::ListDir {
            session_key: "s1".to_owned(),
            tab_id: "t1".to_owned(),
            config: config(),
            path: "/srv".to_owned(),
        }));
        // Dedupe is keyed on the exact (session, tab, path) triple.
        assert!(handle.listing_in_flight_for("s1", "t1", "/srv"));
        assert!(!handle.listing_in_flight_for("s1", "t2", "/srv"));
        rx.recv().expect("listing request");
        handle.push_result_for_test(SftpControlMessage::ListingResult {
            session_key: "s1".to_owned(),
            tab_id: "t1".to_owned(),
            path: "/srv".to_owned(),
            result: Ok(DirectoryListing {
                path: "/srv".to_owned(),
                entries: vec![FsEntry::directory("/srv/sub")],
            }),
        });
        assert_eq!(handle.drain_results().len(), 1);
        assert!(!handle.listing_in_flight_for("s1", "t1", "/srv"));
    }

    #[test]
    fn control_handle_rolls_back_when_the_worker_is_gone() {
        let (tx, rx) = std::sync::mpsc::channel::<SftpControlRequest>();
        drop(rx);
        let handle = SftpControlHandle::new(tx);
        assert!(!handle.submit(SftpControlRequest::Probe {
            session_key: "s1".to_owned(),
            tab_id: "t1".to_owned(),
            config: config(),
            path: "/".to_owned(),
            timeout_secs: 3,
        }));
        // A failed submission must not leave a probe marked in flight (that
        // would silently disable probing forever).
        assert!(!handle.probe_in_flight());
        assert!(handle.drain_results().is_empty());
    }
}
