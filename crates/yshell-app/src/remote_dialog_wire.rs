//! Callback wiring for out-of-process tool windows.
//!
//! Most callbacks are an [`AppRuntimeClient::call_value`] into Host
//! `runtime_dispatch`. Settings L-tier fields (appearance, terminal, SFTP
//! defaults) stay on the window until Save. File picks stay in the Shell
//! (rfd) and come back as `apply_file_dialog_picked`, except the Settings
//! external-editor path, which is written into that local draft.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use serde_json::{json, Value};
use slint::{CloseRequestResponse, ComponentHandle, Model, SharedString, Weak};

use crate::bootstrap::generated_ui::{
    AuthPromptWindow, FolderEditorWindow, HostKeyWindow, HostKeysWindow, KnownHostAddWindow,
    KnownHostExportWindow, KnownHostImportWindow, LoggingWindow, LoginStepKeysWindow,
    LoginStepWaitWindow, MainWindow, MasterPasswordUnlockWindow, MasterPasswordWindow,
    NewFolderWindow, PasswordPromptWindow, PrivateKeyDeployWindow, PrivateKeyExportWindow,
    PrivateKeyGenerateWindow, PrivateKeyImportWindow, PrivateKeysWindow, ProxiesWindow,
    SessionEditorWindow, SettingsWindow, SftpChmodWindow, SftpConflictWindow, SftpEntryWindow,
    SftpPropertiesWindow, ThemeManagerWindow, TunnelEditorWindow,
};
use crate::bootstrap::{
    apply_language, apply_reduce_motion, file_dialogs_available, spawn_open_path,
    spawn_pick_folder, spawn_pick_manager_file, spawn_pick_manager_save_path, DialogKind,
    FileDialogOutcome,
};
use crate::dialog_windows::write_clipboard;
use crate::host::shell_ipc::HostShellClient;
use crate::host::AppRuntimeClient;
use crate::runtime::AppProjection;

pub(crate) type FileTx = Sender<FileDialogOutcome>;

fn rt_of(client: &Arc<HostShellClient>) -> AppRuntimeClient {
    AppRuntimeClient::remote(Arc::clone(client))
}

fn call(rt: &AppRuntimeClient, method: &str, args: Value) {
    if let Err(error) = rt.call_value(method, args) {
        tracing::warn!(method, error = %error, "remote dialog call failed");
    }
}

fn preview_settings_theme(
    window: &SettingsWindow,
    main: Option<&MainWindow>,
    mode: i32,
    accent: i32,
) {
    crate::bootstrap::apply_app_theme_indices(
        &window.global::<crate::bootstrap::generated_ui::Theme>(),
        mode,
        accent,
    );
    if let Some(main) = main {
        crate::bootstrap::apply_app_theme_indices(
            &main.global::<crate::bootstrap::generated_ui::Theme>(),
            mode,
            accent,
        );
    }
}

/// Cancel / × / Esc: drop the Shell draft by asking Host to reload the saved
/// baseline. That call does not receive the draft, so abandoned L-tier edits
/// are not written. Paint the returned theme immediately.
fn revert_settings_preview(
    rt: &AppRuntimeClient,
    settings: &Weak<SettingsWindow>,
    main: &Option<Weak<MainWindow>>,
) {
    let value = match rt.call_value("close_settings_appearance", json!([])) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(error = %error, "could not close settings");
            return;
        }
    };
    let projection = match crate::host::shell_ipc::projection_from_json(value) {
        Ok(projection) => projection,
        Err(error) => {
            tracing::warn!(error = %error, "could not read the closed settings projection");
            return;
        }
    };
    let mode = projection.settings_appearance_mode_index;
    let accent = projection.settings_appearance_accent_index;
    let main = main.as_ref().and_then(|weak| weak.upgrade());
    if let Some(settings) = settings.upgrade() {
        preview_settings_theme(&settings, main.as_ref(), mode, accent);
    } else if let Some(main) = main.as_ref() {
        crate::bootstrap::apply_app_theme_indices(
            &main.global::<crate::bootstrap::generated_ui::Theme>(),
            mode,
            accent,
        );
    }
}

/// Custom is the last SFTP editor option (`system-default`, detected, `custom`).
fn sftp_editor_is_custom(window: &SettingsWindow, index: i32) -> bool {
    let count = i32::try_from(window.get_settings_sftp_editor_options().row_count()).unwrap_or(0);
    count > 0 && index == count - 1
}

fn reset_terminal_draft(window: &SettingsWindow) {
    window.set_scrollback_lines_text(yshell_terminal::DEFAULT_SCROLLBACK_LINES.to_string().into());
    window.set_scrollback_max_cells_text(
        yshell_terminal::DEFAULT_SCROLLBACK_MAX_CELLS
            .to_string()
            .into(),
    );
    // Dropdown order matches `middle_click_index` / `right_click_index` /
    // `multi_line_paste_index`: Paste, Context menu, Ask. Auto copy defaults off.
    window.set_settings_terminal_middle_click_index(0);
    window.set_settings_terminal_right_click_index(0);
    window.set_settings_terminal_multi_line_paste_index(0);
    window.set_settings_terminal_auto_copy(false);
    window.set_settings_terminal_status_kind_text("defaults-loaded".into());
    window.set_settings_terminal_status_field_text("".into());
    window.set_settings_terminal_status_value_text("".into());
    window.set_settings_terminal_status_limit_text("".into());
}

/// Push the Shell draft into Host, then the caller runs `save_settings_all`.
fn push_settings_draft(rt: &AppRuntimeClient, window: &SettingsWindow) {
    call(
        rt,
        "update_settings_appearance_mode",
        json!([window.get_settings_appearance_mode_index()]),
    );
    call(
        rt,
        "update_settings_appearance_accent",
        json!([window.get_settings_appearance_accent_index()]),
    );
    call(
        rt,
        "select_settings_appearance_style",
        json!([window.get_settings_appearance_style_index()]),
    );
    call(
        rt,
        "update_settings_terminal_middle_click",
        json!([window.get_settings_terminal_middle_click_index()]),
    );
    call(
        rt,
        "update_settings_terminal_right_click",
        json!([window.get_settings_terminal_right_click_index()]),
    );
    call(
        rt,
        "update_settings_terminal_multi_line_paste",
        json!([window.get_settings_terminal_multi_line_paste_index()]),
    );
    call(
        rt,
        "update_settings_terminal_auto_copy",
        json!([window.get_settings_terminal_auto_copy()]),
    );
    call(
        rt,
        "update_settings_scrollback_lines",
        json!([window.get_scrollback_lines_text().as_str()]),
    );
    call(
        rt,
        "update_settings_scrollback_max_cells",
        json!([window.get_scrollback_max_cells_text().as_str()]),
    );
    call(
        rt,
        "update_settings_sftp_show_hidden",
        json!([window.get_settings_sftp_show_hidden()]),
    );
    call(
        rt,
        "update_settings_sftp_conflict_action",
        json!([window.get_settings_sftp_conflict_index()]),
    );
    call(
        rt,
        "update_settings_sftp_external_editor",
        json!([window.get_settings_sftp_editor_index()]),
    );
    call(
        rt,
        "update_settings_sftp_external_editor_path",
        json!([window.get_settings_sftp_editor_path_text().as_str()]),
    );
}

fn apply_settings_validation(window: &SettingsWindow, projection: &AppProjection) {
    window.set_settings_terminal_status_kind_text(
        projection
            .settings_terminal_status_kind_text
            .as_str()
            .into(),
    );
    window.set_settings_terminal_status_field_text(
        projection
            .settings_terminal_status_field_text
            .as_str()
            .into(),
    );
    window.set_settings_terminal_status_value_text(
        projection
            .settings_terminal_status_value_text
            .as_str()
            .into(),
    );
    window.set_settings_terminal_status_limit_text(
        projection
            .settings_terminal_status_limit_text
            .as_str()
            .into(),
    );
    window.set_settings_sftp_status_kind_text(
        projection.settings_sftp_status_kind_text.as_str().into(),
    );
    window.set_appearance_status_text(projection.settings_appearance_status_text.as_str().into());
}

macro_rules! title_drag {
    ($window:expr) => {{
        $window.set_native_title_drag(cfg!(windows));
        let weak = $window.as_weak();
        $window.on_title_drag_requested(move || {
            if let Some(window) = weak.upgrade() {
                crate::win_chrome::start_native_window_move(window.window());
                crate::win_chrome::reset_pointer_state_after_native_move(window.as_weak());
            }
        });
    }};
}

macro_rules! cb0 {
    ($window:expr, $rt:expr, $cb:ident, $method:literal) => {{
        let rt = $rt.clone();
        $window.$cb(move || call(&rt, $method, json!([])));
    }};
}

macro_rules! cb_str {
    ($window:expr, $rt:expr, $cb:ident, $method:literal) => {{
        let rt = $rt.clone();
        $window.$cb(move |value: SharedString| call(&rt, $method, json!([value.as_str()])));
    }};
}

macro_rules! cb_i32 {
    ($window:expr, $rt:expr, $cb:ident, $method:literal) => {{
        let rt = $rt.clone();
        $window.$cb(move |value: i32| call(&rt, $method, json!([value])));
    }};
}

macro_rules! cb_bool {
    ($window:expr, $rt:expr, $cb:ident, $method:literal) => {{
        let rt = $rt.clone();
        $window.$cb(move |value: bool| call(&rt, $method, json!([value])));
    }};
}

macro_rules! on_close {
    ($window:expr, $rt:expr, $method:literal) => {{
        let rt = $rt.clone();
        $window.window().on_close_requested(move || {
            call(&rt, $method, json!([]));
            CloseRequestResponse::HideWindow
        });
    }};
}

fn pick_file(tx: &FileTx, kind: DialogKind, title: &str) {
    spawn_pick_manager_file(tx.clone(), kind, title);
}

fn save_name(path: &str, fallback: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return fallback.to_owned();
    }
    let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).trim();
    if name.is_empty() {
        fallback.to_owned()
    } else {
        name.to_owned()
    }
}

fn copy_public_key(rt: &AppRuntimeClient, method: &str) {
    match rt.call_value(method, json!([])) {
        Ok(Value::String(text)) => {
            let copied = write_clipboard(&text);
            let chars = text.chars().count().to_string();
            if copied {
                call(
                    rt,
                    "set_private_keys_status_feedback",
                    json!([
                        "private-key-public-copied",
                        format!("Copied the derived public key ({chars} chars) to the clipboard."),
                        chars,
                        ""
                    ]),
                );
            } else {
                call(
                    rt,
                    "set_private_keys_status_feedback",
                    json!([
                        "private-key-clipboard-unavailable",
                        "Could not access the system clipboard; select the key text manually.",
                        "",
                        ""
                    ]),
                );
            }
        }
        Ok(_) => {}
        Err(error) => {
            let message = error.to_string();
            call(
                rt,
                "set_private_keys_status_feedback",
                json!([
                    "private-key-copy-failed",
                    format!("Copy failed: {message}"),
                    message,
                    ""
                ]),
            );
        }
    }
}

pub(crate) fn wire_unlock(window: &MasterPasswordUnlockWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    window.on_update_password({
        let rt = rt.clone();
        move |text: SharedString| {
            call(
                &rt,
                "update_master_password_unlock_password",
                json!([text.as_str()]),
            )
        }
    });
    window.on_unlock({
        let rt = rt.clone();
        let client = Arc::clone(&client);
        move || {
            call(&rt, "submit_master_password_unlock", json!([]));
            let _ = client.host_tick();
        }
    });
    window.on_forgot({
        let rt = rt.clone();
        move || {
            call(&rt, "forgot_master_password", json!([]));
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
    window.on_quit({
        let rt = rt.clone();
        let client = Arc::clone(&client);
        move || {
            call(&rt, "dismiss_master_password_unlock", json!([]));
            let _ = client.set_window_count(0);
            let _ = client.detach();
            let _ = slint::quit_event_loop();
        }
    });
    window.window().on_close_requested({
        let rt = rt.clone();
        let client = Arc::clone(&client);
        move || {
            call(&rt, "dismiss_master_password_unlock", json!([]));
            let _ = client.set_window_count(0);
            let _ = client.detach();
            let _ = slint::quit_event_loop();
            CloseRequestResponse::HideWindow
        }
    });
}

pub(crate) fn wire_master_password(window: &MasterPasswordWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    window.on_update_current({
        let rt = rt.clone();
        move |text: SharedString| {
            call(
                &rt,
                "update_master_password_field",
                json!(["current", text.as_str()]),
            );
        }
    });
    window.on_update_new({
        let rt = rt.clone();
        move |text: SharedString| {
            call(
                &rt,
                "update_master_password_field",
                json!(["new", text.as_str()]),
            );
        }
    });
    window.on_update_confirm({
        let rt = rt.clone();
        move |text: SharedString| {
            call(
                &rt,
                "update_master_password_field",
                json!(["confirm", text.as_str()]),
            );
        }
    });
    cb0!(window, rt, on_submit, "submit_master_password_dialog");
    cb0!(window, rt, on_cancel, "close_master_password_dialog");
    cb0!(window, rt, on_dismissed, "close_master_password_dialog");
    on_close!(window, rt, "close_master_password_dialog");
}

pub(crate) fn wire_password(window: &PasswordPromptWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_cancel, "cancel_password_prompt");
    let weak = window.as_weak();
    window.on_submit({
        let rt = rt.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                let password = window.get_password_text();
                call(&rt, "submit_password", json!([password.as_str()]));
            }
        }
    });
    on_close!(window, rt, "cancel_password_prompt");
}

pub(crate) fn wire_host_key(window: &HostKeyWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_cancel, "cancel_host_key_prompt");
    cb0!(window, rt, on_trust_once, "trust_host_key_once");
    cb0!(window, rt, on_trust_and_save, "trust_host_key_and_save");
    let weak = window.as_weak();
    window.on_copy_details({
        let rt = rt.clone();
        move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let text = format!(
                "Target: {}\nMode: {}\nKnown hosts: {}\nExpected: {} {}\nPresented: {} {}\n",
                window.get_target_text(),
                window.get_mode_text(),
                window.get_known_hosts_path_text(),
                window.get_expected_algorithm_text(),
                window.get_expected_fingerprint_text(),
                window.get_algorithm_text(),
                window.get_fingerprint_text(),
            );
            let copied = write_clipboard(&text);
            let chars = text.chars().count().to_string();
            if copied {
                call(
                    &rt,
                    "set_status_kind",
                    json!([
                        "host-key-details-copied",
                        format!("Copied the host key details ({chars} chars) to the clipboard."),
                        chars,
                        ""
                    ]),
                );
            } else {
                call(
                    &rt,
                    "set_status_kind",
                    json!([
                        "host-key-clipboard-unavailable",
                        "Could not access the system clipboard; select the details manually.",
                        "",
                        ""
                    ]),
                );
            }
        }
    });
    on_close!(window, rt, "cancel_host_key_prompt");
}

pub(crate) fn wire_auth(window: &AuthPromptWindow, client: Arc<HostShellClient>, tx: FileTx) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_cancel, "cancel_auth_prompt");
    cb_i32!(window, rt, on_select_method, "select_auth_prompt_method");
    cb0!(window, rt, on_submit_password, "submit_auth_prompt");
    cb_str!(
        window,
        rt,
        on_update_password,
        "update_auth_prompt_password"
    );
    cb_i32!(window, rt, on_select_key, "select_auth_prompt_key");
    cb_str!(
        window,
        rt,
        on_update_passphrase,
        "update_auth_prompt_passphrase"
    );
    cb_bool!(
        window,
        rt,
        on_toggle_use_agent,
        "toggle_auth_prompt_use_agent"
    );
    window.on_browse_key_file({
        let tx = tx.clone();
        move || pick_file(&tx, DialogKind::AuthKeyFile, "Select private key file")
    });
    window.on_update_challenge_answer({
        let rt = rt.clone();
        move |index, value: SharedString| {
            call(
                &rt,
                "update_auth_prompt_keyboard_answer",
                json!([index, value.as_str()]),
            );
        }
    });
    cb0!(
        window,
        rt,
        on_submit_keyboard_round,
        "submit_auth_prompt_keyboard_round"
    );
    on_close!(window, rt, "cancel_auth_prompt");
}

pub(crate) fn wire_settings(
    window: &SettingsWindow,
    client: Arc<HostShellClient>,
    tx: FileTx,
    reduce_motion: Rc<Cell<bool>>,
    main: Option<Weak<MainWindow>>,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    let settings_weak = window.as_weak();
    window.on_dismiss({
        let rt = rt.clone();
        let settings_weak = settings_weak.clone();
        let main = main.clone();
        move || revert_settings_preview(&rt, &settings_weak, &main)
    });
    window.window().on_close_requested({
        let rt = rt.clone();
        let main = main.clone();
        move || {
            revert_settings_preview(&rt, &settings_weak, &main);
            CloseRequestResponse::HideWindow
        }
    });
    cb0!(window, rt, on_open_host_keys, "open_host_keys_manager");
    cb0!(
        window,
        rt,
        on_open_private_keys,
        "open_private_keys_manager"
    );
    cb0!(window, rt, on_open_proxies, "open_proxies_manager");
    cb0!(
        window,
        rt,
        on_open_theme_manager,
        "open_settings_terminal_styles_manager"
    );
    cb_bool!(
        window,
        rt,
        on_set_confirm_close_session,
        "set_confirm_close_session"
    );
    cb_bool!(window, rt, on_set_confirm_quit_app, "set_confirm_quit_app");
    cb_bool!(window, rt, on_set_mcp_enabled, "set_mcp_enabled");
    cb_str!(window, rt, on_update_mcp_port, "update_mcp_port");
    cb_bool!(window, rt, on_set_mcp_listen_lan, "set_mcp_listen_lan");
    cb_str!(window, rt, on_update_mcp_api_key, "update_mcp_api_key");
    cb0!(window, rt, on_generate_mcp_api_key, "generate_mcp_api_key");
    let weak_key = window.as_weak();
    window.on_copy_mcp_api_key(move || {
        if let Some(window) = weak_key.upgrade() {
            let key = window.get_mcp_api_key_text();
            if !key.is_empty() {
                let _ = write_clipboard(key.as_str());
            }
        }
    });
    cb_bool!(
        window,
        rt,
        on_set_mcp_view_user_connections,
        "set_mcp_view_user_connections"
    );
    cb_bool!(
        window,
        rt,
        on_set_mcp_control_user_connections,
        "set_mcp_control_user_connections"
    );
    cb_bool!(
        window,
        rt,
        on_set_mcp_auto_approve_control,
        "set_mcp_auto_approve_control"
    );
    // L: appearance / terminal / SFTP stay on the window until Save.
    // Mode and accent preview on this window and the main window. The
    // terminal-style bitmap is rendered on the Host, so it stays at the
    // seeded frame until Save.
    let weak_mode = window.as_weak();
    let main_mode = main.clone();
    window.on_update_settings_appearance_mode(move |index| {
        if let Some(window) = weak_mode.upgrade() {
            window.set_settings_appearance_mode_index(index);
            preview_settings_theme(
                &window,
                main_mode.as_ref().and_then(|weak| weak.upgrade()).as_ref(),
                index,
                window.get_settings_appearance_accent_index(),
            );
        }
    });
    let weak_accent = window.as_weak();
    let main_accent = main.clone();
    window.on_update_settings_appearance_accent(move |index| {
        if let Some(window) = weak_accent.upgrade() {
            window.set_settings_appearance_accent_index(index);
            preview_settings_theme(
                &window,
                main_accent
                    .as_ref()
                    .and_then(|weak| weak.upgrade())
                    .as_ref(),
                window.get_settings_appearance_mode_index(),
                index,
            );
        }
    });
    let weak_style = window.as_weak();
    window.on_select_settings_appearance_style(move |index| {
        if let Some(window) = weak_style.upgrade() {
            window.set_settings_appearance_style_index(index);
        }
    });
    let weak_middle = window.as_weak();
    window.on_update_settings_terminal_middle_click(move |index| {
        if let Some(window) = weak_middle.upgrade() {
            window.set_settings_terminal_middle_click_index(index);
        }
    });
    let weak_right = window.as_weak();
    window.on_update_settings_terminal_right_click(move |index| {
        if let Some(window) = weak_right.upgrade() {
            window.set_settings_terminal_right_click_index(index);
        }
    });
    let weak_paste = window.as_weak();
    window.on_update_settings_terminal_multi_line_paste(move |index| {
        if let Some(window) = weak_paste.upgrade() {
            window.set_settings_terminal_multi_line_paste_index(index);
        }
    });
    let weak_copy = window.as_weak();
    window.on_update_settings_terminal_auto_copy(move |enabled| {
        if let Some(window) = weak_copy.upgrade() {
            window.set_settings_terminal_auto_copy(enabled);
        }
    });
    let weak_lines = window.as_weak();
    window.on_update_scrollback_lines(move |value: SharedString| {
        if let Some(window) = weak_lines.upgrade() {
            window.set_scrollback_lines_text(value);
        }
    });
    let weak_cells = window.as_weak();
    window.on_update_scrollback_max_cells(move |value: SharedString| {
        if let Some(window) = weak_cells.upgrade() {
            window.set_scrollback_max_cells_text(value);
        }
    });
    let weak_reset = window.as_weak();
    window.on_reset_terminal_defaults(move || {
        if let Some(window) = weak_reset.upgrade() {
            reset_terminal_draft(&window);
        }
    });
    let weak_hidden = window.as_weak();
    window.on_update_settings_sftp_show_hidden(move |show_hidden| {
        if let Some(window) = weak_hidden.upgrade() {
            window.set_settings_sftp_show_hidden(show_hidden);
        }
    });
    let weak_conflict = window.as_weak();
    window.on_update_settings_sftp_conflict_action(move |index| {
        if let Some(window) = weak_conflict.upgrade() {
            window.set_settings_sftp_conflict_index(index);
        }
    });
    let weak_editor = window.as_weak();
    window.on_update_settings_sftp_external_editor(move |index| {
        if let Some(window) = weak_editor.upgrade() {
            window.set_settings_sftp_editor_index(index);
            window.set_settings_sftp_editor_custom_visible(sftp_editor_is_custom(&window, index));
        }
    });
    let weak_path = window.as_weak();
    window.on_update_settings_sftp_external_editor_path(move |value: SharedString| {
        if let Some(window) = weak_path.upgrade() {
            window.set_settings_sftp_editor_path_text(value);
        }
    });
    window.on_browse_settings_sftp_external_editor({
        let tx = tx.clone();
        move || pick_file(&tx, DialogKind::SettingsSftpEditor, "External editor")
    });
    cb_str!(
        window,
        rt,
        on_open_master_password_dialog,
        "open_master_password_dialog"
    );
    let weak_dir = window.as_weak();
    window.on_open_config_dir(move || {
        if let Some(window) = weak_dir.upgrade() {
            spawn_open_path(window.get_config_dir_text().to_string());
        }
    });
    let weak_save = window.as_weak();
    window.on_save_settings({
        let rt = rt.clone();
        move || {
            let Some(window) = weak_save.upgrade() else {
                return;
            };
            push_settings_draft(&rt, &window);
            match rt.call_value("save_settings_all", json!([])) {
                Ok(value) => match crate::host::shell_ipc::projection_from_json(value) {
                    Ok(projection) if projection.settings_appearance_visible => {
                        apply_settings_validation(&window, &projection);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "settings save returned an unreadable projection"
                        );
                    }
                },
                Err(error) => tracing::warn!(error = %error, "could not save settings"),
            }
        }
    });
    window.on_set_language({
        let rt = rt.clone();
        move |value: SharedString| match rt.call_value("set_ui_language", json!([value.as_str()])) {
            Ok(value) => {
                if let Some(lang) = value.get("settings_language_text").and_then(|v| v.as_str()) {
                    apply_language(lang);
                }
            }
            Err(error) => tracing::warn!(error = %error, "could not save the language setting"),
        }
    });
    cb_bool!(window, rt, on_set_debug_logging, "set_debug_logging");
    let weak_log = window.as_weak();
    window.on_open_debug_log_dir({
        let rt = rt.clone();
        move || {
            // `ensure_debug_log_dir` is not in runtime_dispatch. Open the path
            // already projected onto the window; try the Host call first in
            // case it lands later and returns a string.
            if let Ok(Value::String(dir)) = rt.call_value("ensure_debug_log_dir", json!([])) {
                if !dir.is_empty() {
                    spawn_open_path(dir);
                    return;
                }
            }
            if let Some(window) = weak_log.upgrade() {
                let dir = window.get_debug_log_dir_text().to_string();
                if !dir.is_empty() {
                    spawn_open_path(dir);
                }
            }
        }
    });
    let weak_motion = window.as_weak();
    window.on_set_reduce_motion({
        let rt = rt.clone();
        let reduce_motion = Rc::clone(&reduce_motion);
        move |enabled| {
            call(&rt, "set_reduce_motion", json!([enabled]));
            reduce_motion.set(enabled);
            if let Some(window) = weak_motion.upgrade() {
                apply_reduce_motion(
                    &window.global::<crate::bootstrap::generated_ui::Theme>(),
                    enabled,
                );
            }
            if let Some(main) = main.as_ref().and_then(|weak| weak.upgrade()) {
                apply_reduce_motion(
                    &main.global::<crate::bootstrap::generated_ui::Theme>(),
                    enabled,
                );
            }
        }
    });
}

pub(crate) fn wire_session_editor(
    window: &SessionEditorWindow,
    client: Arc<HostShellClient>,
    tx: FileTx,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_dismiss, "request_close_session_editor_modal");
    on_close!(window, rt, "request_close_session_editor_modal");
    cb0!(
        window,
        rt,
        on_select_general,
        "select_session_editor_general"
    );
    cb0!(
        window,
        rt,
        on_select_authentication,
        "select_session_editor_authentication"
    );
    cb0!(
        window,
        rt,
        on_select_login_script,
        "select_session_editor_login_script"
    );
    cb0!(window, rt, on_select_proxy, "select_session_editor_proxy");
    cb0!(
        window,
        rt,
        on_select_tunnels,
        "select_session_editor_tunnels"
    );
    cb0!(
        window,
        rt,
        on_select_terminal_style,
        "select_session_editor_appearance"
    );
    cb_bool!(
        window,
        rt,
        on_toggle_inherit_auth,
        "set_editor_inherit_auth"
    );
    cb_bool!(
        window,
        rt,
        on_toggle_inherit_login_script,
        "set_editor_inherit_login_script"
    );
    cb_bool!(
        window,
        rt,
        on_toggle_inherit_proxy,
        "set_editor_inherit_proxy"
    );
    cb_bool!(
        window,
        rt,
        on_toggle_inherit_tunnels,
        "set_editor_inherit_tunnels"
    );
    cb_bool!(
        window,
        rt,
        on_toggle_inherit_terminal_style,
        "set_editor_inherit_terminal_style"
    );
    cb_str!(window, rt, on_update_name, "update_editor_name");
    cb_str!(window, rt, on_update_host, "update_editor_host");
    cb_str!(window, rt, on_update_port, "update_editor_port");
    cb_str!(window, rt, on_update_username, "update_editor_username");
    cb_i32!(window, rt, on_update_icon_index, "update_editor_icon_index");
    window.on_browse_icon({
        let tx = tx.clone();
        move || pick_file(&tx, DialogKind::SessionIcon, "Session icon")
    });
    cb0!(window, rt, on_reset_icon, "reset_editor_icon");
    cb_str!(window, rt, on_update_notes, "update_editor_notes");
    cb0!(
        window,
        rt,
        on_set_auth_method_password,
        "set_editor_auth_method_password"
    );
    cb0!(
        window,
        rt,
        on_set_auth_method_private_key,
        "set_editor_auth_method_private_key"
    );
    cb0!(
        window,
        rt,
        on_set_auth_method_keyboard_interactive,
        "set_editor_auth_method_keyboard_interactive"
    );
    cb_str!(window, rt, on_update_password, "update_editor_password");
    cb_str!(window, rt, on_update_passphrase, "update_editor_passphrase");
    cb_i32!(window, rt, on_select_key, "select_editor_key");
    cb0!(window, rt, on_manage_keys, "open_private_keys_manager");
    cb_str!(window, rt, on_add_login_step, "add_editor_login_step");
    cb_i32!(window, rt, on_edit_login_step, "edit_editor_login_step");
    cb_i32!(window, rt, on_select_login_step, "select_editor_login_step");
    cb_i32!(
        window,
        rt,
        on_duplicate_login_step,
        "duplicate_editor_login_step"
    );
    cb_i32!(window, rt, on_delete_login_step, "delete_editor_login_step");
    window.on_move_login_step({
        let rt = rt.clone();
        move |from: i32, to: i32| call(&rt, "move_editor_login_step", json!([from, to]))
    });
    cb0!(window, rt, on_add_tunnel, "begin_add_editor_tunnel");
    cb_i32!(window, rt, on_edit_tunnel, "begin_edit_editor_tunnel");
    cb_i32!(window, rt, on_select_tunnel, "select_editor_tunnel");
    cb_i32!(window, rt, on_duplicate_tunnel, "duplicate_editor_tunnel");
    cb_i32!(window, rt, on_delete_tunnel, "delete_editor_tunnel");
    cb0!(window, rt, on_clear_tunnels, "clear_editor_tunnels");
    cb0!(
        window,
        rt,
        on_set_proxy_mode_none,
        "set_editor_proxy_mode_none"
    );
    cb_i32!(window, rt, on_select_proxy_profile, "select_editor_proxy");
    cb0!(window, rt, on_manage_proxies, "open_proxies_manager");
    cb_i32!(window, rt, on_select_scheme, "select_editor_scheme_index");
    cb0!(window, rt, on_manage_terminal_styles, "open_theme_manager");
    cb0!(window, rt, on_test_auth, "test_editor_auth");
    cb0!(window, rt, on_save, "save_editor_to_saved_session");
    cb0!(window, rt, on_save_connect, "save_editor_and_connect");
}

pub(crate) fn wire_login_step_keys(window: &LoginStepKeysWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_dismiss, "cancel_editor_login_step");
    on_close!(window, rt, "cancel_editor_login_step");
    cb0!(window, rt, on_save, "save_editor_login_step");
    cb0!(
        window,
        rt,
        on_delete_step,
        "delete_selected_editor_login_step"
    );
    cb_str!(window, rt, on_update_name, "update_editor_login_step_name");
    cb_str!(window, rt, on_update_keys, "update_editor_login_step_keys");
    cb_bool!(window, rt, on_set_tokens, "set_editor_login_step_tokens");
}

pub(crate) fn wire_login_step_wait(window: &LoginStepWaitWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_dismiss, "cancel_editor_login_step");
    on_close!(window, rt, "cancel_editor_login_step");
    cb0!(window, rt, on_save, "save_editor_login_step");
    cb0!(
        window,
        rt,
        on_delete_step,
        "delete_selected_editor_login_step"
    );
    cb_str!(
        window,
        rt,
        on_update_millis,
        "update_editor_login_step_millis"
    );
}

pub(crate) fn wire_tunnel_editor(window: &TunnelEditorWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_dismiss, "cancel_editor_tunnel_dialog");
    on_close!(window, rt, "cancel_editor_tunnel_dialog");
    cb0!(window, rt, on_save, "save_editor_tunnel_dialog");
    cb_str!(window, rt, on_update_name, "update_editor_tunnel_name");
    cb_str!(window, rt, on_set_kind, "set_editor_tunnel_kind");
    cb_str!(
        window,
        rt,
        on_update_bind_host,
        "update_editor_tunnel_dialog_bind_host"
    );
    cb_str!(
        window,
        rt,
        on_update_bind_port,
        "update_editor_tunnel_dialog_bind_port"
    );
    cb_str!(
        window,
        rt,
        on_update_target_host,
        "update_editor_tunnel_dialog_target_host"
    );
    cb_str!(
        window,
        rt,
        on_update_target_port,
        "update_editor_tunnel_dialog_target_port"
    );
    cb_bool!(window, rt, on_set_enabled, "set_editor_tunnel_enabled");
}

pub(crate) fn wire_folder_editor(window: &FolderEditorWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_dismiss, "close_folder_editor");
    on_close!(window, rt, "close_folder_editor");
    cb0!(window, rt, on_save, "save_folder_editor");
    cb0!(window, rt, on_reset_all, "reset_all_folder_editor");
    window.on_field_set_inherit({
        let rt = rt.clone();
        move |key: SharedString| {
            call(
                &rt,
                "apply_folder_editor_field_action",
                json!([key.as_str(), "inherit"]),
            );
        }
    });
    window.on_field_set_explicit({
        let rt = rt.clone();
        move |key: SharedString| {
            call(
                &rt,
                "apply_folder_editor_field_action",
                json!([key.as_str(), "explicit"]),
            );
        }
    });
    window.on_field_reset({
        let rt = rt.clone();
        move |key: SharedString| {
            call(
                &rt,
                "apply_folder_editor_field_action",
                json!([key.as_str(), "reset"]),
            );
        }
    });
    window.on_field_edited({
        let rt = rt.clone();
        move |key: SharedString, value: SharedString| {
            call(
                &rt,
                "update_folder_editor_field",
                json!([key.as_str(), value.as_str()]),
            );
        }
    });
    window.on_field_toggled({
        let rt = rt.clone();
        move |key: SharedString| call(&rt, "folder_editor_field_toggled", json!([key.as_str()]))
    });
    window.on_field_selected({
        let rt = rt.clone();
        move |key: SharedString, index| {
            call(
                &rt,
                "folder_editor_field_selected",
                json!([key.as_str(), index]),
            );
        }
    });
}

pub(crate) fn wire_private_keys(window: &PrivateKeysWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_dismiss, "close_private_keys_manager");
    on_close!(window, rt, "close_private_keys_manager");
    cb_i32!(window, rt, on_select_key, "select_private_key");
    window.on_copy_public_key({
        let rt = rt.clone();
        move || copy_public_key(&rt, "selected_private_key_public_line")
    });
    cb0!(
        window,
        rt,
        on_save_passphrase,
        "save_selected_private_key_passphrase"
    );
    cb0!(
        window,
        rt,
        on_forget_passphrase,
        "forget_selected_private_key_passphrase"
    );
    cb_str!(
        window,
        rt,
        on_update_passphrase,
        "update_private_keys_passphrase_input"
    );
    cb_str!(window, rt, on_rename_key, "rename_selected_private_key");
    cb0!(
        window,
        rt,
        on_open_generate_dialog,
        "open_private_keys_generate_dialog"
    );
    cb0!(
        window,
        rt,
        on_open_import_dialog,
        "open_private_keys_import_dialog"
    );
    cb_i32!(
        window,
        rt,
        on_prepare_context_menu,
        "prepare_private_key_context_menu"
    );
    cb0!(window, rt, on_test_from_menu, "test_private_key_from_menu");
    cb0!(
        window,
        rt,
        on_export_from_menu,
        "export_private_key_from_menu"
    );
    cb0!(
        window,
        rt,
        on_deploy_from_menu,
        "deploy_private_key_from_menu"
    );
    cb0!(
        window,
        rt,
        on_remove_from_menu,
        "remove_private_key_from_menu"
    );
}

pub(crate) fn wire_proxies(window: &ProxiesWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_dismiss, "close_proxies_manager");
    on_close!(window, rt, "close_proxies_manager");
    window.on_select_proxy({
        let rt = rt.clone();
        move |index| {
            call(&rt, "select_proxy", json!([index]));
            call(&rt, "begin_edit_proxy", json!([]));
        }
    });
    cb0!(window, rt, on_start_new_proxy, "start_new_proxy");
    cb0!(window, rt, on_save_proxy_editor, "save_proxy_editor");
    cb0!(window, rt, on_cancel_proxy_editor, "cancel_proxy_editor");
    cb_str!(window, rt, on_update_proxy_name, "update_proxy_editor_name");
    cb_str!(window, rt, on_update_proxy_host, "update_proxy_editor_host");
    cb_str!(window, rt, on_update_proxy_port, "update_proxy_editor_port");
    cb_str!(
        window,
        rt,
        on_update_proxy_username,
        "update_proxy_editor_username"
    );
    cb_str!(
        window,
        rt,
        on_update_proxy_password,
        "update_proxy_editor_password"
    );
    cb_i32!(
        window,
        rt,
        on_select_proxy_protocol,
        "select_proxy_editor_protocol"
    );
    cb0!(window, rt, on_toggle_proxy_dns, "toggle_proxy_editor_dns");
    cb0!(window, rt, on_test_proxy, "test_selected_proxy");
    cb0!(
        window,
        rt,
        on_clear_proxy_references,
        "set_referencing_sessions_to_no_proxy"
    );
    cb0!(
        window,
        rt,
        on_dismiss_proxy_warning,
        "dismiss_proxy_remove_warning"
    );
    cb_i32!(
        window,
        rt,
        on_prepare_context_menu,
        "prepare_proxy_context_menu"
    );
    cb0!(
        window,
        rt,
        on_duplicate_from_menu,
        "duplicate_proxy_from_menu"
    );
    cb0!(window, rt, on_test_from_menu, "test_proxy_from_menu");
    cb0!(window, rt, on_remove_from_menu, "remove_proxy_from_menu");
}

pub(crate) fn wire_theme_manager(
    window: &ThemeManagerWindow,
    client: Arc<HostShellClient>,
    tx: FileTx,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_dismiss, "close_theme_manager");
    on_close!(window, rt, "close_theme_manager");
    cb_i32!(window, rt, on_select_theme, "select_theme_manager_row");
    cb0!(
        window,
        rt,
        on_set_default_theme,
        "apply_theme_manager_selection"
    );
    cb0!(window, rt, on_new_style, "clone_selected_color_theme");
    cb_str!(window, rt, on_clone_theme, "clone_color_theme");
    cb_str!(window, rt, on_edit_theme, "open_theme_editor");
    cb_str!(
        window,
        rt,
        on_request_delete_theme,
        "request_delete_color_theme"
    );
    cb0!(window, rt, on_close_theme_editor, "close_theme_editor");
    cb0!(
        window,
        rt,
        on_save_theme_editor,
        "save_theme_editor_with_appearance"
    );
    cb_str!(
        window,
        rt,
        on_update_theme_editor_name,
        "update_theme_editor_name"
    );
    window.on_update_theme_editor_role({
        let rt = rt.clone();
        move |role: SharedString, hex: SharedString| {
            call(
                &rt,
                "update_theme_editor_role_color",
                json!([role.as_str(), hex.as_str()]),
            );
        }
    });
    window.on_update_theme_editor_ansi({
        let rt = rt.clone();
        move |index, hex: SharedString| {
            call(
                &rt,
                "update_theme_editor_ansi_color",
                json!([index, hex.as_str()]),
            );
        }
    });
    cb_i32!(
        window,
        rt,
        on_prepare_context_menu,
        "select_theme_manager_row"
    );
    cb0!(window, rt, on_clone_from_menu, "clone_selected_color_theme");
    cb0!(
        window,
        rt,
        on_delete_from_menu,
        "request_delete_selected_color_theme"
    );
    cb_str!(
        window,
        rt,
        on_update_font_family,
        "update_settings_appearance_font_family"
    );
    cb_str!(
        window,
        rt,
        on_update_font_size,
        "update_settings_appearance_font_size"
    );
    cb_str!(
        window,
        rt,
        on_update_backup_font,
        "update_settings_appearance_backup_font"
    );
    window.on_browse_font_family_file({
        let tx = tx.clone();
        move || pick_file(&tx, DialogKind::TerminalFontPrimary, "Add font file")
    });
    window.on_browse_font_fallback_file({
        let tx = tx.clone();
        move || {
            pick_file(
                &tx,
                DialogKind::TerminalFontBackup,
                "Add fallback font file",
            )
        }
    });
}

pub(crate) fn wire_host_keys(
    window: &HostKeysWindow,
    client: Arc<HostShellClient>,
    clear: Rc<Cell<bool>>,
    remove: Rc<Cell<bool>>,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_dismiss, "close_host_keys_manager");
    on_close!(window, rt, "close_host_keys_manager");
    window.on_select_entry({
        let rt = rt.clone();
        move |group: i32, entry: i32| call(&rt, "select_host_key", json!([group, entry]))
    });
    cb0!(window, rt, on_open_add_dialog, "open_host_keys_add_dialog");
    cb0!(
        window,
        rt,
        on_open_import_dialog,
        "open_host_keys_import_dialog"
    );
    cb0!(
        window,
        rt,
        on_open_export_dialog,
        "open_host_keys_export_dialog"
    );
    window.on_request_edit({
        let rt = rt.clone();
        move |group: i32, entry: i32| call(&rt, "open_host_keys_edit_dialog", json!([group, entry]))
    });
    window.on_request_remove_at({
        let rt = rt.clone();
        let remove = Rc::clone(&remove);
        move |group, entry| {
            call(&rt, "prepare_host_keys_context_menu", json!([group, entry]));
            remove.set(true);
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
    window.on_request_remove({
        let rt = rt.clone();
        let remove = Rc::clone(&remove);
        move || {
            remove.set(true);
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
    window.on_request_clear_all({
        let rt = rt.clone();
        let clear = Rc::clone(&clear);
        move || {
            clear.set(true);
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
}

pub(crate) fn wire_known_host_add(window: &KnownHostAddWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_canceled, "cancel_host_keys_add_dialog");
    on_close!(window, rt, "cancel_host_keys_add_dialog");
    cb_str!(window, rt, on_update_text, "update_host_keys_add_text");
    cb0!(window, rt, on_submitted, "submit_host_keys_add_dialog");
}

pub(crate) fn wire_known_host_import(
    window: &KnownHostImportWindow,
    client: Arc<HostShellClient>,
    tx: FileTx,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_canceled, "close_host_keys_import_dialog");
    on_close!(window, rt, "close_host_keys_import_dialog");
    cb_str!(window, rt, on_update_path, "update_host_keys_import_path");
    window.on_browse_file({
        let tx = tx.clone();
        move || pick_file(&tx, DialogKind::HostKeysImport, "Import known_hosts file")
    });
    cb0!(window, rt, on_imported, "import_host_keys_from_dialog");
}

pub(crate) fn wire_known_host_export(
    window: &KnownHostExportWindow,
    client: Arc<HostShellClient>,
    tx: FileTx,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_canceled, "close_host_keys_export_dialog");
    on_close!(window, rt, "close_host_keys_export_dialog");
    cb_str!(window, rt, on_update_path, "update_host_keys_export_path");
    let weak = window.as_weak();
    window.on_browse_file({
        let tx = tx.clone();
        move || {
            let file_name = weak
                .upgrade()
                .map(|window| save_name(window.get_path_text().as_str(), "known_hosts_export"))
                .unwrap_or_else(|| "known_hosts_export".to_owned());
            spawn_pick_manager_save_path(
                tx.clone(),
                DialogKind::HostKeysExport,
                "Export known hosts",
                &file_name,
            );
        }
    });
    cb0!(window, rt, on_exported, "export_host_keys_to_path");
}

pub(crate) fn wire_private_key_generate(
    window: &PrivateKeyGenerateWindow,
    client: Arc<HostShellClient>,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(
        window,
        rt,
        on_canceled,
        "close_private_keys_generate_dialog"
    );
    let weak = window.as_weak();
    window.window().on_close_requested({
        let rt = rt.clone();
        move || {
            if weak
                .upgrade()
                .is_some_and(|window| window.get_result_visible())
            {
                call(&rt, "finish_private_keys_generate_dialog", json!([]));
            } else {
                call(&rt, "close_private_keys_generate_dialog", json!([]));
            }
            CloseRequestResponse::HideWindow
        }
    });
    cb_str!(
        window,
        rt,
        on_update_label,
        "update_private_keys_generate_label"
    );
    cb_str!(
        window,
        rt,
        on_update_comment,
        "update_private_keys_generate_comment"
    );
    cb_str!(
        window,
        rt,
        on_update_passphrase,
        "update_private_keys_generate_passphrase"
    );
    cb_str!(
        window,
        rt,
        on_update_confirm,
        "update_private_keys_generate_confirm"
    );
    cb_i32!(
        window,
        rt,
        on_select_algorithm,
        "select_private_keys_generate_algorithm"
    );
    cb0!(window, rt, on_submitted, "generate_private_key_from_form");
    cb0!(window, rt, on_done, "finish_private_keys_generate_dialog");
    window.on_copy_public_key({
        let rt = rt.clone();
        move || copy_public_key(&rt, "generated_private_key_public_line")
    });
}

pub(crate) fn wire_private_key_import(
    window: &PrivateKeyImportWindow,
    client: Arc<HostShellClient>,
    tx: FileTx,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_canceled, "close_private_keys_import_dialog");
    on_close!(window, rt, "close_private_keys_import_dialog");
    cb_i32!(window, rt, on_select_tab, "select_private_keys_import_tab");
    cb_str!(
        window,
        rt,
        on_update_label,
        "update_private_keys_import_label"
    );
    cb_str!(
        window,
        rt,
        on_update_path,
        "update_private_keys_import_path"
    );
    cb_str!(
        window,
        rt,
        on_update_passphrase,
        "update_private_keys_import_passphrase"
    );
    cb_str!(
        window,
        rt,
        on_update_paste,
        "update_private_keys_import_paste_text"
    );
    window.on_browse_file({
        let tx = tx.clone();
        move || pick_file(&tx, DialogKind::PrivateKeyImport, "Select private key file")
    });
    cb0!(window, rt, on_imported, "import_private_key_from_dialog");
}

pub(crate) fn wire_private_key_export(
    window: &PrivateKeyExportWindow,
    client: Arc<HostShellClient>,
    tx: FileTx,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_canceled, "close_private_keys_export_dialog");
    on_close!(window, rt, "close_private_keys_export_dialog");
    cb_str!(
        window,
        rt,
        on_update_path,
        "update_private_keys_export_path"
    );
    let weak = window.as_weak();
    window.on_browse_file({
        let tx = tx.clone();
        move || {
            let file_name = weak
                .upgrade()
                .map(|window| save_name(window.get_path_text().as_str(), "public_key.pub"))
                .unwrap_or_else(|| "public_key.pub".to_owned());
            spawn_pick_manager_save_path(
                tx.clone(),
                DialogKind::PublicKeyExport,
                "Export public key",
                &file_name,
            );
        }
    });
    cb0!(window, rt, on_exported, "export_private_key_to_path");
}

pub(crate) fn wire_private_key_deploy(
    window: &PrivateKeyDeployWindow,
    client: Arc<HostShellClient>,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_canceled, "close_private_keys_deploy_dialog");
    on_close!(window, rt, "close_private_keys_deploy_dialog");
    cb_i32!(
        window,
        rt,
        on_select_target,
        "select_private_key_deploy_target"
    );
    cb0!(window, rt, on_deployed, "confirm_private_key_deploy");
}

pub(crate) fn wire_logging(window: &LoggingWindow, client: Arc<HostShellClient>, tx: FileTx) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_cancel, "close_logging_dialog");
    on_close!(window, rt, "close_logging_dialog");
    cb_str!(
        window,
        rt,
        on_directory_changed,
        "update_logging_dialog_directory"
    );
    cb_str!(
        window,
        rt,
        on_file_name_changed,
        "update_logging_dialog_file_name"
    );
    window.on_options_changed({
        let rt = rt.clone();
        move |raw, timestamps, include_input, input_confirmed, overwrite_confirmed| {
            call(
                &rt,
                "set_logging_dialog_options",
                json!([
                    raw,
                    timestamps,
                    include_input,
                    input_confirmed,
                    overwrite_confirmed
                ]),
            );
        }
    });
    cb0!(window, rt, on_start_requested, "start_logging_from_dialog");
    cb0!(window, rt, on_stop_requested, "stop_logging_from_dialog");
    let weak = window.as_weak();
    window.on_browse_requested({
        let rt = rt.clone();
        let tx = tx.clone();
        move || {
            if !cfg!(windows) && !file_dialogs_available() {
                call(
                    &rt,
                    "set_logging_dialog_path_error",
                    json!([
                        "The system folder picker is unavailable here; type the folder path directly."
                    ]),
                );
                return;
            }
            let start = weak
                .upgrade()
                .map(|window| window.get_directory_text().to_string())
                .unwrap_or_default();
            spawn_pick_folder(tx.clone(), DialogKind::LoggingDirectory, "Log folder", &start);
        }
    });
    window.on_open_file_requested({
        let rt = rt.clone();
        move || {
            if let Ok(Value::String(path)) = rt.call_value("active_session_logging_path", json!([]))
            {
                spawn_open_path(path);
            }
        }
    });
    window.on_open_folder_requested({
        let rt = rt.clone();
        move || {
            if let Ok(Value::String(path)) = rt.call_value("active_session_logging_path", json!([]))
            {
                if path.is_empty() {
                    return;
                }
                let folder = std::path::Path::new(&path)
                    .parent()
                    .map(|parent| parent.display().to_string())
                    .unwrap_or(path);
                spawn_open_path(folder);
            }
        }
    });
}

pub(crate) fn wire_sftp_conflict(window: &SftpConflictWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_canceled, "cancel_sftp_conflict");
    on_close!(window, rt, "cancel_sftp_conflict");
    window.on_resolve({
        let rt = rt.clone();
        move |policy: SharedString| call(&rt, "resolve_sftp_conflict", json!([policy.as_str()]))
    });
}

pub(crate) fn wire_sftp_properties(window: &SftpPropertiesWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_canceled, "close_sftp_properties");
    on_close!(window, rt, "close_sftp_properties");
    let weak = window.as_weak();
    window.on_copy_path(move || {
        if let Some(window) = weak.upgrade() {
            let _ = write_clipboard(window.get_path_text().as_str());
        }
    });
}

pub(crate) fn wire_sftp_chmod(window: &SftpChmodWindow, client: Arc<HostShellClient>) {
    let rt = rt_of(&client);
    title_drag!(window);
    cb0!(window, rt, on_canceled, "cancel_sftp_chmod_dialog");
    on_close!(window, rt, "cancel_sftp_chmod_dialog");
    cb_str!(window, rt, on_update_mode, "update_sftp_chmod_mode");
    cb0!(window, rt, on_applied, "apply_sftp_chmod_dialog");
}

pub(crate) fn wire_new_folder(
    window: &NewFolderWindow,
    client: Arc<HostShellClient>,
    open: Rc<Cell<bool>>,
    refresh: Rc<Cell<bool>>,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    window.on_canceled({
        let open = Rc::clone(&open);
        let refresh = Rc::clone(&refresh);
        move || {
            open.set(false);
            refresh.set(true);
        }
    });
    window.window().on_close_requested({
        let open = Rc::clone(&open);
        let refresh = Rc::clone(&refresh);
        move || {
            open.set(false);
            refresh.set(true);
            CloseRequestResponse::HideWindow
        }
    });
    window.on_confirmed({
        let open = Rc::clone(&open);
        let refresh = Rc::clone(&refresh);
        let rt = rt.clone();
        move |name: SharedString| {
            open.set(false);
            refresh.set(true);
            call(&rt, "create_root_saved_folder", json!([name.as_str()]));
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
}

pub(crate) fn wire_sftp_entry(
    window: &SftpEntryWindow,
    client: Arc<HostShellClient>,
    kind: Rc<RefCell<String>>,
    refresh: Rc<Cell<bool>>,
) {
    let rt = rt_of(&client);
    title_drag!(window);
    let clear_kind = || {
        let kind = Rc::clone(&kind);
        let refresh = Rc::clone(&refresh);
        move || {
            *kind.borrow_mut() = "none".into();
            refresh.set(true);
        }
    };
    window.on_canceled(clear_kind());
    window.window().on_close_requested({
        let clear = clear_kind();
        move || {
            clear();
            CloseRequestResponse::HideWindow
        }
    });
    window.on_create_folder({
        let clear = clear_kind();
        let rt = rt.clone();
        move |name: SharedString| {
            clear();
            call(&rt, "create_sftp_folder_named", json!([name.as_str()]));
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
    window.on_rename_selected({
        let clear = clear_kind();
        let rt = rt.clone();
        move |name: SharedString| {
            clear();
            call(&rt, "rename_sftp_selected", json!([name.as_str()]));
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
    window.on_delete_selected({
        let clear = clear_kind();
        let rt = rt.clone();
        move || {
            clear();
            call(&rt, "delete_sftp_selection", json!([]));
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
    window.on_chmod_selected({
        let clear = clear_kind();
        let rt = rt.clone();
        move |mode: SharedString| {
            clear();
            call(&rt, "chmod_sftp_selection", json!([mode.as_str()]));
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
    window.on_upload_selected({
        let clear = clear_kind();
        let rt = rt.clone();
        move |path: SharedString| {
            clear();
            call(&rt, "upload_sftp_into_current", json!([path.as_str()]));
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
    window.on_download_selected({
        let clear = clear_kind();
        let rt = rt.clone();
        move |path: SharedString| {
            clear();
            call(&rt, "download_sftp_selected", json!([path.as_str()]));
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
    window.on_copy_to_selected({
        let clear = clear_kind();
        let rt = rt.clone();
        move |path: SharedString| {
            clear();
            call(&rt, "copy_sftp_selection_to", json!([path.as_str(), "ask"]));
            call(&rt, "mark_ui_dirty", json!([]));
        }
    });
}
