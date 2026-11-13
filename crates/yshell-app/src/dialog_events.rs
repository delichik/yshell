//! File-dialog result model shared by the UI and the out-of-process Shell.
//!
//! These types used to live in `bootstrap.rs`, but the out-of-process Host also
//! needs them (`host::dialog_dispatch` applies a picked path over UiCall).
//! Keeping them in a Slint-free module lets the Host entry avoid linking the UI.

use crate::runtime::{AppProjection, AppRuntime};

/// N4：rfd 文件对话框的结果（worker 线程 → mpsc → 定时器 drain）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileDialogOutcome {
    /// `pick_file` 返回了一个路径。
    Picked { kind: DialogKind, path: String },
    /// `save_file` 写盘结果（`error` 为空 = 成功）。
    Saved {
        kind: DialogKind,
        path: String,
        error: Option<String>,
    },
}

/// N4：对话框用途（决定结果回填到哪个页面状态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogKind {
    PrivateKeyImport,
    AuthKeyFile,
    PublicKeyExport,
    HostKeysImport,
    HostKeysExport,
    /// N6：终端日志弹窗的保存位置（pick_folder）。
    LoggingDirectory,
    /// N1：SFTP 上传文件（pick_file）→ 上传到当前/选中远端目录。
    SftpUploadFile,
    /// N1：SFTP 下载目标目录（pick_folder）→ 下载当前远端选择。
    SftpDownloadFolder,
    /// N5：终端外观的主字体文件（pick_file）。
    TerminalFontPrimary,
    /// N5：终端外观的备用字体文件（pick_file，单值，选中即替换）。
    TerminalFontBackup,
    /// D33：Settings → SFTP → External editor 的可执行文件（pick_file；仅 Custom）。
    SettingsSftpEditor,
    /// Session Editor：Icon → Custom image（pick_file；Windows 图片/任意文件）。
    SessionIcon,
}

pub(crate) fn dialog_kind_name(kind: DialogKind) -> &'static str {
    match kind {
        DialogKind::PrivateKeyImport => "private_key_import",
        DialogKind::AuthKeyFile => "auth_key_file",
        DialogKind::PublicKeyExport => "public_key_export",
        DialogKind::HostKeysImport => "host_keys_import",
        DialogKind::HostKeysExport => "host_keys_export",
        DialogKind::LoggingDirectory => "logging_directory",
        DialogKind::SftpUploadFile => "sftp_upload_file",
        DialogKind::SftpDownloadFolder => "sftp_download_folder",
        DialogKind::TerminalFontPrimary => "terminal_font_primary",
        DialogKind::TerminalFontBackup => "terminal_font_backup",
        DialogKind::SettingsSftpEditor => "settings_sftp_editor",
        DialogKind::SessionIcon => "session_icon",
    }
}

pub(crate) fn dialog_kind_from_name(name: &str) -> Option<DialogKind> {
    Some(match name {
        "private_key_import" => DialogKind::PrivateKeyImport,
        "auth_key_file" => DialogKind::AuthKeyFile,
        "public_key_export" => DialogKind::PublicKeyExport,
        "host_keys_import" => DialogKind::HostKeysImport,
        "host_keys_export" => DialogKind::HostKeysExport,
        "logging_directory" => DialogKind::LoggingDirectory,
        "sftp_upload_file" => DialogKind::SftpUploadFile,
        "sftp_download_folder" => DialogKind::SftpDownloadFolder,
        "terminal_font_primary" => DialogKind::TerminalFontPrimary,
        "terminal_font_backup" => DialogKind::TerminalFontBackup,
        "settings_sftp_editor" => DialogKind::SettingsSftpEditor,
        "session_icon" => DialogKind::SessionIcon,
        _ => return None,
    })
}

/// N4：把对话框结果回填到运行时（在 UI 线程的定时器 / Host UiCall 里调用）。
pub(crate) fn apply_dialog_outcome(
    runtime: &mut AppRuntime,
    outcome: FileDialogOutcome,
) -> AppProjection {
    match outcome {
        FileDialogOutcome::Picked { kind, path } => match kind {
            DialogKind::PrivateKeyImport => runtime.update_private_keys_import_path(&path),
            DialogKind::AuthKeyFile => runtime.set_auth_prompt_key_path(&path),
            // managers-dialogs：Import 子窗只回填路径，点 Import 再读文件。
            DialogKind::HostKeysImport => runtime.update_host_keys_import_path(&path),
            // N6：终端日志弹窗的保存位置。
            DialogKind::LoggingDirectory => runtime.update_logging_dialog_directory(&path),
            // N1：rfd 直接触发的 SFTP 上传/下载（错误 → 状态栏）。
            DialogKind::SftpUploadFile => runtime
                .upload_local_path_to_remote(&path, "ask")
                .unwrap_or_else(|error| {
                    runtime.set_status_kind(
                        "sftp-upload-error",
                        format!("SFTP upload: {error}"),
                        error.to_string(),
                        String::new(),
                    );
                    runtime.projection()
                }),
            DialogKind::SftpDownloadFolder => runtime
                .download_sftp_selection_to(&path, "ask")
                .unwrap_or_else(|error| {
                    runtime.set_status_kind(
                        "sftp-download-error",
                        format!("SFTP download: {error}"),
                        error.to_string(),
                        String::new(),
                    );
                    runtime.projection()
                }),
            // managers-dialogs：导出子窗的 Browse 只回填目标路径，点 Export 再写盘。
            DialogKind::PublicKeyExport => runtime.update_private_keys_export_path(&path),
            DialogKind::HostKeysExport => runtime.update_host_keys_export_path(&path),
            // N5：终端外观的字体文件（主字体 / 备用字体）。
            DialogKind::TerminalFontPrimary => {
                runtime.update_settings_appearance_font_family(&path)
            }
            DialogKind::TerminalFontBackup => runtime.set_settings_appearance_backup_font(&path),
            // D33：Settings → SFTP 外部编辑器路径（Custom）。
            DialogKind::SettingsSftpEditor => {
                runtime.update_settings_sftp_external_editor_path(&path)
            }
            // Session Editor：Icon → Custom image（Browse 选中即回填草稿）。
            DialogKind::SessionIcon => runtime.update_editor_icon_custom(&path),
        },
        FileDialogOutcome::Saved { kind, path, error } => match kind {
            DialogKind::HostKeysExport | DialogKind::HostKeysImport => match &error {
                Some(error) => runtime.set_host_keys_status_feedback(
                    "host-keys-export-failed",
                    format!("Export failed: {error}"),
                    error.to_string(),
                    String::new(),
                ),
                None => runtime.set_host_keys_status_feedback(
                    "host-keys-exported",
                    format!("Exported host keys to `{path}`."),
                    path.clone(),
                    String::new(),
                ),
            },
            DialogKind::LoggingDirectory => {
                let detail = match &error {
                    Some(error) => format!("Dialog failed: {error}"),
                    None => format!("Saved to `{path}`."),
                };
                runtime.set_logging_dialog_path_error(&detail)
            }
            _ => match (&error, kind) {
                (Some(error), DialogKind::PublicKeyExport) => runtime
                    .set_private_keys_status_feedback(
                        "private-key-export-failed",
                        format!("Export failed: {error}"),
                        error.to_string(),
                        String::new(),
                    ),
                (Some(error), _) => runtime.set_private_keys_status_feedback(
                    "private-key-dialog-failed",
                    format!("Dialog failed: {error}"),
                    error.to_string(),
                    String::new(),
                ),
                (None, DialogKind::PublicKeyExport) => runtime.set_private_keys_status_feedback(
                    "private-key-exported",
                    format!("Exported the public key to `{path}`."),
                    path.clone(),
                    String::new(),
                ),
                (None, _) => runtime.set_private_keys_status_feedback(
                    "private-key-file-saved",
                    format!("Saved to `{path}`."),
                    path.clone(),
                    String::new(),
                ),
            },
        },
    }
}
