//! SFTP browsing: listing state, selection, sorting, breadcrumbs and remote-edit
//! lifecycle tracking.

use std::collections::BTreeSet;

use super::*;
use crate::{
    error::AppError, error::AppResult, local_fs::LocalRowData, sftp_view::format_sftp_modified,
    sftp_view::format_sftp_permissions, sftp_view::format_sftp_permissions_octal,
    sftp_view::format_sftp_size, sftp_view::sftp_kind_text, sftp_view::visible_entries,
    sftp_view::SftpSortColumn,
};
use yshell_sftp::{DirectoryListing, FsEntry, FsEntryKind, RealSftpBackend, SftpClient};

/// UI-thread live SFTP client bound to one runtime session key.
pub(crate) struct LiveSftpClient {
    #[allow(dead_code)]
    pub(crate) session_key: String,
    pub(crate) client: SftpClient<RealSftpBackend>,
}

/// Per-SFTP-tab view state parked while another tab is focused.
///
/// The active tab is mirrored into the `AppRuntime::sftp_*`/`local_pane`
/// fields so the existing SFTP browsing/mutation code can keep operating on
/// one set of fields. Switching tabs parks the current snapshot here and
/// restores the next one.
#[derive(Debug, Clone)]
pub(crate) struct SftpTabState {
    pub(crate) path: String,
    pub(crate) listing: SftpListingState,
    pub(crate) entries: Vec<FsEntry>,
    pub(crate) selected_index: Option<usize>,
    pub(crate) sort_column: SftpSortColumn,
    pub(crate) sort_ascending: bool,
    pub(crate) show_hidden: bool,
    pub(crate) selection: BTreeSet<String>,
    pub(crate) selection_anchor: Option<String>,
    pub(crate) session: SftpSessionLifecycle,
    pub(crate) listing_pending: bool,
    pub(crate) remote_edit_session: Option<RemoteEditSession>,
    pub(crate) local: LocalPaneState,
    /// Left local / right remote split ratio (0..1). Default 0.5, not
    /// persisted and not shared between tabs.
    pub(crate) column_ratio: f32,
}

impl AppRuntime {
    /// Whether `tab_id` is currently a SFTP tab.
    pub(crate) fn is_sftp_tab(&self, tab_id: &str) -> bool {
        self.tab_index(tab_id)
            .is_some_and(|index| self.tabs[index].is_sftp())
    }

    fn sftp_tab_session_key(&self, tab_id: &str) -> Option<String> {
        self.tab_index(tab_id)
            .and_then(|index| self.tabs[index].session_id().map(str::to_owned))
    }

    /// Creates the parked state for a new SFTP tab (local listing included).
    fn fresh_sftp_tab_state(&self, tab_id: &str) -> SftpTabState {
        let session_key = self.sftp_tab_session_key(tab_id);
        let session = match &session_key {
            Some(key) => {
                let connected = self
                    .sessions
                    .get(key)
                    .is_some_and(|runtime| runtime.state == yshell_core::SessionState::Connected);
                if connected {
                    SftpSessionLifecycle::Ready {
                        session_key: key.clone(),
                    }
                } else {
                    SftpSessionLifecycle::Disconnected {
                        session_key: Some(key.clone()),
                    }
                }
            }
            None => SftpSessionLifecycle::Disconnected { session_key: None },
        };
        let mut local = LocalPaneState {
            dir: self.default_local_dir.clone(),
            ..LocalPaneState::default()
        };
        match crate::local_fs::list_pane(&local.dir, crate::local_fs::LocalPaneOptions::default()) {
            Ok(listing) => {
                local.listing = Some(listing);
                local.error = None;
            }
            Err(error) => {
                local.error = Some(error);
            }
        }
        SftpTabState {
            path: "/".to_owned(),
            listing: SftpListingState::ConnectReal,
            entries: Vec::new(),
            selected_index: None,
            sort_column: SftpSortColumn::Name,
            sort_ascending: true,
            show_hidden: false,
            selection: BTreeSet::new(),
            selection_anchor: None,
            session,
            listing_pending: false,
            remote_edit_session: None,
            local,
            column_ratio: 0.5,
        }
    }

    /// Ensures a parked state exists for `tab_id` (does not restore it).
    pub(crate) fn ensure_sftp_tab_state(&mut self, tab_id: &str) {
        if self.sftp_tab_states.contains_key(tab_id) {
            return;
        }
        let state = self.fresh_sftp_tab_state(tab_id);
        self.sftp_tab_states.insert(tab_id.to_owned(), state);
    }

    pub(crate) fn capture_live_sftp_state(&self) -> SftpTabState {
        SftpTabState {
            path: self.sftp_path.clone(),
            listing: self.sftp_listing.clone(),
            entries: self.sftp_entries.clone(),
            selected_index: self.sftp_selected_index,
            sort_column: self.sftp_sort_column,
            sort_ascending: self.sftp_sort_ascending,
            show_hidden: self.sftp_show_hidden,
            selection: self.sftp_selection.clone(),
            selection_anchor: self.sftp_selection_anchor.clone(),
            session: self.sftp_session.clone(),
            listing_pending: self.sftp_listing_pending,
            remote_edit_session: self.remote_edit_session.clone(),
            local: self.local_pane.clone(),
            column_ratio: self.sftp_column_ratio,
        }
    }

    fn apply_sftp_tab_state(&mut self, state: &SftpTabState) {
        self.sftp_column_ratio = state.column_ratio;
        self.sftp_path = state.path.clone();
        self.sftp_listing = state.listing.clone();
        self.sftp_entries = state.entries.clone();
        self.sftp_selected_index = state.selected_index;
        self.sftp_sort_column = state.sort_column;
        self.sftp_sort_ascending = state.sort_ascending;
        self.sftp_show_hidden = state.show_hidden;
        self.sftp_selection = state.selection.clone();
        self.sftp_selection_anchor = state.selection_anchor.clone();
        self.sftp_session = state.session.clone();
        self.sftp_listing_pending = state.listing_pending;
        self.remote_edit_session = state.remote_edit_session.clone();
        self.local_pane = state.local.clone();
    }

    /// Parks the focused SFTP tab's live mirrors into `sftp_tab_states`.
    pub(crate) fn park_active_sftp_tab(&mut self) {
        let Some(tab_id) = self.active_tab_id.clone() else {
            return;
        };
        if !self.is_sftp_tab(&tab_id) {
            return;
        }
        let state = self.capture_live_sftp_state();
        self.sftp_tab_states.insert(tab_id, state);
    }

    /// Restores a parked SFTP tab into the live mirrors. Returns false when
    /// the tab has no parked state.
    pub(crate) fn restore_sftp_tab(&mut self, tab_id: &str) -> bool {
        let Some(state) = self.sftp_tab_states.get(tab_id).cloned() else {
            return false;
        };
        self.apply_sftp_tab_state(&state);
        true
    }

    /// After `active_tab_id` changes, restore the active SFTP tab's mirrors or
    /// clear them for New Tab / Terminal pages.
    pub(crate) fn refresh_active_view_mirrors(&mut self) {
        let Some(tab_id) = self.active_tab_id.clone() else {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.sftp_listing = SftpListingState::RefreshNeedsSession;
            self.sftp_listing_pending = false;
            return;
        };
        if self.is_sftp_tab(&tab_id) {
            self.restore_sftp_tab(&tab_id);
            if let Some(session_key) = self.sftp_tab_session_key(&tab_id) {
                let _ = self.sync_sftp_lifecycle_for_session(&session_key, false);
                if matches!(self.sftp_listing, SftpListingState::ConnectReal) {
                    self.sftp_listing_pending = true;
                }
            }
        } else {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.sftp_listing = SftpListingState::RefreshNeedsSession;
            self.sftp_listing_pending = false;
        }
    }

    /// Drops the parked state and live client when an SFTP tab closes.
    pub(crate) fn drop_sftp_tab_state(&mut self, tab_id: &str) {
        self.sftp_tab_states.remove(tab_id);
        if let Some(session_key) = self.sftp_tab_session_key(tab_id) {
            self.sftp_live.remove(&session_key);
        }
    }

    /// Sets the per-tab local/remote ratio (0.2..0.8).
    pub fn set_sftp_column_ratio(&mut self, ratio: f32) -> AppProjection {
        let clamped = ratio.clamp(0.2, 0.8);
        self.sftp_column_ratio = clamped;
        if let Some(tab_id) = self.active_tab_id.clone() {
            if let Some(state) = self.sftp_tab_states.get_mut(&tab_id) {
                state.column_ratio = clamped;
            }
        }
        self.projection()
    }

    /// Marks an SFTP tab connected. The focused tab updates the live mirrors;
    /// background tabs only update their parked state.
    pub(crate) fn mark_sftp_tab_connected(&mut self, tab_id: &str, session_key: &str) {
        if self.active_tab_id.as_deref() == Some(tab_id) {
            let _ = self.sync_sftp_lifecycle_for_session(session_key, true);
            self.park_active_sftp_tab();
            return;
        }
        if let Some(state) = self.sftp_tab_states.get_mut(tab_id) {
            state.session = SftpSessionLifecycle::Ready {
                session_key: session_key.to_owned(),
            };
            state.listing_pending = true;
            if matches!(
                state.listing,
                SftpListingState::SyncDisconnected
                    | SftpListingState::RefreshNeedsSession
                    | SftpListingState::ConnectReal
            ) {
                state.listing = SftpListingState::ConnectReal;
            }
        }
    }

    pub(crate) fn sftp_tab_id_for_session(&self, session_key: &str) -> Option<String> {
        self.tabs
            .iter()
            .find(|tab| tab.is_sftp() && tab.session_id() == Some(session_key))
            .map(|tab| tab.tab_id.clone())
    }

    /// Marks an SFTP tab disconnected/failed in its parked state.
    pub(crate) fn mark_sftp_tab_disconnected(&mut self, session_key: &str) {
        let Some(tab_id) = self.sftp_tab_id_for_session(session_key) else {
            return;
        };
        if self.active_tab_id.as_deref() == Some(tab_id.as_str()) {
            self.sftp_session = SftpSessionLifecycle::Disconnected {
                session_key: Some(session_key.to_owned()),
            };
            self.sftp_listing = SftpListingState::SyncDisconnected;
            self.park_active_sftp_tab();
        } else if let Some(state) = self.sftp_tab_states.get_mut(&tab_id) {
            state.session = SftpSessionLifecycle::Disconnected {
                session_key: Some(session_key.to_owned()),
            };
            state.listing = SftpListingState::SyncDisconnected;
            state.listing_pending = false;
        }
    }

    /// Drop every parked SFTP TCP/SSH session (backend switch / hard reset).
    #[allow(dead_code)]
    pub(crate) fn drop_sftp_live_client(&mut self) {
        self.sftp_live.clear();
    }

    /// Drop the live client for one session (disconnect / transport failure).
    pub(crate) fn drop_sftp_live_client_for(&mut self, session_key: &str) {
        self.sftp_live.remove(session_key);
    }

    /// Reuse or create the live SFTP client for `session_key`.
    ///
    /// Clients stay in `sftp_live` across tab switches so revisiting a connected
    /// session does not redo TCP+SSH+auth on the UI thread.
    pub(crate) fn sftp_live_client_mut(
        &mut self,
        session_key: &str,
    ) -> AppResult<&mut SftpClient<RealSftpBackend>> {
        // TAB-4：本地终端没有 SSH/SFTP 后端；任何误入的 SFTP 操作直接失败。
        if self
            .sessions
            .get(session_key)
            .is_some_and(|runtime| runtime.is_local())
        {
            return Err(AppError::new(
                "SFTP is unavailable for local terminal sessions",
            ));
        }
        if !self.sftp_live.contains_key(session_key) {
            let (transport, ssh_config) = {
                let runtime = self
                    .sessions
                    .get(session_key)
                    .ok_or_else(|| AppError::new("active runtime session is missing"))?;
                (
                    runtime.transport.clone(),
                    self.effective_ssh_config(&runtime.ssh_config),
                )
            };
            let client = match transport {
                Some(transport) => SftpClient::with_shared_transport(transport),
                // Test/legacy path: no app-owned transport (unit tests or a
                // session that was never connected). Opening a fresh SSH
                // connection keeps the existing behavior.
                None => SftpClient::with_real_backend(ssh_config),
            };
            self.sftp_live.insert(
                session_key.to_owned(),
                LiveSftpClient {
                    session_key: session_key.to_owned(),
                    client,
                },
            );
        }
        self.sftp_live
            .get_mut(session_key)
            .map(|live| &mut live.client)
            .ok_or_else(|| AppError::new("SFTP live client missing after ensure"))
    }

    /// Drain a deferred listing requested by tab activation (non-blocking switch).
    pub fn drain_pending_sftp_listing(&mut self) -> Option<AppProjection> {
        if !self.sftp_listing_pending {
            return None;
        }
        self.sftp_listing_pending = false;
        match self.refresh_active_sftp_listing() {
            Ok(projection) => Some(projection),
            Err(error) => {
                tracing::warn!(
                    target: "yshell::app",
                    "deferred SFTP listing after tab switch failed: {error}"
                );
                None
            }
        }
    }

    pub fn refresh_active_sftp_listing(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.sftp_listing = SftpListingState::RefreshNeedsSession;
            let text = self.sftp_session.legacy_status_text();
            self.set_status_kind("sftp-disconnected", text, String::new(), String::new());
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
            let session_key = self.sftp_session.status_session_key().to_owned();
            self.set_status_kind("sftp-disconnected", text, session_key, String::new());
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

    /// Ensures the SFTP transfer drawer is expanded (shared queue with Transfers dock).
    pub fn open_transfer_drawer(&mut self) -> AppProjection {
        self.transfer_drawer_expanded = true;
        self.projection()
    }

    /// Opens the Properties dialog for the current remote selection.
    pub fn open_sftp_properties(&mut self) -> AppProjection {
        if self.selected_sftp_entry().is_none() {
            self.set_status_kind(
                "sftp-properties-requires-selection",
                "Select an SFTP entry before opening its properties.".to_owned(),
                String::new(),
                String::new(),
            );
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
            self.set_status_kind(
                "sftp-open-requires-selection",
                "Select an SFTP entry before opening it.".to_owned(),
                String::new(),
                String::new(),
            );
            return Ok(self.projection());
        };
        if matches!(entry.kind, FsEntryKind::Directory) {
            return self.open_sftp_path(&entry.path);
        }
        self.edit_sftp_selected()
    }

    pub fn sort_sftp_by(&mut self, column: &str) -> AppProjection {
        let Some(column) = SftpSortColumn::from_label(column) else {
            let column_text = column.trim().to_owned();
            self.set_status_kind(
                "sftp-unknown-sort-column",
                format!("Unknown SFTP sort column `{column_text}`."),
                column_text,
                String::new(),
            );
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
        if self.sftp_show_hidden {
            self.set_status_kind(
                "sftp-hidden-shown",
                "SFTP hidden files are now shown.".to_owned(),
                String::new(),
                String::new(),
            );
        } else {
            self.set_status_kind(
                "sftp-hidden-hidden",
                "SFTP hidden files are now hidden.".to_owned(),
                String::new(),
                String::new(),
            );
        }
        self.projection()
    }

    pub fn open_sftp_crumb(&mut self, index: i32) -> AppResult<AppProjection> {
        let crumbs = self.sftp_crumbs();
        let Some(crumb) = usize::try_from(index)
            .ok()
            .and_then(|index| crumbs.get(index))
        else {
            self.set_status_kind(
                "sftp-breadcrumb-missing",
                "That SFTP breadcrumb is no longer available.".to_owned(),
                String::new(),
                String::new(),
            );
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
        sftp_rows_for(
            &self.sftp_entries,
            self.sftp_show_hidden,
            self.sftp_sort_column,
            self.sftp_sort_ascending,
            &self.sftp_selection,
        )
    }

    pub(crate) fn sftp_crumbs(&self) -> Vec<SftpCrumbData> {
        sftp_crumbs_for(&self.sftp_path)
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
        action_id: &str,
    ) -> AppResult<Option<String>> {
        let action = sftp_action_english_label(action_id);
        let Some(session_key) = self.active_session_id.clone() else {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            let lifecycle = self.sftp_session.legacy_status_text();
            tracing::info!(
                target: "yshell::runtime",
                "SFTP operation blocked because no runtime session is active: {lifecycle}"
            );
            self.set_status_kind(
                "sftp-not-ready",
                format!(
                    "{lifecycle} Cannot continue {action} until a runtime session is connected."
                ),
                action_id.to_owned(),
                String::new(),
            );
            return Ok(None);
        };
        let message = self.sync_sftp_lifecycle_for_session(&session_key, false);
        if self.sftp_ready_session_key().is_some() {
            Ok(Some(session_key))
        } else {
            tracing::info!(
                target: "yshell::runtime",
                "SFTP operation `{action_id}` cannot continue: {message}"
            );
            self.set_status_kind(
                "sftp-not-ready",
                format!("{message} Cannot continue {action}."),
                action_id.to_owned(),
                String::new(),
            );
            Ok(None)
        }
    }

    pub(crate) fn sync_sftp_lifecycle_for_session(
        &mut self,
        session_key: &str,
        refresh_listing: bool,
    ) -> String {
        let Some((session_state, is_local)) = self
            .sessions
            .get(session_key)
            .map(|runtime| (runtime.state, runtime.is_local()))
        else {
            self.drop_sftp_live_client_for(session_key);
            self.sftp_session = SftpSessionLifecycle::Failed {
                session_key: Some(session_key.to_owned()),
                reason: "runtime session is missing".to_owned(),
            };
            self.sftp_listing = SftpListingState::SyncMissingSession;
            return self.sftp_session.legacy_status_text();
        };

        // TAB-4：本地终端没有远端文件系统；生命周期固定为 disconnected，
        // 不建 SFTP 连接（即使 shell 本身处于 connected）。
        if is_local {
            self.drop_sftp_live_client_for(session_key);
            self.sftp_session = SftpSessionLifecycle::Disconnected {
                session_key: Some(session_key.to_owned()),
            };
            self.sftp_listing = SftpListingState::SyncDisconnected;
            return "SFTP is unavailable for local terminal sessions.".to_owned();
        }

        if session_state != yshell_core::SessionState::Connected {
            self.drop_sftp_live_client_for(session_key);
            self.sftp_session = SftpSessionLifecycle::Disconnected {
                session_key: Some(session_key.to_owned()),
            };
            self.sftp_listing = SftpListingState::SyncDisconnected;
            return self.sftp_session.legacy_status_text();
        }

        self.sftp_session = SftpSessionLifecycle::Ready {
            session_key: session_key.to_owned(),
        };
        if !refresh_listing {
            return self.sftp_session.legacy_status_text();
        }
        // Unit tests use a fake shell and have no app-owned SSH transport; do
        // not attempt a real TCP/SSH connection to the fixture host.
        #[cfg(test)]
        if self
            .sessions
            .get(session_key)
            .is_some_and(|runtime| runtime.is_sftp() && runtime.transport.is_none())
        {
            self.sftp_listing = SftpListingState::EmptyDirectory;
            self.sftp_listing_pending = false;
            return self.sftp_session.legacy_status_text();
        }

        let path = self.sftp_path.clone();
        let list_result = self
            .sftp_live_client_mut(session_key)
            .and_then(|client| client.list_dir(&path).map_err(AppError::from_error));
        match list_result {
            Ok(listing) => {
                let entry_count = listing.entries.len();
                let listing_path = listing.path.clone();
                self.apply_sftp_listing(&listing);
                self.sftp_listing_pending = false;
                format!(
                    "SFTP session ready. Loaded {entry_count} SFTP entr{} from `{listing_path}`.",
                    if entry_count == 1 { "y" } else { "ies" }
                )
            }
            Err(error) => {
                // Force the next refresh to open a new SSH/SFTP channel for this session.
                self.drop_sftp_live_client_for(session_key);
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

    pub(crate) fn sftp_ready_session_key(&self) -> Option<&str> {
        match &self.sftp_session {
            SftpSessionLifecycle::Ready { session_key } => Some(session_key.as_str()),
            SftpSessionLifecycle::Disconnected { .. } | SftpSessionLifecycle::Failed { .. } => None,
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

fn sftp_rows_for(
    entries: &[FsEntry],
    show_hidden: bool,
    sort_column: SftpSortColumn,
    sort_ascending: bool,
    selection: &BTreeSet<String>,
) -> Vec<SftpRowData> {
    visible_entries(entries, show_hidden, sort_column, sort_ascending)
        .iter()
        .map(|entry| sftp_row_from(entry, selection.contains(&entry.path)))
        .collect()
}

fn sftp_row_from(entry: &FsEntry, selected: bool) -> SftpRowData {
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
        selected,
    }
}

fn sftp_crumbs_for(path: &str) -> Vec<SftpCrumbData> {
    let mut crumbs = vec![SftpCrumbData {
        label: "/".to_owned(),
        path: "/".to_owned(),
    }];
    let normalized = normalize_remote_path(path).unwrap_or_else(|_| "/".to_owned());
    let mut current = String::new();
    for part in normalized.split('/').filter(|part| !part.is_empty()) {
        current.push('/');
        current.push_str(part);
        crumbs.push(SftpCrumbData {
            label: part.to_owned(),
            path: current.clone(),
        });
    }
    crumbs
}

/// Per-SFTP-tab data used by split panes (and reusable for a single page).
#[derive(Debug, Clone, PartialEq)]
pub struct SftpPaneProjection {
    pub path_text: String,
    pub rows: Vec<SftpRowData>,
    pub crumbs: Vec<SftpCrumbData>,
    pub selected_index: i32,
    pub connected: bool,
    pub sort_column_text: String,
    pub sort_ascending: bool,
    pub sort_text: String,
    pub show_hidden: bool,
    pub item_count: i32,
    pub dir_count: i32,
    pub selected_count: i32,
    pub empty_text: String,
    pub error_text: String,
    pub remote_edit_active: bool,
    pub remote_edit_text: String,
    pub local_rows: Vec<LocalRowData>,
    pub local_path_text: String,
    pub local_sort_column_text: String,
    pub local_sort_ascending: bool,
    pub local_sort_text: String,
    pub local_show_hidden: bool,
    pub local_item_count: i32,
    pub local_dir_count: i32,
    pub local_selected_count: i32,
    pub local_empty_text: String,
    pub local_error_text: String,
    pub local_retry_visible: bool,
    pub local_selected_name_text: String,
    pub local_selection_key: String,
    pub sftp_selection_key: String,
    pub column_ratio: f32,
}

impl Default for SftpPaneProjection {
    fn default() -> Self {
        Self {
            path_text: "/".to_owned(),
            rows: Vec::new(),
            crumbs: Vec::new(),
            selected_index: -1,
            connected: false,
            sort_column_text: "name".to_owned(),
            sort_ascending: true,
            sort_text: "Name ↑".to_owned(),
            show_hidden: false,
            item_count: 0,
            dir_count: 0,
            selected_count: 0,
            empty_text: String::new(),
            error_text: String::new(),
            remote_edit_active: false,
            remote_edit_text: String::new(),
            local_rows: Vec::new(),
            local_path_text: "/".to_owned(),
            local_sort_column_text: "name".to_owned(),
            local_sort_ascending: true,
            local_sort_text: "Name ↑".to_owned(),
            local_show_hidden: false,
            local_item_count: 0,
            local_dir_count: 0,
            local_selected_count: 0,
            local_empty_text: String::new(),
            local_error_text: String::new(),
            local_retry_visible: true,
            local_selected_name_text: String::new(),
            local_selection_key: String::new(),
            sftp_selection_key: String::new(),
            column_ratio: 0.5,
        }
    }
}

impl SftpPaneProjection {
    /// Builds a pane projection from a parked per-tab state snapshot. This is
    /// also used for the focused pane, so split panes never need to share the
    /// mutable `sftp_*` mirrors.
    pub(crate) fn from_state(state: &SftpTabState, session_connected: bool) -> Self {
        let rows = sftp_rows_for(
            &state.entries,
            state.show_hidden,
            state.sort_column,
            state.sort_ascending,
            &state.selection,
        );
        let visible = visible_entries(
            &state.entries,
            state.show_hidden,
            state.sort_column,
            state.sort_ascending,
        );
        let item_count = visible.len();
        let dir_count = visible
            .iter()
            .filter(|entry| matches!(entry.kind, FsEntryKind::Directory))
            .count();
        let hidden_count = state
            .entries
            .iter()
            .filter(|entry| entry.name.starts_with('.'))
            .count();
        let empty_text = if item_count > 0 {
            String::new()
        } else if hidden_count > 0 && !state.show_hidden {
            "No visible files. Enable Hidden to include dotfiles.".to_owned()
        } else {
            "This directory is empty.".to_owned()
        };
        let selected_paths = state
            .selection
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let local_rows = crate::runtime::local_pane::local_rows_for(&state.local);
        let local_selected_paths = state.local.selected.iter().collect::<Vec<_>>();
        let local_selected_name_text = local_selected_paths
            .iter()
            .filter_map(|path| path.file_name())
            .take(3)
            .map(|name| name.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        let local_empty_text = match &state.local.listing {
            Some(listing) if listing.entries.is_empty() => {
                if listing.hidden_count > 0 && !state.local.show_hidden {
                    "No visible files. Enable Hidden to include dotfiles.".to_owned()
                } else {
                    "This directory is empty.".to_owned()
                }
            }
            _ => String::new(),
        };
        let local_error_text = state
            .local
            .error
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        let local_retry_visible = state
            .local
            .error
            .as_ref()
            .is_none_or(crate::local_fs::LocalFsError::retryable);
        Self {
            path_text: state.path.clone(),
            rows,
            crumbs: sftp_crumbs_for(&state.path),
            selected_index: state
                .selected_index
                .and_then(|index| i32::try_from(index).ok())
                .unwrap_or(-1),
            connected: session_connected
                && matches!(state.session, SftpSessionLifecycle::Ready { .. }),
            sort_column_text: state.sort_column.label().to_ascii_lowercase(),
            sort_ascending: state.sort_ascending,
            sort_text: format!(
                "{} {}",
                state.sort_column.label(),
                if state.sort_ascending { "↑" } else { "↓" }
            ),
            show_hidden: state.show_hidden,
            item_count: i32::try_from(item_count).unwrap_or(i32::MAX),
            dir_count: i32::try_from(dir_count).unwrap_or(i32::MAX),
            selected_count: i32::try_from(state.selection.len()).unwrap_or(i32::MAX),
            empty_text,
            error_text: state.listing.error().to_owned(),
            remote_edit_active: state.remote_edit_session.is_some(),
            remote_edit_text: state
                .remote_edit_session
                .as_ref()
                .map(|session| format!("{} → {}", session.remote_path, session.local_temp_path))
                .unwrap_or_default(),
            local_rows,
            local_path_text: state.local.dir.display().to_string(),
            local_sort_column_text: state.local.sort_column.label().to_ascii_lowercase(),
            local_sort_ascending: state.local.sort_ascending,
            local_sort_text: format!(
                "{} {}",
                state.local.sort_column.label(),
                if state.local.sort_ascending {
                    "↑"
                } else {
                    "↓"
                }
            ),
            local_show_hidden: state.local.show_hidden,
            local_item_count: state
                .local
                .listing
                .as_ref()
                .map(|listing| i32::try_from(listing.item_count).unwrap_or(i32::MAX))
                .unwrap_or(0),
            local_dir_count: state
                .local
                .listing
                .as_ref()
                .map(|listing| i32::try_from(listing.dir_count).unwrap_or(i32::MAX))
                .unwrap_or(0),
            local_selected_count: i32::try_from(state.local.selected.len()).unwrap_or(i32::MAX),
            local_empty_text,
            local_error_text,
            local_retry_visible,
            local_selected_name_text,
            local_selection_key: local_selected_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            sftp_selection_key: selected_paths,
            column_ratio: state.column_ratio,
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
    EmptyDirectory,
    Rows(String),
    ReconnectFailed,
    SyncMissingSession,
    SyncDisconnected,
    #[allow(dead_code)]
    SyncReconnect,
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
            Self::EmptyDirectory => "empty-directory",
            Self::Rows(_) => "rows",
            Self::ReconnectFailed => "reconnect-failed",
            Self::SyncMissingSession => "sync-missing-session",
            Self::SyncDisconnected => "sync-disconnected",
            Self::SyncReconnect => "sync-reconnect",
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
            Self::Disconnected { .. } => "disconnected",
            Self::Ready { .. } => "ready",
            Self::Failed { .. } => "failed",
        }
    }

    pub(crate) fn status_reason(&self) -> &'static str {
        ""
    }

    pub(crate) fn status_detail(&self) -> &str {
        match self {
            Self::Failed { reason, .. } => reason,
            Self::Disconnected { .. } | Self::Ready { .. } => "",
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
            Self::Disconnected { session_key: None }
            | Self::Failed {
                session_key: None, ..
            } => "",
        }
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    pub(crate) fn legacy_status_text(&self) -> String {
        match self {
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

/// I18N-1：SFTP 操作 id → 英文回退文案里的动作短语。
///
/// 只用于 `status_text` 的英文回退；界面句子由
/// `TextFormats.sftp-action-label` 按同一个 id 翻译。
fn sftp_action_english_label(action_id: &str) -> &'static str {
    match action_id {
        "upload" => "uploading through SFTP",
        "download" => "downloading through SFTP",
        "delete" => "deleting through SFTP",
        "rename" => "renaming paths through SFTP",
        "chmod" => "changing permissions through SFTP",
        "mkdir" => "creating directories through SFTP",
        "remote-edit" => "starting remote file edit",
        "remote-edit-save" => "saving remote file edit",
        _ => "using SFTP",
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
