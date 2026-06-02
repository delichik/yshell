use chrono::Utc;
use serde::Deserialize;
use tauri::{AppHandle, State};

use crate::{
    config::{AppSettings, ConfigStore, SessionProfile},
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
pub fn sessions_list(config_store: State<'_, ConfigStore>) -> Result<Vec<SessionProfile>, String> {
    config_store.list_sessions()
}

#[tauri::command]
pub fn sessions_save(
    config_store: State<'_, ConfigStore>,
    mut profile: SessionProfile,
) -> Result<SessionProfile, String> {
    let now = Utc::now();
    profile.updated_at = now;
    config_store.save_session(profile)
}

#[tauri::command]
pub fn settings_load(config_store: State<'_, ConfigStore>) -> Result<AppSettings, String> {
    config_store.load_settings()
}

#[tauri::command]
pub fn settings_save(
    config_store: State<'_, ConfigStore>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    config_store.save_settings(settings)
}

#[tauri::command]
pub fn terminal_open_local(
    app_handle: AppHandle,
    registry: State<'_, RuntimeRegistry>,
    tab_id: String,
    pane_id: String,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<TerminalRuntime, String> {
    registry.open_local(app_handle, tab_id, pane_id, cols, rows)
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
