//! Saved sessions: profile table operations, recent sessions, folder tree and
//! sidebar tree projection support.

use crate::{
    error::AppError, error::AppResult, session_runtime::SessionRuntime,
    session_runtime::SessionSource,
};
use std::collections::BTreeSet;
use yshell_config::{FolderProfile, LoadOutcome, SessionProfile};
use yshell_core::SessionEvent;

use super::*;

impl AppRuntime {
    pub fn create_folder_under_editor_target(&mut self) -> AppResult<AppProjection> {
        let folder_name = self.new_folder_name.trim().to_owned();
        if folder_name.is_empty() {
            return Err(AppError::new("new folder name must not be empty"));
        }
        if folder_name.contains('/') || folder_name.contains('\0') {
            return Err(AppError::new(
                "new folder name must not contain `/` or null characters",
            ));
        }
        let parent_folder_id = self.editor.target_folder_id.trim().to_owned();
        if parent_folder_id.is_empty() {
            return Err(AppError::new("editor target folder must not be empty"));
        }
        let new_folder_id = self.next_folder_id(&folder_name);
        let new_folder = FolderProfile::new(new_folder_id.clone(), folder_name.clone());
        let parent_label = {
            let parent = self.target_saved_sessions_folder_mut(&parent_folder_id)?;
            if parent
                .folders
                .iter()
                .any(|folder| folder.name.eq_ignore_ascii_case(&folder_name))
            {
                return Err(AppError::new(format!(
                    "folder `{folder_name}` already exists under `{}`",
                    parent.name
                )));
            }
            let label = parent.name.clone();
            parent.folders.push(new_folder);
            label
        };
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        self.editor.target_folder_id = new_folder_id;
        self.new_folder_name.clear();
        self.status_text = format!(
            "Created folder `{folder_name}` under `{parent_label}` and selected it for the editor."
        );
        Ok(self.projection())
    }

    pub fn create_root_saved_folder(&mut self, name: &str) -> AppResult<AppProjection> {
        let folder_name = name.trim().to_owned();
        if folder_name.is_empty() {
            return Err(AppError::new("new folder name must not be empty"));
        }
        if folder_name.contains('/') || folder_name.contains('\0') {
            return Err(AppError::new(
                "new folder name must not contain `/` or null characters",
            ));
        }
        if self
            .config_document
            .folders
            .iter()
            .any(|folder| folder.name.eq_ignore_ascii_case(&folder_name))
        {
            return Err(AppError::new(format!(
                "folder `{folder_name}` already exists in the session tree root"
            )));
        }
        let new_folder_id = self.next_folder_id(&folder_name);
        self.config_document
            .folders
            .push(FolderProfile::new(new_folder_id, folder_name.clone()));
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        self.status_text = format!(
            "Created folder `{folder_name}` in the session tree root and persisted the updated config."
        );
        Ok(self.projection())
    }

    pub fn refresh_saved_sessions(&mut self) -> AppResult<AppProjection> {
        let LoadOutcome {
            document,
            recovered_from_backup,
        } = self
            .config_store
            .load_or_recover()
            .map_err(AppError::from_error)?;
        self.config_document = document;
        if recovered_from_backup.is_some() {
            self.recovered_from_backup = recovered_from_backup;
        }
        if self
            .selected_saved_session_id
            .as_deref()
            .is_some_and(|id| self.config_document.find_session(id).is_none())
        {
            self.selected_saved_session_id = self.first_saved_session_id();
        }
        if self
            .selected_saved_folder_id
            .as_deref()
            .is_some_and(|id| self.config_document.find_folder(id).is_none())
        {
            self.selected_saved_folder_id = None;
        }
        let config_document = &self.config_document;
        self.collapsed_saved_folders
            .retain(|id| config_document.find_folder(id).is_some());
        self.status_text = format!(
            "Reloaded saved sessions from config.toml. Saved sessions discovered: {}.",
            self.saved_session_count()
        );
        Ok(self.projection())
    }

    pub fn save_active_session(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let active_runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let profile_id = self.next_saved_profile_id(&active_runtime.ssh_config.host);
        let profile = active_runtime.to_session_profile(profile_id.clone());

        if let Some(existing) = self.config_document.find_session_mut(&profile_id) {
            *existing = profile;
        } else {
            self.ensure_saved_sessions_folder().sessions.push(profile);
        }
        self.selected_saved_session_id = Some(profile_id.clone());
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        let saved_count = self.saved_session_count().to_string();
        self.set_status_kind(
            "session-saved",
            format!(
                "Saved active runtime session as profile `{profile_id}`. Config now contains {saved_count} saved session(s)."
            ),
            profile_id,
            saved_count,
        );
        Ok(self.projection())
    }

    pub fn open_saved_session(&mut self, profile_id: &str) -> AppResult<AppProjection> {
        let profile = self
            .config_document
            .find_session(profile_id)
            .cloned()
            .ok_or_else(|| AppError::new(format!("saved session `{profile_id}` was not found")))?;
        let runtime = SessionRuntime::from_profile(&profile, self.allocate_runtime_ordinal());
        self.activate_runtime_session(runtime)
    }

    pub fn open_first_saved_session(&mut self) -> AppResult<AppProjection> {
        let first_profile_id = self
            .first_saved_session_id()
            .ok_or_else(|| AppError::new("no saved sessions are available"))?;
        self.open_saved_session(&first_profile_id)
    }

    pub fn open_selected_saved_session(&mut self) -> AppResult<AppProjection> {
        let profile_id = self
            .selected_saved_session_id
            .clone()
            .ok_or_else(|| AppError::new("no saved session is currently selected"))?;
        self.open_saved_session(&profile_id)
    }

    pub fn update_selected_saved_session_from_active(&mut self) -> AppResult<AppProjection> {
        let profile_id = self
            .selected_saved_session_id
            .clone()
            .ok_or_else(|| AppError::new("no saved session is currently selected"))?;
        let session_key = self.active_session_key()?;
        let active_runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let updated = active_runtime.to_session_profile(profile_id.clone());
        let existing = self
            .config_document
            .find_session_mut(&profile_id)
            .ok_or_else(|| AppError::new(format!("saved session `{profile_id}` was not found")))?;
        *existing = updated;
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        self.status_text =
            format!("Updated saved session `{profile_id}` from the active runtime session.");
        Ok(self.projection())
    }

    pub fn delete_selected_saved_session(&mut self) -> AppResult<AppProjection> {
        let profile_id = self
            .selected_saved_session_id
            .clone()
            .ok_or_else(|| AppError::new("no saved session is currently selected"))?;
        let removed = self
            .config_document
            .remove_session(&profile_id)
            .ok_or_else(|| AppError::new(format!("saved session `{profile_id}` was not found")))?;
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        // N0：该 profile 可能已被重复打开成多个标签；全部关闭并清理其运行时。
        let profile_session_keys: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, runtime)| {
                matches!(
                    &runtime.source,
                    SessionSource::SavedSession { profile_id: current } if current == &profile_id
                )
            })
            .map(|(key, _)| key.clone())
            .collect();
        let profile_tab_ids: Vec<String> = self
            .tabs
            .iter()
            .filter(|tab| {
                tab.session_id().is_some_and(|session_id| {
                    profile_session_keys.iter().any(|key| key == session_id)
                })
            })
            .map(|tab| tab.tab_id.clone())
            .collect();
        for tab_id in &profile_tab_ids {
            self.close_tab_immediate(tab_id);
        }
        for key in &profile_session_keys {
            self.sessions.remove(key);
            self.recent_session_ids.retain(|id| id != key);
        }
        self.pending_close_tabs = None;
        self.selected_saved_session_id = self.first_saved_session_id();
        let removed_id = removed.id;
        self.set_status_kind(
            "session-deleted",
            format!("Deleted saved session `{removed_id}` and persisted the updated config."),
            removed_id,
            String::new(),
        );
        Ok(self.projection())
    }

    pub fn select_next_saved_session(&mut self) -> AppProjection {
        self.rotate_saved_session_selection(true);
        self.status_text = format!(
            "Selected saved session: {}",
            self.saved_session_selection_legacy_text()
        );
        self.projection()
    }

    pub fn select_previous_saved_session(&mut self) -> AppProjection {
        self.rotate_saved_session_selection(false);
        self.status_text = format!(
            "Selected saved session: {}",
            self.saved_session_selection_legacy_text()
        );
        self.projection()
    }

    pub fn toggle_saved_folder(&mut self, folder_id: &str) -> AppResult<AppProjection> {
        if self.config_document.find_folder(folder_id).is_none() {
            return Err(AppError::new(format!(
                "saved-session folder `{folder_id}` was not found in the saved-session tree"
            )));
        }
        if !self.collapsed_saved_folders.remove(folder_id) {
            self.collapsed_saved_folders.insert(folder_id.to_owned());
        }
        self.status_text = if self.collapsed_saved_folders.contains(folder_id) {
            format!("Collapsed saved-session folder `{folder_id}`.")
        } else {
            format!("Expanded saved-session folder `{folder_id}`.")
        };
        Ok(self.projection())
    }

    pub fn select_saved_session_by_id(&mut self, id: &str) -> AppProjection {
        if self.config_document.find_folder(id).is_some() {
            self.selected_saved_folder_id = Some(id.to_owned());
            self.selected_saved_session_id = None;
            self.status_text = format!("Selected saved-session folder `{id}`.");
            return self.projection();
        }
        if let Some(profile) = self.config_document.find_session(id) {
            let label = profile.name.clone();
            self.selected_saved_folder_id = None;
            self.selected_saved_session_id = Some(id.to_owned());
            self.status_text = format!("Selected saved session: {label}");
            return self.projection();
        }
        self.status_text = format!("Saved-session tree node `{id}` was not found.");
        self.projection()
    }

    pub fn activate_saved_session_tree_node(&mut self, id: &str) -> AppResult<AppProjection> {
        if self.config_document.find_folder(id).is_some() {
            return self.toggle_saved_folder(id);
        }
        if self.config_document.find_session(id).is_none() {
            return Err(AppError::new(format!(
                "saved session `{id}` was not found in the saved-session tree"
            )));
        }
        let _ = self.select_saved_session_by_id(id);
        self.open_saved_session(id)
    }

    pub fn update_session_search(&mut self, query: &str) -> AppProjection {
        let previous_query = self.session_search_query.trim().to_owned();
        self.session_search_query = query.to_owned();
        let next_query = self.session_search_query.trim().to_owned();
        if previous_query.is_empty() && !next_query.is_empty() {
            // Entering search expands every folder: a hit must never stay hidden
            // behind a folder the user collapsed before searching.
            self.collapsed_saved_folders.clear();
        }
        self.status_text = if self.session_search_query.trim().is_empty() {
            "Session search cleared.".to_owned()
        } else {
            format!(
                "Filtering saved sessions for `{}`.",
                self.session_search_query
            )
        };
        self.projection()
    }

    pub fn saved_session_profiles(&self) -> Vec<SessionProfile> {
        let mut profiles = Vec::new();
        collect_session_profiles(&self.config_document.folders, &mut profiles);
        profiles
    }

    pub(crate) fn session_tree_rows(&self) -> Vec<SessionTreeRow> {
        let mut rows = Vec::new();
        let projection = SavedTreeProjection {
            query: self.session_search_query.trim(),
            selected_session_id: self.selected_saved_session_id.as_deref(),
            selected_folder_id: self.selected_saved_folder_id.as_deref(),
            collapsed_folders: &self.collapsed_saved_folders,
        };
        collect_session_tree_rows(
            &self.config_document.folders,
            0,
            false,
            &projection,
            &mut rows,
        );
        rows
    }

    pub(crate) fn filtered_saved_session_profiles(&self) -> Vec<SessionProfile> {
        let mut profiles = Vec::new();
        collect_filtered_session_profiles(
            &self.config_document.folders,
            self.session_search_query.trim(),
            &mut profiles,
        );
        profiles
    }

    pub(crate) fn hydrate_saved_sessions(&mut self) {
        let profiles = self.saved_session_profiles();
        for (index, profile) in profiles.iter().enumerate() {
            let runtime = SessionRuntime::from_profile(profile, index + 1);
            self.sessions
                .insert(runtime.session_id().as_str().to_owned(), runtime);
        }
        if self.selected_saved_session_id.is_none() {
            self.selected_saved_session_id = self.first_saved_session_id();
        }
        self.next_runtime_ordinal = self
            .next_runtime_ordinal
            .max(self.saved_session_count() + 1);
    }

    pub(crate) fn rotate_saved_session_selection(&mut self, forward: bool) {
        let profiles = self.filtered_saved_session_profiles();
        if profiles.is_empty() {
            self.selected_saved_session_id = None;
            return;
        }
        let current_index = self
            .selected_saved_session_id
            .as_ref()
            .and_then(|selected| profiles.iter().position(|profile| &profile.id == selected))
            .unwrap_or(0);
        let len = profiles.len();
        let next_index = if forward {
            (current_index + 1) % len
        } else if current_index == 0 {
            len - 1
        } else {
            current_index - 1
        };
        self.selected_saved_session_id = Some(profiles[next_index].id.clone());
    }

    pub(crate) fn record_recent_session(&mut self, session_id: &str) {
        self.recent_session_ids
            .retain(|existing| existing != session_id);
        self.recent_session_ids.insert(0, session_id.to_owned());
        self.recent_session_ids.truncate(8);
    }

    pub(crate) fn next_saved_profile_id(&self, host: &str) -> String {
        let mut stem = host
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_owned();
        if stem.is_empty() {
            stem = "session".to_owned();
        }
        let base = format!("saved-{stem}");
        if self.config_document.find_session(&base).is_none() {
            return base;
        }
        let mut ordinal = 2usize;
        loop {
            let candidate = format!("{base}-{ordinal}");
            if self.config_document.find_session(&candidate).is_none() {
                return candidate;
            }
            ordinal += 1;
        }
    }

    pub(crate) fn next_folder_id(&self, name: &str) -> String {
        let mut stem = name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_owned();
        if stem.is_empty() {
            stem = "folder".to_owned();
        }
        let base = format!("folder-{stem}");
        if self.config_document.find_folder(&base).is_none() {
            return base;
        }
        let mut ordinal = 2usize;
        loop {
            let candidate = format!("{base}-{ordinal}");
            if self.config_document.find_folder(&candidate).is_none() {
                return candidate;
            }
            ordinal += 1;
        }
    }

    pub(crate) fn saved_session_count(&self) -> usize {
        count_sessions(&self.config_document.folders)
    }

    pub(crate) fn first_saved_session_id(&self) -> Option<String> {
        first_session_id(&self.config_document.folders)
    }

    pub(crate) fn ensure_saved_sessions_folder(&mut self) -> &mut FolderProfile {
        if let Some(index) = self
            .config_document
            .folders
            .iter()
            .position(|folder| folder.id == SAVED_SESSIONS_FOLDER_ID)
        {
            return &mut self.config_document.folders[index];
        }
        self.config_document.folders.push(FolderProfile::new(
            SAVED_SESSIONS_FOLDER_ID,
            SAVED_SESSIONS_FOLDER_NAME,
        ));
        let last_index = self.config_document.folders.len() - 1;
        &mut self.config_document.folders[last_index]
    }

    pub(crate) fn target_saved_sessions_folder_mut(
        &mut self,
        folder_id: &str,
    ) -> AppResult<&mut FolderProfile> {
        if folder_id == SAVED_SESSIONS_FOLDER_ID {
            return Ok(self.ensure_saved_sessions_folder());
        }
        self.config_document
            .find_folder_mut(folder_id)
            .ok_or_else(|| {
                AppError::new(format!(
                    "editor target folder `{folder_id}` does not exist in the saved-session tree"
                ))
            })
    }

    pub(crate) fn apply_session_events(
        expected_session_id: &yshell_core::SessionId,
        runtime: &mut SessionRuntime,
        events: Vec<SessionEvent>,
    ) {
        for event in events {
            match event {
                SessionEvent::TabOpened { session_id, .. }
                    if &session_id == expected_session_id =>
                {
                    runtime.append_status_line("Core tab opened for session.");
                }
                SessionEvent::StateChanged { session_id, state }
                    if &session_id == expected_session_id =>
                {
                    runtime.set_state(state);
                    runtime.append_status_line(&format!(
                        "Core state updated: {}",
                        runtime.state_label()
                    ));
                }
                SessionEvent::Connecting { session_id } if &session_id == expected_session_id => {
                    runtime.set_state(yshell_core::SessionState::Connecting);
                    runtime.append_status_line("Connection entering connecting state.");
                }
                SessionEvent::Connected { session_id } if &session_id == expected_session_id => {
                    runtime.set_state(yshell_core::SessionState::Connected);
                    runtime.append_status_line("Connection established.");
                }
                SessionEvent::Disconnected { session_id, reason }
                    if &session_id == expected_session_id =>
                {
                    runtime.set_state(yshell_core::SessionState::Disconnected);
                    runtime.append_status_line(&format!("Disconnected: {reason}"));
                }
                SessionEvent::Error { session_id, error } if &session_id == expected_session_id => {
                    runtime.set_state(yshell_core::SessionState::Failed);
                    runtime.append_status_line(&format!("Session error: {error}"));
                }
                SessionEvent::TerminalOutput { session_id, bytes }
                    if &session_id == expected_session_id =>
                {
                    runtime
                        .terminal_parser
                        .advance(&mut runtime.terminal_grid, &bytes);
                }
                _ => {}
            }
        }
    }
}

/// One visible row of the sidebar saved-session tree.
///
/// The tree is flattened in display order (a folder row followed by its
/// visible children) so the Slint side can render it as a single list without
/// nested models. Folders that are collapsed simply do not emit their children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreeRow {
    /// `"folder"` or `"session"`.
    pub kind: String,
    /// Nesting level; `0` is a top-level folder.
    pub depth: i32,
    /// Folder name or session name.
    pub label: String,
    /// Sessions: `user@host`; folders: the number of visible sessions below.
    pub detail: String,
    /// Folder id or saved-session profile id.
    pub id: String,
    /// Whether a folder row is expanded (always `false` for sessions).
    pub expanded: bool,
    /// Whether this row is the current tree selection.
    pub selected: bool,
}

pub(crate) const SAVED_SESSIONS_FOLDER_ID: &str = "saved-sessions";

pub(crate) const SAVED_SESSIONS_FOLDER_NAME: &str = "Saved Sessions";

pub(crate) fn collect_session_profiles(folders: &[FolderProfile], out: &mut Vec<SessionProfile>) {
    for folder in folders {
        out.extend(folder.sessions.iter().cloned());
        collect_session_profiles(&folder.folders, out);
    }
}

pub(crate) fn collect_filtered_session_profiles(
    folders: &[FolderProfile],
    query: &str,
    out: &mut Vec<SessionProfile>,
) {
    for folder in folders {
        for session in &folder.sessions {
            if session_matches_query(session, query) {
                out.push(session.clone());
            }
        }
        collect_filtered_session_profiles(&folder.folders, query, out);
    }
}

pub(crate) fn collect_session_inventory_lines(
    folders: &[FolderProfile],
    depth: usize,
    selected_id: Option<&str>,
    query: &str,
    out: &mut Vec<String>,
) -> bool {
    let mut any_visible = false;
    for folder in folders {
        let mut folder_lines = Vec::new();
        let child_visible = collect_session_inventory_lines(
            &folder.folders,
            depth + 1,
            selected_id,
            query,
            &mut folder_lines,
        );

        let mut session_lines = folder
            .sessions
            .iter()
            .filter(|session| session_matches_query(session, query))
            .map(|session| {
                let marker = if selected_id == Some(session.id.as_str()) {
                    ">"
                } else {
                    "-"
                };
                format!(
                    "{}{} {} [{}:{}]",
                    "  ".repeat(depth + 1),
                    marker,
                    session.name,
                    session.host,
                    session.port
                )
            })
            .collect::<Vec<_>>();

        if child_visible || !session_lines.is_empty() || folder_matches_query(folder, query) {
            out.push(format!("{}[folder] {}", "  ".repeat(depth), folder.name));
            out.append(&mut session_lines);
            out.append(&mut folder_lines);
            any_visible = true;
        }
    }
    any_visible
}

/// Read-only inputs of the sidebar session-tree projection.
pub(crate) struct SavedTreeProjection<'a> {
    pub(crate) query: &'a str,
    pub(crate) selected_session_id: Option<&'a str>,
    pub(crate) selected_folder_id: Option<&'a str>,
    pub(crate) collapsed_folders: &'a BTreeSet<String>,
}

/// Appends the visible tree rows of `folders` in display order (folder row
/// first, then its visible sessions, then nested folders) and returns how many
/// sessions are visible below them.
///
/// `show_all` is `true` when an ancestor folder matched the query: the whole
/// subtree is then visible regardless of its own matches.
pub(crate) fn collect_session_tree_rows(
    folders: &[FolderProfile],
    depth: usize,
    show_all: bool,
    projection: &SavedTreeProjection<'_>,
    out: &mut Vec<SessionTreeRow>,
) -> usize {
    let row_depth = i32::try_from(depth).unwrap_or(i32::MAX);
    let mut visible_sessions = 0usize;
    for folder in folders {
        let folder_matches = show_all || folder_matches_query(folder, projection.query);

        let mut session_rows = Vec::new();
        for session in &folder.sessions {
            if !folder_matches && !session_matches_query(session, projection.query) {
                continue;
            }
            session_rows.push(SessionTreeRow {
                kind: "session".to_owned(),
                depth: row_depth.saturating_add(1),
                label: session.name.clone(),
                detail: session_user_host(session),
                id: session.id.clone(),
                expanded: false,
                selected: projection.selected_session_id == Some(session.id.as_str()),
            });
        }

        let mut nested_rows = Vec::new();
        let nested_sessions = collect_session_tree_rows(
            &folder.folders,
            depth + 1,
            folder_matches,
            projection,
            &mut nested_rows,
        );
        let subtree_sessions = session_rows.len() + nested_sessions;
        if !folder_matches && subtree_sessions == 0 {
            continue;
        }

        let expanded = !projection.collapsed_folders.contains(&folder.id);
        out.push(SessionTreeRow {
            kind: "folder".to_owned(),
            depth: row_depth,
            label: folder.name.clone(),
            detail: subtree_sessions.to_string(),
            id: folder.id.clone(),
            expanded,
            selected: projection.selected_folder_id == Some(folder.id.as_str()),
        });
        if expanded {
            out.append(&mut session_rows);
            out.append(&mut nested_rows);
        }
        visible_sessions += subtree_sessions;
    }
    visible_sessions
}

/// `user@host` label of a saved session (host only when no username is set).
pub(crate) fn session_user_host(session: &SessionProfile) -> String {
    match session.username.as_deref() {
        Some(username) if !username.is_empty() => format!("{username}@{}", session.host),
        _ => session.host.clone(),
    }
}

pub(crate) fn session_matches_query(session: &SessionProfile, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    let query = query.to_ascii_lowercase();
    session.name.to_ascii_lowercase().contains(&query)
        || session.host.to_ascii_lowercase().contains(&query)
        || session
            .username
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(&query)
        || session.id.to_ascii_lowercase().contains(&query)
}

pub(crate) fn folder_matches_query(folder: &FolderProfile, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    folder
        .name
        .to_ascii_lowercase()
        .contains(&query.to_ascii_lowercase())
}

pub(crate) fn count_sessions(folders: &[FolderProfile]) -> usize {
    folders
        .iter()
        .map(|folder| folder.sessions.len() + count_sessions(&folder.folders))
        .sum()
}

pub(crate) fn first_session_id(folders: &[FolderProfile]) -> Option<String> {
    for folder in folders {
        if let Some(session) = folder.sessions.first() {
            return Some(session.id.clone());
        }
        if let Some(nested) = first_session_id(&folder.folders) {
            return Some(nested);
        }
    }
    None
}

pub(crate) fn find_session_folder_id(
    folders: &[FolderProfile],
    session_id: &str,
) -> Option<String> {
    for folder in folders {
        if folder
            .sessions
            .iter()
            .any(|session| session.id == session_id)
        {
            return Some(folder.id.clone());
        }
        if let Some(nested) = find_session_folder_id(&folder.folders, session_id) {
            return Some(nested);
        }
    }
    None
}
