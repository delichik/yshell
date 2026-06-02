use chrono::Utc;
use serde::Deserialize;
use tauri::{AppHandle, State};

use crate::{
    config::{AppSettings, SessionProfile},
    terminal::{RuntimeRegistry, TerminalRuntime},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickConnectDraft {
    pub protocol: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: String,
    pub save_as_session: bool,
}

#[tauri::command]
pub fn sessions_list() -> Result<Vec<SessionProfile>, String> {
    Ok(Vec::new())
}

#[tauri::command]
pub fn sessions_save(mut profile: SessionProfile) -> Result<SessionProfile, String> {
    let now = Utc::now();
    profile.updated_at = now;
    Ok(profile)
}

#[tauri::command]
pub fn settings_load() -> Result<AppSettings, String> {
    Ok(AppSettings::default())
}

#[tauri::command]
pub fn terminal_open_local(
    app_handle: AppHandle,
    registry: State<'_, RuntimeRegistry>,
    tab_id: String,
    pane_id: String,
) -> Result<TerminalRuntime, String> {
    registry.open_local(app_handle, tab_id, pane_id)
}

#[tauri::command]
pub fn terminal_open_ssh(
    registry: State<'_, RuntimeRegistry>,
    draft: QuickConnectDraft,
    tab_id: String,
    pane_id: String,
) -> Result<TerminalRuntime, String> {
    let title = if draft.name.is_empty() {
        format!("{}@{}:{}", draft.username, draft.host, draft.port)
    } else {
        draft.name
    };
    registry.open_ssh_placeholder(tab_id, pane_id, title)
}

#[tauri::command]
pub fn terminal_write(
    registry: State<'_, RuntimeRegistry>,
    runtime_id: String,
    data: String,
) -> Result<(), String> {
    registry.write(&runtime_id, &data)
}

#[tauri::command]
pub fn terminal_resize(
    registry: State<'_, RuntimeRegistry>,
    runtime_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    registry.resize(&runtime_id, cols, rows)
}

#[tauri::command]
pub fn terminal_close(
    registry: State<'_, RuntimeRegistry>,
    runtime_id: String,
) -> Result<(), String> {
    registry.close(&runtime_id)
}
