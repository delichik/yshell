//! Recursive tree transfers and batch-operation helpers.
//!
//! The engine in this module is generic over [`SftpBackend`], so
//! [`FakeSftpBackend`](crate::FakeSftpBackend) and
//! [`RealSftpBackend`](crate::RealSftpBackend) share exactly the same
//! traversal, overwrite-policy and reporting behaviour.
//!
//! Semantics worth calling out:
//!
//! - Transfers never stop on a single item failure; every item is attempted
//!   and recorded in the returned [`TreeTransferReport`].
//! - `bytes_total` in [`TransferProgress`] is the pre-scan estimate over every
//!   planned file, so it does not shrink when an item is later skipped by the
//!   overwrite policy. `bytes_transferred` in the report is exact.
//! - Directories are merged, never treated as conflicts: an existing
//!   destination directory is reused even under [`OverwritePolicy::Ask`] and
//!   [`OverwritePolicy::Skip`].
//! - Under [`OverwritePolicy::Ask`] conflicting paths are skipped and listed in
//!   [`TreeTransferReport::conflicts`] so the caller can prompt the user and
//!   re-run with an explicit policy.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    client::SftpBackend,
    error::{SftpError, SftpErrorKind, SftpResult},
    fs_entry::{FsEntry, FsEntryKind},
    transfer_task::TransferDirection,
};

/// Safety valve for pathological trees.
const MAX_TREE_DEPTH: usize = 128;
/// Remote symlinked directories are followed at most this many levels per branch.
const MAX_SYMLINK_DEPTH: usize = 8;
/// Number of `name (n)` candidates tried before giving up on a rename.
const RENAME_ATTEMPTS: u32 = 10_000;

const SYMLINK_SKIPPED: &str = "symlink skipped (follow_symlinks = false)";

/// What to do when a tree-transfer destination already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverwritePolicy {
    /// Do not touch existing destinations. Conflicting paths are counted as
    /// skipped and recorded in [`TreeTransferReport::conflicts`] so the caller
    /// can prompt the user and re-run with an explicit policy.
    #[default]
    Ask,
    /// Replace existing files and merge into existing directories.
    Overwrite,
    /// Keep both sides by transferring to a numbered sibling name
    /// (`report.txt` becomes `report (1).txt`).
    Rename,
    /// Leave existing destinations untouched, without recording conflicts.
    Skip,
}

/// Options for [`SftpBackend::upload_tree`] / [`SftpBackend::download_tree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeTransferOptions {
    /// How to treat destinations that already exist.
    pub overwrite: OverwritePolicy,
    /// Follow symlinks instead of skipping them.
    ///
    /// Local symlinks are resolved through the filesystem; remote symlinks are
    /// probed with a directory listing (falling back to a plain file
    /// download). Symlink cycles are detected where possible and reported as
    /// skipped items.
    pub follow_symlinks: bool,
}

impl Default for TreeTransferOptions {
    fn default() -> Self {
        Self {
            overwrite: OverwritePolicy::Ask,
            follow_symlinks: false,
        }
    }
}

/// Item kinds reported through [`TransferProgress`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferItemKind {
    /// A regular file that is being transferred.
    File,
    /// A directory that was created (or confirmed) at the destination.
    Dir,
    /// A symlink item. Symlinks are resolved (see
    /// [`TreeTransferOptions::follow_symlinks`]) or skipped, so this kind is
    /// currently never emitted; it exists for forward compatibility with
    /// callers that render per-kind counters.
    Symlink,
}

/// One progress event emitted while a tree transfer runs.
///
/// `path` is always the *remote* path of the item (the destination for
/// uploads, the source for downloads) and `bytes_done`/`bytes_total` are
/// cumulative over the whole transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferProgress {
    pub path: String,
    pub kind: TransferItemKind,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub direction: TransferDirection,
}

/// Summary of a finished (or cancelled) tree transfer.
///
/// For a transfer that was not cancelled the following invariant holds:
/// `completed + skipped + failed.len() == total number of scanned items`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeTransferReport {
    /// Items that were created or transferred successfully. Directories count
    /// once each (created or merged).
    pub completed: u64,
    /// Items that were intentionally not transferred (symlinks with
    /// `follow_symlinks = false`, unsupported file types, overwrite-policy
    /// skips, and descendants of skipped directories).
    pub skipped: u64,
    /// Item-level skips as `(remote path, reason)`. Descendants of a skipped
    /// directory are only counted in [`TreeTransferReport::skipped`].
    pub skipped_items: Vec<(String, String)>,
    /// Per-item failures as `(remote path, reason)`; a failure never stops the
    /// rest of the transfer.
    pub failed: Vec<(String, String)>,
    /// Remote paths skipped because of [`OverwritePolicy::Ask`].
    pub conflicts: Vec<String>,
    /// True when the transfer stopped early because the cancel flag was set.
    pub cancelled: bool,
    /// Exact number of bytes transferred. Unlike progress `bytes_total` this
    /// excludes skipped and failed items.
    pub bytes_transferred: u64,
}

impl TreeTransferReport {
    /// True when nothing failed and the transfer was not cancelled.
    pub fn is_success(&self) -> bool {
        !self.cancelled && self.failed.is_empty()
    }
}

/// Result of one item of a batch operation such as
/// [`SftpBackend::delete_many`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchOpResult {
    pub path: String,
    pub result: SftpResult<()>,
}

impl BatchOpResult {
    /// A successful per-item result.
    pub fn ok(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            result: Ok(()),
        }
    }

    /// A failed per-item result.
    pub fn error(path: impl Into<String>, error: SftpError) -> Self {
        Self {
            path: path.into(),
            result: Err(error),
        }
    }

    /// True when this item succeeded.
    pub fn is_ok(&self) -> bool {
        self.result.is_ok()
    }

    /// The error of this item, if it failed.
    pub fn error_message(&self) -> Option<String> {
        self.result.as_ref().err().map(ToString::to_string)
    }
}

/// Apply [`SftpBackend::delete`] to every path, collecting per-item results
/// without stopping on failures.
pub(crate) fn delete_many<B: SftpBackend + ?Sized>(
    backend: &mut B,
    paths: &[String],
) -> Vec<BatchOpResult> {
    paths
        .iter()
        .map(
            |path| match backend.delete(path) {
                Ok(()) => BatchOpResult::ok(path.clone()),
                Err(error) => BatchOpResult::error(path.clone(), error),
            },
        )
        .collect()
}

/// Apply [`SftpBackend::chmod`] to every path, collecting per-item results
/// without stopping on failures.
pub(crate) fn chmod_many<B: SftpBackend + ?Sized>(
    backend: &mut B,
    paths: &[String],
    permissions: u32,
) -> Vec<BatchOpResult> {
    paths
        .iter()
        .map(
            |path| match backend.chmod(path, permissions) {
                Ok(()) => BatchOpResult::ok(path.clone()),
                Err(error) => BatchOpResult::error(path.clone(), error),
            },
        )
        .collect()
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Upload a local directory tree below `remote_root`.
pub(crate) fn upload_tree<B: SftpBackend + ?Sized>(
    backend: &mut B,
    local_root: &Path,
    remote_root: &str,
    options: TreeTransferOptions,
    progress: &mut dyn FnMut(TransferProgress),
    cancel: &AtomicBool,
) -> SftpResult<TreeTransferReport> {
    let metadata = fs::metadata(local_root).map_err(|error| {
        SftpError::new(
            io_error_kind(&error),
            format!(
                "failed to read local upload root `{}`: {error}",
                local_root.display()
            ),
        )
    })?;
    if !metadata.is_dir() {
        return Err(SftpError::new(
            SftpErrorKind::InvalidPath,
            format!(
                "local upload root `{}` is not a directory",
                local_root.display()
            ),
        ));
    }
    let remote_root = normalize_remote_root(remote_root)?;

    let mut scan = UploadScan::default();
    if let Ok(canonical) = fs::canonicalize(local_root) {
        scan.visited_dirs.insert(canonical);
    }
    let children = match scan_local_dir(local_root, 0, &options, &mut scan) {
        ScanDir::Ready(nodes) => nodes,
        ScanDir::Skipped(reason) | ScanDir::Failed(reason) => {
            return Err(SftpError::new(SftpErrorKind::InvalidPath, reason));
        }
    };

    let mut ctx = ProcessCtx::new(options, cancel, progress, TransferDirection::Upload, scan.bytes_total);
    if ctx.is_cancelled() {
        return Ok(ctx.finish());
    }

    let root = resolve_upload_root(backend, &remote_root, options, &mut ctx)?;
    match root {
        Some(root) => process_upload_nodes(backend, &root, &children, &mut ctx),
        None => ctx.skipped += 1 + upload_node_count(&children),
    }
    Ok(ctx.finish())
}

/// Download a remote directory tree below `local_root`.
pub(crate) fn download_tree<B: SftpBackend + ?Sized>(
    backend: &mut B,
    remote_root: &str,
    local_root: &Path,
    options: TreeTransferOptions,
    progress: &mut dyn FnMut(TransferProgress),
    cancel: &AtomicBool,
) -> SftpResult<TreeTransferReport> {
    let remote_root = normalize_remote_root(remote_root)?;

    let mut scan = DownloadScan::default();
    let children = match scan_remote_dir(backend, &remote_root, 0, 0, &options, &mut scan) {
        ScanDir::Ready(nodes) => nodes,
        ScanDir::Skipped(reason) | ScanDir::Failed(reason) => {
            return Err(SftpError::new(SftpErrorKind::Backend, reason));
        }
    };

    let mut ctx = ProcessCtx::new(options, cancel, progress, TransferDirection::Download, scan.bytes_total);
    if ctx.is_cancelled() {
        return Ok(ctx.finish());
    }

    match ensure_local_dir(local_root, options.overwrite, &remote_root, true, &mut ctx)? {
        Some(root) => process_download_nodes(backend, &remote_root, &root, &children, &mut ctx),
        None => ctx.skipped += 1 + download_node_count(&children),
    }
    Ok(ctx.finish())
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn normalize_remote_root(remote_root: &str) -> SftpResult<String> {
    let trimmed = remote_root.trim();
    if !trimmed.starts_with('/') {
        return Err(SftpError::new(
            SftpErrorKind::InvalidPath,
            format!("remote root `{remote_root}` must be an absolute POSIX path"),
        ));
    }
    if trimmed == "/" {
        return Ok("/".to_owned());
    }
    Ok(trimmed.trim_end_matches('/').to_owned())
}

fn join_remote(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
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

/// Remote kind of `path`, or `None` when the parent cannot be listed or the
/// entry does not exist.
fn remote_kind<B: SftpBackend + ?Sized>(backend: &B, path: &str) -> Option<FsEntryKind> {
    let (parent, name) = split_remote(path)?;
    let listing = backend.list_dir(&parent).ok()?;
    listing
        .entries
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.kind)
}

/// First free sibling of `path`, using the `name (n).ext` scheme.
fn unique_remote_path<B: SftpBackend + ?Sized>(backend: &B, path: &str) -> String {
    let Some((parent, name)) = split_remote(path) else {
        return path.to_owned();
    };
    for index in 1..=RENAME_ATTEMPTS {
        let candidate = join_remote(&parent, &numbered_name(&name, index));
        if remote_kind(backend, &candidate).is_none() {
            return candidate;
        }
    }
    path.to_owned()
}

/// Insert ` (n)` before the extension of `name` (`a.txt` -> `a (1).txt`).
fn numbered_name(name: &str, index: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => {
            format!("{} ({index}).{}", &name[..dot], &name[dot + 1..])
        }
        _ => format!("{name} ({index})"),
    }
}

fn unique_local_path(path: &Path) -> PathBuf {
    let Some(name) = path.file_name() else {
        return path.to_path_buf();
    };
    let name = name.to_string_lossy();
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    for index in 1..=RENAME_ATTEMPTS {
        let candidate = parent.join(numbered_name(&name, index));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    path.to_path_buf()
}

fn io_error_kind(error: &io::Error) -> SftpErrorKind {
    match error.kind() {
        io::ErrorKind::NotFound => SftpErrorKind::NotFound,
        io::ErrorKind::PermissionDenied => SftpErrorKind::PermissionDenied,
        io::ErrorKind::AlreadyExists => SftpErrorKind::AlreadyExists,
        _ => SftpErrorKind::Backend,
    }
}

fn local_io_error(path: &Path, action: &str, error: io::Error) -> SftpError {
    SftpError::new(
        io_error_kind(&error),
        format!("failed to {action} `{}`: {error}", path.display()),
    )
}

// ---------------------------------------------------------------------------
// Scan phase
// ---------------------------------------------------------------------------

/// Children of a scanned directory.
enum ScanDir<T> {
    Ready(Vec<T>),
    Skipped(String),
    Failed(String),
}

enum UploadNode {
    Dir {
        name: String,
        children: ScanDir<UploadNode>,
    },
    File {
        local: PathBuf,
        name: String,
        size: u64,
    },
    Skipped {
        name: String,
        reason: String,
    },
    Failed {
        name: String,
        reason: String,
    },
}

enum DownloadNode {
    Dir {
        name: String,
        children: ScanDir<DownloadNode>,
    },
    File {
        name: String,
        size: u64,
    },
    Skipped {
        name: String,
        reason: String,
    },
}

#[derive(Default)]
struct UploadScan {
    bytes_total: u64,
    visited_dirs: BTreeSet<PathBuf>,
}

#[derive(Default)]
struct DownloadScan {
    bytes_total: u64,
}

fn scan_local_dir(
    dir: &Path,
    depth: usize,
    options: &TreeTransferOptions,
    scan: &mut UploadScan,
) -> ScanDir<UploadNode> {
    if depth >= MAX_TREE_DEPTH {
        return ScanDir::Skipped(format!(
            "tree depth limit ({MAX_TREE_DEPTH}) exceeded at `{}`",
            dir.display()
        ));
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => return ScanDir::Failed(local_io_error(dir, "read local directory", error).to_string()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(error) => {
                return ScanDir::Failed(format!(
                    "failed to read an entry of `{}`: {error}",
                    dir.display()
                ));
            }
        }
    }
    paths.sort_by(|left, right| left.file_name().cmp(&right.file_name()));

    let mut nodes = Vec::with_capacity(paths.len());
    for path in paths {
        let name = local_file_name(&path);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                nodes.push(UploadNode::Failed {
                    name,
                    reason: local_io_error(&path, "read local metadata", error).to_string(),
                });
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            if !options.follow_symlinks {
                nodes.push(UploadNode::Skipped {
                    name,
                    reason: SYMLINK_SKIPPED.to_owned(),
                });
                continue;
            }
            let target = match fs::metadata(&path) {
                Ok(target) => target,
                Err(error) => {
                    nodes.push(UploadNode::Failed {
                        name,
                        reason: format!("broken symlink `{}`: {error}", path.display()),
                    });
                    continue;
                }
            };
            if target.is_dir() {
                let children = scan_local_symlink_dir(&path, depth, options, scan);
                nodes.push(UploadNode::Dir { name, children });
            } else if target.is_file() {
                scan.bytes_total += target.len();
                nodes.push(UploadNode::File {
                    local: path,
                    name,
                    size: target.len(),
                });
            } else {
                nodes.push(UploadNode::Skipped {
                    name,
                    reason: "unsupported symlink target type".to_owned(),
                });
            }
        } else if metadata.is_dir() {
            let children = scan_local_dir(&path, depth + 1, options, scan);
            nodes.push(UploadNode::Dir { name, children });
        } else if metadata.is_file() {
            scan.bytes_total += metadata.len();
            nodes.push(UploadNode::File {
                local: path,
                name,
                size: metadata.len(),
            });
        } else {
            nodes.push(UploadNode::Skipped {
                name,
                reason: "unsupported file type".to_owned(),
            });
        }
    }
    ScanDir::Ready(nodes)
}

/// Scan a directory reached through a local symlink, guarding against cycles.
fn scan_local_symlink_dir(
    path: &Path,
    depth: usize,
    options: &TreeTransferOptions,
    scan: &mut UploadScan,
) -> ScanDir<UploadNode> {
    let canonical = match fs::canonicalize(path) {
        Ok(canonical) => canonical,
        Err(error) => {
            return ScanDir::Failed(format!(
                "failed to resolve local symlink `{}`: {error}",
                path.display()
            ));
        }
    };
    if scan.visited_dirs.contains(&canonical) {
        return ScanDir::Skipped("symlink cycle detected".to_owned());
    }
    scan.visited_dirs.insert(canonical.clone());
    let result = scan_local_dir(path, depth + 1, options, scan);
    scan.visited_dirs.remove(&canonical);
    result
}

fn local_file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "?".to_owned())
}

fn scan_remote_dir<B: SftpBackend + ?Sized>(
    backend: &mut B,
    dir: &str,
    depth: usize,
    symlink_depth: usize,
    options: &TreeTransferOptions,
    scan: &mut DownloadScan,
) -> ScanDir<DownloadNode> {
    if depth >= MAX_TREE_DEPTH {
        return ScanDir::Skipped(format!(
            "tree depth limit ({MAX_TREE_DEPTH}) exceeded at `{dir}`"
        ));
    }
    let listing = match backend.list_dir(dir) {
        Ok(listing) => listing,
        Err(error) => {
            return ScanDir::Failed(format!("failed to list remote directory `{dir}`: {error}"));
        }
    };
    ScanDir::Ready(build_download_nodes(
        backend,
        dir,
        listing.entries,
        depth,
        symlink_depth,
        options,
        scan,
    ))
}

fn build_download_nodes<B: SftpBackend + ?Sized>(
    backend: &mut B,
    dir: &str,
    mut entries: Vec<FsEntry>,
    depth: usize,
    symlink_depth: usize,
    options: &TreeTransferOptions,
    scan: &mut DownloadScan,
) -> Vec<DownloadNode> {
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    let mut nodes = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry.name.clone();
        match entry.kind {
            FsEntryKind::Directory => {
                let children = scan_remote_dir(
                    backend,
                    &join_remote(dir, &name),
                    depth + 1,
                    symlink_depth,
                    options,
                    scan,
                );
                nodes.push(DownloadNode::Dir { name, children });
            }
            FsEntryKind::File => {
                scan.bytes_total += entry.size_bytes;
                nodes.push(DownloadNode::File {
                    name,
                    size: entry.size_bytes,
                });
            }
            FsEntryKind::Symlink => {
                if !options.follow_symlinks {
                    nodes.push(DownloadNode::Skipped {
                        name,
                        reason: SYMLINK_SKIPPED.to_owned(),
                    });
                    continue;
                }
                let child = join_remote(dir, &name);
                match backend.list_dir(&child) {
                    Ok(inner) if depth + 1 < MAX_TREE_DEPTH && symlink_depth < MAX_SYMLINK_DEPTH => {
                        let children = ScanDir::Ready(build_download_nodes(
                            backend,
                            &child,
                            inner.entries,
                            depth + 1,
                            symlink_depth + 1,
                            options,
                            scan,
                        ));
                        nodes.push(DownloadNode::Dir { name, children });
                    }
                    Ok(_) => nodes.push(DownloadNode::Skipped {
                        name,
                        reason: "symlink depth limit exceeded".to_owned(),
                    }),
                    Err(_) => {
                        // A symlink that cannot be listed behaves like a file
                        // for transfer purposes.
                        scan.bytes_total += entry.size_bytes;
                        nodes.push(DownloadNode::File {
                            name,
                            size: entry.size_bytes,
                        });
                    }
                }
            }
            FsEntryKind::Other => nodes.push(DownloadNode::Skipped {
                name,
                reason: "unsupported entry type".to_owned(),
            }),
        }
    }
    nodes
}

// ---------------------------------------------------------------------------
// Processing phase
// ---------------------------------------------------------------------------

struct ProcessCtx<'a> {
    options: TreeTransferOptions,
    cancel: &'a AtomicBool,
    progress: &'a mut dyn FnMut(TransferProgress),
    direction: TransferDirection,
    bytes_total: u64,
    bytes_done: u64,
    completed: u64,
    skipped: u64,
    skipped_items: Vec<(String, String)>,
    failed: Vec<(String, String)>,
    conflicts: Vec<String>,
    bytes_transferred: u64,
    cancelled: bool,
}

impl<'a> ProcessCtx<'a> {
    fn new(
        options: TreeTransferOptions,
        cancel: &'a AtomicBool,
        progress: &'a mut dyn FnMut(TransferProgress),
        direction: TransferDirection,
        bytes_total: u64,
    ) -> Self {
        Self {
            options,
            cancel,
            progress,
            direction,
            bytes_total,
            bytes_done: 0,
            completed: 0,
            skipped: 0,
            skipped_items: Vec::new(),
            failed: Vec::new(),
            conflicts: Vec::new(),
            bytes_transferred: 0,
            cancelled: false,
        }
    }

    fn is_cancelled(&mut self) -> bool {
        if !self.cancelled && self.cancel.load(Ordering::Relaxed) {
            self.cancelled = true;
        }
        self.cancelled
    }

    fn emit(&mut self, path: &str, kind: TransferItemKind) {
        (self.progress)(TransferProgress {
            path: path.to_owned(),
            kind,
            bytes_done: self.bytes_done,
            bytes_total: self.bytes_total,
            direction: self.direction,
        });
    }

    fn complete_directory(&mut self, path: &str) {
        self.completed += 1;
        self.emit(path, TransferItemKind::Dir);
    }

    fn record_failure(&mut self, path: &str, error: &SftpError) {
        self.failed.push((path.to_owned(), error.to_string()));
    }

    fn record_failure_reason(&mut self, path: &str, reason: String) {
        self.failed.push((path.to_owned(), reason));
    }

    /// Count one item-level skip and keep its reason for the report.
    fn skip_item(&mut self, path: &str, reason: String) {
        self.skipped += 1;
        self.skipped_items.push((path.to_owned(), reason));
    }

    fn finish(self) -> TreeTransferReport {
        TreeTransferReport {
            completed: self.completed,
            skipped: self.skipped,
            skipped_items: self.skipped_items,
            failed: self.failed,
            conflicts: self.conflicts,
            cancelled: self.cancelled,
            bytes_transferred: self.bytes_transferred,
        }
    }
}

fn upload_node_count(nodes: &[UploadNode]) -> u64 {
    nodes
        .iter()
        .map(|node| {
            1 + match node {
                UploadNode::Dir { children, .. } => match children {
                    ScanDir::Ready(children) => upload_node_count(children),
                    ScanDir::Skipped(_) | ScanDir::Failed(_) => 0,
                },
                _ => 0,
            }
        })
        .sum()
}

fn download_node_count(nodes: &[DownloadNode]) -> u64 {
    nodes
        .iter()
        .map(|node| {
            1 + match node {
                DownloadNode::Dir { children, .. } => match children {
                    ScanDir::Ready(children) => download_node_count(children),
                    ScanDir::Skipped(_) | ScanDir::Failed(_) => 0,
                },
                _ => 0,
            }
        })
        .sum()
}

/// Prepare the remote transfer root. `Ok(None)` means the root was skipped by
/// policy (conflict or skip); `Err` means the transfer cannot start.
fn resolve_upload_root<B: SftpBackend + ?Sized>(
    backend: &mut B,
    remote_root: &str,
    options: TreeTransferOptions,
    ctx: &mut ProcessCtx<'_>,
) -> SftpResult<Option<String>> {
    match remote_kind(backend, remote_root) {
        Some(FsEntryKind::Directory) => {
            ctx.complete_directory(remote_root);
            Ok(Some(remote_root.to_owned()))
        }
        None => {
            ensure_remote_dir_chain(backend, remote_root)?;
            ctx.complete_directory(remote_root);
            Ok(Some(remote_root.to_owned()))
        }
        Some(_) => match options.overwrite {
            OverwritePolicy::Overwrite => {
                backend.delete(remote_root)?;
                ensure_remote_dir_chain(backend, remote_root)?;
                ctx.complete_directory(remote_root);
                Ok(Some(remote_root.to_owned()))
            }
            OverwritePolicy::Skip => Ok(None),
            OverwritePolicy::Ask => {
                ctx.conflicts.push(remote_root.to_owned());
                Ok(None)
            }
            OverwritePolicy::Rename => Err(SftpError::new(
                SftpErrorKind::InvalidPath,
                format!(
                    "remote upload root `{remote_root}` already exists and is not a directory; \
                     use Overwrite, Ask or Skip instead of Rename for the transfer root"
                ),
            )),
        },
    }
}

/// Create every missing segment of an absolute remote directory path.
fn ensure_remote_dir_chain<B: SftpBackend + ?Sized>(
    backend: &mut B,
    path: &str,
) -> SftpResult<()> {
    let mut prefix = String::new();
    for segment in path.trim_matches('/').split('/') {
        if segment.is_empty() {
            continue;
        }
        prefix.push('/');
        prefix.push_str(segment);
        if remote_kind(backend, &prefix) == Some(FsEntryKind::Directory) {
            continue;
        }
        match backend.mkdir(&prefix) {
            Ok(()) => {}
            Err(error) if error.kind == SftpErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Create or reuse a destination directory for an upload. Returns the actual
/// remote directory when available, `None` when skipped or failed (already
/// recorded in `ctx`).
fn ensure_remote_dir<B: SftpBackend + ?Sized>(
    backend: &mut B,
    desired: &str,
    overwrite: OverwritePolicy,
    ctx: &mut ProcessCtx<'_>,
) -> Option<String> {
    match remote_kind(backend, desired) {
        Some(FsEntryKind::Directory) => {
            ctx.complete_directory(desired);
            Some(desired.to_owned())
        }
        None => match backend.mkdir(desired) {
            Ok(()) => {
                ctx.complete_directory(desired);
                Some(desired.to_owned())
            }
            Err(error) if error.kind == SftpErrorKind::AlreadyExists => {
                ctx.complete_directory(desired);
                Some(desired.to_owned())
            }
            Err(error) => {
                ctx.record_failure(desired, &error);
                None
            }
        },
        Some(_) => match overwrite {
            OverwritePolicy::Overwrite => {
                if let Err(error) = backend.delete(desired) {
                    ctx.record_failure(desired, &error);
                    return None;
                }
                match backend.mkdir(desired) {
                    Ok(()) => {
                        ctx.complete_directory(desired);
                        Some(desired.to_owned())
                    }
                    Err(error) => {
                        ctx.record_failure(desired, &error);
                        None
                    }
                }
            }
            OverwritePolicy::Rename => {
                let candidate = unique_remote_path(backend, desired);
                match backend.mkdir(&candidate) {
                    Ok(()) => {
                        ctx.complete_directory(&candidate);
                        Some(candidate)
                    }
                    Err(error) => {
                        ctx.record_failure(&candidate, &error);
                        None
                    }
                }
            }
            OverwritePolicy::Skip => None,
            OverwritePolicy::Ask => {
                ctx.conflicts.push(desired.to_owned());
                None
            }
        },
    }
}

fn process_upload_nodes<B: SftpBackend + ?Sized>(
    backend: &mut B,
    base: &str,
    nodes: &[UploadNode],
    ctx: &mut ProcessCtx<'_>,
) {
    for node in nodes {
        if ctx.is_cancelled() {
            return;
        }
        match node {
            UploadNode::Dir { name, children } => {
                let desired = join_remote(base, name);
                match children {
                    ScanDir::Skipped(reason) => ctx.skip_item(&desired, reason.clone()),
                    ScanDir::Failed(reason) => {
                        ctx.record_failure_reason(&desired, reason.clone());
                    }
                    ScanDir::Ready(children) => {
                        match ensure_remote_dir(backend, &desired, ctx.options.overwrite, ctx) {
                            Some(actual) => {
                                process_upload_nodes(backend, &actual, children, ctx);
                            }
                            None => {
                                ctx.skipped += 1 + upload_node_count(children);
                            }
                        }
                    }
                }
            }
            UploadNode::File {
                local,
                name,
                size,
            } => process_upload_file(backend, base, name, local, *size, ctx),
            UploadNode::Skipped { name, reason } => {
                let path = join_remote(base, name);
                ctx.skip_item(&path, reason.clone());
            }
            UploadNode::Failed { name, reason } => {
                let path = join_remote(base, name);
                ctx.record_failure_reason(&path, reason.clone());
            }
        }
    }
}

fn process_upload_file<B: SftpBackend + ?Sized>(
    backend: &mut B,
    base: &str,
    name: &str,
    local: &Path,
    size: u64,
    ctx: &mut ProcessCtx<'_>,
) {
    let desired = join_remote(base, name);
    let mut destination = desired.clone();
    if let Some(kind) = remote_kind(backend, &desired) {
        match ctx.options.overwrite {
            OverwritePolicy::Overwrite => {
                if kind != FsEntryKind::File {
                    if let Err(error) = backend.delete(&desired) {
                        ctx.record_failure(&desired, &error);
                        return;
                    }
                }
            }
            OverwritePolicy::Rename => destination = unique_remote_path(backend, &desired),
            OverwritePolicy::Skip => {
                ctx.skipped += 1;
                return;
            }
            OverwritePolicy::Ask => {
                ctx.skipped += 1;
                ctx.conflicts.push(desired);
                return;
            }
        }
    }
    if ctx.is_cancelled() {
        return;
    }
    ctx.emit(&destination, TransferItemKind::File);
    match backend.upload_file(local, &destination) {
        Ok(()) => {
            ctx.completed += 1;
            ctx.bytes_done += size;
            ctx.bytes_transferred += size;
            ctx.emit(&destination, TransferItemKind::File);
        }
        Err(error) => ctx.record_failure(&destination, &error),
    }
}

/// Create or reuse a destination directory for a download.
///
/// `is_root` rejects [`OverwritePolicy::Rename`] for the transfer root: the
/// report has no "actual destination" field, so silently transferring into a
/// numbered sibling directory would surprise the caller.
fn ensure_local_dir(
    desired: &Path,
    overwrite: OverwritePolicy,
    remote_path: &str,
    is_root: bool,
    ctx: &mut ProcessCtx<'_>,
) -> SftpResult<Option<PathBuf>> {
    let existing = match fs::symlink_metadata(desired) {
        Ok(metadata) if metadata.is_dir() => Some(true),
        Ok(_) => Some(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(local_io_error(desired, "read local metadata of", error)),
    };
    match existing {
        Some(true) => {
            ctx.complete_directory(remote_path);
            Ok(Some(desired.to_path_buf()))
        }
        None => {
            fs::create_dir_all(desired)
                .map_err(|error| local_io_error(desired, "create local directory", error))?;
            ctx.complete_directory(remote_path);
            Ok(Some(desired.to_path_buf()))
        }
        Some(false) => match overwrite {
            OverwritePolicy::Rename if is_root => Err(SftpError::new(
                SftpErrorKind::InvalidPath,
                format!(
                    "local download root `{}` already exists and is not a directory; \
                     use Overwrite, Ask or Skip instead of Rename for the transfer root",
                    desired.display()
                ),
            )),
            OverwritePolicy::Overwrite => {
                remove_local_entry(desired)?;
                fs::create_dir_all(desired)
                    .map_err(|error| local_io_error(desired, "create local directory", error))?;
                ctx.complete_directory(remote_path);
                Ok(Some(desired.to_path_buf()))
            }
            OverwritePolicy::Rename => {
                let candidate = unique_local_path(desired);
                fs::create_dir_all(&candidate)
                    .map_err(|error| local_io_error(&candidate, "create local directory", error))?;
                ctx.complete_directory(remote_path);
                Ok(Some(candidate))
            }
            OverwritePolicy::Skip => Ok(None),
            OverwritePolicy::Ask => {
                ctx.conflicts.push(remote_path.to_owned());
                Ok(None)
            }
        },
    }
}

fn remove_local_entry(path: &Path) -> SftpResult<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(local_io_error(path, "read local metadata of", error)),
    };
    let result = if metadata.is_dir() {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|error| local_io_error(path, "replace local entry", error))
}

fn process_download_nodes<B: SftpBackend + ?Sized>(
    backend: &mut B,
    remote_base: &str,
    local_base: &Path,
    nodes: &[DownloadNode],
    ctx: &mut ProcessCtx<'_>,
) {
    for node in nodes {
        if ctx.is_cancelled() {
            return;
        }
        match node {
            DownloadNode::Dir { name, children } => {
                let remote_path = join_remote(remote_base, name);
                match children {
                    ScanDir::Skipped(reason) => ctx.skip_item(&remote_path, reason.clone()),
                    ScanDir::Failed(reason) => ctx.record_failure_reason(&remote_path, reason.clone()),
                    ScanDir::Ready(children) => {
                        match ensure_local_dir(
                            &local_base.join(name),
                            ctx.options.overwrite,
                            &remote_path,
                            false,
                            ctx,
                        ) {
                            Ok(Some(actual)) => {
                                process_download_nodes(backend, &remote_path, &actual, children, ctx);
                            }
                            Ok(None) => {
                                ctx.skipped += 1 + download_node_count(children);
                            }
                            Err(error) => {
                                ctx.record_failure(&remote_path, &error);
                                ctx.skipped += 1 + download_node_count(children);
                            }
                        }
                    }
                }
            }
            DownloadNode::File { name, size } => {
                process_download_file(backend, remote_base, local_base, name, *size, ctx);
            }
            DownloadNode::Skipped { name, reason } => {
                let remote_path = join_remote(remote_base, name);
                ctx.skip_item(&remote_path, reason.clone());
            }
        }
    }
}

fn process_download_file<B: SftpBackend + ?Sized>(
    backend: &mut B,
    remote_base: &str,
    local_base: &Path,
    name: &str,
    size: u64,
    ctx: &mut ProcessCtx<'_>,
) {
    let remote_path = join_remote(remote_base, name);
    let desired = local_base.join(name);
    let mut destination = desired.clone();
    match fs::symlink_metadata(&desired) {
        Ok(metadata) => match ctx.options.overwrite {
            OverwritePolicy::Overwrite => {
                if !metadata.is_file() {
                    if let Err(error) = remove_local_entry(&desired) {
                        ctx.record_failure(&remote_path, &error);
                        return;
                    }
                }
            }
            OverwritePolicy::Rename => destination = unique_local_path(&desired),
            OverwritePolicy::Skip => {
                ctx.skipped += 1;
                return;
            }
            OverwritePolicy::Ask => {
                ctx.skipped += 1;
                ctx.conflicts.push(remote_path);
                return;
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            ctx.record_failure(&remote_path, &local_io_error(&desired, "read local metadata of", error));
            return;
        }
    }
    if ctx.is_cancelled() {
        return;
    }
    ctx.emit(&remote_path, TransferItemKind::File);
    match backend.download_file(&remote_path, &destination) {
        Ok(()) => {
            ctx.completed += 1;
            ctx.bytes_done += size;
            ctx.bytes_transferred += size;
            ctx.emit(&remote_path, TransferItemKind::File);
        }
        Err(error) => ctx.record_failure(&remote_path, &error),
    }
}
