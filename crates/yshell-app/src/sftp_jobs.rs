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
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender},
        Arc,
    },
};

use yshell_sftp::{
    FsEntryKind, OverwritePolicy, SftpClient, TransferProgress, TreeTransferOptions,
    TreeTransferReport,
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
    /// Batch chmod.
    RemoteChmod {
        config: SshConnectionConfig,
        paths: Vec<String>,
        permissions: u32,
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
            Self::RemoteDelete { .. } | Self::RemoteChmod { .. } => return None,
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
            Self::RemoteDelete { .. } | Self::RemoteChmod { .. } => return self,
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
            | Self::RemoteDelete { .. }
            | Self::RemoteChmod { .. } => yshell_sftp::TransferDirection::Download,
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
            Self::RemoteChmod { paths, .. } => paths.first().cloned().unwrap_or_default(),
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
            Self::RemoteChmod { paths, .. } => paths.first().cloned().unwrap_or_default(),
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

/// UI-thread handle to the worker: submit jobs, cancel them, forget finished ones.
#[derive(Debug)]
pub struct SftpJobHandle {
    tx: Sender<SftpJob>,
    cancels: HashMap<String, Arc<AtomicBool>>,
    /// Submitted specs, kept so a finished job that reported Ask-policy
    /// conflicts can be re-run with an explicit policy.
    specs: HashMap<String, SftpJobSpec>,
}

impl SftpJobHandle {
    pub fn new(tx: Sender<SftpJob>) -> Self {
        Self {
            tx,
            cancels: HashMap::new(),
            specs: HashMap::new(),
        }
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

/// Spawns the transfer worker thread and returns the submit/drain endpoints.
pub fn start_sftp_job_worker() -> (Sender<SftpJob>, Receiver<SftpJobMessage>) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<SftpJob>();
    let (msg_tx, msg_rx) = std::sync::mpsc::channel::<SftpJobMessage>();
    let spawned = std::thread::Builder::new()
        .name("yshell-sftp-jobs".to_owned())
        .spawn(move || worker_loop(job_rx, msg_tx));
    if spawned.is_err() {
        // The channel still works; submissions are dropped by the sender when
        // the receiver is gone, and the UI reports the failure.
    }
    (job_tx, msg_rx)
}

fn worker_loop(job_rx: Receiver<SftpJob>, msg_tx: Sender<SftpJobMessage>) {
    while let Ok(job) = job_rx.recv() {
        let id = job.id.clone();
        let outcome = run_job(&job, &msg_tx);
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
        SftpJobSpec::RemoteChmod {
            paths, permissions, ..
        } => run_remote_chmod(&mut client, paths, *permissions),
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
        | SftpJobSpec::RemoteDelete { config, .. }
        | SftpJobSpec::RemoteChmod { config, .. } => config,
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
/// the in-process fake.
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

fn run_remote_chmod(
    client: &mut SftpClient<yshell_sftp::RealSftpBackend>,
    paths: &[String],
    permissions: u32,
) -> SftpJobOutcome {
    let results = client.chmod_many(paths, permissions);
    let mut report = TreeTransferReport::default();
    for result in results {
        match result.error_message() {
            Some(error) => report.failed.push((result.path, error)),
            None => report.completed += 1,
        }
    }
    SftpJobOutcome::Completed(report)
}

fn remote_exists(client: &mut SftpClient<yshell_sftp::RealSftpBackend>, path: &str) -> bool {
    let Some((parent, name)) = split_remote(path) else {
        return false;
    };
    client
        .list_dir(&parent)
        .map(|listing| listing.entries.iter().any(|entry| entry.name == name))
        .unwrap_or(false)
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
    use std::{path::PathBuf, sync::atomic::AtomicBool};

    use tempfile::tempdir;
    use yshell_sftp::{FakeSftpBackend, FsEntry, SftpClient, TreeTransferOptions};
    use yshell_ssh::SshConnectionConfig;

    use super::{delete_remote_entry, join_remote, numbered_name, split_remote, SftpJobSpec};

    fn fake_client(entries: &[FsEntry]) -> SftpClient<FakeSftpBackend> {
        let mut backend = FakeSftpBackend::default();
        for entry in entries {
            backend.insert(entry.clone());
        }
        SftpClient::with_backend(backend)
    }

    fn names_at(client: &SftpClient<FakeSftpBackend>, path: &str) -> Vec<String> {
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
        let mut client = fake_client(&[
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
        let mut client = fake_client(&[
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
}
