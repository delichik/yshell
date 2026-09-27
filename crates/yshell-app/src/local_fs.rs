//! Local filesystem browsing for the SFTP panel's local pane (N1).
//!
//! This module is deliberately read-only: it lists directory entries with
//! [`std::fs::symlink_metadata`] so symlinks are reported as-is and never
//! followed, filters/sorts them into the same shape the remote pane uses
//! (directories first, case-insensitive names) and maps `std::io::Error` to a
//! small kind enum the panel can render inline with a retry entry point.
//!
//! Navigation is lexical: `..` is resolved by path arithmetic instead of
//! `canonicalize`, because resolving symlinks would silently move the pane to
//! the link target. The local pane therefore shows exactly the path the user
//! navigated to.
//!
//! Everything past [`list_dir`] is a pure function over already-read metadata,
//! so the row model and sorting can be tested without touching the filesystem.
//!
//! N1 Phase 1 note: the local pane wiring lands in Phase 2 (`sftp_panel.slint`
//! rewrite + `bootstrap.rs`), so until then only the unit tests below reference
//! this module. The module-wide allow below keeps `cargo xtask lint` green in
//! the meantime and must be deleted when the pane is wired up.
#![allow(dead_code)]

use std::{
    fmt, fs, io,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

use yshell_sftp::FsEntryKind;

use crate::sftp_view::{
    format_sftp_modified, format_sftp_permissions, format_sftp_size, sftp_kind_text,
};

/// What a local entry is. Symlinks keep their own kind even when the target is
/// a directory, mirroring `ls -l`/`FsEntryKind` semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalEntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

impl LocalEntryKind {
    /// True for real directories only; symlinks never count as directories.
    pub const fn is_dir(self) -> bool {
        matches!(self, Self::Directory)
    }

    /// Display label shared with the remote pane (`Directory`/`File`/...).
    pub const fn label(self) -> &'static str {
        match self {
            Self::Directory => "Directory",
            Self::File => "File",
            Self::Symlink => "Symlink",
            Self::Other => "Other",
        }
    }
}

/// One local filesystem entry with read-only metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: LocalEntryKind,
    pub size_bytes: u64,
    /// Unix permission bits (0 on platforms without them).
    pub permissions: u32,
    pub modified: Option<SystemTime>,
}

/// Sortable local list columns; ids/labels match the remote pane so the two
/// sides sort/select by the same callback payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocalSortColumn {
    #[default]
    Name,
    Size,
    Modified,
    Permissions,
}

impl LocalSortColumn {
    /// Parses the column label sent by the Slint callbacks (case-insensitive).
    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "name" => Some(Self::Name),
            "size" => Some(Self::Size),
            "modified" => Some(Self::Modified),
            "permissions" => Some(Self::Permissions),
            _ => None,
        }
    }

    /// Column label shown in the header and the sort summary.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Modified => "Modified",
            Self::Permissions => "Permissions",
        }
    }

    /// Stable id used by the UI projection (`name`, `size`, ...).
    pub const fn id(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Size => "size",
            Self::Modified => "modified",
            Self::Permissions => "permissions",
        }
    }
}

/// Coarse error kind so the panel can decide whether to offer a retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalFsErrorKind {
    NotFound,
    PermissionDenied,
    NotADirectory,
    InvalidPath,
    Other,
}

impl LocalFsErrorKind {
    /// Stable id for the UI projection (`not-found`, ...).
    pub const fn id(self) -> &'static str {
        match self {
            Self::NotFound => "not-found",
            Self::PermissionDenied => "permission-denied",
            Self::NotADirectory => "not-a-directory",
            Self::InvalidPath => "invalid-path",
            Self::Other => "other",
        }
    }
}

/// A local pane failure with the offending path kept for the inline error bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFsError {
    pub kind: LocalFsErrorKind,
    pub path: Option<PathBuf>,
    pub message: String,
}

impl LocalFsError {
    /// A user-input problem (empty path, unsupported `~user`, ...).
    pub fn invalid_path(path: Option<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            kind: LocalFsErrorKind::InvalidPath,
            path,
            message: message.into(),
        }
    }

    /// Maps an I/O failure on `path` to a kind and a readable sentence.
    pub fn from_io(path: &Path, error: io::Error) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::NotFound => LocalFsErrorKind::NotFound,
            io::ErrorKind::PermissionDenied => LocalFsErrorKind::PermissionDenied,
            io::ErrorKind::NotADirectory => LocalFsErrorKind::NotADirectory,
            _ => LocalFsErrorKind::Other,
        };
        Self {
            kind,
            path: Some(path.to_path_buf()),
            message: format!("failed to read `{}`: {error}", path.display()),
        }
    }

    /// Stable id for the UI projection.
    pub const fn kind_id(&self) -> &'static str {
        self.kind.id()
    }

    /// Invalid navigation input cannot be fixed by retrying the same path; every
    /// other failure can (the user may have created/mounted the directory).
    pub const fn retryable(&self) -> bool {
        !matches!(self.kind, LocalFsErrorKind::InvalidPath)
    }

    /// Path text for the inline error bar, or `""` when no path is known.
    pub fn path_text(&self) -> String {
        self.path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    }
}

impl fmt::Display for LocalFsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LocalFsError {}

/// One local file-list row, ready for the Slint model. The `..` marker row has
/// `is_parent = true`, `is_dir = true` and an empty size/modified/permissions.
///
/// `selected` is filled in by the runtime (multi-select state), not by
/// [`row_data`]: a raw listing has no selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRowData {
    pub name: String,
    pub path_text: String,
    pub kind_text: String,
    pub size_text: String,
    pub modified_text: String,
    pub permissions_text: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_parent: bool,
    pub selected: bool,
}

/// Options carried by the local pane session (path/sort/hidden are separate
/// from the remote pane's state per the N1 design).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalPaneOptions {
    pub show_hidden: bool,
    pub sort_column: LocalSortColumn,
    pub sort_ascending: bool,
}

impl Default for LocalPaneOptions {
    fn default() -> Self {
        Self {
            show_hidden: false,
            sort_column: LocalSortColumn::Name,
            sort_ascending: true,
        }
    }
}

/// Everything the local pane needs for one render pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPaneListing {
    /// Lexically normalized directory this listing came from.
    pub dir: PathBuf,
    /// Parent directory (`..` target), `None` at a filesystem root.
    pub parent: Option<PathBuf>,
    pub options: LocalPaneOptions,
    /// Visible entries only (hidden filtered), sorted; never contains `..`.
    pub entries: Vec<LocalEntry>,
    /// `..` row first (when a parent exists) followed by one row per entry.
    pub rows: Vec<LocalRowData>,
    /// Number of hidden entries the current filter removed.
    pub hidden_count: usize,
    /// Visible entry count (excludes the `..` row).
    pub item_count: usize,
    /// Visible directory count (symlinks excluded).
    pub dir_count: usize,
}

impl LocalPaneListing {
    /// Maps a row index (as sent by the Slint list) back to an entry.
    /// Returns `None` for the `..` row and out-of-range indices.
    pub fn entry_for_row(&self, row_index: usize) -> Option<&LocalEntry> {
        let offset = usize::from(self.parent.is_some());
        self.entries.get(row_index.checked_sub(offset)?)
    }

    /// True when the given row is the synthetic `..` row.
    pub fn is_parent_row(&self, row_index: usize) -> bool {
        self.parent.is_some() && row_index == 0
    }
}

/// Lists one directory with read-only metadata. Entries are returned in
/// `read_dir` order; callers use [`visible_entries`]/[`list_pane`] to sort.
///
/// A single entry whose `lstat` fails (it disappeared mid-listing or is not
/// resolvable) is skipped instead of failing the whole refresh: it cannot be
/// rendered anyway, and one vanished file should not blank the pane.
pub fn list_dir(dir: &Path) -> Result<Vec<LocalEntry>, LocalFsError> {
    let reader = fs::read_dir(dir).map_err(|error| LocalFsError::from_io(dir, error))?;
    let mut entries = Vec::new();
    for item in reader {
        let item = item.map_err(|error| LocalFsError::from_io(dir, error))?;
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();
        // `symlink_metadata` is the whole point: symlinks are reported as
        // symlinks and never followed into their target.
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            LocalEntryKind::Symlink
        } else if file_type.is_dir() {
            LocalEntryKind::Directory
        } else if file_type.is_file() {
            LocalEntryKind::File
        } else {
            LocalEntryKind::Other
        };
        entries.push(LocalEntry {
            path,
            name,
            kind,
            size_bytes: metadata.len(),
            permissions: permissions_of(&metadata),
            modified: metadata.modified().ok(),
        });
    }
    Ok(entries)
}

/// Filters dotfiles and sorts the way the remote pane does: directories first,
/// then the selected column, then case-insensitive name, then exact name.
pub fn visible_entries(
    entries: &[LocalEntry],
    show_hidden: bool,
    sort_column: LocalSortColumn,
    ascending: bool,
) -> Vec<LocalEntry> {
    let mut visible = entries
        .iter()
        .filter(|entry| show_hidden || !entry.name.starts_with('.'))
        .cloned()
        .collect::<Vec<_>>();
    visible.sort_by(|left, right| compare_entries(left, right, sort_column, ascending));
    visible
}

/// Counts entries the hidden filter removed (drives the empty-state hint).
pub fn hidden_count(entries: &[LocalEntry]) -> usize {
    entries
        .iter()
        .filter(|entry| entry.name.starts_with('.'))
        .count()
}

/// Builds one UI row from an entry.
pub fn row_data(entry: &LocalEntry) -> LocalRowData {
    let is_dir = entry.kind.is_dir();
    LocalRowData {
        name: entry.name.clone(),
        path_text: entry.path.display().to_string(),
        kind_text: sftp_kind_text(fs_entry_kind(entry.kind)).to_owned(),
        size_text: if is_dir {
            String::new()
        } else {
            format_sftp_size(entry.size_bytes)
        },
        modified_text: format_sftp_modified(entry.modified),
        permissions_text: format_sftp_permissions(fs_entry_kind(entry.kind), entry.permissions),
        is_dir,
        is_symlink: matches!(entry.kind, LocalEntryKind::Symlink),
        is_parent: false,
        // Multi-select state is applied by the runtime; a raw row starts
        // unselected.
        selected: false,
    }
}

/// The synthetic `..` row for `parent`, or `None` for an empty path.
pub fn parent_row_data(parent: &Path) -> LocalRowData {
    LocalRowData {
        name: "..".to_owned(),
        path_text: parent.display().to_string(),
        kind_text: LocalEntryKind::Directory.label().to_owned(),
        size_text: String::new(),
        modified_text: String::new(),
        permissions_text: String::new(),
        is_dir: true,
        is_symlink: false,
        is_parent: true,
        selected: false,
    }
}

/// Full read + filter + sort pass for the local pane.
pub fn list_pane(dir: &Path, options: LocalPaneOptions) -> Result<LocalPaneListing, LocalFsError> {
    let dir = lexical_normalize(dir);
    let entries = list_dir(&dir)?;
    let hidden_count = hidden_count(&entries);
    let visible = visible_entries(
        &entries,
        options.show_hidden,
        options.sort_column,
        options.sort_ascending,
    );
    let dir_count = visible.iter().filter(|entry| entry.kind.is_dir()).count();
    let parent = parent_dir(&dir);
    let mut rows = Vec::with_capacity(visible.len() + 1);
    if let Some(parent) = &parent {
        rows.push(parent_row_data(parent));
    }
    rows.extend(visible.iter().map(row_data));
    let item_count = visible.len();
    Ok(LocalPaneListing {
        dir,
        parent,
        options,
        entries: visible,
        rows,
        hidden_count,
        item_count,
        dir_count,
    })
}

/// Lexical parent of a directory (no symlink resolution). `None` at a root or
/// when the path has no usable parent (relative single components).
pub fn parent_dir(dir: &Path) -> Option<PathBuf> {
    let parent = dir.parent()?;
    if parent.as_os_str().is_empty() || parent == dir {
        return None;
    }
    Some(parent.to_path_buf())
}

/// Resolves typed path input against the current pane directory:
///
/// * an empty/whitespace path is rejected,
/// * a leading `~` expands to the home directory (`~user` is rejected),
/// * relative paths are joined onto `current`,
/// * `.`/`..` are folded lexically, keeping symlinks untouched.
pub fn normalize_local_dir(input: &str, current: &Path) -> Result<PathBuf, LocalFsError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(LocalFsError::invalid_path(
            None,
            "local directory path must not be empty",
        ));
    }
    let expanded = expand_home(trimmed)?;
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        current.join(expanded)
    };
    Ok(lexical_normalize(&joined))
}

/// Home directory used for `~` expansion (`HOME`, then `USERPROFILE`).
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// Default directory for a fresh local pane: home, then the process cwd.
pub fn default_local_dir() -> PathBuf {
    home_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn expand_home(input: &str) -> Result<PathBuf, LocalFsError> {
    let Some(rest) = input.strip_prefix('~') else {
        return Ok(PathBuf::from(input));
    };
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\')) {
        return Err(LocalFsError::invalid_path(
            Some(PathBuf::from(input)),
            format!("`~user` expansion is not supported: `{input}`"),
        ));
    }
    let Some(home) = home_dir() else {
        return Err(LocalFsError::invalid_path(
            Some(PathBuf::from(input)),
            "cannot expand `~`: the home directory is unknown",
        ));
    };
    let rest = rest.trim_start_matches(['/', '\\']);
    Ok(if rest.is_empty() {
        home
    } else {
        home.join(rest)
    })
}

/// Folds `.`/`..` segments without touching the filesystem, so symlinked
/// directory components keep their textual identity.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            Component::Normal(name) => normalized.push(name),
        }
    }
    if normalized.as_os_str().is_empty() {
        normalized.push(".");
    }
    normalized
}

/// `name.txt` → `name (1).txt`; used when a copy must not clobber its target.
pub(crate) fn numbered_name(name: &str, index: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => {
            format!("{} ({index}).{}", &name[..dot], &name[dot + 1..])
        }
        _ => format!("{name} ({index})"),
    }
}

fn compare_entries(
    left: &LocalEntry,
    right: &LocalEntry,
    sort_column: LocalSortColumn,
    ascending: bool,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let directories_first = right.kind.is_dir().cmp(&left.kind.is_dir());
    if directories_first != Ordering::Equal {
        return directories_first;
    }
    let ordering = match sort_column {
        LocalSortColumn::Name => name_key(left).cmp(&name_key(right)),
        LocalSortColumn::Size => left.size_bytes.cmp(&right.size_bytes),
        LocalSortColumn::Modified => left.modified.cmp(&right.modified),
        LocalSortColumn::Permissions => left.permissions.cmp(&right.permissions),
    };
    let ordering = if ascending {
        ordering
    } else {
        ordering.reverse()
    };
    ordering
        .then_with(|| name_key(left).cmp(&name_key(right)))
        .then_with(|| left.name.cmp(&right.name))
}

fn name_key(entry: &LocalEntry) -> String {
    entry.name.to_lowercase()
}

fn fs_entry_kind(kind: LocalEntryKind) -> FsEntryKind {
    match kind {
        LocalEntryKind::Directory => FsEntryKind::Directory,
        LocalEntryKind::File => FsEntryKind::File,
        LocalEntryKind::Symlink => FsEntryKind::Symlink,
        LocalEntryKind::Other => FsEntryKind::Other,
    }
}

#[cfg(unix)]
fn permissions_of(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn permissions_of(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{Duration, UNIX_EPOCH},
    };

    use tempfile::tempdir;

    use super::{
        default_local_dir, hidden_count, list_dir, list_pane, normalize_local_dir, parent_dir,
        parent_row_data, row_data, visible_entries, LocalEntry, LocalEntryKind, LocalFsErrorKind,
        LocalPaneOptions, LocalSortColumn,
    };

    fn entry(
        name: &str,
        kind: LocalEntryKind,
        size: u64,
        permissions: u32,
        modified: Option<u64>,
    ) -> LocalEntry {
        LocalEntry {
            path: PathBuf::from("/demo").join(name),
            name: name.to_owned(),
            kind,
            size_bytes: size,
            permissions,
            modified: modified.map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds)),
        }
    }

    fn option_file_names(entries: &[LocalEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.name.as_str()).collect()
    }

    #[test]
    fn list_dir_reports_kinds_and_does_not_follow_symlinks() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        fs::create_dir(root.join("nested")).expect("mkdir");
        fs::write(root.join("notes.txt"), b"hello").expect("write file");
        fs::write(root.join(".profile"), b"x").expect("write hidden");

        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("nested"), root.join("link-to-dir")).expect("symlink");

        let entries = list_dir(root).expect("list");
        assert_eq!(entries.len(), if cfg!(unix) { 4 } else { 3 });

        let nested = entries
            .iter()
            .find(|entry| entry.name == "nested")
            .expect("nested");
        assert_eq!(nested.kind, LocalEntryKind::Directory);

        let notes = entries
            .iter()
            .find(|entry| entry.name == "notes.txt")
            .expect("notes");
        assert_eq!(notes.kind, LocalEntryKind::File);
        assert_eq!(notes.size_bytes, 5);

        #[cfg(unix)]
        {
            let link = entries
                .iter()
                .find(|entry| entry.name == "link-to-dir")
                .expect("link");
            // As-is: a symlink to a directory stays a symlink, and its path is
            // the link itself, not the target.
            assert_eq!(link.kind, LocalEntryKind::Symlink);
            assert_eq!(link.path, root.join("link-to-dir"));
        }
    }

    #[test]
    fn hidden_entries_are_filtered_and_counted() {
        let entries = vec![
            entry(".gitignore", LocalEntryKind::File, 12, 0o644, None),
            entry("readme.md", LocalEntryKind::File, 34, 0o644, None),
            entry(".config", LocalEntryKind::Directory, 0, 0o755, None),
        ];
        let filtered = visible_entries(&entries, false, LocalSortColumn::Name, true);
        assert_eq!(option_file_names(&filtered), vec!["readme.md"]);
        assert_eq!(hidden_count(&entries), 2);

        let shown = visible_entries(&entries, true, LocalSortColumn::Name, true);
        assert_eq!(option_file_names(&shown).len(), 3);
    }

    #[test]
    fn sorting_keeps_directories_first_and_names_case_insensitive() {
        let entries = vec![
            entry(
                "Beta.txt",
                LocalEntryKind::File,
                2048,
                0o644,
                Some(1_700_000_000),
            ),
            entry("zeta", LocalEntryKind::Directory, 0, 0o755, None),
            entry(
                "alpha.txt",
                LocalEntryKind::File,
                1024,
                0o600,
                Some(1_700_000_100),
            ),
            entry("Alpha", LocalEntryKind::Directory, 0, 0o755, None),
        ];

        let ascending = visible_entries(&entries, false, LocalSortColumn::Name, true);
        assert_eq!(
            option_file_names(&ascending),
            vec!["Alpha", "zeta", "alpha.txt", "Beta.txt"]
        );

        let descending = visible_entries(&entries, false, LocalSortColumn::Name, false);
        assert_eq!(
            option_file_names(&descending),
            vec!["zeta", "Alpha", "Beta.txt", "alpha.txt"]
        );
    }

    #[test]
    fn sorting_switches_direction_for_data_columns() {
        let entries = vec![
            entry("small.bin", LocalEntryKind::File, 10, 0o600, Some(10)),
            entry("large.bin", LocalEntryKind::File, 4096, 0o644, Some(30)),
            entry("dir", LocalEntryKind::Directory, 0, 0o700, None),
        ];

        let by_size = visible_entries(&entries, false, LocalSortColumn::Size, true);
        assert_eq!(
            option_file_names(&by_size),
            vec!["dir", "small.bin", "large.bin"]
        );
        let by_size_desc = visible_entries(&entries, false, LocalSortColumn::Size, false);
        assert_eq!(
            option_file_names(&by_size_desc),
            vec!["dir", "large.bin", "small.bin"]
        );

        let by_modified = visible_entries(&entries, false, LocalSortColumn::Modified, true);
        assert_eq!(
            option_file_names(&by_modified),
            vec!["dir", "small.bin", "large.bin"]
        );

        let by_permissions = visible_entries(&entries, false, LocalSortColumn::Permissions, true);
        assert_eq!(
            option_file_names(&by_permissions),
            vec!["dir", "small.bin", "large.bin"]
        );
        let by_permissions_desc =
            visible_entries(&entries, false, LocalSortColumn::Permissions, false);
        assert_eq!(
            option_file_names(&by_permissions_desc),
            vec!["dir", "large.bin", "small.bin"]
        );
    }

    #[test]
    fn row_model_formats_like_the_remote_pane() {
        let dir = entry("nested", LocalEntryKind::Directory, 0, 0o755, None);
        let file = entry(
            "report.txt",
            LocalEntryKind::File,
            1536,
            0o644,
            Some(1_700_000_000),
        );
        #[cfg(unix)]
        let link = entry("latest", LocalEntryKind::Symlink, 0, 0o777, None);

        let dir_row = row_data(&dir);
        assert_eq!(dir_row.kind_text, "Directory");
        assert!(dir_row.is_dir);
        assert_eq!(dir_row.size_text, "");
        assert_eq!(dir_row.permissions_text, "drwxr-xr-x");
        assert!(!dir_row.is_parent);

        let file_row = row_data(&file);
        assert_eq!(file_row.kind_text, "File");
        assert!(!file_row.is_dir);
        assert_eq!(file_row.size_text, "1.5 KB");
        assert_eq!(file_row.permissions_text, "-rw-r--r--");
        assert_eq!(file_row.modified_text, "2023-11-14 22:13");

        #[cfg(unix)]
        {
            let link_row = row_data(&link);
            assert!(!link_row.is_dir);
            assert!(link_row.is_symlink);
            assert_eq!(link_row.permissions_text, "lrwxrwxrwx");
        }
    }

    #[test]
    fn list_pane_puts_the_parent_row_first_and_maps_rows_back() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("sub")).expect("mkdir");
        fs::write(temp.path().join("file.txt"), b"x").expect("write");

        let listing = list_pane(temp.path(), LocalPaneOptions::default()).expect("pane");
        assert_eq!(listing.dir, temp.path());
        assert_eq!(listing.parent.as_deref(), temp.path().parent());
        assert_eq!(
            listing.rows.first().map(|row| row.name.as_str()),
            Some("..")
        );
        assert!(listing.rows[0].is_parent);
        assert_eq!(listing.item_count, 2);
        assert_eq!(listing.dir_count, 1);
        assert_eq!(listing.rows.len(), 3);
        assert!(listing.is_parent_row(0));

        assert!(listing.entry_for_row(0).is_none());
        assert_eq!(
            listing.entry_for_row(1).map(|e| e.name.as_str()),
            Some("sub")
        );
        assert_eq!(
            listing.entry_for_row(2).map(|e| e.name.as_str()),
            Some("file.txt")
        );
        assert!(listing.entry_for_row(3).is_none());
    }

    #[test]
    fn parent_dir_is_none_at_a_filesystem_root() {
        assert_eq!(parent_dir(PathBuf::from("/").as_path()), None);
        assert_eq!(
            parent_dir(PathBuf::from("/srv/logs").as_path()),
            Some(PathBuf::from("/srv"))
        );
    }

    #[test]
    fn parent_row_data_is_always_a_directory_marker() {
        let row = parent_row_data(PathBuf::from("/srv").as_path());
        assert_eq!(row.name, "..");
        assert!(row.is_parent);
        assert!(row.is_dir);
        assert!(!row.is_symlink);
        assert_eq!(row.path_text, "/srv");
        assert_eq!(row.kind_text, "Directory");
    }

    #[test]
    fn normalize_local_dir_joins_and_folds_without_resolving_symlinks() {
        let current = PathBuf::from("/srv/logs");
        assert_eq!(
            normalize_local_dir("archive", &current).expect("join"),
            PathBuf::from("/srv/logs/archive")
        );
        assert_eq!(
            normalize_local_dir("../etc/../etc/ssh", &current).expect("fold"),
            PathBuf::from("/srv/etc/ssh")
        );
        assert_eq!(
            normalize_local_dir("/absolute/./path", &current).expect("absolute"),
            PathBuf::from("/absolute/path")
        );
        let error = normalize_local_dir("   ", &current).expect_err("empty");
        assert_eq!(error.kind, LocalFsErrorKind::InvalidPath);
        assert!(!error.retryable());

        let unsupported = normalize_local_dir("~other/secret", &current).expect_err("~user");
        assert_eq!(unsupported.kind, LocalFsErrorKind::InvalidPath);
    }

    #[test]
    fn normalize_local_dir_expands_the_home_shortcut() {
        let Some(home) = super::home_dir() else {
            return;
        };
        let current = PathBuf::from("/srv");
        assert_eq!(normalize_local_dir("~", &current).expect("~"), home);
        assert_eq!(
            normalize_local_dir("~/downloads", &current).expect("~/"),
            home.join("downloads")
        );
    }

    #[test]
    fn missing_and_non_directory_paths_map_to_distinct_error_kinds() {
        let temp = tempdir().expect("tempdir");
        let missing = temp.path().join("nope");
        let error = list_dir(&missing).expect_err("missing");
        assert_eq!(error.kind, LocalFsErrorKind::NotFound);
        assert!(error.retryable());
        assert!(error.path_text().contains("nope"));
        assert!(error.to_string().contains("failed to read"));

        let file = temp.path().join("file.txt");
        fs::write(&file, b"x").expect("write");
        let error = list_dir(&file).expect_err("file");
        assert_eq!(error.kind, LocalFsErrorKind::NotADirectory);
    }

    #[test]
    fn default_local_dir_prefers_the_home_directory() {
        let dir = default_local_dir();
        if let Some(home) = super::home_dir() {
            assert_eq!(dir, home);
        } else {
            assert!(!dir.as_os_str().is_empty());
        }
    }
}
