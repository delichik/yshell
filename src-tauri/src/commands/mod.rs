use chrono::Utc;
use serde::Deserialize;
use tauri::{AppHandle, State};

use crate::{
    config::{AppSettings, ConfigStore, SessionExportBundle, SessionProfile, TerminalConfig},
    ssh::HostKeyPolicy,
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
    pub password: Option<String>,
    pub private_key_path: Option<String>,
    pub passphrase: Option<String>,
    pub host_key_policy: HostKeyPolicy,
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
    profile.auth.credential_ref = profile
        .auth
        .credential_ref
        .filter(|reference| !reference.trim().is_empty());
    config_store.save_session(profile)
}

#[tauri::command]
pub fn sessions_delete(
    config_store: State<'_, ConfigStore>,
    session_id: String,
) -> Result<Vec<SessionProfile>, String> {
    config_store.delete_session(&session_id)
}

#[tauri::command]
pub fn sessions_duplicate(
    config_store: State<'_, ConfigStore>,
    session_id: String,
) -> Result<SessionProfile, String> {
    config_store.duplicate_session(&session_id)
}

#[tauri::command]
pub fn sessions_export(
    config_store: State<'_, ConfigStore>,
) -> Result<SessionExportBundle, String> {
    config_store.export_sessions()
}

#[tauri::command]
pub fn sessions_import(
    config_store: State<'_, ConfigStore>,
    bundle: SessionExportBundle,
) -> Result<Vec<SessionProfile>, String> {
    config_store.import_sessions(bundle)
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
    terminal: Option<TerminalConfig>,
) -> Result<TerminalRuntime, String> {
    let shell = terminal.as_ref().and_then(|config| config.shell.clone());
    let working_directory = terminal.and_then(|config| config.working_directory);
    registry.open_local(
        app_handle,
        tab_id,
        pane_id,
        cols,
        rows,
        shell,
        working_directory,
    )
}

#[tauri::command]
pub fn terminal_open_ssh(
    app_handle: AppHandle,
    registry: State<'_, RuntimeRegistry>,
    draft: QuickConnectDraft,
    tab_id: String,
    pane_id: String,
) -> Result<TerminalRuntime, String> {
    if draft.protocol != "ssh" {
        return Err(format!("unsupported terminal protocol: {}", draft.protocol));
    }

    if draft.host.trim().is_empty() {
        return Err("SSH host is required".to_string());
    }

    if !matches!(
        draft.auth_method.as_str(),
        "password" | "private_key" | "agent"
    ) {
        return Err(format!(
            "unsupported SSH authentication method: {}",
            draft.auth_method
        ));
    }

    if draft.auth_method == "private_key"
        && draft
            .private_key_path
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
    {
        return Err("private key authentication requires a private key path".to_string());
    }

    let title = if draft.name.is_empty() {
        format!("{}@{}:{}", draft.username, draft.host, draft.port)
    } else {
        draft.name.clone()
    };
    let has_ephemeral_secret = draft
        .password
        .as_deref()
        .is_some_and(|value| !value.is_empty())
        || draft
            .passphrase
            .as_deref()
            .is_some_and(|value| !value.is_empty());
    let notice = format!(
        "YShell SSH runtime boundary is ready, but the protocol backend is not enabled in this build.\r\nRequested: {}@{}:{}\r\nAuthentication: {}{}\r\nHost key policy: {:?}\r\nSecrets are treated as ephemeral input and are not written to the session config or export bundle.\r\n",
        if draft.username.is_empty() { "<current-user>" } else { &draft.username },
        draft.host,
        draft.port,
        draft.auth_method,
        if has_ephemeral_secret { " (ephemeral secret supplied)" } else { "" },
        draft.host_key_policy,
    );
    let runtime =
        registry.open_ssh_placeholder_with_notice(app_handle, tab_id, pane_id, title, notice)?;
    let _should_persist_profile = draft.save_as_session;
    Ok(runtime)
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

#[tauri::command]
pub fn terminal_close_all(registry: State<'_, RuntimeRegistry>) -> Result<(), String> {
    registry.close_all()
}
