//! Local pane (N1 §2): directory state, navigation, multi-selection and the
//! Slint row projection.
//!
//! Listing/sorting/navigation come from `crate::local_fs` (read-only metadata,
//! symlinks as-is). This module only owns the session state: current directory,
//! sort/hidden options, the last listing, the inline error and the selection
//! set. Transfer jobs are built here and handed to `crate::sftp_jobs` by the
//! runtime.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

use crate::local_fs::{
    self, LocalFsError, LocalPaneListing, LocalPaneOptions, LocalRowData, LocalSortColumn,
};

use super::*;

/// Local pane session state (separate from the remote pane's path/sort/hidden).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalPaneState {
    pub dir: PathBuf,
    pub show_hidden: bool,
    pub sort_column: LocalSortColumn,
    pub sort_ascending: bool,
    /// Last successful listing; kept across errors so the pane is not blank.
    pub listing: Option<LocalPaneListing>,
    pub error: Option<LocalFsError>,
    /// Multi-select set (paths of real entries; the `..` row is never selected).
    pub selected: BTreeSet<PathBuf>,
    /// Shift-range anchor.
    pub anchor: Option<PathBuf>,
    pub collapsed: bool,
}

impl Default for LocalPaneState {
    fn default() -> Self {
        Self {
            dir: local_fs::default_local_dir(),
            show_hidden: false,
            sort_column: LocalSortColumn::Name,
            sort_ascending: true,
            listing: None,
            error: None,
            selected: BTreeSet::new(),
            anchor: None,
            collapsed: false,
        }
    }
}

impl LocalPaneState {
    pub(crate) fn options(&self) -> LocalPaneOptions {
        LocalPaneOptions {
            show_hidden: self.show_hidden,
            sort_column: self.sort_column,
            sort_ascending: self.sort_ascending,
        }
    }
}

impl AppRuntime {
    /// Persist the Local panel directory so the next launch restores it.
    fn remember_local_directory(&mut self) {
        let path = self.local_pane.dir.display().to_string();
        if self.config_document.ui.local_directory.as_deref() == Some(path.as_str()) {
            return;
        }
        self.config_document.ui.local_directory = Some(path);
        if let Err(error) = self.config_store.save(&self.config_document) {
            tracing::warn!(error = %error, "failed to persist local directory");
        }
    }

    /// Re-reads the current local directory. Errors are kept inline (the pane
    /// shows the previous listing plus a retry entry) instead of clearing it.
    pub fn refresh_local_pane(&mut self) -> AppProjection {
        let options = self.local_pane.options();
        match local_fs::list_pane(&self.local_pane.dir, options) {
            Ok(listing) => {
                self.local_pane.dir = listing.dir.clone();
                self.local_pane.error = None;
                self.prune_local_selection(&listing);
                self.remember_local_directory();
                self.set_status_kind(
                    "local-dir-loaded",
                    format!(
                        "Loaded local directory `{}` ({} items).",
                        listing.dir.display(),
                        listing.item_count
                    ),
                    listing.dir.display().to_string(),
                    listing.item_count.to_string(),
                );
                self.local_pane.listing = Some(listing);
            }
            Err(error) => {
                let detail = error.to_string();
                self.set_status_kind(
                    "local-dir-error",
                    format!("Could not load the local directory: {detail}"),
                    detail,
                    String::new(),
                );
                self.local_pane.error = Some(error);
            }
        }
        self.projection()
    }

    /// Retry entry of the inline error bar.
    pub fn retry_local_pane(&mut self) -> AppProjection {
        self.refresh_local_pane()
    }

    /// Opens a typed path (relative paths resolve against the current directory).
    pub fn open_local_path(&mut self, path: &str) -> AppProjection {
        match local_fs::normalize_local_dir(path, &self.local_pane.dir) {
            Ok(dir) => {
                if dir != self.local_pane.dir {
                    self.local_pane.selected.clear();
                    self.local_pane.anchor = None;
                }
                self.local_pane.dir = dir;
                self.refresh_local_pane()
            }
            Err(error) => {
                let detail = error.to_string();
                self.set_status_kind(
                    "local-open-error",
                    format!("Could not open the local path: {detail}"),
                    detail,
                    String::new(),
                );
                self.local_pane.error = Some(error);
                self.projection()
            }
        }
    }

    /// `..`: lexical parent, no symlink resolution.
    pub fn open_local_parent(&mut self) -> AppProjection {
        let Some(parent) = local_fs::parent_dir(&self.local_pane.dir) else {
            self.set_status_kind(
                "local-root",
                "Already at the local filesystem root.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        };
        let parent = parent.display().to_string();
        self.open_local_path(&parent)
    }

    /// Home shortcut in the local pane header (Downloads, then home).
    pub fn open_local_home(&mut self) -> AppProjection {
        let home = local_fs::default_local_dir().display().to_string();
        self.open_local_path(&home)
    }

    pub fn toggle_local_hidden(&mut self) -> AppProjection {
        self.local_pane.show_hidden = !self.local_pane.show_hidden;
        if self.local_pane.show_hidden {
            self.set_status_kind(
                "local-hidden-shown",
                "Local hidden files are now shown.".to_owned(),
                String::new(),
                String::new(),
            );
        } else {
            self.set_status_kind(
                "local-hidden-hidden",
                "Local hidden files are now hidden.".to_owned(),
                String::new(),
                String::new(),
            );
        }
        self.refresh_local_pane()
    }

    pub fn sort_local_by(&mut self, column: &str) -> AppProjection {
        let Some(column) = LocalSortColumn::from_label(column) else {
            let column_text = column.trim().to_owned();
            self.set_status_kind(
                "local-unknown-sort",
                format!("Unknown local sort column `{column_text}`."),
                column_text,
                String::new(),
            );
            return self.projection();
        };
        if self.local_pane.sort_column == column {
            self.local_pane.sort_ascending = !self.local_pane.sort_ascending;
        } else {
            self.local_pane.sort_column = column;
            self.local_pane.sort_ascending = true;
        }
        self.refresh_local_pane()
    }

    pub fn toggle_local_collapsed(&mut self) -> AppProjection {
        self.local_pane.collapsed = !self.local_pane.collapsed;
        self.projection()
    }

    /// Multi-select click: `ctrl` toggles, `shift` extends from the anchor.
    pub fn select_local_row(&mut self, index: i32, ctrl: bool, shift: bool) -> AppProjection {
        let Some(listing) = self.local_pane.listing.clone() else {
            return self.projection();
        };
        let Ok(row_index) = usize::try_from(index) else {
            return self.projection();
        };
        if listing.is_parent_row(row_index) {
            // The `..` row is navigation, not an operation target.
            if !ctrl && !shift {
                self.local_pane.selected.clear();
                self.local_pane.anchor = None;
            }
            return self.projection();
        }
        let Some(entry) = listing.entry_for_row(row_index) else {
            return self.projection();
        };
        let path = entry.path.clone();
        if shift {
            if let Some(anchor) = self.local_pane.anchor.clone() {
                let range = local_entry_range(&listing, &anchor, &path);
                self.local_pane.selected.extend(range);
            } else {
                self.local_pane.selected.insert(path.clone());
            }
        } else if ctrl {
            if !self.local_pane.selected.remove(&path) {
                self.local_pane.selected.insert(path.clone());
            }
            self.local_pane.anchor = Some(path);
        } else {
            self.local_pane.selected.clear();
            self.local_pane.selected.insert(path.clone());
            self.local_pane.anchor = Some(path);
        }
        self.projection()
    }

    /// Double click: `..` navigates up, a directory enters it, files only get a
    /// status hint (opening files is a system action, not an in-app one).
    pub fn activate_local_row(&mut self, index: i32) -> AppProjection {
        let Some(listing) = self.local_pane.listing.clone() else {
            return self.projection();
        };
        let Ok(row_index) = usize::try_from(index) else {
            return self.projection();
        };
        if listing.is_parent_row(row_index) {
            return self.open_local_parent();
        }
        let Some(entry) = listing.entry_for_row(row_index) else {
            return self.projection();
        };
        if entry.kind.is_dir() {
            return self.open_local_path(&entry.path.display().to_string());
        }
        let path_text = entry.path.display().to_string();
        self.set_status_kind(
            "local-file-selected",
            format!("Selected local file `{path_text}`. Drag it to the remote pane or use Upload."),
            path_text,
            String::new(),
        );
        self.projection()
    }

    /// Row model for the Slint list (selection flags applied).
    pub(crate) fn local_rows(&self) -> Vec<LocalRowData> {
        local_rows_for(&self.local_pane)
    }

    /// Breadcrumb segments of the local pane.
    ///
    /// The local and the remote pane share the same segmented path bar, so
    /// the local side produces the very same shape as the remote one.
    pub(crate) fn local_crumbs(&self) -> Vec<FileCrumbData> {
        local_crumbs_for(&self.local_pane.dir)
    }

    /// Deletes the local selection (files and directories, recursively).
    ///
    /// The local pane had no delete entry before; the action is exposed by the
    /// shared context menu and mirrors the remote delete semantics.
    pub fn delete_local_selection(&mut self) -> AppResult<AppProjection> {
        let paths = self.local_selected_paths();
        if paths.is_empty() {
            self.set_status_kind(
                "local-delete-requires-selection",
                "Select local entries before deleting them.".to_owned(),
                String::new(),
                String::new(),
            );
            return Ok(self.projection());
        }
        let mut removed = 0usize;
        let mut failures = Vec::new();
        for path in &paths {
            let result = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
            match result {
                Ok(()) => removed += 1,
                Err(error) => failures.push(format!("{}: {error}", path.display())),
            }
        }
        self.local_pane.selected.clear();
        self.local_pane.anchor = None;
        // 刷新列表（这一步会改写状态栏，所以放在设置删除状态之前）。
        let _ = self.refresh_local_pane();
        if failures.is_empty() {
            self.set_status_kind(
                "local-deleted",
                format!(
                    "Deleted {removed} local entr{}.",
                    if removed == 1 { "y" } else { "ies" }
                ),
                removed.to_string(),
                String::new(),
            );
        } else {
            self.set_status_kind(
                "local-delete-failed",
                format!(
                    "Deleted {removed} of {} local entr{}; {} failed.",
                    paths.len(),
                    if paths.len() == 1 { "y" } else { "ies" },
                    failures.join("; ")
                ),
                removed.to_string(),
                failures.join("; "),
            );
        }
        Ok(self.projection())
    }

    /// Selected real paths, in listing order.
    pub(crate) fn local_selected_paths(&self) -> Vec<PathBuf> {
        let Some(listing) = &self.local_pane.listing else {
            return Vec::new();
        };
        listing
            .entries
            .iter()
            .filter(|entry| self.local_pane.selected.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect()
    }

    pub(crate) fn local_selection_count(&self) -> usize {
        self.local_selected_paths().len()
    }

    /// Selected names for the summary row (up to three, comma separated).
    pub(crate) fn local_selected_name_text(&self) -> String {
        self.local_selected_paths()
            .iter()
            .filter_map(|path| path.file_name())
            .take(3)
            .map(|name| name.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(crate) fn local_error_text(&self) -> String {
        self.local_pane
            .error
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    pub(crate) fn local_error_retryable(&self) -> bool {
        self.local_pane
            .error
            .as_ref()
            .is_some_and(LocalFsError::retryable)
    }

    pub(crate) fn local_empty_text(&self) -> String {
        let Some(listing) = &self.local_pane.listing else {
            return String::new();
        };
        // `listing.rows` always contains the leading `..` marker; visible
        // entries are the real emptiness signal.
        if !listing.entries.is_empty() {
            return String::new();
        }
        if listing.hidden_count > 0 && !self.local_pane.show_hidden {
            return "No visible files. Enable Hidden to include dotfiles.".to_owned();
        }
        "This directory is empty.".to_owned()
    }

    pub(crate) fn local_summary_counts(&self) -> (usize, usize) {
        match &self.local_pane.listing {
            Some(listing) => (listing.item_count, listing.dir_count),
            None => (0, 0),
        }
    }

    fn prune_local_selection(&mut self, listing: &LocalPaneListing) {
        let existing: BTreeSet<&PathBuf> =
            listing.entries.iter().map(|entry| &entry.path).collect();
        self.local_pane
            .selected
            .retain(|path| existing.contains(path));
        if let Some(anchor) = &self.local_pane.anchor {
            if !existing.contains(anchor) {
                self.local_pane.anchor = None;
            }
        }
    }
}

/// Row model for any local pane state (used by the focused SFTP tab and by
/// split-pane snapshots alike).
pub(crate) fn local_rows_for(state: &LocalPaneState) -> Vec<LocalRowData> {
    let Some(listing) = &state.listing else {
        return Vec::new();
    };
    listing
        .rows
        .iter()
        .map(|row| {
            let mut row = row.clone();
            row.selected = !row.is_parent && state.selected.contains(Path::new(&row.path_text));
            row
        })
        .collect()
}

/// Breadcrumb segments of a local directory.
///
/// The segments mirror the remote ones: the first crumb is the filesystem
/// root (`/` or the drive), every following crumb is one path component.
pub(crate) fn local_crumbs_for(dir: &Path) -> Vec<FileCrumbData> {
    let mut crumbs = Vec::new();
    let mut current = PathBuf::new();
    for component in dir.components() {
        match component {
            Component::Prefix(prefix) => {
                // The drive letter is part of the root crumb, not its own.
                current.push(prefix.as_os_str());
            }
            Component::RootDir => {
                current.push(Component::RootDir.as_os_str());
                crumbs.push(FileCrumbData {
                    label: current.display().to_string(),
                    path: current.display().to_string(),
                });
            }
            Component::CurDir => {}
            Component::ParentDir => {
                current.push("..");
            }
            Component::Normal(name) => {
                current.push(name);
                crumbs.push(FileCrumbData {
                    label: name.to_string_lossy().into_owned(),
                    path: current.display().to_string(),
                });
            }
        }
    }
    if crumbs.is_empty() {
        crumbs.push(FileCrumbData {
            label: dir.display().to_string(),
            path: dir.display().to_string(),
        });
    }
    crumbs
}

/// Inclusive range between two visible entries (used by Shift selection).
fn local_entry_range(listing: &LocalPaneListing, from: &Path, to: &Path) -> Vec<PathBuf> {
    let position = |path: &Path| listing.entries.iter().position(|entry| entry.path == path);
    let (Some(from_index), Some(to_index)) = (position(from), position(to)) else {
        return vec![to.to_path_buf()];
    };
    let (start, end) = if from_index <= to_index {
        (from_index, to_index)
    } else {
        (to_index, from_index)
    };
    listing.entries[start..=end]
        .iter()
        .map(|entry| entry.path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    /// Runtime whose local pane points at `<temp>/pane` (config lives next to it
    /// so it never shows up in the listing).
    fn runtime_with_local(temp: &tempfile::TempDir) -> AppRuntime {
        let config = temp.path().join("yshell-config");
        fs::create_dir_all(&config).expect("config dir");
        let pane = temp.path().join("pane");
        fs::create_dir_all(&pane).expect("pane dir");
        let mut runtime = AppRuntime::new(config).expect("runtime");
        runtime.local_pane.dir = pane;
        runtime
    }

    /// Creates (and returns) the directory the tests list.
    fn pane_dir(temp: &tempfile::TempDir) -> std::path::PathBuf {
        let pane = temp.path().join("pane");
        fs::create_dir_all(&pane).expect("pane dir");
        pane
    }

    #[test]
    fn local_pane_lists_and_navigates_a_temp_directory() {
        let temp = tempdir().expect("tempdir");
        let pane = pane_dir(&temp);
        fs::create_dir(pane.join("sub")).expect("mkdir");
        fs::write(pane.join("a.txt"), b"a").expect("write");
        fs::write(pane.join(".hidden"), b"h").expect("write");

        let mut runtime = runtime_with_local(&temp);
        let projection = runtime.refresh_local_pane();
        assert!(projection.local_rows.len() >= 3, "rows plus parent row");
        assert_eq!(projection.local_item_count, 2);
        assert_eq!(projection.local_dir_count, 1);
        assert!(projection.local_error_text.is_empty());

        // Enter the subdirectory through the row model.
        let sub_row = projection
            .local_rows
            .iter()
            .position(|row| row.name == "sub")
            .expect("sub row");
        let entered = runtime.activate_local_row(sub_row as i32);
        assert_eq!(
            entered.local_path_text,
            pane.join("sub").display().to_string()
        );

        // Back to the parent through `..`.
        let back = runtime.activate_local_row(0);
        assert_eq!(back.local_path_text, pane.display().to_string());
    }

    #[test]
    fn local_pane_hidden_filter_and_empty_hint() {
        let temp = tempdir().expect("tempdir");
        let pane = pane_dir(&temp);
        fs::write(pane.join(".only-hidden"), b"h").expect("write");

        let mut runtime = runtime_with_local(&temp);
        let hidden_off = runtime.refresh_local_pane();
        assert_eq!(hidden_off.local_item_count, 0);
        assert!(hidden_off.local_empty_text.contains("Enable Hidden"));

        let shown = runtime.toggle_local_hidden();
        assert_eq!(shown.local_item_count, 1);
        assert!(shown.local_empty_text.is_empty());
    }

    #[test]
    fn local_pane_selection_supports_ctrl_and_shift() {
        let temp = tempdir().expect("tempdir");
        let pane = pane_dir(&temp);
        fs::write(pane.join("a.txt"), b"a").expect("write");
        fs::write(pane.join("b.txt"), b"b").expect("write");
        fs::write(pane.join("c.txt"), b"c").expect("write");

        let mut runtime = runtime_with_local(&temp);
        runtime.refresh_local_pane();
        // Rows: 0 = `..`, 1 = a.txt, 2 = b.txt, 3 = c.txt.
        let first = runtime.select_local_row(1, false, false);
        assert_eq!(first.local_selected_count, 1);
        let second = runtime.select_local_row(2, true, false);
        assert_eq!(second.local_selected_count, 2);
        let range = runtime.select_local_row(3, false, true);
        assert_eq!(
            range.local_selected_count, 3,
            "shift extends from the anchor"
        );
        assert_eq!(
            runtime
                .local_selected_paths()
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["a.txt", "b.txt", "c.txt"]
        );

        // Selecting `..` clears the selection instead of adding a target.
        let cleared = runtime.select_local_row(0, false, false);
        assert_eq!(cleared.local_selected_count, 0);
    }

    #[test]
    fn local_pane_errors_are_inline_and_retryable() {
        let temp = tempdir().expect("tempdir");
        let pane = pane_dir(&temp);
        let mut runtime = runtime_with_local(&temp);
        runtime.local_pane.dir = pane.join("missing");
        let projection = runtime.refresh_local_pane();
        assert!(projection.local_error_text.contains("missing"));
        assert_eq!(projection.local_error_kind_text, "not-found");
        assert!(projection.local_error_retryable);

        // Retry after creating the directory succeeds without clearing state.
        fs::create_dir(pane.join("missing")).expect("mkdir");
        let retried = runtime.retry_local_pane();
        assert!(retried.local_error_text.is_empty());
    }

    #[test]
    fn local_pane_sorting_toggles_direction() {
        let temp = tempdir().expect("tempdir");
        let pane = pane_dir(&temp);
        fs::write(pane.join("b.txt"), b"b").expect("write");
        fs::write(pane.join("a.txt"), b"a").expect("write");

        let mut runtime = runtime_with_local(&temp);
        runtime.refresh_local_pane();
        // Default sort is Name ascending; clicking the same column flips it.
        let desc = runtime.sort_local_by("name");
        assert!(!desc.local_sort_ascending);
        assert_eq!(
            desc.local_rows
                .iter()
                .filter(|row| !row.is_parent)
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b.txt", "a.txt"]
        );
        let asc = runtime.sort_local_by("name");
        assert!(asc.local_sort_ascending);
        assert_eq!(
            asc.local_rows
                .iter()
                .filter(|row| !row.is_parent)
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a.txt", "b.txt"]
        );
    }

    #[test]
    fn local_pane_exposes_breadcrumb_segments() {
        let temp = tempdir().expect("tempdir");
        let pane = pane_dir(&temp);
        let mut runtime = runtime_with_local(&temp);
        runtime.refresh_local_pane();

        // The crumbs mirror the remote ones: a root crumb plus one per segment.
        let crumbs = runtime.local_crumbs();
        assert!(!crumbs.is_empty());
        assert_eq!(crumbs.last().map(|crumb| crumb.path.clone()), Some(
            pane.display().to_string()
        ));
        assert!(crumbs
            .iter()
            .all(|crumb| !crumb.label.is_empty() && !crumb.path.is_empty()));

        // The separator follows the platform: `\` on Windows, `/` elsewhere.
        let separator = crate::runtime::local_path_separator();
        assert_eq!(separator, if cfg!(windows) { "\\" } else { "/" });
        if cfg!(windows) {
            assert!(crumbs
                .iter()
                .any(|crumb| crumb.path.contains('\\')));
        }
    }

    #[test]
    fn local_pane_deletes_the_selection() {
        let temp = tempdir().expect("tempdir");
        let pane = pane_dir(&temp);
        fs::write(pane.join("a.txt"), b"a").expect("write");
        fs::create_dir(pane.join("dir")).expect("mkdir");
        fs::write(pane.join("dir/b.txt"), b"b").expect("write");

        let mut runtime = runtime_with_local(&temp);
        runtime.refresh_local_pane();
        // Rows: 0 = `..`, 1 = a.txt, 2 = dir.
        runtime.select_local_row(1, false, false);
        runtime.select_local_row(2, true, false);
        assert_eq!(runtime.local_selection_count(), 2);

        let deleted = runtime
            .delete_local_selection()
            .expect("delete local selection");
        assert!(!pane.join("a.txt").exists());
        assert!(!pane.join("dir").exists());
        assert_eq!(deleted.local_selected_count, 0);
        assert!(deleted.status_text.contains("Deleted 2 local"));
        assert_eq!(deleted.local_item_count, 0);
    }

    #[test]
    fn local_pane_delete_without_selection_is_rejected() {
        let temp = tempdir().expect("tempdir");
        let _pane = pane_dir(&temp);
        let mut runtime = runtime_with_local(&temp);
        runtime.refresh_local_pane();

        let rejected = runtime
            .delete_local_selection()
            .expect("delete without selection");
        assert!(rejected.status_text.contains("Select local entries"));
    }
}
