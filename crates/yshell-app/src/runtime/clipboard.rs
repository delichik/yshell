//! In-app file clipboard (N1 §2): Ctrl+C / Ctrl+V between the local and the
//! remote pane. Not connected to the OS clipboard (the design explicitly does
//! not promise that).
//!
//! Paste always targets the *current* directory of the opposite pane:
//!
//! * local → remote uploads into the current remote directory,
//! * remote → local downloads into the current local directory,
//! * same side copies/moves in place (remote moves use the F0 rename path,
//!   local moves use `std::fs`).

use std::{
    fs,
    path::{Path, PathBuf},
};

use yshell_sftp::OverwritePolicy;

use crate::local_fs;

use super::*;

/// Which pane a clipboard payload came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardSide {
    Local,
    Remote,
}

/// One in-app copy/cut payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileClipboard {
    pub side: ClipboardSide,
    pub paths: Vec<PathBuf>,
    /// Cut (Ctrl+X): paste moves instead of copies.
    pub cut: bool,
}

/// Post-transfer cleanup for a "move" gesture (Ctrl-drag / cut+paste):
/// after the copy job succeeds, the source is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveCleanup {
    /// Local source paths to delete after a successful upload.
    LocalPaths(Vec<PathBuf>),
    /// Remote source entries `(path, is_dir)` to delete after a successful
    /// download.
    RemoteEntries(Vec<(String, bool)>),
}

impl AppRuntime {
    /// Ctrl+C on the focused remote list.
    pub fn copy_remote_selection_to_clipboard(&mut self) -> AppProjection {
        let paths = self
            .sftp_selected_paths()
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if paths.is_empty() {
            self.status_text = "Select remote entries before copying.".to_owned();
            return self.projection();
        }
        let count = paths.len();
        self.file_clipboard = Some(FileClipboard {
            side: ClipboardSide::Remote,
            paths,
            cut: false,
        });
        self.status_text = format!("Copied {count} remote entr{}.", plural_ies(count));
        self.projection()
    }

    /// Ctrl+X on the focused remote list.
    pub fn cut_remote_selection_to_clipboard(&mut self) -> AppProjection {
        let projection = self.copy_remote_selection_to_clipboard();
        if let Some(clipboard) = self.file_clipboard.as_mut() {
            clipboard.cut = true;
            let count = clipboard.paths.len();
            self.status_text = format!("Cut {count} remote entr{}.", plural_ies(count));
        }
        projection
    }

    /// Ctrl+C on the focused local list.
    pub fn copy_local_selection_to_clipboard(&mut self) -> AppProjection {
        let paths = self.local_selected_paths();
        if paths.is_empty() {
            self.status_text = "Select local entries before copying.".to_owned();
            return self.projection();
        }
        let count = paths.len();
        self.file_clipboard = Some(FileClipboard {
            side: ClipboardSide::Local,
            paths,
            cut: false,
        });
        self.status_text = format!("Copied {count} local entr{}.", plural_ies(count));
        self.projection()
    }

    /// Ctrl+X on the focused local list.
    pub fn cut_local_selection_to_clipboard(&mut self) -> AppProjection {
        let projection = self.copy_local_selection_to_clipboard();
        if let Some(clipboard) = self.file_clipboard.as_mut() {
            clipboard.cut = true;
            let count = clipboard.paths.len();
            self.status_text = format!("Cut {count} local entr{}.", plural_ies(count));
        }
        projection
    }

    /// Ctrl+C heuristic: copy whatever pane has a selection (remote wins when
    /// both do).
    pub fn copy_selection_to_clipboard(&mut self) -> AppProjection {
        if !self.sftp_selected_paths().is_empty() {
            self.copy_remote_selection_to_clipboard()
        } else {
            self.copy_local_selection_to_clipboard()
        }
    }

    /// Ctrl+X heuristic (same pane choice as [`Self::copy_selection_to_clipboard`]).
    pub fn cut_selection_to_clipboard(&mut self) -> AppProjection {
        if !self.sftp_selected_paths().is_empty() {
            self.cut_remote_selection_to_clipboard()
        } else {
            self.cut_local_selection_to_clipboard()
        }
    }

    /// Ctrl+V: paste across panes (local → remote, remote → local).
    pub fn paste_cross_pane(&mut self) -> AppProjection {
        match self.file_clipboard.as_ref().map(|clipboard| clipboard.side) {
            Some(ClipboardSide::Local) => self.paste_file_clipboard_into_remote(),
            Some(ClipboardSide::Remote) => self.paste_file_clipboard_into_local(),
            None => {
                self.status_text = "The file clipboard is empty.".to_owned();
                self.projection()
            }
        }
    }

    /// Ctrl+V: paste the clipboard into the current directory of the target pane.
    pub fn paste_file_clipboard_into_remote(&mut self) -> AppProjection {
        let Some(clipboard) = self.file_clipboard.clone() else {
            self.status_text = "The file clipboard is empty.".to_owned();
            return self.projection();
        };
        match clipboard.side {
            ClipboardSide::Local => {
                let paths = clipboard.paths.clone();
                let ids = match self.submit_upload_paths(&paths, None, OverwritePolicy::Ask) {
                    Ok(ids) => ids,
                    Err(error) => {
                        self.status_text = error.to_string();
                        return self.projection();
                    }
                };
                if clipboard.cut {
                    for transfer_id in ids {
                        self.pending_move_cleanup
                            .insert(transfer_id, MoveCleanup::LocalPaths(paths.clone()));
                    }
                    self.file_clipboard = None;
                }
                self.projection()
            }
            ClipboardSide::Remote => {
                // Same-pane paste: copy into the current remote directory.
                let entries = self.clipboard_remote_entries(&clipboard.paths);
                let destination = self.sftp_path.clone();
                if let Err(error) =
                    self.submit_remote_copy_entries(&entries, &destination, OverwritePolicy::Ask)
                {
                    self.status_text = error.to_string();
                    return self.projection();
                }
                if clipboard.cut {
                    self.file_clipboard = None;
                }
                self.projection()
            }
        }
    }

    /// Ctrl+V into the local pane.
    pub fn paste_file_clipboard_into_local(&mut self) -> AppProjection {
        let Some(clipboard) = self.file_clipboard.clone() else {
            self.status_text = "The file clipboard is empty.".to_owned();
            return self.projection();
        };
        match clipboard.side {
            ClipboardSide::Remote => {
                let entries = self.clipboard_remote_entries(&clipboard.paths);
                if entries.is_empty() {
                    self.status_text = "The copied remote entries are no longer listed.".to_owned();
                    return self.projection();
                }
                let destination = self.local_pane.dir.clone();
                let ids = match self.submit_download_entries(
                    &entries,
                    &destination,
                    OverwritePolicy::Ask,
                ) {
                    Ok(ids) => ids,
                    Err(error) => {
                        self.status_text = error.to_string();
                        return self.projection();
                    }
                };
                if clipboard.cut {
                    for transfer_id in ids {
                        self.pending_move_cleanup
                            .insert(transfer_id, MoveCleanup::RemoteEntries(entries.clone()));
                    }
                    self.file_clipboard = None;
                }
                self.projection()
            }
            ClipboardSide::Local => {
                // Same-pane paste: copy/move inside the local filesystem.
                self.paste_local_paths_into_local(&clipboard);
                self.projection()
            }
        }
    }

    /// Resolves the `(path, is_dir)` pairs for clipboard payloads that came from
    /// the remote pane. Entries no longer listed are treated as files.
    pub(crate) fn clipboard_remote_entries(&self, paths: &[PathBuf]) -> Vec<(String, bool)> {
        paths
            .iter()
            .map(|path| {
                let text = path.display().to_string();
                let is_dir = self
                    .sftp_entries
                    .iter()
                    .find(|entry| entry.path == text)
                    .is_some_and(|entry| matches!(entry.kind, yshell_sftp::FsEntryKind::Directory));
                (text, is_dir)
            })
            .collect()
    }

    fn paste_local_paths_into_local(&mut self, clipboard: &FileClipboard) {
        let destination_dir = self.local_pane.dir.clone();
        let mut copied = 0usize;
        let mut failures = Vec::new();
        for path in &clipboard.paths {
            let Some(name) = path.file_name() else {
                continue;
            };
            let desired = destination_dir.join(name);
            if desired == *path {
                continue;
            }
            let result = if clipboard.cut {
                fs::rename(path, &desired)
            } else {
                copy_local_entry(path, &desired)
            };
            match result {
                Ok(()) => copied += 1,
                Err(error) => failures.push(format!("{}: {error}", path.display())),
            }
        }
        self.refresh_local_pane();
        if clipboard.cut {
            self.file_clipboard = None;
        }
        self.status_text = if failures.is_empty() {
            format!("Pasted {copied} local entr{}.", plural_ies(copied))
        } else {
            format!(
                "Pasted {copied} local entr{}; {} failed: {}",
                plural_ies(copied),
                failures.len(),
                failures.join("; ")
            )
        };
    }
}

/// Recursive local copy used by same-pane paste. Never clobbers: an existing
/// destination gets a ` (n)` sibling name.
pub(crate) fn copy_local_entry(from: &Path, to: &Path) -> std::io::Result<()> {
    let target = if to.exists()
        || fs::symlink_metadata(to)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    {
        unique_local_path(to)
    } else {
        to.to_path_buf()
    };
    let metadata = fs::symlink_metadata(from)?;
    if metadata.is_symlink() {
        // Copy the link itself (read-only semantics: never follow).
        #[cfg(unix)]
        {
            let link = fs::read_link(from)?;
            std::os::unix::fs::symlink(link, &target)?;
            return Ok(());
        }
        #[cfg(not(unix))]
        {
            fs::copy(from, &target)?;
            return Ok(());
        }
    }
    if metadata.is_dir() {
        fs::create_dir_all(&target)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_local_entry(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }
    fs::copy(from, &target)?;
    Ok(())
}

/// `name.txt` → `name (1).txt`, used when the paste destination exists.
pub(crate) fn unique_local_path(path: &Path) -> PathBuf {
    let Some(name) = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return path.to_path_buf();
    };
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    for index in 1..10_000 {
        let candidate = parent.join(local_fs::numbered_name(&name, index));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    path.to_path_buf()
}

fn plural_ies(count: usize) -> &'static str {
    if count == 1 {
        "y"
    } else {
        "ies"
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{copy_local_entry, unique_local_path};

    #[test]
    fn local_copy_renames_existing_destinations_and_recurses() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("tree");
        fs::create_dir_all(source.join("nested")).expect("mkdir");
        fs::write(source.join("a.txt"), b"a").expect("write");
        fs::write(source.join("nested/b.txt"), b"b").expect("write");

        let destination = temp.path().join("tree-copy");
        copy_local_entry(&source, &destination).expect("copy");
        assert_eq!(fs::read(destination.join("a.txt")).expect("read"), b"a");
        assert_eq!(
            fs::read(destination.join("nested/b.txt")).expect("read"),
            b"b"
        );

        // A second copy lands in a numbered sibling instead of clobbering.
        copy_local_entry(&source, &destination).expect("copy again");
        assert!(temp.path().join("tree-copy (1)").is_dir());
    }

    #[test]
    fn unique_local_path_numbers_extensions() {
        let temp = tempdir().expect("tempdir");
        let file = temp.path().join("report.txt");
        fs::write(&file, b"x").expect("write");
        assert_eq!(unique_local_path(&file), temp.path().join("report (1).txt"));
    }
}
