//! Independent non-modal dialog / tool windows.
//!
//! Former L2 overlays inside `MainWindow` are hosted here as separate Slint
//! `Window`s. Compact confirms keep the ContentDialog panel; managers/forms use
//! fixed-size tool windows. The main window stays interactive.
//!
//! Opening signal: keep the historical `*_visible` / projection flags on
//! `MainWindow` / `AppProjection`. Each tick, [`DialogWindowManager::sync`]
//! promotes local MainWindow flags into app-scoped state and show/hides aux
//! windows. Overlay widgets themselves are removed from `MainWindow`.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use slint::{CloseRequestResponse, ComponentHandle, SharedString};

use crate::bootstrap::generated_ui::{
    AboutWindow, AuthPromptWindow, CompactConfirmWindow, FolderEditorWindow, HostKeyWindow,
    HostKeysWindow, KnownHostsWindow, LoggingWindow, MainWindow, NewFolderWindow,
    PasswordPromptWindow, PrivateKeysWindow, ProxiesWindow, SessionEditorWindow, SettingsWindow,
    SftpConflictWindow, SftpEntryWindow, SftpPropertiesWindow, ThemeManagerWindow,
};
use crate::error::{AppError, AppResult};
use crate::runtime::AppProjection;
use crate::windows::{WindowManager, WindowRuntime};

macro_rules! prep_tool_window {
    ($window:expr) => {{
        $window.set_native_title_drag(cfg!(windows));
    }};
}

macro_rules! wire_tool_title_drag {
    ($window:expr) => {{
        let weak = $window.as_weak();
        $window.on_title_drag_requested(move || {
            if let Some(window) = weak.upgrade() {
                crate::win_chrome::start_native_window_move(window.window());
            }
        });
    }};
}

macro_rules! wire_tool_on_close {
    ($window:expr, $runtime:expr, $close:expr) => {{
        let close_fn: fn(&mut crate::runtime::AppRuntime) -> crate::runtime::AppProjection =
            $close;
        let rt = Rc::clone(&$runtime);
        $window.on_dismiss(move || {
            let _ = close_fn(&mut rt.borrow_mut());
        });
        let rt = Rc::clone(&$runtime);
        $window.window().on_close_requested(move || {
            let _ = close_fn(&mut rt.borrow_mut());
            CloseRequestResponse::HideWindow
        });
    }};
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmKind {
    ClearScrollback,
    DeleteSession,
    Quit,
    CloseTabs,
    CloseWindow,
    EditorDiscard,
    PrivateKeysRemove,
    PrivateKeysDeploy,
    ProxiesRemove,
    ThemeManagerRemove,
    HostKeysClear,
    HostKeysRemove,
}

struct ConfirmSpec {
    kind: ConfirmKind,
    title: String,
    message: String,
    confirm_label: String,
    destructive: bool,
    show_dont_ask: bool,
    dont_ask: bool,
}

#[derive(Debug, Default)]
struct LocalDialogFlags {
    about: bool,
    settings: bool,
    clear_scrollback: bool,
    delete_session: bool,
    quit: bool,
    new_folder: bool,
    tab_rename: bool,
    tab_rename_tab_id: String,
    host_keys_clear: bool,
    host_keys_remove: bool,
    /// Non-"none" opens the SFTP entry tool window.
    sftp_dialog_kind: String,
}

pub(crate) struct DialogHost {
    pub dialogs: DialogWindowManager,
}

pub(crate) struct DialogWindowManager {
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
    host: Weak<RefCell<DialogHost>>,
    manager: Weak<RefCell<WindowManager>>,
    /// Window id used for close-window confirm (last requester).
    close_window_id: Option<u64>,
    local: LocalDialogFlags,
    about: Option<AboutWindow>,
    about_shown: bool,
    confirm: Option<CompactConfirmWindow>,
    confirm_kind: Option<ConfirmKind>,
    settings: Option<SettingsWindow>,
    settings_shown: bool,
    // Additional tool windows are created lazily in sync_* helpers.
    session_editor: Option<SessionEditorWindow>,
    session_editor_shown: bool,
    folder_editor: Option<FolderEditorWindow>,
    folder_editor_shown: bool,
    known_hosts: Option<KnownHostsWindow>,
    known_hosts_shown: bool,
    private_keys: Option<PrivateKeysWindow>,
    private_keys_shown: bool,
    proxies: Option<ProxiesWindow>,
    proxies_shown: bool,
    theme_manager: Option<ThemeManagerWindow>,
    theme_manager_shown: bool,
    host_keys: Option<HostKeysWindow>,
    host_keys_shown: bool,
    logging: Option<LoggingWindow>,
    logging_shown: bool,
    password: Option<PasswordPromptWindow>,
    password_shown: bool,
    auth: Option<AuthPromptWindow>,
    auth_shown: bool,
    host_key: Option<HostKeyWindow>,
    host_key_shown: bool,
    new_folder: Option<NewFolderWindow>,
    new_folder_shown: bool,
    tab_rename: Option<NewFolderWindow>,
    tab_rename_shown: bool,
    sftp_entry: Option<SftpEntryWindow>,
    sftp_entry_shown: bool,
    sftp_conflict: Option<SftpConflictWindow>,
    sftp_conflict_shown: bool,
    sftp_properties: Option<SftpPropertiesWindow>,
    sftp_properties_shown: bool,
}

impl DialogWindowManager {
    pub(crate) fn new(
        runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
    ) -> Rc<RefCell<DialogHost>> {
        let host = Rc::new(RefCell::new(DialogHost {
            dialogs: DialogWindowManager {
                runtime,
                host: Weak::new(),
                manager: Weak::new(),
                close_window_id: None,
                local: LocalDialogFlags {
                    sftp_dialog_kind: "none".into(),
                    ..LocalDialogFlags::default()
                },
                about: None,
                about_shown: false,
                confirm: None,
                confirm_kind: None,
                settings: None,
                settings_shown: false,
                session_editor: None,
                session_editor_shown: false,
                folder_editor: None,
                folder_editor_shown: false,
                known_hosts: None,
                known_hosts_shown: false,
                private_keys: None,
                private_keys_shown: false,
                proxies: None,
                proxies_shown: false,
                theme_manager: None,
                theme_manager_shown: false,
                host_keys: None,
                host_keys_shown: false,
                logging: None,
                logging_shown: false,
                password: None,
                password_shown: false,
                auth: None,
                auth_shown: false,
                host_key: None,
                host_key_shown: false,
                new_folder: None,
                new_folder_shown: false,
                tab_rename: None,
                tab_rename_shown: false,
                sftp_entry: None,
                sftp_entry_shown: false,
                sftp_conflict: None,
                sftp_conflict_shown: false,
                sftp_properties: None,
                sftp_properties_shown: false,
            },
        }));
        host.borrow_mut().dialogs.host = Rc::downgrade(&host);
        host
    }

    pub(crate) fn set_window_manager(&mut self, manager: Weak<RefCell<WindowManager>>) {
        self.manager = manager;
    }

    pub(crate) fn set_close_window_id(&mut self, window_id: u64) {
        self.close_window_id = Some(window_id);
    }

    /// Pull open signals from one main window into app-scoped local flags.
    pub(crate) fn ingest_one(&mut self, window: &MainWindow) {
        if window.get_about_dialog_visible() {
            self.local.about = true;
            window.set_about_dialog_visible(false);
        }
        if window.get_settings_dialog_visible() {
            self.local.settings = true;
            window.set_settings_dialog_visible(false);
        }
        if window.get_clear_scrollback_dialog_visible() {
            self.local.clear_scrollback = true;
            window.set_clear_scrollback_dialog_visible(false);
        }
        if window.get_delete_session_dialog_visible() {
            self.local.delete_session = true;
            window.set_delete_session_dialog_visible(false);
        }
        if window.get_quit_dialog_visible() {
            self.local.quit = true;
            window.set_quit_dialog_visible(false);
        }
        if window.get_new_folder_dialog_visible() {
            self.local.new_folder = true;
            window.set_new_folder_dialog_visible(false);
        }
        if window.get_tab_rename_dialog_visible() {
            self.local.tab_rename = true;
            self.local.tab_rename_tab_id = window.get_tab_rename_tab_id().to_string();
            window.set_tab_rename_dialog_visible(false);
        }
        if window.get_host_keys_clear_dialog_visible() {
            self.local.host_keys_clear = true;
            window.set_host_keys_clear_dialog_visible(false);
        }
        if window.get_host_keys_remove_dialog_visible() {
            self.local.host_keys_remove = true;
            window.set_host_keys_remove_dialog_visible(false);
        }
        let kind = window.get_sftp_dialog_kind().to_string();
        if kind != "none" {
            self.local.sftp_dialog_kind = kind;
            window.set_sftp_dialog_kind("none".into());
        }
    }

    pub(crate) fn sync(
        &mut self,
        projection: &AppProjection,
        main: Option<&MainWindow>,
        window_runtime: Option<Rc<WindowRuntime>>,
    ) -> AppResult<()> {
        let runtime = Rc::clone(&self.runtime);
        let host = self.host.clone();
        self.sync_about(projection)?;
        self.sync_confirm(projection, main, window_runtime.clone())?;
        self.sync_settings(projection, main, window_runtime.clone())?;
        sync_projection_tool(
            projection.editor_modal_visible,
            &mut self.session_editor_shown,
            &mut self.session_editor,
            || SessionEditorWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_session_editor_window(w, Rc::clone(&runtime)),
            |w, p, m| sync_session_editor(w, p, m),
            projection,
            main,
        )?;
        sync_projection_tool(
            projection.folder_editor_visible,
            &mut self.folder_editor_shown,
            &mut self.folder_editor,
            || FolderEditorWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_folder_editor_window(w, Rc::clone(&runtime)),
            |w, p, m| sync_folder_editor(w, p, m),
            projection,
            main,
        )?;
        sync_projection_tool(
            projection.known_hosts_modal_visible,
            &mut self.known_hosts_shown,
            &mut self.known_hosts,
            || KnownHostsWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_known_hosts_window(w, Rc::clone(&runtime)),
            |w, p, _| sync_known_hosts(w, p),
            projection,
            main,
        )?;
        sync_projection_tool(
            projection.private_keys_modal_visible,
            &mut self.private_keys_shown,
            &mut self.private_keys,
            || PrivateKeysWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_private_keys_window(w, Rc::clone(&runtime)),
            |w, p, m| sync_private_keys(w, p, m),
            projection,
            main,
        )?;
        sync_projection_tool(
            projection.proxies_modal_visible,
            &mut self.proxies_shown,
            &mut self.proxies,
            || ProxiesWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_proxies_window(w, Rc::clone(&runtime)),
            |w, p, m| sync_proxies(w, p, m),
            projection,
            main,
        )?;
        sync_projection_tool(
            projection.theme_manager_visible,
            &mut self.theme_manager_shown,
            &mut self.theme_manager,
            || ThemeManagerWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_theme_manager_window(w, Rc::clone(&runtime)),
            |w, p, m| sync_theme_manager(w, p, m),
            projection,
            main,
        )?;
        sync_projection_tool(
            projection.host_keys_modal_visible,
            &mut self.host_keys_shown,
            &mut self.host_keys,
            || HostKeysWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_host_keys_window(w, Rc::clone(&runtime), host.clone()),
            |w, p, m| sync_host_keys(w, p, m),
            projection,
            main,
        )?;
        sync_projection_tool(
            projection.logging_dialog_visible,
            &mut self.logging_shown,
            &mut self.logging,
            || LoggingWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_logging_window(w, Rc::clone(&runtime)),
            |w, p, m| sync_logging(w, p, m),
            projection,
            main,
        )?;
        sync_projection_tool(
            projection.password_prompt_visible,
            &mut self.password_shown,
            &mut self.password,
            || PasswordPromptWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_password_prompt_window(w, Rc::clone(&runtime)),
            |w, p, m| sync_password(w, p, m),
            projection,
            main,
        )?;
        sync_projection_tool(
            projection.auth_prompt_visible,
            &mut self.auth_shown,
            &mut self.auth,
            || AuthPromptWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_auth_prompt_window(w, Rc::clone(&runtime)),
            |w, p, m| sync_auth(w, p, m),
            projection,
            main,
        )?;
        sync_projection_tool(
            projection.host_key_prompt_visible,
            &mut self.host_key_shown,
            &mut self.host_key,
            || HostKeyWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_host_key_window(w, Rc::clone(&runtime)),
            |w, p, _| sync_host_key_prompt(w, p),
            projection,
            main,
        )?;
        self.sync_local_new_folder(Rc::clone(&runtime), host.clone(), main)?;
        self.sync_local_tab_rename(Rc::clone(&runtime), host.clone(), main)?;
        self.sync_sftp_entry(projection, main)?;
        sync_projection_tool(
            projection.sftp_conflict_visible,
            &mut self.sftp_conflict_shown,
            &mut self.sftp_conflict,
            || SftpConflictWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_sftp_conflict_window(w, Rc::clone(&runtime)),
            |w, p, _| sync_sftp_conflict(w, p),
            projection,
            main,
        )?;
        sync_projection_tool(
            projection.sftp_properties_visible,
            &mut self.sftp_properties_shown,
            &mut self.sftp_properties,
            || SftpPropertiesWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_sftp_properties_window(w, Rc::clone(&runtime)),
            |w, p, _| sync_sftp_properties(w, p),
            projection,
            main,
        )?;
        let _ = window_runtime;
        Ok(())
    }

    fn sync_about(&mut self, projection: &AppProjection) -> AppResult<()> {
        if self.local.about {
            if self.about.is_none() {
                let window = AboutWindow::new().map_err(AppError::from_error)?;
                window.set_native_title_drag(cfg!(windows));
                let host = self.host.clone();
                window.on_dismiss(move || {
                    if let Some(host) = host.upgrade() {
                        host.borrow_mut().dialogs.local.about = false;
                        host.borrow_mut().dialogs.hide_about();
                    }
                });
                let host = self.host.clone();
                window.on_title_drag_requested(move || {
                    if let Some(host) = host.upgrade() {
                        if let Some(about) = host.borrow().dialogs.about.as_ref() {
                            crate::win_chrome::start_native_window_move(about.window());
                        }
                    }
                });
                let host = self.host.clone();
                window.window().on_close_requested(move || {
                    if let Some(host) = host.upgrade() {
                        host.borrow_mut().dialogs.local.about = false;
                        host.borrow_mut().dialogs.hide_about();
                    }
                    CloseRequestResponse::HideWindow
                });
                self.about = Some(window);
            }
            if let Some(about) = self.about.as_ref() {
                about.set_version_text(projection.app_version_text.clone().into());
                about.set_config_dir_text(projection.config_dir_text.clone().into());
                if !self.about_shown {
                    show_aux(about)?;
                    self.about_shown = true;
                }
            }
        } else {
            self.hide_about();
        }
        Ok(())
    }

    fn hide_about(&mut self) {
        if let Some(about) = self.about.as_ref() {
            let _ = about.hide();
        }
        self.about_shown = false;
    }

    fn sync_confirm(
        &mut self,
        projection: &AppProjection,
        main: Option<&MainWindow>,
        window_runtime: Option<Rc<WindowRuntime>>,
    ) -> AppResult<()> {
        match self.active_confirm_spec(projection, main) {
            Some(spec) => self.show_confirm(spec, window_runtime)?,
            None => self.hide_confirm(),
        }
        Ok(())
    }

    fn active_confirm_spec(
        &self,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> Option<ConfirmSpec> {
        if projection.private_keys_remove_confirm_visible {
            return Some(ConfirmSpec {
                kind: ConfirmKind::PrivateKeysRemove,
                title: "Remove Managed Key".into(),
                message: projection.private_keys_remove_warning_text.clone(),
                confirm_label: "Remove".into(),
                destructive: true,
                show_dont_ask: false,
                dont_ask: false,
            });
        }
        if projection.private_keys_deploy_confirm_visible {
            let message = if projection.private_keys_deploy_command_text.is_empty() {
                projection.private_keys_deploy_summary_text.clone()
            } else {
                format!(
                    "{}\n\n{}",
                    projection.private_keys_deploy_summary_text,
                    projection.private_keys_deploy_command_text
                )
            };
            return Some(ConfirmSpec {
                kind: ConfirmKind::PrivateKeysDeploy,
                title: "Deploy Public Key".into(),
                message,
                confirm_label: "Deploy".into(),
                destructive: false,
                show_dont_ask: false,
                dont_ask: false,
            });
        }
        if projection.proxies_remove_confirm_visible {
            return Some(ConfirmSpec {
                kind: ConfirmKind::ProxiesRemove,
                title: "Delete Proxy Profile".into(),
                message: "This removes the managed proxy profile and its saved password from the secret store.".into(),
                confirm_label: "Delete".into(),
                destructive: true,
                show_dont_ask: false,
                dont_ask: false,
            });
        }
        if projection.theme_manager_remove_confirm_visible {
            let message = if projection.theme_manager_remove_reference_count > 0 {
                format!(
                    "Delete “{}”? {} theme reference(s) will switch to the built-in default theme.",
                    projection.theme_manager_remove_name_text,
                    projection.theme_manager_remove_reference_count
                )
            } else {
                format!(
                    "Delete “{}”? This removes the user theme file and cannot be undone.",
                    projection.theme_manager_remove_name_text
                )
            };
            return Some(ConfirmSpec {
                kind: ConfirmKind::ThemeManagerRemove,
                title: "Delete Theme".into(),
                message,
                confirm_label: "Delete".into(),
                destructive: true,
                show_dont_ask: false,
                dont_ask: false,
            });
        }
        if self.local.host_keys_clear {
            return Some(ConfirmSpec {
                kind: ConfirmKind::HostKeysClear,
                title: "Clear All Host Keys".into(),
                message: "Remove every saved host key from disk? This cannot be undone.".into(),
                confirm_label: "Clear All".into(),
                destructive: true,
                show_dont_ask: false,
                dont_ask: false,
            });
        }
        if self.local.host_keys_remove {
            return Some(ConfirmSpec {
                kind: ConfirmKind::HostKeysRemove,
                title: "Remove Host Keys".into(),
                message: "Remove all saved keys for the selected host:port? This deletes the entire group, not just the selected key.".into(),
                confirm_label: "Remove".into(),
                destructive: true,
                show_dont_ask: false,
                dont_ask: false,
            });
        }
        if projection.editor_discard_dialog_visible {
            return Some(ConfirmSpec {
                kind: ConfirmKind::EditorDiscard,
                title: "Discard Changes".into(),
                message: "Discard unsaved changes to this session?".into(),
                confirm_label: "Discard".into(),
                destructive: true,
                show_dont_ask: false,
                dont_ask: false,
            });
        }
        if projection.close_tabs_confirm_visible {
            let (title, message) = close_tabs_copy(projection);
            return Some(ConfirmSpec {
                kind: ConfirmKind::CloseTabs,
                title,
                message,
                confirm_label: "Close".into(),
                destructive: true,
                show_dont_ask: true,
                dont_ask: main.map(|w| w.get_close_confirm_dont_ask_again()).unwrap_or(false),
            });
        }
        if projection.close_window_confirm_visible {
            let message = if projection.close_window_confirm_active_count == 1 {
                "Close this window? 1 active session will be disconnected.".into()
            } else {
                format!(
                    "Close this window? {} active sessions will be disconnected.",
                    projection.close_window_confirm_active_count
                )
            };
            return Some(ConfirmSpec {
                kind: ConfirmKind::CloseWindow,
                title: "Close Window".into(),
                message,
                confirm_label: "Close".into(),
                destructive: true,
                show_dont_ask: true,
                dont_ask: main
                    .map(|w| w.get_close_window_confirm_dont_ask_again())
                    .unwrap_or(false),
            });
        }
        if self.local.clear_scrollback {
            return Some(ConfirmSpec {
                kind: ConfirmKind::ClearScrollback,
                title: "Clear Scrollback".into(),
                message: "Clear the terminal scrollback buffer? The live screen stays as-is."
                    .into(),
                confirm_label: "Clear Scrollback".into(),
                destructive: true,
                show_dont_ask: false,
                dont_ask: false,
            });
        }
        if self.local.delete_session {
            return Some(ConfirmSpec {
                kind: ConfirmKind::DeleteSession,
                title: "Delete Saved Session".into(),
                message: "This permanently removes the selected saved session from disk.".into(),
                confirm_label: "Delete".into(),
                destructive: true,
                show_dont_ask: false,
                dont_ask: false,
            });
        }
        if self.local.quit {
            return Some(ConfirmSpec {
                kind: ConfirmKind::Quit,
                title: "Quit YShell".into(),
                message: "Quit YShell now? Active runtime sessions will be disconnected.".into(),
                confirm_label: "Quit".into(),
                destructive: true,
                show_dont_ask: true,
                dont_ask: main
                    .map(|w| w.get_quit_confirm_dont_ask_again())
                    .unwrap_or(false),
            });
        }
        None
    }

    fn show_confirm(
        &mut self,
        spec: ConfirmSpec,
        window_runtime: Option<Rc<WindowRuntime>>,
    ) -> AppResult<()> {
        let kind_changed = self.confirm_kind != Some(spec.kind);
        if self.confirm.is_none() {
            let window = CompactConfirmWindow::new().map_err(AppError::from_error)?;
            self.wire_confirm(&window, window_runtime);
            self.confirm = Some(window);
        }
        let window = self.confirm.as_ref().expect("confirm");
        window.set_title_text(spec.title.into());
        window.set_message_text(spec.message.into());
        window.set_confirm_label(spec.confirm_label.into());
        window.set_destructive(spec.destructive);
        window.set_show_dont_ask_again(spec.show_dont_ask);
        window.set_dont_ask_again(spec.dont_ask);
        window.set_confirmation_token("".into());
        window.set_confirmation_text("".into());
        let need_show = kind_changed || self.confirm_kind.is_none();
        self.confirm_kind = Some(spec.kind);
        if need_show {
            show_aux(window)?;
            window.invoke_focus_dialog();
        }
        Ok(())
    }

    fn hide_confirm(&mut self) {
        if let Some(window) = self.confirm.as_ref() {
            let _ = window.hide();
        }
        self.confirm_kind = None;
    }

    fn wire_confirm(&self, window: &CompactConfirmWindow, window_runtime: Option<Rc<WindowRuntime>>) {
        let host = self.host.clone();
        let wr = window_runtime.clone();
        window.on_confirmed(move || {
            let Some(host_rc) = host.upgrade() else {
                return;
            };
            let kind = host_rc.borrow().dialogs.confirm_kind;
            let dont_ask = host_rc
                .borrow()
                .dialogs
                .confirm
                .as_ref()
                .map(|w| w.get_dont_ask_again())
                .unwrap_or(false);
            dispatch_confirm(&host_rc, wr.as_ref(), kind, true, dont_ask);
        });
        let host = self.host.clone();
        let wr = window_runtime;
        window.on_canceled(move || {
            let Some(host_rc) = host.upgrade() else {
                return;
            };
            let kind = host_rc.borrow().dialogs.confirm_kind;
            dispatch_confirm(&host_rc, wr.as_ref(), kind, false, false);
        });
        let host = self.host.clone();
        window.window().on_close_requested(move || {
            if let Some(host_rc) = host.upgrade() {
                let kind = host_rc.borrow().dialogs.confirm_kind;
                dispatch_confirm(&host_rc, None, kind, false, false);
            }
            CloseRequestResponse::HideWindow
        });
    }

    fn sync_settings(
        &mut self,
        projection: &AppProjection,
        main: Option<&MainWindow>,
        window_runtime: Option<Rc<WindowRuntime>>,
    ) -> AppResult<()> {
        if self.local.settings {
            if self.settings.is_none() {
                let window = SettingsWindow::new().map_err(AppError::from_error)?;
                window.set_native_title_drag(cfg!(windows));
                let host = self.host.clone();
                window.on_dismiss(move || {
                    if let Some(host) = host.upgrade() {
                        host.borrow_mut().dialogs.local.settings = false;
                        if let Some(settings) = host.borrow().dialogs.settings.as_ref() {
                            let _ = settings.hide();
                        }
                        host.borrow_mut().dialogs.settings_shown = false;
                    }
                });
                let host = self.host.clone();
                window.on_title_drag_requested(move || {
                    if let Some(host) = host.upgrade() {
                        if let Some(settings) = host.borrow().dialogs.settings.as_ref() {
                            crate::win_chrome::start_native_window_move(settings.window());
                        }
                    }
                });
                wire_settings_actions(&window, window_runtime.clone(), self.host.clone());
                self.settings = Some(window);
            }
            if let Some(settings) = self.settings.as_ref() {
                sync_settings_from(settings, projection, main);
                if !self.settings_shown {
                    show_aux(settings)?;
                    self.settings_shown = true;
                }
            }
        } else if self.settings_shown {
            if let Some(settings) = self.settings.as_ref() {
                let _ = settings.hide();
            }
            self.settings_shown = false;
        }
        Ok(())
    }

    fn sync_local_new_folder(
        &mut self,
        runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
        host: Weak<RefCell<DialogHost>>,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let want = self.local.new_folder;
        if want {
            if self.new_folder.is_none() {
                let window = NewFolderWindow::new().map_err(AppError::from_error)?;
                wire_new_folder_window(&window, runtime, host, false);
                self.new_folder = Some(window);
            }
            if let Some(window) = self.new_folder.as_ref() {
                if let Some(main) = main {
                    window.set_name_text(main.get_new_root_folder_name_text());
                }
                if !self.new_folder_shown {
                    show_aux(window)?;
                    self.new_folder_shown = true;
                }
            }
        } else if self.new_folder_shown {
            if let Some(window) = self.new_folder.as_ref() {
                let _ = window.hide();
            }
            self.new_folder_shown = false;
        }
        Ok(())
    }

    fn sync_local_tab_rename(
        &mut self,
        runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
        host: Weak<RefCell<DialogHost>>,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let want = self.local.tab_rename;
        if want {
            if self.tab_rename.is_none() {
                let window = NewFolderWindow::new().map_err(AppError::from_error)?;
                window.set_title_text("Rename Tab".into());
                window.set_confirm_label("Rename".into());
                wire_new_folder_window(&window, runtime, host, true);
                self.tab_rename = Some(window);
            }
            if let Some(window) = self.tab_rename.as_ref() {
                if let Some(main) = main {
                    window.set_name_text(main.get_tab_rename_name_text());
                }
                if !self.tab_rename_shown {
                    show_aux(window)?;
                    self.tab_rename_shown = true;
                }
            }
        } else if self.tab_rename_shown {
            if let Some(window) = self.tab_rename.as_ref() {
                let _ = window.hide();
            }
            self.tab_rename_shown = false;
        }
        Ok(())
    }

    fn sync_sftp_entry(
        &mut self,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let want = self.local.sftp_dialog_kind != "none";
        if want {
            if self.sftp_entry.is_none() {
                let window = SftpEntryWindow::new().map_err(AppError::from_error)?;
                let host = self.host.clone();
                window.on_canceled(move || {
                    if let Some(host) = host.upgrade() {
                        host.borrow_mut().dialogs.local.sftp_dialog_kind = "none".into();
                    }
                });
                self.sftp_entry = Some(window);
            }
            if let Some(window) = self.sftp_entry.as_ref() {
                window.set_kind(self.local.sftp_dialog_kind.clone().into());
                if let Some(main) = main {
                    window.set_selected_name_text(main.get_sftp_selected_name_text());
                    window.set_selected_path_text(main.get_sftp_selected_path_text());
                    window.set_selected_count(main.get_sftp_selected_count());
                    window.set_name_text(main.get_sftp_dialog_name_text());
                    window.set_permissions_text(main.get_sftp_dialog_permissions_text());
                    window.set_local_path_text(main.get_sftp_dialog_local_text());
                }
                let _ = projection;
                if !self.sftp_entry_shown {
                    show_aux(window)?;
                    self.sftp_entry_shown = true;
                }
            }
        } else if self.sftp_entry_shown {
            if let Some(window) = self.sftp_entry.as_ref() {
                let _ = window.hide();
            }
            self.sftp_entry_shown = false;
        }
        Ok(())
    }
}

fn sync_projection_tool<W, Create, Prep, Wire, SyncProps>(
    want: bool,
    shown: &mut bool,
    slot: &mut Option<W>,
    create: Create,
    prep: Prep,
    wire: Wire,
    sync_props: SyncProps,
    projection: &AppProjection,
    main: Option<&MainWindow>,
) -> AppResult<()>
where
    W: ComponentHandle + 'static,
    Create: FnOnce() -> AppResult<W>,
    Prep: FnOnce(&W),
    Wire: FnOnce(&W),
    SyncProps: FnOnce(&W, &AppProjection, Option<&MainWindow>),
{
    if want {
        if slot.is_none() {
            let window = create()?;
            prep(&window);
            wire(&window);
            *slot = Some(window);
        }
        if let Some(window) = slot.as_ref() {
            sync_props(window, projection, main);
            if !*shown {
                show_aux(window)?;
                *shown = true;
            }
        }
    } else if *shown {
        if let Some(window) = slot.as_ref() {
            let _ = window.hide();
        }
        *shown = false;
    }
    Ok(())
}

fn wire_session_editor_window(window: &SessionEditorWindow, runtime: Rc<RefCell<crate::runtime::AppRuntime>>) {
    wire_tool_title_drag!(window);
    let rt = Rc::clone(&runtime);
    window.on_dismiss(move || {
        let _ = rt.borrow_mut().request_close_session_editor_modal();
    });
    let rt = Rc::clone(&runtime);
    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().request_close_session_editor_modal();
        let _ = weak;
        CloseRequestResponse::HideWindow
    });
}

fn wire_folder_editor_window(window: &FolderEditorWindow, runtime: Rc<RefCell<crate::runtime::AppRuntime>>) {
    wire_tool_title_drag!(window);
    wire_tool_on_close!(window, runtime, |r| r.close_folder_editor());
}

fn wire_known_hosts_window(window: &KnownHostsWindow, runtime: Rc<RefCell<crate::runtime::AppRuntime>>) {
    wire_tool_title_drag!(window);
    wire_tool_on_close!(window, runtime, |r| r.close_known_hosts_manager());
}

fn wire_private_keys_window(window: &PrivateKeysWindow, runtime: Rc<RefCell<crate::runtime::AppRuntime>>) {
    wire_tool_title_drag!(window);
    wire_tool_on_close!(window, runtime, |r| r.close_private_keys_manager());
    let rt = Rc::clone(&runtime);
    window.on_request_remove(move || {
        let _ = rt.borrow_mut().request_private_key_remove();
    });
    let rt = Rc::clone(&runtime);
    window.on_request_deploy(move || {
        let _ = rt.borrow_mut().request_private_key_deploy();
    });
}

fn wire_proxies_window(window: &ProxiesWindow, runtime: Rc<RefCell<crate::runtime::AppRuntime>>) {
    wire_tool_title_drag!(window);
    wire_tool_on_close!(window, runtime, |r| r.close_proxies_manager());
    let rt = Rc::clone(&runtime);
    window.on_request_remove_proxy(move || {
        let _ = rt.borrow_mut().request_proxy_remove();
    });
}

fn wire_theme_manager_window(window: &ThemeManagerWindow, runtime: Rc<RefCell<crate::runtime::AppRuntime>>) {
    wire_tool_title_drag!(window);
    wire_tool_on_close!(window, runtime, |r| r.close_theme_manager());
    let rt = Rc::clone(&runtime);
    window.on_request_delete_theme(move |id: SharedString| {
        let _ = rt.borrow_mut().request_delete_color_theme(id.as_ref());
    });
}

fn wire_host_keys_window(
    window: &HostKeysWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
    host: Weak<RefCell<DialogHost>>,
) {
    wire_tool_title_drag!(window);
    wire_tool_on_close!(window, runtime, |r| r.close_host_keys_manager());
    let host_clear = host.clone();
    window.on_request_clear_all(move || {
        if let Some(host) = host_clear.upgrade() {
            host.borrow_mut().dialogs.local.host_keys_clear = true;
        }
    });
    let host_remove = host.clone();
    window.on_request_remove(move || {
        if let Some(host) = host_remove.upgrade() {
            host.borrow_mut().dialogs.local.host_keys_remove = true;
        }
    });
}

fn wire_logging_window(window: &LoggingWindow, runtime: Rc<RefCell<crate::runtime::AppRuntime>>) {
    wire_tool_title_drag!(window);
    let rt = Rc::clone(&runtime);
    window.on_cancel(move || {
        let _ = rt.borrow_mut().close_logging_dialog();
    });
    let rt = Rc::clone(&runtime);
    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().close_logging_dialog();
        let _ = weak;
        CloseRequestResponse::HideWindow
    });
}

fn wire_password_prompt_window(
    window: &PasswordPromptWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
    wire_tool_title_drag!(window);
    let rt = Rc::clone(&runtime);
    window.on_cancel(move || {
        let _ = rt.borrow_mut().cancel_password_prompt();
    });
    let rt = Rc::clone(&runtime);
    let weak = window.as_weak();
    window.on_submit(move || {
        if let Some(window) = weak.upgrade() {
            let password = window.get_password_text();
            let _ = rt.borrow_mut().submit_password(password.as_ref());
        }
    });
    let rt = Rc::clone(&runtime);
    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().cancel_password_prompt();
        let _ = weak;
        CloseRequestResponse::HideWindow
    });
}

fn wire_auth_prompt_window(window: &AuthPromptWindow, runtime: Rc<RefCell<crate::runtime::AppRuntime>>) {
    wire_tool_title_drag!(window);
    let rt = Rc::clone(&runtime);
    window.on_cancel(move || {
        let _ = rt.borrow_mut().cancel_auth_prompt();
    });
    let rt = Rc::clone(&runtime);
    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().cancel_auth_prompt();
        let _ = weak;
        CloseRequestResponse::HideWindow
    });
}

fn wire_host_key_window(window: &HostKeyWindow, runtime: Rc<RefCell<crate::runtime::AppRuntime>>) {
    wire_tool_title_drag!(window);
    let rt = Rc::clone(&runtime);
    window.on_cancel(move || {
        let _ = rt.borrow_mut().cancel_host_key_prompt();
    });
    let rt = Rc::clone(&runtime);
    window.on_trust_once(move || {
        let _ = rt.borrow_mut().trust_host_key_once();
    });
    let rt = Rc::clone(&runtime);
    window.on_trust_and_save(move || {
        let _ = rt.borrow_mut().trust_host_key_and_save();
    });
    let rt = Rc::clone(&runtime);
    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().cancel_host_key_prompt();
        let _ = weak;
        CloseRequestResponse::HideWindow
    });
}

fn wire_sftp_conflict_window(window: &SftpConflictWindow, runtime: Rc<RefCell<crate::runtime::AppRuntime>>) {
    wire_tool_title_drag!(window);
    let rt = Rc::clone(&runtime);
    window.on_canceled(move || {
        let _ = rt.borrow_mut().cancel_sftp_conflict();
    });
    let rt = Rc::clone(&runtime);
    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().cancel_sftp_conflict();
        let _ = weak;
        CloseRequestResponse::HideWindow
    });
}

fn wire_sftp_properties_window(
    window: &SftpPropertiesWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
    wire_tool_title_drag!(window);
    let rt = Rc::clone(&runtime);
    window.on_canceled(move || {
        let _ = rt.borrow_mut().close_sftp_properties();
    });
    let rt = Rc::clone(&runtime);
    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().close_sftp_properties();
        let _ = weak;
        CloseRequestResponse::HideWindow
    });
}

fn wire_new_folder_window(
    window: &NewFolderWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
    host: Weak<RefCell<DialogHost>>,
    tab_rename: bool,
) {
    wire_tool_title_drag!(window);
    let host_cancel = host.clone();
    window.on_canceled(move || {
        if let Some(host) = host_cancel.upgrade() {
            if tab_rename {
                host.borrow_mut().dialogs.local.tab_rename = false;
            } else {
                host.borrow_mut().dialogs.local.new_folder = false;
            }
        }
    });
    let rt = Rc::clone(&runtime);
    let host_confirm = host.clone();
    window.on_confirmed(move |name| {
        if let Some(host) = host_confirm.upgrade() {
            if tab_rename {
                let tab_id = host.borrow().dialogs.local.tab_rename_tab_id.clone();
                host.borrow_mut().dialogs.local.tab_rename = false;
                if !tab_id.is_empty() {
                    let _ = rt.borrow_mut().rename_tab(tab_id.as_ref(), name.as_ref());
                }
            } else {
                host.borrow_mut().dialogs.local.new_folder = false;
                let _ = rt.borrow_mut().create_root_saved_folder(name.as_ref());
            }
        }
    });
}

fn show_aux<W: ComponentHandle + 'static>(window: &W) -> AppResult<()> {
    window.show().map_err(AppError::from_error)?;
    crate::win_chrome::prepare_aux_window(window.window());
    let weak = window.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = weak.upgrade() {
            crate::win_chrome::prepare_aux_window(window.window());
        }
    });
    Ok(())
}

fn dispatch_confirm(
    host: &Rc<RefCell<DialogHost>>,
    window_runtime: Option<&Rc<WindowRuntime>>,
    kind: Option<ConfirmKind>,
    confirmed: bool,
    dont_ask: bool,
) {
    let Some(kind) = kind else {
        return;
    };
    {
        let mut host = host.borrow_mut();
        match kind {
            ConfirmKind::ClearScrollback => host.dialogs.local.clear_scrollback = false,
            ConfirmKind::DeleteSession => host.dialogs.local.delete_session = false,
            ConfirmKind::Quit => host.dialogs.local.quit = false,
            ConfirmKind::HostKeysClear => host.dialogs.local.host_keys_clear = false,
            ConfirmKind::HostKeysRemove => host.dialogs.local.host_keys_remove = false,
            _ => {}
        }
        host.dialogs.hide_confirm();
    }

    if kind == ConfirmKind::Quit && confirmed {
        if dont_ask {
            let _ = host
                .borrow()
                .dialogs
                .runtime
                .borrow_mut()
                .set_confirm_quit_app(false);
        }
        if let Some(manager) = host.borrow().dialogs.manager.upgrade() {
            manager.borrow_mut().shutdown();
        }
        return;
    }
    if kind == ConfirmKind::CloseWindow {
        let window_id = host.borrow().dialogs.close_window_id;
        if let (Some(manager), Some(window_id)) =
            (host.borrow().dialogs.manager.upgrade(), window_id)
        {
            if confirmed {
                manager
                    .borrow_mut()
                    .confirm_close_window(window_id, dont_ask);
            } else {
                manager.borrow_mut().cancel_close_window(window_id);
            }
        }
        return;
    }

    let apply = |runtime: &mut crate::runtime::AppRuntime| match kind {
        ConfirmKind::ClearScrollback if confirmed => {
            let _ = runtime.clear_active_terminal_scrollback();
        }
        ConfirmKind::DeleteSession if confirmed => {
            let _ = runtime.delete_selected_saved_session();
        }
        ConfirmKind::CloseTabs if confirmed => {
            let _ = runtime.confirm_close_tabs_with(dont_ask);
        }
        ConfirmKind::CloseTabs => {
            let _ = runtime.cancel_close_tabs();
        }
        ConfirmKind::EditorDiscard if confirmed => {
            let _ = runtime.confirm_discard_session_editor();
        }
        ConfirmKind::EditorDiscard => {
            let _ = runtime.cancel_discard_session_editor();
        }
        ConfirmKind::PrivateKeysRemove if confirmed => {
            let _ = runtime.confirm_private_key_remove();
        }
        ConfirmKind::PrivateKeysRemove => {
            let _ = runtime.cancel_private_key_remove();
        }
        ConfirmKind::PrivateKeysDeploy if confirmed => {
            let _ = runtime.confirm_private_key_deploy();
        }
        ConfirmKind::PrivateKeysDeploy => {
            let _ = runtime.cancel_private_key_deploy();
        }
        ConfirmKind::ProxiesRemove if confirmed => {
            let _ = runtime.confirm_proxy_remove();
        }
        ConfirmKind::ProxiesRemove => {
            let _ = runtime.cancel_proxy_remove();
        }
        ConfirmKind::ThemeManagerRemove if confirmed => {
            let _ = runtime.confirm_delete_color_theme();
        }
        ConfirmKind::ThemeManagerRemove => {
            let _ = runtime.cancel_delete_color_theme();
        }
        ConfirmKind::HostKeysClear if confirmed => {
            let _ = runtime.clear_all_host_keys();
        }
        ConfirmKind::HostKeysRemove if confirmed => {
            let _ = runtime.remove_selected_host_key();
        }
        _ => {}
    };

    if let Some(wr) = window_runtime {
        apply(&mut wr.borrow_mut());
    } else {
        apply(&mut host.borrow().dialogs.runtime.borrow_mut());
    }
}

fn close_tabs_copy(projection: &AppProjection) -> (String, String) {
    if projection.close_tabs_confirm_single {
        (
            "Close Session".into(),
            format!(
                "Close “{}”? The active connection will be disconnected.",
                projection.close_tabs_confirm_name_text
            ),
        )
    } else {
        (
            "Close Tabs".into(),
            format!(
                "Close {} tabs? {} active connection(s) will be disconnected.",
                projection.close_tabs_confirm_count, projection.close_tabs_confirm_active_count
            ),
        )
    }
}

fn sync_session_editor(w: &SessionEditorWindow, p: &AppProjection, m: Option<&MainWindow>) {
    w.set_name_text(p.editor_name_text.clone().into());
    w.set_host_text(p.editor_host_text.clone().into());
    w.set_port_text(p.editor_port_text.clone().into());
    w.set_username_text(p.editor_username_text.clone().into());
    if let Some(main) = m {
        w.set_preview_frame(main.get_session_editor_preview());
        w.set_preview_background(main.get_session_editor_preview_background());
    }
}

fn wire_settings_actions(
    window: &SettingsWindow,
    window_runtime: Option<Rc<WindowRuntime>>,
    host: Weak<RefCell<DialogHost>>,
) {
    let wr = window_runtime.clone();
    window.on_open_host_keys(move || {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().open_host_keys_manager();
        }
    });
    let wr = window_runtime.clone();
    window.on_open_private_keys(move || {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().open_private_keys_manager();
        }
    });
    let wr = window_runtime.clone();
    window.on_open_proxies(move || {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().open_proxies_manager();
        }
    });
    let wr = window_runtime;
    window.on_open_theme_manager(move || {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().open_theme_manager();
        }
    });
    let _ = host;
}

fn sync_settings_from(
    settings: &SettingsWindow,
    projection: &AppProjection,
    main: Option<&MainWindow>,
) {
    settings.set_config_dir_text(projection.config_dir_text.clone().into());
    if let Some(main) = main {
        settings.set_scrollback_lines_text(main.get_settings_scrollback_lines_text());
        settings.set_scrollback_max_cells_text(main.get_settings_scrollback_max_cells_text());
        settings.set_settings_sftp_max_parallel_transfers_text(
            main.get_settings_sftp_max_parallel_transfers_text(),
        );
        settings.set_debug_logging_enabled(main.get_settings_debug_logging_enabled());
        settings.set_confirm_close_session_enabled(main.get_settings_confirm_close_session());
        settings.set_confirm_quit_app_enabled(main.get_settings_confirm_quit_app());
        settings.set_language_text(main.get_settings_language_text());
        settings.set_debug_log_dir_text(main.get_settings_debug_log_dir_text());
        settings.set_appearance_color_scheme_name_text(
            main.get_settings_appearance_color_scheme_name_text(),
        );
        settings.set_appearance_color_scheme_builtin(
            main.get_settings_appearance_color_scheme_builtin(),
        );
        settings.set_appearance_font_index(main.get_settings_appearance_font_index());
        settings.set_appearance_font_size_text(main.get_settings_appearance_font_size_text());
        settings.set_appearance_fallback_fonts_text(
            main.get_settings_appearance_fallback_fonts_text(),
        );
        settings.set_appearance_status_text(main.get_settings_appearance_status_text());
        settings.set_appearance_preview(main.get_settings_appearance_preview());
        settings.set_appearance_preview_background(main.get_settings_appearance_preview_background());
    }
    settings.set_confirm_close_session_enabled(projection.settings_confirm_close_session);
    settings.set_confirm_quit_app_enabled(projection.settings_confirm_quit_app);
}

fn sync_folder_editor(w: &FolderEditorWindow, p: &AppProjection, _m: Option<&MainWindow>) {
    w.set_folder_name(p.folder_editor_name_text.clone().into());
    w.set_folder_path(p.folder_editor_path_text.clone().into());
    w.set_status_text(p.folder_editor_status_text.clone().into());
}

fn sync_known_hosts(w: &KnownHostsWindow, p: &AppProjection) {
    w.set_inventory_text(p.known_hosts_inventory_rows_text.clone().into());
    let selection = if p.known_hosts_selection_total > 0 {
        format!(
            "{} {} ({}/{})",
            p.known_hosts_selection_kind_text,
            p.known_hosts_selection_host_text,
            p.known_hosts_selection_index + 1,
            p.known_hosts_selection_total
        )
    } else {
        String::new()
    };
    w.set_selection_text(selection.into());
    w.set_path_text(p.known_hosts_path_text.clone().into());
    w.set_details_text(p.known_hosts_details_text.clone().into());
    w.set_detail_host_text(p.known_hosts_detail_host_text.clone().into());
    w.set_detail_port_text(p.known_hosts_detail_port_text.clone().into());
    w.set_detail_algorithm_text(p.known_hosts_detail_algorithm_text.clone().into());
    w.set_detail_fingerprint_text(p.known_hosts_detail_fingerprint_text.clone().into());
    w.set_detail_path_text(p.known_hosts_detail_path_text.clone().into());
    w.set_clear_confirmation_text(p.known_hosts_clear_confirmation_text.clone().into());
}

fn sync_private_keys(w: &PrivateKeysWindow, p: &AppProjection, m: Option<&MainWindow>) {
    w.set_list_summary_text(p.private_keys_summary_text.clone().into());
    w.set_details_text(p.private_keys_details_text.clone().into());
    w.set_status_text(p.private_keys_status_text.clone().into());
    w.set_status_kind_text(p.private_keys_status_kind_text.clone().into());
    w.set_status_param_1_text(p.private_keys_status_param_1_text.clone().into());
    w.set_status_param_2_text(p.private_keys_status_param_2_text.clone().into());
    w.set_remove_warning_text(p.private_keys_remove_warning_text.clone().into());
    w.set_selected_index(p.private_keys_selected_index);
    // TODO: sync `keys` ModelRc from `p.private_keys_rows` (same as MainWindow projection).
    if let Some(main) = m {
        w.set_import_label_text(main.get_private_keys_import_label_text());
        w.set_import_path_text(main.get_private_keys_import_path_text());
        w.set_passphrase_text(main.get_private_keys_passphrase_text());
    }
}

fn sync_proxies(w: &ProxiesWindow, p: &AppProjection, m: Option<&MainWindow>) {
    w.set_list_summary_text(p.proxies_summary_text.clone().into());
    w.set_status_text(p.proxies_status_text.clone().into());
    w.set_status_kind_text(p.proxies_status_kind_text.clone().into());
    w.set_status_param_1_text(p.proxies_status_param_1_text.clone().into());
    w.set_status_param_2_text(p.proxies_status_param_2_text.clone().into());
    w.set_selected_index(p.proxies_selected_index);
    // TODO: sync `proxies` ModelRc from `p.proxies_rows`.
    let _ = m;
}

fn sync_theme_manager(w: &ThemeManagerWindow, p: &AppProjection, m: Option<&MainWindow>) {
    w.set_default_name_text(p.theme_manager_default_name_text.clone().into());
    w.set_selected_index(p.theme_manager_selected_index);
    w.set_selected_is_default(p.theme_manager_selected_is_default);
    w.set_builtin_count(p.theme_manager_builtin_count);
    w.set_user_count(p.theme_manager_user_count);
    // TODO: sync `themes` ModelRc from `p.theme_manager_rows`.
    if let Some(main) = m {
        w.set_preview(main.get_theme_manager_preview());
        w.set_preview_background(main.get_theme_manager_preview_background());
    }
}

fn sync_host_keys(w: &HostKeysWindow, p: &AppProjection, _m: Option<&MainWindow>) {
    w.set_path_text(p.host_keys_path_text.clone().into());
    w.set_details_text(p.host_keys_details_text.clone().into());
    w.set_status_text(p.host_keys_status_text.clone().into());
    w.set_status_kind_text(p.host_keys_status_kind_text.clone().into());
    w.set_status_param_1_text(p.host_keys_status_param_1_text.clone().into());
    w.set_status_param_2_text(p.host_keys_status_param_2_text.clone().into());
    w.set_key_count(p.host_keys_key_count);
    w.set_selected_group(p.host_keys_selected_group);
    w.set_selected_entry(p.host_keys_selected_entry);
    w.set_detail_host_text(p.host_keys_detail_host_text.clone().into());
    w.set_detail_algorithm_text(p.host_keys_detail_algorithm_text.clone().into());
    w.set_detail_fingerprint_text(p.host_keys_detail_fingerprint_text.clone().into());
    w.set_detail_path_text(p.host_keys_detail_path_text.clone().into());
    w.set_import_visible(p.host_keys_import_visible);
    w.set_import_text(p.host_keys_import_text.clone().into());
    // TODO: sync `groups` ModelRc from `p.host_keys_groups`.
}

fn sync_logging(w: &LoggingWindow, p: &AppProjection, _m: Option<&MainWindow>) {
    w.set_session_name_text(p.logging_dialog_session_text.clone().into());
    w.set_logging_active(p.logging_active);
    w.set_active_path_text(p.logging_path_text.clone().into());
    w.set_directory_text(p.logging_dialog_directory_text.clone().into());
    w.set_file_name_text(p.logging_dialog_file_name_text.clone().into());
}

fn sync_password(w: &PasswordPromptWindow, _p: &AppProjection, m: Option<&MainWindow>) {
    if let Some(main) = m {
        w.set_title_text(main.get_password_prompt_title_text());
        w.set_message_text(main.get_password_prompt_message_text());
        w.set_password_text(main.get_password_prompt_value_text());
        w.set_connect_enabled(!main.get_password_prompt_value_text().is_empty());
    }
}

fn sync_auth(w: &AuthPromptWindow, p: &AppProjection, _m: Option<&MainWindow>) {
    w.set_host_text(p.auth_prompt_host_text.clone().into());
    w.set_problem_text(p.auth_prompt_problem_text.clone().into());
}

fn sync_host_key_prompt(w: &HostKeyWindow, p: &AppProjection) {
    w.set_prompt_text(p.host_key_prompt_text.clone().into());
    w.set_mode_text(p.host_key_prompt_mode_text.clone().into());
    w.set_target_text(p.host_key_prompt_target_text.clone().into());
    w.set_known_hosts_path_text(p.host_key_prompt_known_hosts_path_text.clone().into());
    w.set_algorithm_text(p.host_key_prompt_algorithm_text.clone().into());
    w.set_fingerprint_text(p.host_key_prompt_fingerprint_text.clone().into());
    w.set_expected_algorithm_text(p.host_key_prompt_expected_algorithm_text.clone().into());
    w.set_expected_fingerprint_text(p.host_key_prompt_expected_fingerprint_text.clone().into());
}

fn sync_sftp_conflict(w: &SftpConflictWindow, p: &AppProjection) {
    w.set_source_text(p.sftp_conflict_source_text.clone().into());
    w.set_destination_text(p.sftp_conflict_destination_text.clone().into());
    w.set_conflict_count(p.sftp_conflict_count);
}

fn sync_sftp_properties(w: &SftpPropertiesWindow, p: &AppProjection) {
    w.set_name_text(p.sftp_properties_name_text.clone().into());
    w.set_path_text(p.sftp_properties_path_text.clone().into());
}
