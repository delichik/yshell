//! SFTP browsing: listing state, selection, sorting, breadcrumbs and remote-edit
//! lifecycle tracking.

use crate::{
    error::AppError, error::AppResult, sftp_view::format_sftp_modified,
    sftp_view::format_sftp_permissions, sftp_view::format_sftp_permissions_octal,
    sftp_view::format_sftp_size, sftp_view::sftp_kind_text, sftp_view::visible_entries,
    sftp_view::SftpSortColumn,
};
use yshell_sftp::{DirectoryListing, FsEntry, FsEntryKind, SftpClient};
use yshell_ssh::TransportBackend;

use super::*;

impl AppRuntime {
    pub fn refresh_active_sftp_listing(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.sftp_listing = SftpListingState::RefreshNeedsSession;
            self.status_text = self.sftp_session.legacy_status_text();
            return Ok(self.projection());
        };
        let status_text = self.sync_sftp_lifecycle_for_session(&session_key, true);
        self.record_sftp_refresh_status(status_text);
        Ok(self.projection())
    }

    pub(crate) fn record_sftp_refresh_status(&mut self, text: String) {
        let ready = matches!(self.sftp_session, SftpSessionLifecycle::Ready { .. });
        let failed = matches!(self.sftp_session, SftpSessionLifecycle::Failed { .. });
        if ready {
            let path = self.sftp_path.clone();
            let count = self.sftp_entries.len().to_string();
            self.set_status_kind("sftp-ready", text, path, count);
        } else if failed {
            let session_key = self.sftp_session.status_session_key().to_owned();
            let reason = self.sftp_session.status_detail().to_owned();
            self.set_status_kind("sftp-failed", text, session_key, reason);
        } else {
            self.status_text = text;
        }
    }

    pub fn open_sftp_path(&mut self, path: &str) -> AppResult<AppProjection> {
        let path = normalize_remote_path(path)?;
        self.sftp_path = path;
        self.refresh_active_sftp_listing()
    }

    pub fn open_sftp_parent(&mut self) -> AppResult<AppProjection> {
        self.sftp_path = parent_remote_path(&self.sftp_path);
        self.refresh_active_sftp_listing()
    }

    pub fn select_sftp_entry(&mut self, index: i32) -> AppProjection {
        self.select_sftp_row(index, false, false)
    }

    /// Multi-select click on the remote list: `ctrl` toggles, `shift` extends
    /// the range from the anchor (both mirror the local pane's semantics).
    pub fn select_sftp_row(&mut self, index: i32, ctrl: bool, shift: bool) -> AppProjection {
        // A negative index is the "clear selection" signal used by the context
        // menu's blank-area path.
        if index < 0 {
            self.sftp_selected_index = None;
            self.sftp_selection.clear();
            self.sftp_selection_anchor = None;
            return self.projection();
        }
        let visible = self.visible_sftp_entries();
        let Ok(row_index) = usize::try_from(index) else {
            return self.projection();
        };
        let Some(entry) = visible.get(row_index).cloned() else {
            return self.projection();
        };
        self.sftp_selected_index = Some(row_index);
        let path = entry.path.clone();
        if shift {
            if let Some(anchor) = self.sftp_selection_anchor.clone() {
                let range = remote_entry_range(&visible, &anchor, &path);
                self.sftp_selection = range.into_iter().collect();
            } else {
                self.sftp_selection.clear();
                self.sftp_selection.insert(path.clone());
            }
        } else if ctrl {
            if !self.sftp_selection.remove(&path) {
                self.sftp_selection.insert(path.clone());
            }
            self.sftp_selection_anchor = Some(path);
        } else {
            self.sftp_selection.clear();
            self.sftp_selection.insert(path.clone());
            self.sftp_selection_anchor = Some(path);
        }
        self.projection()
    }

    /// Selected remote paths, in listing order (stable for batch operations).
    pub(crate) fn sftp_selected_paths(&self) -> Vec<String> {
        self.sftp_entries
            .iter()
            .filter(|entry| self.sftp_selection.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// Selected remote entries (kind-aware, for batch jobs).
    pub(crate) fn selected_sftp_entries(&self) -> Vec<FsEntry> {
        self.sftp_entries
            .iter()
            .filter(|entry| self.sftp_selection.contains(&entry.path))
            .cloned()
            .collect()
    }

    pub(crate) fn sftp_selection_count(&self) -> usize {
        self.sftp_selected_paths().len()
    }

    /// True when the selection is a single regular file (single-file actions).
    pub(crate) fn sftp_selection_is_single_file(&self) -> bool {
        let entries = self.selected_sftp_entries();
        entries.len() == 1 && !matches!(entries[0].kind, FsEntryKind::Directory)
    }

    /// Prunes the multi-selection after a listing refresh.
    pub(crate) fn prune_sftp_selection(&mut self) {
        let existing = self
            .sftp_entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<BTreeSet<_>>();
        self.sftp_selection.retain(|path| existing.contains(path));
        if let Some(anchor) = &self.sftp_selection_anchor {
            if !existing.contains(anchor) {
                self.sftp_selection_anchor = None;
            }
        }
    }

    /// The transfer drawer's disclosure state.
    pub fn toggle_transfer_drawer(&mut self) -> AppProjection {
        self.transfer_drawer_expanded = !self.transfer_drawer_expanded;
        self.projection()
    }

    /// Opens the Properties dialog for the current remote selection.
    pub fn open_sftp_properties(&mut self) -> AppProjection {
        if self.selected_sftp_entry().is_none() {
            self.status_text = "Select an SFTP entry before opening its properties.".to_owned();
            return self.projection();
        }
        self.sftp_properties_open = true;
        self.projection()
    }

    pub fn close_sftp_properties(&mut self) -> AppProjection {
        self.sftp_properties_open = false;
        self.projection()
    }

    /// The remote context menu's target kind: `file`/`dir`/`blank`/`multi`.
    pub(crate) fn sftp_menu_target_kind(&self) -> &'static str {
        let entries = self.selected_sftp_entries();
        if entries.is_empty() {
            "blank"
        } else if entries.len() > 1 {
            "multi"
        } else if matches!(entries[0].kind, FsEntryKind::Directory) {
            "dir"
        } else {
            "file"
        }
    }

    pub fn activate_sftp_entry(&mut self) -> AppResult<AppProjection> {
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP entry before opening it.".to_owned();
            return Ok(self.projection());
        };
        if matches!(entry.kind, FsEntryKind::Directory) {
            return self.open_sftp_path(&entry.path);
        }
        self.edit_sftp_selected()
    }

    pub fn sort_sftp_by(&mut self, column: &str) -> AppProjection {
        let Some(column) = SftpSortColumn::from_label(column) else {
            self.status_text = format!("Unknown SFTP sort column `{}`.", column.trim());
            return self.projection();
        };
        let selected = self.selected_sftp_entry().map(|entry| entry.path);
        if self.sftp_sort_column == column {
            self.sftp_sort_ascending = !self.sftp_sort_ascending;
        } else {
            self.sftp_sort_column = column;
            self.sftp_sort_ascending = true;
        }
        self.reconcile_sftp_selection(selected.as_deref());
        let sort_text = self.sftp_sort_legacy_text();
        let column_id = self.sftp_sort_column.label().to_ascii_lowercase();
        let direction = if self.sftp_sort_ascending {
            "ascending"
        } else {
            "descending"
        };
        self.set_status_kind(
            "sftp-sorted",
            format!("Sorted SFTP entries by {sort_text}."),
            column_id,
            direction.to_owned(),
        );
        self.projection()
    }

    pub fn toggle_sftp_hidden_files(&mut self) -> AppProjection {
        let selected = self.selected_sftp_entry().map(|entry| entry.path);
        self.sftp_show_hidden = !self.sftp_show_hidden;
        self.reconcile_sftp_selection(selected.as_deref());
        self.status_text = if self.sftp_show_hidden {
            "SFTP hidden files are now shown.".to_owned()
        } else {
            "SFTP hidden files are now hidden.".to_owned()
        };
        self.projection()
    }

    pub fn open_sftp_crumb(&mut self, index: i32) -> AppResult<AppProjection> {
        let crumbs = self.sftp_crumbs();
        let Some(crumb) = usize::try_from(index)
            .ok()
            .and_then(|index| crumbs.get(index))
        else {
            self.status_text = "That SFTP breadcrumb is no longer available.".to_owned();
            return Ok(self.projection());
        };
        let path = crumb.path.clone();
        self.open_sftp_path(&path)
    }

    pub(crate) fn visible_sftp_entries(&self) -> Vec<FsEntry> {
        visible_entries(
            &self.sftp_entries,
            self.sftp_show_hidden,
            self.sftp_sort_column,
            self.sftp_sort_ascending,
        )
    }

    pub(crate) fn selected_sftp_entry(&self) -> Option<FsEntry> {
        let index = self.sftp_selected_index?;
        self.visible_sftp_entries().into_iter().nth(index)
    }

    /// Visible entry at a row index (drop targets resolve rows through this).
    pub(crate) fn sftp_visible_entry_at(&self, index: i32) -> Option<FsEntry> {
        let index = usize::try_from(index).ok()?;
        self.visible_sftp_entries().into_iter().nth(index)
    }

    pub(crate) fn reconcile_sftp_selection(&mut self, preferred_path: Option<&str>) {
        let visible = self.visible_sftp_entries();
        self.sftp_selected_index =
            preferred_path.and_then(|path| visible.iter().position(|entry| entry.path == path));
    }

    pub(crate) fn reselect_sftp_path(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        if let Some(index) = self
            .visible_sftp_entries()
            .iter()
            .position(|entry| entry.path == path)
        {
            self.sftp_selected_index = Some(index);
        }
    }

    pub(crate) fn sftp_relative_or_absolute_path(&self, target: &str) -> String {
        if target.starts_with('/') {
            normalize_remote_path(target).unwrap_or_else(|_| target.to_owned())
        } else {
            join_remote_path(&self.sftp_path, target)
        }
    }

    pub(crate) fn sftp_rows(&self) -> Vec<SftpRowData> {
        self.visible_sftp_entries()
            .iter()
            .map(|entry| {
                let is_dir = matches!(entry.kind, FsEntryKind::Directory);
                SftpRowData {
                    name: entry.name.clone(),
                    path_text: entry.path.clone(),
                    kind_text: sftp_kind_text(entry.kind).to_owned(),
                    size_text: if is_dir {
                        String::new()
                    } else {
                        format_sftp_size(entry.size_bytes)
                    },
                    modified_text: format_sftp_modified(entry.modified),
                    permissions_text: format_sftp_permissions(entry.kind, entry.permissions),
                    is_dir,
                    is_symlink: matches!(entry.kind, FsEntryKind::Symlink),
                    selected: self.sftp_selection.contains(&entry.path),
                }
            })
            .collect()
    }

    pub(crate) fn sftp_crumbs(&self) -> Vec<SftpCrumbData> {
        let mut crumbs = vec![SftpCrumbData {
            label: "/".to_owned(),
            path: "/".to_owned(),
        }];
        let normalized = normalize_remote_path(&self.sftp_path).unwrap_or_else(|_| "/".to_owned());
        let mut path = String::new();
        for part in normalized.split('/').filter(|part| !part.is_empty()) {
            path.push('/');
            path.push_str(part);
            crumbs.push(SftpCrumbData {
                label: part.to_owned(),
                path: path.clone(),
            });
        }
        crumbs
    }

    pub(crate) fn sftp_sort_legacy_text(&self) -> String {
        let arrow = if self.sftp_sort_ascending {
            "↑"
        } else {
            "↓"
        };
        format!("{} {arrow}", self.sftp_sort_column.label())
    }

    pub(crate) fn sftp_item_summary_text(&self) -> String {
        let (items, directories) = self.sftp_visible_counts();
        format!(
            "{items} {} · {directories} {}",
            if items == 1 { "item" } else { "items" },
            if directories == 1 { "dir" } else { "dirs" }
        )
    }

    pub(crate) fn sftp_visible_counts(&self) -> (usize, usize) {
        let visible = self.visible_sftp_entries();
        let directories = visible
            .iter()
            .filter(|entry| matches!(entry.kind, FsEntryKind::Directory))
            .count();
        (visible.len(), directories)
    }

    pub(crate) fn sftp_empty_text(&self) -> String {
        if !self.visible_sftp_entries().is_empty() {
            return String::new();
        }
        if self.sftp_entries.is_empty() {
            // Slint falls back to the translated listing state for this case.
            return String::new();
        }
        if !self.sftp_show_hidden {
            return "No visible files. Enable Hidden to include dotfiles.".to_owned();
        }
        "This directory is empty.".to_owned()
    }

    pub(crate) fn sftp_selected_name_text(&self) -> String {
        self.selected_sftp_entry()
            .map(|entry| entry.name)
            .unwrap_or_default()
    }

    pub(crate) fn sftp_selected_path_text(&self) -> String {
        self.selected_sftp_entry()
            .map(|entry| entry.path)
            .unwrap_or_default()
    }

    pub(crate) fn sftp_selected_permissions_text(&self) -> String {
        self.selected_sftp_entry()
            .map(|entry| format_sftp_permissions_octal(entry.permissions))
            .unwrap_or_default()
    }

    pub(crate) fn apply_sftp_listing(&mut self, listing: &DirectoryListing) {
        self.sftp_path = listing.path.clone();
        let selected = self.selected_sftp_entry().map(|entry| entry.path);
        self.sftp_entries = listing.entries.clone();
        self.prune_sftp_selection();
        self.reconcile_sftp_selection(selected.as_deref());
        if listing.entries.is_empty() {
            self.sftp_listing = SftpListingState::EmptyDirectory;
            return;
        }
        if self.sftp_remote_target.trim().is_empty() {
            if let Some(first_file) = listing
                .entries
                .iter()
                .find(|entry| matches!(entry.kind, FsEntryKind::File))
            {
                self.sftp_remote_target = first_file.path.clone();
            }
        }
        self.sftp_listing = SftpListingState::Rows(
            listing
                .entries
                .iter()
                .map(|entry| {
                    let prefix = match entry.kind {
                        FsEntryKind::Directory => "[dir]",
                        FsEntryKind::File => "[file]",
                        FsEntryKind::Symlink => "[link]",
                        FsEntryKind::Other => "[other]",
                    };
                    if matches!(entry.kind, FsEntryKind::File) {
                        format!("{prefix} {} ({} B)", entry.name, entry.size_bytes)
                    } else {
                        format!("{prefix} {}", entry.name)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    pub(crate) fn prepare_active_sftp_operation(
        &mut self,
        action: &str,
    ) -> AppResult<Option<String>> {
        let Some(session_key) = self.active_session_id.clone() else {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.status_text = format!(
                "{} Cannot continue {action} until a runtime session is connected.",
                self.sftp_session.legacy_status_text()
            );
            return Ok(None);
        };
        let message = self.sync_sftp_lifecycle_for_session(&session_key, false);
        if self.sftp_ready_session_key().is_some() {
            Ok(Some(session_key))
        } else {
            self.status_text = format!("{message} Cannot continue {action}.");
            Ok(None)
        }
    }

    pub(crate) fn sync_sftp_lifecycle_for_session(
        &mut self,
        session_key: &str,
        refresh_listing: bool,
    ) -> String {
        if self.transport_backend != TransportBackend::Real {
            self.sftp_session = SftpSessionLifecycle::Unavailable {
                reason: SftpUnavailableReason::SelectNativeSsh,
            };
            self.sftp_listing = SftpListingState::SyncFakeBackend;
            return self.sftp_session.legacy_status_text();
        }

        let Some((session_state, session_backend, ssh_config)) =
            self.sessions.get(session_key).map(|runtime| {
                (
                    runtime.state,
                    runtime.transport_backend,
                    runtime.ssh_config.clone(),
                )
            })
        else {
            self.sftp_session = SftpSessionLifecycle::Failed {
                session_key: Some(session_key.to_owned()),
                reason: "runtime session is missing".to_owned(),
            };
            self.sftp_listing = SftpListingState::SyncMissingSession;
            return self.sftp_session.legacy_status_text();
        };

        if session_state != yshell_core::SessionState::Connected {
            self.sftp_session = SftpSessionLifecycle::Disconnected {
                session_key: Some(session_key.to_owned()),
            };
            self.sftp_listing = SftpListingState::SyncDisconnected;
            return self.sftp_session.legacy_status_text();
        }

        if session_backend != TransportBackend::Real {
            self.sftp_session = SftpSessionLifecycle::Disconnected {
                session_key: Some(session_key.to_owned()),
            };
            self.sftp_listing = SftpListingState::SyncReconnectNative;
            return self.sftp_session.legacy_status_text();
        }

        self.sftp_session = SftpSessionLifecycle::Ready {
            session_key: session_key.to_owned(),
        };
        if !refresh_listing {
            return self.sftp_session.legacy_status_text();
        }

        let ssh_config = self.effective_ssh_config(&ssh_config);
        let client = SftpClient::with_real_backend(ssh_config);
        match client
            .list_dir(&self.sftp_path)
            .map_err(AppError::from_error)
        {
            Ok(listing) => {
                let entry_count = listing.entries.len();
                let listing_path = listing.path.clone();
                self.apply_sftp_listing(&listing);
                format!(
                    "SFTP session ready. Loaded {entry_count} SFTP entr{} from `{listing_path}`.",
                    if entry_count == 1 { "y" } else { "ies" }
                )
            }
            Err(error) => {
                self.sftp_session = SftpSessionLifecycle::Failed {
                    session_key: Some(session_key.to_owned()),
                    reason: error.to_string(),
                };
                self.sftp_listing = SftpListingState::SyncFailed {
                    session_key: session_key.to_owned(),
                    path: self.sftp_path.clone(),
                    error: error.to_string(),
                };
                self.sftp_session.legacy_status_text()
            }
        }
    }

    /// D16：清空 SFTP 面板视图状态（切换会话/切到无会话标签时调用）。
    ///
    /// 只清"上一个会话的目录"这类残留；生命周期由调用方随后用
    /// [`Self::sync_sftp_lifecycle_for_session`] 重建。
    pub(crate) fn reset_sftp_view_state(&mut self) {
        self.sftp_entries.clear();
        self.sftp_selection.clear();
        self.sftp_selection_anchor = None;
        self.sftp_selected_index = None;
        self.sftp_path = "/".to_owned();
        self.sftp_listing = SftpListingState::ConnectReal;
    }

    pub(crate) fn sftp_ready_session_key(&self) -> Option<&str> {
        match &self.sftp_session {
            SftpSessionLifecycle::Ready { session_key } => Some(session_key.as_str()),
            SftpSessionLifecycle::Unavailable { .. }
            | SftpSessionLifecycle::Disconnected { .. }
            | SftpSessionLifecycle::Failed { .. } => None,
        }
    }

    pub(crate) fn sftp_remote_edit_parts(&self) -> (String, String) {
        match &self.remote_edit_session {
            Some(session) => (session.remote_path.clone(), session.local_temp_path.clone()),
            None => (String::new(), String::new()),
        }
    }
}

/// One visible SFTP file-list row, ready for the Slint model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpRowData {
    pub name: String,
    /// Absolute remote path (drag payloads / path actions).
    pub path_text: String,
    pub kind_text: String,
    pub size_text: String,
    pub modified_text: String,
    pub permissions_text: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    /// Multi-select highlight (the runtime owns the selection set).
    pub selected: bool,
}

/// One clickable breadcrumb of the current remote path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpCrumbData {
    pub label: String,
    pub path: String,
}

/// Why the SFTP lifecycle is unavailable (enum id; Slint renders the sentence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SftpUnavailableReason {
    SelectNativeSsh,
    FakeBackendSelected,
}

impl SftpUnavailableReason {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::SelectNativeSsh => "select-native-ssh",
            Self::FakeBackendSelected => "fake-backend",
        }
    }

    pub(crate) const fn legacy_text(self) -> &'static str {
        match self {
            Self::SelectNativeSsh => "select the native-ssh backend before using SFTP",
            Self::FakeBackendSelected => "fake transport backend is selected",
        }
    }
}

/// Value-only SFTP listing placeholder state (the row listing itself is data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SftpListingState {
    ConnectReal,
    /// 草稿会话（旧 `+` 行为）下的占位状态；N2 起草稿标签暂无可达 UI 入口，
    /// 保留变体以维持既有投影代码（后续随草稿路径一起清理）。
    #[allow(dead_code)]
    DraftDisconnected,
    RefreshNeedsSession,
    FakeBackendSelected,
    NativeSshSelected,
    EmptyDirectory,
    Rows(String),
    ReconnectFailed,
    SyncFakeBackend,
    SyncMissingSession,
    SyncDisconnected,
    SyncReconnectNative,
    SyncFailed {
        session_key: String,
        path: String,
        error: String,
    },
}

impl SftpListingState {
    pub(crate) const fn kind_id(&self) -> &'static str {
        match self {
            Self::ConnectReal => "connect-real",
            Self::DraftDisconnected => "draft-disconnected",
            Self::RefreshNeedsSession => "refresh-needs-session",
            Self::FakeBackendSelected => "fake-backend-selected",
            Self::NativeSshSelected => "native-ssh-selected",
            Self::EmptyDirectory => "empty-directory",
            Self::Rows(_) => "rows",
            Self::ReconnectFailed => "reconnect-failed",
            Self::SyncFakeBackend => "sync-fake-backend",
            Self::SyncMissingSession => "sync-missing-session",
            Self::SyncDisconnected => "sync-disconnected",
            Self::SyncReconnectNative => "sync-reconnect-native",
            Self::SyncFailed { .. } => "sync-failed",
        }
    }

    pub(crate) fn rows(&self) -> &str {
        match self {
            Self::Rows(rows) => rows,
            _ => "",
        }
    }

    pub(crate) fn session_key(&self) -> &str {
        match self {
            Self::SyncFailed { session_key, .. } => session_key,
            _ => "",
        }
    }

    pub(crate) fn path(&self) -> &str {
        match self {
            Self::SyncFailed { path, .. } => path,
            _ => "",
        }
    }

    pub(crate) fn error(&self) -> &str {
        match self {
            Self::SyncFailed { error, .. } => error,
            _ => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SftpSessionLifecycle {
    Unavailable {
        reason: SftpUnavailableReason,
    },
    Disconnected {
        session_key: Option<String>,
    },
    Ready {
        session_key: String,
    },
    Failed {
        session_key: Option<String>,
        reason: String,
    },
}

impl SftpSessionLifecycle {
    pub(crate) const fn status_kind(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "unavailable",
            Self::Disconnected { .. } => "disconnected",
            Self::Ready { .. } => "ready",
            Self::Failed { .. } => "failed",
        }
    }

    pub(crate) fn status_reason(&self) -> &'static str {
        match self {
            Self::Unavailable { reason } => reason.id(),
            Self::Disconnected { .. } | Self::Ready { .. } | Self::Failed { .. } => "",
        }
    }

    pub(crate) fn status_detail(&self) -> &str {
        match self {
            Self::Failed { reason, .. } => reason,
            Self::Unavailable { .. } | Self::Disconnected { .. } | Self::Ready { .. } => "",
        }
    }

    pub(crate) fn status_session_key(&self) -> &str {
        match self {
            Self::Disconnected {
                session_key: Some(session_key),
            }
            | Self::Ready { session_key } => session_key,
            Self::Failed {
                session_key: Some(session_key),
                ..
            } => session_key,
            Self::Unavailable { .. }
            | Self::Disconnected { session_key: None }
            | Self::Failed {
                session_key: None, ..
            } => "",
        }
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    pub(crate) fn legacy_status_text(&self) -> String {
        match self {
            Self::Unavailable { reason } => {
                format!("SFTP unavailable: {}", reason.legacy_text())
            }
            Self::Disconnected { session_key } => match session_key {
                Some(session_key) => {
                    format!("SFTP session disconnected for runtime session `{session_key}`.")
                }
                None => "SFTP session disconnected. Connect a runtime session first.".to_owned(),
            },
            Self::Ready { session_key } => {
                format!("SFTP session ready for runtime session `{session_key}`.")
            }
            Self::Failed {
                session_key,
                reason,
            } => match session_key {
                Some(session_key) => {
                    format!("SFTP session failed for runtime session `{session_key}`: {reason}")
                }
                None => format!("SFTP session failed: {reason}"),
            },
        }
    }
}

pub(crate) fn normalize_remote_path(path: &str) -> AppResult<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppError::new("remote path must not be empty"));
    }
    if !trimmed.starts_with('/') {
        return Err(AppError::new(format!(
            "remote path `{trimmed}` must start with `/`"
        )));
    }
    let normalized = if trimmed == "/" {
        "/".to_owned()
    } else {
        format!("/{}", trimmed.trim_matches('/'))
    };
    Ok(normalized)
}

pub(crate) fn parent_remote_path(path: &str) -> String {
    let normalized = normalize_remote_path(path).unwrap_or_else(|_| "/".to_owned());
    if normalized == "/" {
        return normalized;
    }
    let trimmed = normalized.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", _)) | None => "/".to_owned(),
        Some((parent, _)) => parent.to_owned(),
    }
}

pub(crate) fn join_remote_path(base: &str, name: &str) -> String {
    let base = normalize_remote_path(base).unwrap_or_else(|_| "/".to_owned());
    if base == "/" {
        format!("/{}", name.trim_start_matches('/'))
    } else {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            name.trim_start_matches('/')
        )
    }
}

/// Inclusive range between two visible remote entries (Shift selection).
fn remote_entry_range(visible: &[FsEntry], from: &str, to: &str) -> Vec<String> {
    let position = |path: &str| visible.iter().position(|entry| entry.path == path);
    let (Some(from_index), Some(to_index)) = (position(from), position(to)) else {
        return vec![to.to_owned()];
    };
    let (start, end) = if from_index <= to_index {
        (from_index, to_index)
    } else {
        (to_index, from_index)
    };
    visible[start..=end]
        .iter()
        .map(|entry| entry.path.clone())
        .collect()
}

pub(crate) fn parse_sftp_permissions(value: &str) -> AppResult<u32> {
    let trimmed = value.trim();
    let octal = trimmed.strip_prefix("0o").unwrap_or(trimmed);
    if octal.len() != 3 || !octal.chars().all(|digit| matches!(digit, '0'..='7')) {
        return Err(AppError::new(
            "SFTP permissions must be a three-digit octal value such as 644 or 755",
        ));
    }
    u32::from_str_radix(octal, 8)
        .map_err(|_| AppError::new("SFTP permissions must be valid octal digits"))
}
