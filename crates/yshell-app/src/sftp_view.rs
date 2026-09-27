//! SFTP file-list view helpers: hidden-file filtering, deterministic sorting
//! and the display formatting used by the SFTP panel.
//!
//! The list keeps every directory ahead of every file for all sort columns and
//! directions, so navigation stays predictable while sorting.

use std::{
    cmp::Ordering,
    time::{SystemTime, UNIX_EPOCH},
};

use yshell_sftp::{FsEntry, FsEntryKind};

/// Sortable SFTP list columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SftpSortColumn {
    Name,
    Size,
    Modified,
    Permissions,
}

impl SftpSortColumn {
    /// Parses the column label used by the UI callbacks (case-insensitive).
    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "name" => Some(Self::Name),
            "size" => Some(Self::Size),
            "modified" => Some(Self::Modified),
            "permissions" => Some(Self::Permissions),
            _ => None,
        }
    }

    /// Column label shown in the status line and the column header.
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Modified => "Modified",
            Self::Permissions => "Permissions",
        }
    }
}

/// Returns the visible rows for a directory: `.`/`..` are always dropped,
/// dotfiles only when hidden files are enabled, then the rows are sorted with
/// directories first.
pub fn visible_entries(
    entries: &[FsEntry],
    show_hidden: bool,
    sort_column: SftpSortColumn,
    ascending: bool,
) -> Vec<FsEntry> {
    let mut visible = entries
        .iter()
        .filter(|entry| !matches!(entry.name.as_str(), "." | ".."))
        .filter(|entry| show_hidden || !entry.name.starts_with('.'))
        .cloned()
        .collect::<Vec<_>>();
    visible.sort_by(|left, right| compare_entries(left, right, sort_column, ascending));
    visible
}

fn compare_entries(
    left: &FsEntry,
    right: &FsEntry,
    sort_column: SftpSortColumn,
    ascending: bool,
) -> Ordering {
    let directories_first = is_directory(right)
        .cmp(&is_directory(left))
        .then_with(|| Ordering::Equal);
    if directories_first != Ordering::Equal {
        return directories_first;
    }
    let ordering = match sort_column {
        SftpSortColumn::Name => name_key(left).cmp(&name_key(right)),
        SftpSortColumn::Size => left.size_bytes.cmp(&right.size_bytes),
        SftpSortColumn::Modified => left.modified.cmp(&right.modified),
        SftpSortColumn::Permissions => left.permissions.cmp(&right.permissions),
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

fn is_directory(entry: &FsEntry) -> bool {
    matches!(entry.kind, FsEntryKind::Directory)
}

fn name_key(entry: &FsEntry) -> String {
    entry.name.to_lowercase()
}

/// Human readable file size: `B`, `KB`, `MB`, `GB` with a 1024 divisor.
pub fn format_sftp_size(size_bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = size_bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size_bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// UTC `YYYY-MM-DD HH:MM` timestamp, or `-` when the backend did not report one.
pub fn format_sftp_modified(modified: Option<SystemTime>) -> String {
    let Some(modified) = modified else {
        return "-".to_owned();
    };
    let seconds = match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    };
    let (year, month, day, hour, minute) = civil_from_unix_seconds(seconds);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// `drwxr-xr-x` style permission text: a `d`/`-`/`l` file-type byte followed by
/// the nine rwx permission cells.
pub fn format_sftp_permissions(kind: FsEntryKind, permissions: u32) -> String {
    let mut text = String::with_capacity(10);
    text.push(match kind {
        FsEntryKind::Directory => 'd',
        FsEntryKind::Symlink => 'l',
        FsEntryKind::File | FsEntryKind::Other => '-',
    });
    for shift in [6, 3, 0] {
        let bits = (permissions >> shift) & 0o7;
        text.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        text.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        text.push(if bits & 0o1 != 0 { 'x' } else { '-' });
    }
    text
}

/// Three-digit octal permission text used to prefill the Chmod dialog.
pub fn format_sftp_permissions_octal(permissions: u32) -> String {
    format!("{:03o}", permissions & 0o777)
}

/// Display label for an entry kind.
pub fn sftp_kind_text(kind: FsEntryKind) -> &'static str {
    match kind {
        FsEntryKind::Directory => "Directory",
        FsEntryKind::File => "File",
        FsEntryKind::Symlink => "Symlink",
        FsEntryKind::Other => "Other",
    }
}

/// Converts Unix seconds to UTC `(year, month, day, hour, minute)` using the
/// civil-from-days algorithm (no calendar dependency).
fn civil_from_unix_seconds(seconds: i64) -> (i64, u32, u32, u32, u32) {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let hour = (seconds_of_day / 3_600) as u32;
    let minute = ((seconds_of_day % 3_600) / 60) as u32;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day, hour, minute)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use yshell_sftp::{FsEntry, FsEntryKind};

    use super::{
        format_sftp_modified, format_sftp_permissions, format_sftp_permissions_octal,
        format_sftp_size, visible_entries, SftpSortColumn,
    };

    fn file(name: &str, size: u64, permissions: u32, modified: Option<u64>) -> FsEntry {
        FsEntry {
            path: format!("/demo/{name}"),
            name: name.to_owned(),
            kind: FsEntryKind::File,
            size_bytes: size,
            permissions,
            modified: modified.map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
        }
    }

    fn directory(name: &str) -> FsEntry {
        FsEntry {
            path: format!("/demo/{name}"),
            name: name.to_owned(),
            kind: FsEntryKind::Directory,
            size_bytes: 0,
            permissions: 0o755,
            modified: None,
        }
    }

    fn fixture() -> Vec<FsEntry> {
        vec![
            file("Beta.txt", 2048, 0o644, Some(1_700_000_000)),
            directory("zeta"),
            file("alpha.txt", 1024, 0o600, Some(1_700_000_100)),
            directory("Alpha"),
            file(".hidden.txt", 10, 0o600, Some(1_700_000_200)),
        ]
    }

    fn names(entries: &[FsEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.name.as_str()).collect()
    }

    #[test]
    fn sorting_keeps_directories_first_and_names_case_insensitive() {
        let ascending = visible_entries(&fixture(), false, SftpSortColumn::Name, true);
        assert_eq!(
            names(&ascending),
            vec!["Alpha", "zeta", "alpha.txt", "Beta.txt"]
        );

        let descending = visible_entries(&fixture(), false, SftpSortColumn::Name, false);
        assert_eq!(
            names(&descending),
            vec!["zeta", "Alpha", "Beta.txt", "alpha.txt"]
        );
    }

    #[test]
    fn sorting_switches_direction_for_data_columns() {
        let by_size = visible_entries(&fixture(), false, SftpSortColumn::Size, true);
        assert_eq!(
            names(&by_size),
            vec!["Alpha", "zeta", "alpha.txt", "Beta.txt"]
        );
        let by_size_desc = visible_entries(&fixture(), false, SftpSortColumn::Size, false);
        assert_eq!(
            names(&by_size_desc),
            vec!["Alpha", "zeta", "Beta.txt", "alpha.txt"]
        );

        let by_modified = visible_entries(&fixture(), false, SftpSortColumn::Modified, true);
        assert_eq!(
            names(&by_modified),
            vec!["Alpha", "zeta", "Beta.txt", "alpha.txt"]
        );
        let by_modified_desc = visible_entries(&fixture(), false, SftpSortColumn::Modified, false);
        assert_eq!(
            names(&by_modified_desc),
            vec!["Alpha", "zeta", "alpha.txt", "Beta.txt"]
        );

        let by_permissions = visible_entries(&fixture(), false, SftpSortColumn::Permissions, true);
        assert_eq!(
            names(&by_permissions),
            vec!["Alpha", "zeta", "alpha.txt", "Beta.txt"]
        );
        let by_permissions_desc =
            visible_entries(&fixture(), false, SftpSortColumn::Permissions, false);
        assert_eq!(
            names(&by_permissions_desc),
            vec!["Alpha", "zeta", "Beta.txt", "alpha.txt"]
        );
    }

    #[test]
    fn hidden_files_are_filtered_until_enabled() {
        let filtered = visible_entries(&fixture(), false, SftpSortColumn::Name, true);
        assert!(!names(&filtered).contains(&".hidden.txt"));

        let shown = visible_entries(&fixture(), true, SftpSortColumn::Name, true);
        assert_eq!(
            names(&shown),
            vec!["Alpha", "zeta", ".hidden.txt", "alpha.txt", "Beta.txt"]
        );
    }

    #[test]
    fn dot_and_dotdot_entries_are_always_dropped() {
        let mut entries = fixture();
        entries.push(directory("."));
        entries.push(directory(".."));
        let visible = visible_entries(&entries, true, SftpSortColumn::Name, true);
        assert!(!names(&visible)
            .iter()
            .any(|name| *name == "." || *name == ".."));
    }

    #[test]
    fn size_formatting_covers_every_unit() {
        assert_eq!(format_sftp_size(0), "0 B");
        assert_eq!(format_sftp_size(999), "999 B");
        assert_eq!(format_sftp_size(1024), "1.0 KB");
        assert_eq!(format_sftp_size(1536), "1.5 KB");
        assert_eq!(format_sftp_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_sftp_size(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn permission_formatting_uses_type_byte_and_rwx_cells() {
        assert_eq!(
            format_sftp_permissions(FsEntryKind::Directory, 0o755),
            "drwxr-xr-x"
        );
        assert_eq!(
            format_sftp_permissions(FsEntryKind::File, 0o644),
            "-rw-r--r--"
        );
        assert_eq!(
            format_sftp_permissions(FsEntryKind::File, 0o600),
            "-rw-------"
        );
        assert_eq!(
            format_sftp_permissions(FsEntryKind::Symlink, 0o777),
            "lrwxrwxrwx"
        );
        assert_eq!(format_sftp_permissions(FsEntryKind::Other, 0), "----------");
        assert_eq!(format_sftp_permissions_octal(0o600), "600");
        assert_eq!(format_sftp_permissions_octal(0o100644 & 0o777), "644");
    }

    #[test]
    fn modified_formatting_is_utc_and_handles_missing_timestamps() {
        assert_eq!(format_sftp_modified(None), "-");
        assert_eq!(
            format_sftp_modified(Some(SystemTime::UNIX_EPOCH)),
            "1970-01-01 00:00"
        );
        assert_eq!(
            format_sftp_modified(Some(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
            )),
            "2023-11-14 22:13"
        );
        assert_eq!(
            format_sftp_modified(Some(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_774_569_600)
            )),
            "2026-03-27 00:00"
        );
    }
}
