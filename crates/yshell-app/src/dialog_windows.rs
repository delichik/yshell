//! Independent modal aux dialog / tool windows.
//!
//! Confirms, managers and prompts are separate Slint `Window`s. On Windows they
//! are owned by the focused MainWindow (no taskbar button) and modal (owner
//! input disabled while any aux is open).
//!
//! Opening signal: historical `*_visible` / projection flags. Each tick,
//! [`DialogWindowManager::sync`] show/hides aux windows.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::bootstrap::generated_ui::{
    AboutWindow, AuthKeyOption, AuthPromptWindow, CompactConfirmWindow, EditorTunnelRow,
    FolderEditorWindow, HostKeyWindow, HostKeysWindow, InheritSource, KnownHostsWindow,
    LoggingWindow, LoginStepKeysWindow, LoginStepRow, LoginStepWaitWindow, MainWindow,
    MasterPasswordUnlockWindow, MasterPasswordWindow, NewFolderWindow, PasswordPromptWindow,
    PrivateKeysWindow, ProxiesWindow, SessionEditorWindow, SettingsWindow, SftpConflictWindow,
    SftpEntryWindow, SftpPropertiesWindow, Theme, ThemeManagerWindow, TunnelEditorWindow,
};
use crate::error::{AppError, AppResult};
use crate::runtime::{
    AppProjection, EditorInheritSourceData, EditorLoginStepRowData, EditorTunnelRowData,
};
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
                // HTCAPTION 移动循环吞掉 mouse-up：移动结束后清掉 Slint 残留的抓取。
                crate::win_chrome::reset_pointer_state_after_native_move(window.as_weak());
            }
        });
    }};
}

macro_rules! wire_tool_on_close {
    ($window:expr, $runtime:expr, $close:expr) => {{
        let close_fn: fn(&mut crate::runtime::AppRuntime) -> crate::runtime::AppProjection = $close;
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
    McpControl,
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
    /// Nested modal aux count; owner re-enabled when this hits 0.
    /// `Cell` so show/hide helpers can run under shared `&self` next to field borrows.
    modal_depth: Cell<u32>,
    /// Main window id whose input is disabled for the modal session.
    modal_owner_id: Cell<Option<u64>>,
    /// Owner re-enabled/focused after a modal hide that happened inside the app
    /// tick (the timer already holds the manager borrow there).
    pending_restore_owner: Cell<Option<u64>>,
    local: LocalDialogFlags,
    about: Option<AboutWindow>,
    about_shown: bool,
    confirm: Option<CompactConfirmWindow>,
    confirm_kind: Option<ConfirmKind>,
    settings: Option<SettingsWindow>,
    settings_shown: bool,
    // D35: master-password sub-dialog + startup unlock are independent compact
    // tool windows created lazily when their projection flag turns true.
    master_password: Option<MasterPasswordWindow>,
    master_password_shown: bool,
    master_password_unlock: Option<MasterPasswordUnlockWindow>,
    master_password_unlock_shown: bool,
    // Additional tool windows are created lazily in sync_* helpers.
    session_editor: Option<SessionEditorWindow>,
    session_editor_shown: bool,
    // Session Editor 的子窗（Login Script 步骤 / Tunnel 编辑）：叠在编辑器之上。
    login_step_keys: Option<LoginStepKeysWindow>,
    login_step_keys_shown: bool,
    login_step_wait: Option<LoginStepWaitWindow>,
    login_step_wait_shown: bool,
    tunnel_editor: Option<TunnelEditorWindow>,
    tunnel_editor_shown: bool,
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
    pub(crate) fn create(
        runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
    ) -> Rc<RefCell<DialogHost>> {
        let host = Rc::new(RefCell::new(DialogHost {
            dialogs: DialogWindowManager {
                runtime,
                host: Weak::new(),
                manager: Weak::new(),
                close_window_id: None,
                modal_depth: Cell::new(0),
                modal_owner_id: Cell::new(None),
                pending_restore_owner: Cell::new(None),
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
                master_password: None,
                master_password_shown: false,
                master_password_unlock: None,
                master_password_unlock_shown: false,
                session_editor: None,
                session_editor_shown: false,
                login_step_keys: None,
                login_step_keys_shown: false,
                login_step_wait: None,
                login_step_wait_shown: false,
                tunnel_editor: None,
                tunnel_editor_shown: false,
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
        owner_id: Option<u64>,
        window_runtime: Option<Rc<WindowRuntime>>,
    ) -> AppResult<()> {
        let runtime = Rc::clone(&self.runtime);
        let host = self.host.clone();
        self.sync_about(projection, main, owner_id)?;
        self.sync_confirm(projection, main, owner_id, window_runtime.clone())?;
        self.sync_settings(projection, main, owner_id, window_runtime.clone())?;
        // D35: startup unlock first, then the Set/Change/Remove/Reset sub-dialog,
        // so "Forgot password?" from the unlock window stacks the Reset dialog
        // above it (Cancel returns to the unlock window).
        self.sync_master_password_unlock(projection, main, owner_id)?;
        self.sync_master_password(projection, main, owner_id)?;
        macro_rules! apply_modal {
            ($action:expr, $slot:ident) => {{
                match $action {
                    ModalAction::Show => {
                        if let Some(window) = self.$slot.as_ref() {
                            self.show_aux_modal(window, main, owner_id)?;
                        }
                    }
                    ModalAction::Hide => self.end_modal(),
                    ModalAction::None => {}
                }
            }};
        }
        apply_modal!(
            sync_projection_tool(
                projection.editor_modal_visible,
                &mut self.session_editor_shown,
                &mut self.session_editor,
                || SessionEditorWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_session_editor_window(w, Rc::clone(&runtime)),
                sync_session_editor,
                projection,
                main,
            )?,
            session_editor
        );
        // Session Editor 子窗：Login Script 步骤（Keys / Wait）与 Tunnel 编辑。
        // 只在对应分区/编辑态需要时叠在编辑器之上；同一时刻两窗最多一个可见。
        apply_modal!(
            sync_projection_tool(
                projection.editor_login_step_dialog_visible
                    && projection.editor_login_step_kind_text == "keys",
                &mut self.login_step_keys_shown,
                &mut self.login_step_keys,
                || LoginStepKeysWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_login_step_keys_window(w, Rc::clone(&runtime)),
                sync_login_step_keys,
                projection,
                main,
            )?,
            login_step_keys
        );
        apply_modal!(
            sync_projection_tool(
                projection.editor_login_step_dialog_visible
                    && projection.editor_login_step_kind_text == "wait",
                &mut self.login_step_wait_shown,
                &mut self.login_step_wait,
                || LoginStepWaitWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_login_step_wait_window(w, Rc::clone(&runtime)),
                sync_login_step_wait,
                projection,
                main,
            )?,
            login_step_wait
        );
        apply_modal!(
            sync_projection_tool(
                projection.editor_tunnel_dialog_visible,
                &mut self.tunnel_editor_shown,
                &mut self.tunnel_editor,
                || TunnelEditorWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_tunnel_editor_window(w, Rc::clone(&runtime)),
                sync_tunnel_editor,
                projection,
                main,
            )?,
            tunnel_editor
        );
        apply_modal!(
            sync_projection_tool(
                projection.folder_editor_visible,
                &mut self.folder_editor_shown,
                &mut self.folder_editor,
                || FolderEditorWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_folder_editor_window(w, Rc::clone(&runtime)),
                sync_folder_editor,
                projection,
                main,
            )?,
            folder_editor
        );
        apply_modal!(
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
            )?,
            known_hosts
        );
        apply_modal!(
            sync_projection_tool(
                projection.private_keys_modal_visible,
                &mut self.private_keys_shown,
                &mut self.private_keys,
                || PrivateKeysWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_private_keys_window(w, Rc::clone(&runtime)),
                sync_private_keys,
                projection,
                main,
            )?,
            private_keys
        );
        apply_modal!(
            sync_projection_tool(
                projection.proxies_modal_visible,
                &mut self.proxies_shown,
                &mut self.proxies,
                || ProxiesWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_proxies_window(w, Rc::clone(&runtime)),
                sync_proxies,
                projection,
                main,
            )?,
            proxies
        );
        apply_modal!(
            sync_projection_tool(
                projection.theme_manager_visible,
                &mut self.theme_manager_shown,
                &mut self.theme_manager,
                || ThemeManagerWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_theme_manager_window(w, Rc::clone(&runtime)),
                sync_theme_manager,
                projection,
                main,
            )?,
            theme_manager
        );
        apply_modal!(
            sync_projection_tool(
                projection.host_keys_modal_visible,
                &mut self.host_keys_shown,
                &mut self.host_keys,
                || HostKeysWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_host_keys_window(w, Rc::clone(&runtime), host.clone()),
                sync_host_keys,
                projection,
                main,
            )?,
            host_keys
        );
        apply_modal!(
            sync_projection_tool(
                projection.logging_dialog_visible,
                &mut self.logging_shown,
                &mut self.logging,
                || LoggingWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_logging_window(w, Rc::clone(&runtime)),
                sync_logging,
                projection,
                main,
            )?,
            logging
        );
        apply_modal!(
            sync_projection_tool(
                projection.password_prompt_visible,
                &mut self.password_shown,
                &mut self.password,
                || PasswordPromptWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_password_prompt_window(w, Rc::clone(&runtime)),
                sync_password,
                projection,
                main,
            )?,
            password
        );
        apply_modal!(
            sync_projection_tool(
                projection.auth_prompt_visible,
                &mut self.auth_shown,
                &mut self.auth,
                || AuthPromptWindow::new().map_err(AppError::from_error),
                |w| prep_tool_window!(w),
                |w| wire_auth_prompt_window(w, Rc::clone(&runtime)),
                sync_auth,
                projection,
                main,
            )?,
            auth
        );
        apply_modal!(
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
            )?,
            host_key
        );
        self.sync_local_new_folder(Rc::clone(&runtime), host.clone(), main, owner_id)?;
        self.sync_local_tab_rename(Rc::clone(&runtime), host.clone(), main, owner_id)?;
        self.sync_sftp_entry(projection, main, owner_id)?;
        apply_modal!(
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
            )?,
            sftp_conflict
        );
        apply_modal!(
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
            )?,
            sftp_properties
        );
        let _ = window_runtime;
        Ok(())
    }

    fn sync_about(
        &mut self,
        projection: &AppProjection,
        main: Option<&MainWindow>,
        owner_id: Option<u64>,
    ) -> AppResult<()> {
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
                            crate::win_chrome::reset_pointer_state_after_native_move(
                                about.as_weak(),
                            );
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
                    self.show_aux_modal(about, main, owner_id)?;
                    self.about_shown = true;
                }
            }
        } else {
            self.hide_about();
        }
        Ok(())
    }

    fn hide_about(&mut self) {
        if !self.about_shown {
            return;
        }
        if let Some(about) = self.about.as_ref() {
            let _ = about.hide();
        }
        self.about_shown = false;
        self.end_modal();
    }

    fn sync_confirm(
        &mut self,
        projection: &AppProjection,
        main: Option<&MainWindow>,
        owner_id: Option<u64>,
        window_runtime: Option<Rc<WindowRuntime>>,
    ) -> AppResult<()> {
        match self.active_confirm_spec(projection, main) {
            Some(spec) => self.show_confirm(spec, main, owner_id, window_runtime)?,
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
                dont_ask: main
                    .map(|w| w.get_close_confirm_dont_ask_again())
                    .unwrap_or(false),
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
        if projection.mcp_consent_visible {
            return Some(ConfirmSpec {
                kind: ConfirmKind::McpControl,
                title: "Allow AI control of terminals".into(),
                message: "An AI agent wants control of YShell terminal sessions: run commands, read the screen, and open or close tabs. It cannot fill passwords or accept host keys — those stay in this app. Allow?".into(),
                confirm_label: "Allow".into(),
                destructive: false,
                show_dont_ask: true,
                dont_ask: projection.settings_mcp_auto_approve_control,
            });
        }
        None
    }

    fn show_confirm(
        &mut self,
        spec: ConfirmSpec,
        main: Option<&MainWindow>,
        owner_id: Option<u64>,
        window_runtime: Option<Rc<WindowRuntime>>,
    ) -> AppResult<()> {
        if self.confirm.is_none() {
            let window = CompactConfirmWindow::new().map_err(AppError::from_error)?;
            prep_tool_window!(window);
            wire_tool_title_drag!(window);
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
        let was_open = self.confirm_kind.is_some();
        self.confirm_kind = Some(spec.kind);
        if !was_open {
            self.show_aux_modal(window, main, owner_id)?;
        }
        window.invoke_focus_dialog();
        Ok(())
    }

    fn hide_confirm(&mut self) {
        let was_shown = self.confirm_kind.is_some();
        if let Some(window) = self.confirm.as_ref() {
            let _ = window.hide();
        }
        self.confirm_kind = None;
        if was_shown {
            self.end_modal();
        }
    }

    fn wire_confirm(
        &self,
        window: &CompactConfirmWindow,
        window_runtime: Option<Rc<WindowRuntime>>,
    ) {
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
        owner_id: Option<u64>,
        window_runtime: Option<Rc<WindowRuntime>>,
    ) -> AppResult<()> {
        let want = projection.settings_appearance_visible || self.local.settings;
        if projection.settings_appearance_visible {
            self.local.settings = true;
        }
        if want {
            if self.settings.is_none() {
                let window = SettingsWindow::new().map_err(AppError::from_error)?;
                window.set_native_title_drag(cfg!(windows));
                let host = self.host.clone();
                let runtime = Rc::clone(&self.runtime);
                window.on_dismiss(move || {
                    {
                        // Cancel / × / Esc: discard every draft. `close_settings_appearance`
                        // reloads the persisted baseline; marking the UI dirty makes the
                        // next tick re-push the reverted App theme to every window.
                        let mut runtime = runtime.borrow_mut();
                        let _ = runtime.close_settings_appearance();
                        runtime.mark_ui_dirty();
                    }
                    if let Some(host) = host.upgrade() {
                        let mut host = host.borrow_mut();
                        host.dialogs.local.settings = false;
                        if host.dialogs.settings_shown {
                            if let Some(settings) = host.dialogs.settings.as_ref() {
                                let _ = settings.hide();
                            }
                            host.dialogs.settings_shown = false;
                            host.dialogs.end_modal();
                        }
                    }
                });
                let host = self.host.clone();
                let runtime = Rc::clone(&self.runtime);
                window.window().on_close_requested(move || {
                    {
                        // Cancel / × / Esc: discard every draft. `close_settings_appearance`
                        // reloads the persisted baseline; marking the UI dirty makes the
                        // next tick re-push the reverted App theme to every window.
                        let mut runtime = runtime.borrow_mut();
                        let _ = runtime.close_settings_appearance();
                        runtime.mark_ui_dirty();
                    }
                    if let Some(host) = host.upgrade() {
                        let mut host = host.borrow_mut();
                        host.dialogs.local.settings = false;
                        if host.dialogs.settings_shown {
                            if let Some(settings) = host.dialogs.settings.as_ref() {
                                let _ = settings.hide();
                            }
                            host.dialogs.settings_shown = false;
                            host.dialogs.end_modal();
                        }
                    }
                    CloseRequestResponse::HideWindow
                });
                let host = self.host.clone();
                window.on_title_drag_requested(move || {
                    if let Some(host) = host.upgrade() {
                        if let Some(settings) = host.borrow().dialogs.settings.as_ref() {
                            crate::win_chrome::start_native_window_move(settings.window());
                            crate::win_chrome::reset_pointer_state_after_native_move(
                                settings.as_weak(),
                            );
                        }
                    }
                });
                wire_settings_actions(&window, window_runtime.clone(), self.host.clone());
                self.settings = Some(window);
            }
            if let Some(settings) = self.settings.as_ref() {
                let reduce_motion = self.runtime.borrow().config_document.ui.reduce_motion;
                sync_settings_from(settings, projection, main, reduce_motion);
                if !self.settings_shown {
                    self.show_aux_modal(settings, main, owner_id)?;
                    self.settings_shown = true;
                }
            }
        } else if self.settings_shown {
            if let Some(settings) = self.settings.as_ref() {
                let _ = settings.hide();
            }
            self.settings_shown = false;
            self.local.settings = false;
            self.end_modal();
        }
        Ok(())
    }

    /// D35: startup master-password unlock window (`Quit` / `×` / Esc exits).
    fn sync_master_password_unlock(
        &mut self,
        projection: &AppProjection,
        main: Option<&MainWindow>,
        owner_id: Option<u64>,
    ) -> AppResult<()> {
        let manager = self.manager.clone();
        let runtime = Rc::clone(&self.runtime);
        let action = sync_projection_tool(
            projection.settings_master_password_unlock_visible,
            &mut self.master_password_unlock_shown,
            &mut self.master_password_unlock,
            || MasterPasswordUnlockWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_master_password_unlock_window(w, runtime, manager),
            sync_master_password_unlock_props,
            projection,
            main,
        )?;
        match action {
            ModalAction::Show => {
                if let Some(window) = self.master_password_unlock.as_ref() {
                    // The projection never carries plaintext back: a fresh gate
                    // always starts with an empty field.
                    window.set_password_text("".into());
                    self.show_aux_modal(window, main, owner_id)?;
                    window.invoke_focus_dialog();
                }
            }
            ModalAction::Hide => self.end_modal(),
            ModalAction::None => {}
        }
        Ok(())
    }

    /// D35: Settings → Security master-password sub-dialog.
    fn sync_master_password(
        &mut self,
        projection: &AppProjection,
        main: Option<&MainWindow>,
        owner_id: Option<u64>,
    ) -> AppResult<()> {
        let runtime = Rc::clone(&self.runtime);
        let action = sync_projection_tool(
            projection.settings_master_password_dialog_visible,
            &mut self.master_password_shown,
            &mut self.master_password,
            || MasterPasswordWindow::new().map_err(AppError::from_error),
            |w| prep_tool_window!(w),
            |w| wire_master_password_window(w, runtime),
            sync_master_password_props,
            projection,
            main,
        )?;
        match action {
            ModalAction::Show => {
                if let Some(window) = self.master_password.as_ref() {
                    // Reused window: clear the previous draft's plaintext (it is
                    // never projected back) and hand over the keyboard.
                    window.set_current_text("".into());
                    window.set_new_text("".into());
                    window.set_confirm_text("".into());
                    self.show_aux_modal(window, main, owner_id)?;
                    window.invoke_focus_dialog();
                }
            }
            ModalAction::Hide => self.end_modal(),
            ModalAction::None => {}
        }
        Ok(())
    }

    fn sync_local_new_folder(
        &mut self,
        runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
        host: Weak<RefCell<DialogHost>>,
        main: Option<&MainWindow>,
        owner_id: Option<u64>,
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
                    self.show_aux_modal(window, main, owner_id)?;
                    self.new_folder_shown = true;
                }
            }
        } else if self.new_folder_shown {
            if let Some(window) = self.new_folder.as_ref() {
                let _ = window.hide();
            }
            self.new_folder_shown = false;
            self.end_modal();
        }
        Ok(())
    }

    fn sync_local_tab_rename(
        &mut self,
        runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
        host: Weak<RefCell<DialogHost>>,
        main: Option<&MainWindow>,
        owner_id: Option<u64>,
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
                    self.show_aux_modal(window, main, owner_id)?;
                    self.tab_rename_shown = true;
                }
            }
        } else if self.tab_rename_shown {
            if let Some(window) = self.tab_rename.as_ref() {
                let _ = window.hide();
            }
            self.tab_rename_shown = false;
            self.end_modal();
        }
        Ok(())
    }

    fn sync_sftp_entry(
        &mut self,
        projection: &AppProjection,
        main: Option<&MainWindow>,
        owner_id: Option<u64>,
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
                    self.show_aux_modal(window, main, owner_id)?;
                    self.sftp_entry_shown = true;
                }
            }
        } else if self.sftp_entry_shown {
            if let Some(window) = self.sftp_entry.as_ref() {
                let _ = window.hide();
            }
            self.sftp_entry_shown = false;
            self.end_modal();
        }
        Ok(())
    }

    fn begin_modal(&self, owner: &MainWindow, owner_id: Option<u64>) {
        if self.modal_depth.get() == 0 {
            crate::win_chrome::set_window_enabled(owner.window(), false);
            self.modal_owner_id.set(owner_id);
        }
        self.modal_depth
            .set(self.modal_depth.get().saturating_add(1));
    }

    pub(crate) fn end_modal(&self) {
        let depth = self.modal_depth.get();
        if depth == 0 {
            return;
        }
        let depth = depth - 1;
        self.modal_depth.set(depth);
        if depth > 0 {
            return;
        }
        let owner_id = self.modal_owner_id.take();
        let Some(owner_id) = owner_id else {
            return;
        };
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        if let Ok(mut manager) = manager.try_borrow_mut() {
            manager.set_main_window_enabled(owner_id, true);
            // 系统不会因为 owned 子窗隐藏就把前台还给 owner；显式激活父窗并把
            // Slint 键盘焦点交还快捷键 FocusScope，否则关窗后快捷键不响应。
            manager.restore_main_window_focus(owner_id);
            return;
        }
        // Called from inside the app tick (`sync_dialog_windows` hides a window
        // whose projection flag cleared): the manager is already mutably
        // borrowed by the timer. Record the owner and let the tick re-enable it
        // right after the dialog sync returns (see `take_pending_restore_owner`).
        self.pending_restore_owner.set(Some(owner_id));
    }

    /// Owner window id whose modal state ended inside the app tick.
    ///
    /// `WindowManager::sync_dialog_windows` consumes this after the dialog sync
    /// (the timer already holds the manager borrow while dialogs sync).
    pub(crate) fn take_pending_restore_owner(&self) -> Option<u64> {
        self.pending_restore_owner.take()
    }

    fn show_aux_modal<W: ComponentHandle + 'static>(
        &self,
        window: &W,
        owner: Option<&MainWindow>,
        owner_id: Option<u64>,
    ) -> AppResult<()> {
        window.show().map_err(AppError::from_error)?;
        crate::win_chrome::prepare_aux_window(window.window());
        if let Some(owner) = owner {
            crate::win_chrome::attach_owned_popup(window.window(), owner.window());
            // 子窗默认居中于父窗客户区（Windows）。show 时 HWND 往往还没创建，
            // 先就地试一次，再由 `finish_owned_aux_window_when_ready` 在句柄就绪
            // 后补 owner 关系 + 居中。
            crate::win_chrome::center_aux_window(window.window(), owner.window());
            self.begin_modal(owner, owner_id);
            crate::win_chrome::finish_owned_aux_window_when_ready(
                window.as_weak(),
                owner.as_weak(),
                AUX_WINDOW_READY_ATTEMPTS,
            );
        } else {
            crate::win_chrome::prepare_aux_window_when_ready(
                window.as_weak(),
                AUX_WINDOW_READY_ATTEMPTS,
            );
        }
        Ok(())
    }
}

enum ModalAction {
    None,
    Show,
    Hide,
}

/// 子窗 `show()` 后等待 winit 惰性创建 HWND 的事件循环轮数上限。
/// 实测 2~3 轮即就绪；留 8 轮覆盖慢机器/首次创建窗口。
const AUX_WINDOW_READY_ATTEMPTS: u8 = 8;

// A dialog pipeline has one parameter per stage (create / prep / wire / sync /
// projection / main); bundling them into a struct would only hide the coupling.
#[allow(clippy::too_many_arguments)]
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
) -> AppResult<ModalAction>
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
                *shown = true;
                return Ok(ModalAction::Show);
            }
        }
    } else if *shown {
        if let Some(window) = slot.as_ref() {
            let _ = window.hide();
        }
        *shown = false;
        return Ok(ModalAction::Hide);
    }
    Ok(ModalAction::None)
}

/// Session Editor 回调：执行运行时动作并把 UI 标记为脏（下一个 tick 重新投影）。
macro_rules! editor_cb {
    ($window:expr, $runtime:expr, $callback:ident, |$rt:ident| $body:expr) => {{
        let rt = Rc::clone(&$runtime);
        $window.$callback(move || {
            let mut $rt = rt.borrow_mut();
            #[allow(unused_must_use)]
            {
                $body;
            }
            $rt.mark_ui_dirty();
        });
    }};
}

/// 同 [`editor_cb!`]，但回调带一个参数（string / int）。
macro_rules! editor_cb1 {
    ($window:expr, $runtime:expr, $callback:ident, $arg:ident : $arg_ty:ty, |$rt:ident| $body:expr) => {{
        let rt = Rc::clone(&$runtime);
        $window.$callback(move |$arg: $arg_ty| {
            let mut $rt = rt.borrow_mut();
            #[allow(unused_must_use)]
            {
                $body;
            }
            $rt.mark_ui_dirty();
        });
    }};
}

/// 同 [`editor_cb!`]，但动作为 `AppResult`：失败写状态栏（窗口保持打开）。
macro_rules! editor_cb_result {
    ($window:expr, $runtime:expr, $callback:ident, |$rt:ident| $body:expr) => {{
        let rt = Rc::clone(&$runtime);
        $window.$callback(move || {
            let mut $rt = rt.borrow_mut();
            if let Err(error) = $body {
                $rt.set_status_kind(
                    "editor-submit-failed",
                    format!("{error}"),
                    String::new(),
                    String::new(),
                );
            }
            $rt.mark_ui_dirty();
        });
    }};
}

fn wire_session_editor_window(
    window: &SessionEditorWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
    wire_tool_title_drag!(window);
    editor_cb!(window, runtime, on_dismiss, |rt| {
        let _ = rt.request_close_session_editor_modal();
    });
    let rt = Rc::clone(&runtime);
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().request_close_session_editor_modal();
        CloseRequestResponse::HideWindow
    });

    // --- 分区导航 / 页级 Inherit -----------------------------------------
    editor_cb!(window, runtime, on_select_general, |rt| {
        let _ = rt.select_session_editor_general();
    });
    editor_cb!(window, runtime, on_select_authentication, |rt| {
        let _ = rt.select_session_editor_authentication();
    });
    editor_cb!(window, runtime, on_select_login_script, |rt| {
        let _ = rt.select_session_editor_login_script();
    });
    editor_cb!(window, runtime, on_select_proxy, |rt| {
        let _ = rt.select_session_editor_proxy();
    });
    editor_cb!(window, runtime, on_select_tunnels, |rt| {
        let _ = rt.select_session_editor_tunnels();
    });
    editor_cb!(window, runtime, on_select_terminal_style, |rt| {
        let _ = rt.select_session_editor_appearance();
    });
    editor_cb1!(window, runtime, on_toggle_inherit_auth, value: bool, |rt| {
        let _ = rt.set_editor_inherit_auth(value);
    });
    editor_cb1!(
        window,
        runtime,
        on_toggle_inherit_login_script,
        value: bool,
        |rt| {
            let _ = rt.set_editor_inherit_login_script(value);
        }
    );
    editor_cb1!(window, runtime, on_toggle_inherit_proxy, value: bool, |rt| {
        let _ = rt.set_editor_inherit_proxy(value);
    });
    editor_cb1!(
        window,
        runtime,
        on_toggle_inherit_tunnels,
        value: bool,
        |rt| {
            let _ = rt.set_editor_inherit_tunnels(value);
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_toggle_inherit_terminal_style,
        value: bool,
        |rt| {
            let _ = rt.set_editor_inherit_terminal_style(value);
        }
    );

    // --- General：Name / Host / Port / Username / Icon / Notes -------------
    editor_cb1!(window, runtime, on_update_name, value: SharedString, |rt| {
        let _ = rt.update_editor_name(value.as_ref());
    });
    editor_cb1!(window, runtime, on_update_host, value: SharedString, |rt| {
        let _ = rt.update_editor_host(value.as_ref());
    });
    editor_cb1!(window, runtime, on_update_port, value: SharedString, |rt| {
        let _ = rt.update_editor_port(value.as_ref());
    });
    editor_cb1!(
        window,
        runtime,
        on_update_username,
        value: SharedString,
        |rt| {
            let _ = rt.update_editor_username(value.as_ref());
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_update_icon_index,
        value: i32,
        |rt| {
            let _ = rt.update_editor_icon_index(value);
        }
    );
    editor_cb!(window, runtime, on_browse_icon, |rt| {
        rt.request_editor_icon_browse();
    });
    editor_cb!(window, runtime, on_reset_icon, |rt| {
        let _ = rt.reset_editor_icon();
    });
    editor_cb1!(window, runtime, on_update_notes, value: SharedString, |rt| {
        let _ = rt.update_editor_notes(value.as_ref());
    });

    // --- Authentication ---------------------------------------------------
    editor_cb!(window, runtime, on_set_auth_method_password, |rt| {
        let _ = rt.set_editor_auth_method_password();
    });
    editor_cb!(window, runtime, on_set_auth_method_private_key, |rt| {
        let _ = rt.set_editor_auth_method_private_key();
    });
    editor_cb!(
        window,
        runtime,
        on_set_auth_method_keyboard_interactive,
        |rt| {
            let _ = rt.set_editor_auth_method_keyboard_interactive();
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_update_password,
        value: SharedString,
        |rt| {
            let _ = rt.update_editor_password(value.as_ref());
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_update_passphrase,
        value: SharedString,
        |rt| {
            let _ = rt.update_editor_passphrase(value.as_ref());
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_select_key,
        value: i32,
        |rt| {
            let _ = rt.select_editor_key(value);
        }
    );
    editor_cb!(window, runtime, on_manage_keys, |rt| {
        let _ = rt.open_private_keys_manager();
    });

    // --- Login Script 清单 ------------------------------------------------
    editor_cb1!(
        window,
        runtime,
        on_add_login_step,
        kind: SharedString,
        |rt| {
            let _ = rt.add_editor_login_step(kind.as_ref());
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_edit_login_step,
        index: i32,
        |rt| {
            let _ = rt.edit_editor_login_step(index);
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_select_login_step,
        index: i32,
        |rt| {
            let _ = rt.select_editor_login_step(index);
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_duplicate_login_step,
        index: i32,
        |rt| {
            let _ = rt.duplicate_editor_login_step(index);
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_delete_login_step,
        index: i32,
        |rt| {
            let _ = rt.delete_editor_login_step(index);
        }
    );
    let rt = Rc::clone(&runtime);
    window.on_move_login_step(move |from: i32, to: i32| {
        let mut runtime = rt.borrow_mut();
        let _ = runtime.move_editor_login_step(from, to);
        runtime.mark_ui_dirty();
    });

    // --- Tunnels 清单 -----------------------------------------------------
    editor_cb!(window, runtime, on_add_tunnel, |rt| {
        let _ = rt.begin_add_editor_tunnel();
    });
    editor_cb1!(window, runtime, on_edit_tunnel, index: i32, |rt| {
        let _ = rt.begin_edit_editor_tunnel(index);
    });
    editor_cb1!(window, runtime, on_select_tunnel, index: i32, |rt| {
        let _ = rt.select_editor_tunnel(index);
    });
    editor_cb1!(
        window,
        runtime,
        on_duplicate_tunnel,
        index: i32,
        |rt| {
            let _ = rt.duplicate_editor_tunnel(index);
        }
    );
    editor_cb1!(window, runtime, on_delete_tunnel, index: i32, |rt| {
        let _ = rt.delete_editor_tunnel(index);
    });
    editor_cb!(window, runtime, on_clear_tunnels, |rt| {
        let _ = rt.clear_editor_tunnels();
    });

    // --- Proxy / Terminal Style ------------------------------------------
    editor_cb!(window, runtime, on_set_proxy_mode_none, |rt| {
        let _ = rt.set_editor_proxy_mode_none();
    });
    editor_cb1!(
        window,
        runtime,
        on_select_proxy_profile,
        index: i32,
        |rt| {
            let _ = rt.select_editor_proxy(index);
        }
    );
    editor_cb!(window, runtime, on_manage_proxies, |rt| {
        let _ = rt.open_proxies_manager();
    });
    editor_cb1!(window, runtime, on_select_scheme, index: i32, |rt| {
        let _ = rt.select_editor_scheme_index(index);
    });
    editor_cb!(window, runtime, on_manage_terminal_styles, |rt| {
        let _ = rt.open_theme_manager();
    });

    // --- 页脚：Test Connection | Cancel | Save | Save and Connect ---------
    editor_cb!(window, runtime, on_test_auth, |rt| {
        let _ = rt.test_editor_auth();
    });
    editor_cb_result!(window, runtime, on_save, |rt| rt
        .save_editor_to_saved_session());
    editor_cb_result!(window, runtime, on_save_connect, |rt| rt
        .save_editor_and_connect());
}

/// Login Script · Keys 子窗：保存 / 删除 / 取消与字段编辑。
fn wire_login_step_keys_window(
    window: &LoginStepKeysWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
    wire_tool_title_drag!(window);
    editor_cb!(window, runtime, on_dismiss, |rt| {
        let _ = rt.cancel_editor_login_step();
    });
    let rt = Rc::clone(&runtime);
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().cancel_editor_login_step();
        CloseRequestResponse::HideWindow
    });
    editor_cb_result!(window, runtime, on_save, |rt| rt.save_editor_login_step());
    editor_cb!(window, runtime, on_delete_step, |rt| {
        let index = rt.editor_selected_login_step_index();
        let _ = rt.delete_editor_login_step(index);
    });
    editor_cb1!(window, runtime, on_update_name, value: SharedString, |rt| {
        let _ = rt.update_editor_login_step_name(value.as_ref());
    });
    editor_cb1!(window, runtime, on_update_keys, value: SharedString, |rt| {
        let _ = rt.update_editor_login_step_keys(value.as_ref());
    });
    editor_cb1!(window, runtime, on_set_tokens, tokens: bool, |rt| {
        let _ = rt.set_editor_login_step_tokens(tokens);
    });
}

/// Login Script · Wait 子窗：保存 / 删除 / 取消与毫秒编辑。
fn wire_login_step_wait_window(
    window: &LoginStepWaitWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
    wire_tool_title_drag!(window);
    editor_cb!(window, runtime, on_dismiss, |rt| {
        let _ = rt.cancel_editor_login_step();
    });
    let rt = Rc::clone(&runtime);
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().cancel_editor_login_step();
        CloseRequestResponse::HideWindow
    });
    editor_cb_result!(window, runtime, on_save, |rt| rt.save_editor_login_step());
    editor_cb!(window, runtime, on_delete_step, |rt| {
        let index = rt.editor_selected_login_step_index();
        let _ = rt.delete_editor_login_step(index);
    });
    editor_cb1!(window, runtime, on_update_millis, value: SharedString, |rt| {
        let _ = rt.update_editor_login_step_millis(value.as_ref());
    });
}

/// Tunnel Editor 子窗：保存 / 取消与字段编辑。
fn wire_tunnel_editor_window(
    window: &TunnelEditorWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
    wire_tool_title_drag!(window);
    editor_cb!(window, runtime, on_dismiss, |rt| {
        let _ = rt.cancel_editor_tunnel_dialog();
    });
    let rt = Rc::clone(&runtime);
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().cancel_editor_tunnel_dialog();
        CloseRequestResponse::HideWindow
    });
    editor_cb_result!(window, runtime, on_save, |rt| rt
        .save_editor_tunnel_dialog());
    editor_cb1!(window, runtime, on_update_name, value: SharedString, |rt| {
        let _ = rt.update_editor_tunnel_name(value.as_ref());
    });
    editor_cb1!(window, runtime, on_set_kind, kind: SharedString, |rt| {
        let _ = rt.set_editor_tunnel_kind(kind.as_ref());
    });
    editor_cb1!(
        window,
        runtime,
        on_update_bind_host,
        value: SharedString,
        |rt| {
            let _ = rt.update_editor_tunnel_dialog_bind_host(value.as_ref());
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_update_bind_port,
        value: SharedString,
        |rt| {
            let _ = rt.update_editor_tunnel_dialog_bind_port(value.as_ref());
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_update_target_host,
        value: SharedString,
        |rt| {
            let _ = rt.update_editor_tunnel_dialog_target_host(value.as_ref());
        }
    );
    editor_cb1!(
        window,
        runtime,
        on_update_target_port,
        value: SharedString,
        |rt| {
            let _ = rt.update_editor_tunnel_dialog_target_port(value.as_ref());
        }
    );
    editor_cb1!(window, runtime, on_set_enabled, enabled: bool, |rt| {
        let _ = rt.set_editor_tunnel_enabled(enabled);
    });
}

fn wire_folder_editor_window(
    window: &FolderEditorWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
    wire_tool_title_drag!(window);
    wire_tool_on_close!(window, runtime, |r| r.close_folder_editor());
}

fn wire_known_hosts_window(
    window: &KnownHostsWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
    wire_tool_title_drag!(window);
    wire_tool_on_close!(window, runtime, |r| r.close_known_hosts_manager());
}

fn wire_private_keys_window(
    window: &PrivateKeysWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
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

fn wire_theme_manager_window(
    window: &ThemeManagerWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
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

fn wire_auth_prompt_window(
    window: &AuthPromptWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
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

fn wire_sftp_conflict_window(
    window: &SftpConflictWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
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

    if kind == ConfirmKind::McpControl {
        let decision = if confirmed {
            crate::runtime::mcp::McpConsentDecision::Accept {
                dont_ask_again: dont_ask,
            }
        } else {
            crate::runtime::mcp::McpConsentDecision::Reject
        };
        if let Some(wr) = window_runtime {
            wr.borrow_mut().resolve_mcp_consent(decision);
        } else {
            host.borrow()
                .dialogs
                .runtime
                .borrow_mut()
                .resolve_mcp_consent(decision);
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
    // 分区 / 目标
    w.set_section(p.editor_section_text.clone().into());
    w.set_target_text(
        if p.editor_target_editing {
            p.editor_target_session_id_text.clone()
        } else {
            String::new()
        }
        .into(),
    );
    w.set_folder_text(
        if p.editor_folder_known {
            format!(
                "{} ({})",
                p.editor_folder_label_text, p.editor_folder_id_text
            )
        } else {
            format!("Unknown folder ({})", p.editor_folder_id_text)
        }
        .into(),
    );
    w.set_folder_mode(false);
    // General
    w.set_name_text(p.editor_name_text.clone().into());
    w.set_host_text(p.editor_host_text.clone().into());
    w.set_port_text(p.editor_port_text.clone().into());
    w.set_username_text(p.editor_username_text.clone().into());
    w.set_icon_index(p.editor_icon_index);
    w.set_icon_custom_text(p.editor_icon_custom_text.clone().into());
    w.set_notes_text(p.editor_notes_text.clone().into());
    // Authentication
    w.set_auth_method_text(p.editor_auth_method_text.clone().into());
    w.set_password_text(p.editor_password_text.clone().into());
    w.set_passphrase_text(p.editor_passphrase_text.clone().into());
    w.set_key_options(ModelRc::new(VecModel::from(
        p.editor_key_options
            .iter()
            .map(|option| AuthKeyOption {
                id: option.id.clone().into(),
                label: option.label.clone().into(),
                detail: option.detail.clone().into(),
            })
            .collect::<Vec<_>>(),
    )));
    w.set_selected_key_index(p.editor_selected_key_index);
    w.set_selected_key_missing(p.editor_selected_key_missing);
    w.set_legacy_key_path_text(p.editor_legacy_key_path_text.clone().into());
    w.set_legacy_key_has_passphrase(p.editor_legacy_key_has_passphrase);
    w.set_has_stored_password(p.editor_has_stored_password);
    w.set_auth_test_kind_text(p.editor_auth_test_kind_text.clone().into());
    w.set_auth_test_host_text(p.editor_auth_test_host_text.clone().into());
    w.set_auth_test_startup_text(p.editor_auth_test_startup_text.clone().into());
    w.set_auth_test_error_text(p.editor_auth_test_error_text.clone().into());
    // 页级 Inherit
    w.set_inherit_auth(p.editor_inherit_auth);
    w.set_inherit_login_script(p.editor_inherit_login_script);
    w.set_inherit_proxy(p.editor_inherit_proxy);
    w.set_inherit_tunnels(p.editor_inherit_tunnels);
    w.set_inherit_terminal_style(p.editor_inherit_terminal_style);
    w.set_inherit_sources(inherit_source_model(&p.editor_inherit_sources));
    w.set_parent_auth_text(p.editor_parent_auth_summary_text.clone().into());
    w.set_parent_proxy_text(p.editor_parent_proxy_summary_text.clone().into());
    w.set_parent_terminal_style_text(p.editor_parent_terminal_style_text.clone().into());
    w.set_parent_login_step_count(p.editor_parent_login_step_count);
    w.set_parent_tunnel_count(p.editor_parent_tunnel_count);
    // Login Script
    w.set_login_steps(login_step_row_model(&p.editor_login_steps));
    w.set_selected_login_step(p.editor_selected_login_step);
    // Tunnels
    w.set_tunnels(tunnel_editor_row_model(&p.editor_tunnels));
    w.set_selected_tunnel(p.editor_selected_tunnel);
    // Proxy / Terminal Style
    w.set_proxy_option_labels(ModelRc::new(VecModel::from(
        p.editor_proxy_option_labels
            .iter()
            .map(|label| SharedString::from(label.as_str()))
            .collect::<Vec<_>>(),
    )));
    w.set_proxy_option_index(p.editor_proxy_option_index);
    w.set_proxy_selected_detail_text(p.editor_proxy_selected_detail_text.clone().into());
    w.set_proxy_legacy_text(p.editor_proxy_legacy_text.clone().into());
    w.set_proxy_selected_missing(p.editor_proxy_selected_missing);
    w.set_scheme_options(ModelRc::new(VecModel::from(
        p.editor_scheme_options
            .iter()
            .map(|label| SharedString::from(label.as_str()))
            .collect::<Vec<_>>(),
    )));
    w.set_scheme_index(p.editor_scheme_index);
    if let Some(main) = m {
        w.set_preview_frame(main.get_session_editor_preview());
        w.set_preview_background(main.get_session_editor_preview_background());
    }
}

fn inherit_source_model(items: &[EditorInheritSourceData]) -> ModelRc<InheritSource> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|item| InheritSource {
                kind: item.kind.into(),
                param: item.param.clone().into(),
            })
            .collect::<Vec<_>>(),
    ))
}

fn login_step_row_model(items: &[EditorLoginStepRowData]) -> ModelRc<LoginStepRow> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|item| LoginStepRow {
                kind: item.kind.into(),
                name: item.name.clone().into(),
                detail: item.detail.clone().into(),
            })
            .collect::<Vec<_>>(),
    ))
}

fn tunnel_editor_row_model(items: &[EditorTunnelRowData]) -> ModelRc<EditorTunnelRow> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|item| EditorTunnelRow {
                kind: item.kind.into(),
                name: item.name.clone().into(),
                route: item.route.clone().into(),
            })
            .collect::<Vec<_>>(),
    ))
}

fn sync_login_step_keys(w: &LoginStepKeysWindow, p: &AppProjection, _m: Option<&MainWindow>) {
    w.set_edit_mode(p.editor_login_step_is_edit);
    w.set_name_text(p.editor_login_step_name_text.clone().into());
    w.set_keys_text(p.editor_login_step_keys_text.clone().into());
    w.set_tokens(p.editor_login_step_tokens);
}

fn sync_login_step_wait(w: &LoginStepWaitWindow, p: &AppProjection, _m: Option<&MainWindow>) {
    w.set_edit_mode(p.editor_login_step_is_edit);
    w.set_millis_text(p.editor_login_step_millis_text.clone().into());
}

fn sync_tunnel_editor(w: &TunnelEditorWindow, p: &AppProjection, _m: Option<&MainWindow>) {
    w.set_edit_mode(p.editor_tunnel_is_edit);
    w.set_name_text(p.editor_tunnel_name_text.clone().into());
    w.set_kind_text(p.editor_tunnel_dialog_kind_text.clone().into());
    w.set_bind_host_text(p.editor_tunnel_dialog_bind_host_text.clone().into());
    w.set_bind_port_text(p.editor_tunnel_dialog_bind_port_text.clone().into());
    w.set_target_host_text(p.editor_tunnel_dialog_target_host_text.clone().into());
    w.set_target_port_text(p.editor_tunnel_dialog_target_port_text.clone().into());
    w.set_enabled(p.editor_tunnel_enabled);
}

// --- D35: master-password windows -----------------------------------------

/// Quits YShell from the startup unlock window (`Quit` / `×`).
///
/// Reuses the existing shutdown path: no protected file is read by the unlock
/// gate, so there is nothing else to confirm.
fn quit_from_master_password_unlock(
    runtime: &Rc<RefCell<crate::runtime::AppRuntime>>,
    manager: &Weak<RefCell<WindowManager>>,
) {
    let _ = runtime.borrow_mut().dismiss_master_password_unlock();
    if let Some(manager) = manager.upgrade() {
        if let Ok(mut manager) = manager.try_borrow_mut() {
            manager.shutdown();
        }
    }
}

fn wire_master_password_window(
    window: &MasterPasswordWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
) {
    wire_tool_title_drag!(window);
    let rt = Rc::clone(&runtime);
    window.on_update_current(move |text: SharedString| {
        let _ = rt
            .borrow_mut()
            .update_master_password_field("current", text.as_ref());
    });
    let rt = Rc::clone(&runtime);
    window.on_update_new(move |text: SharedString| {
        let _ = rt
            .borrow_mut()
            .update_master_password_field("new", text.as_ref());
    });
    let rt = Rc::clone(&runtime);
    window.on_update_confirm(move |text: SharedString| {
        let _ = rt
            .borrow_mut()
            .update_master_password_field("confirm", text.as_ref());
    });
    let rt = Rc::clone(&runtime);
    window.on_submit(move || {
        if let Err(error) = rt.borrow_mut().submit_master_password_dialog() {
            tracing::warn!(
                target: "yshell::app",
                "master password submit failed: {error}"
            );
        }
    });
    let rt = Rc::clone(&runtime);
    window.on_cancel(move || {
        let _ = rt.borrow_mut().close_master_password_dialog();
    });
    let rt = Rc::clone(&runtime);
    window.on_dismissed(move || {
        let _ = rt.borrow_mut().close_master_password_dialog();
    });
    let rt = Rc::clone(&runtime);
    window.window().on_close_requested(move || {
        let _ = rt.borrow_mut().close_master_password_dialog();
        CloseRequestResponse::HideWindow
    });
}

fn wire_master_password_unlock_window(
    window: &MasterPasswordUnlockWindow,
    runtime: Rc<RefCell<crate::runtime::AppRuntime>>,
    manager: Weak<RefCell<WindowManager>>,
) {
    wire_tool_title_drag!(window);
    let rt = Rc::clone(&runtime);
    window.on_update_password(move |text: SharedString| {
        let _ = rt
            .borrow_mut()
            .update_master_password_unlock_password(text.as_ref());
    });
    let rt = Rc::clone(&runtime);
    window.on_unlock(move || {
        let _ = rt.borrow_mut().submit_master_password_unlock();
    });
    let rt = Rc::clone(&runtime);
    window.on_forgot(move || {
        // Opens the destructive Reset sub-dialog; Cancel returns to this window.
        let _ = rt.borrow_mut().forgot_master_password();
        rt.borrow_mut().mark_ui_dirty();
    });
    let rt = Rc::clone(&runtime);
    let mgr = manager.clone();
    window.on_quit(move || quit_from_master_password_unlock(&rt, &mgr));
    let rt = Rc::clone(&runtime);
    window.window().on_close_requested(move || {
        quit_from_master_password_unlock(&rt, &manager);
        CloseRequestResponse::HideWindow
    });
}

fn sync_master_password_props(
    w: &MasterPasswordWindow,
    p: &AppProjection,
    _m: Option<&MainWindow>,
) {
    w.set_kind(p.settings_master_password_dialog_kind_text.clone().into());
    w.set_error_id(p.settings_master_password_dialog_error_text.clone().into());
    // D36: the aux window created from Settings follows the live App theme too.
    crate::bootstrap::apply_app_theme_preview(&w.global::<Theme>(), p);
}

fn sync_master_password_unlock_props(
    w: &MasterPasswordUnlockWindow,
    p: &AppProjection,
    _m: Option<&MainWindow>,
) {
    w.set_error_id(p.settings_master_password_unlock_error_text.clone().into());
    crate::bootstrap::apply_app_theme_preview(&w.global::<Theme>(), p);
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
    let wr = window_runtime.clone();
    window.on_open_theme_manager(move || {
        if let Some(wr) = wr.as_ref() {
            // Appearance → Terminal style → Manage: the runtime owns the
            // Settings-specific entry point (it keeps the draft in sync).
            let _ = wr.borrow_mut().open_settings_terminal_styles_manager();
        }
    });
    let wr = window_runtime.clone();
    window.on_set_confirm_close_session(move |enabled| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().set_confirm_close_session(enabled);
        }
    });
    let wr = window_runtime.clone();
    window.on_set_confirm_quit_app(move |enabled| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().set_confirm_quit_app(enabled);
        }
    });
    let wr_enable = window_runtime.clone();
    window.on_set_mcp_enabled(move |enabled| {
        if let Some(wr) = wr_enable.as_ref() {
            let _ = wr.borrow_mut().set_mcp_enabled(enabled);
        }
    });
    let wr = window_runtime.clone();
    window.on_set_mcp_auto_approve_control(move |enabled| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().set_mcp_auto_approve_control(enabled);
        }
    });
    // --- D36 / D37 / D33: Settings sections (whole-window draft + Save) -----
    let wr = window_runtime.clone();
    window.on_update_settings_appearance_mode(move |index| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().update_settings_appearance_mode(index);
        }
    });
    let wr = window_runtime.clone();
    window.on_update_settings_appearance_accent(move |index| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().update_settings_appearance_accent(index);
        }
    });
    let wr = window_runtime.clone();
    window.on_select_settings_appearance_style(move |index| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().select_settings_appearance_style(index);
        }
    });

    let wr = window_runtime.clone();
    window.on_update_settings_terminal_middle_click(move |index| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().update_settings_terminal_middle_click(index);
        }
    });
    let wr = window_runtime.clone();
    window.on_update_settings_terminal_right_click(move |index| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().update_settings_terminal_right_click(index);
        }
    });
    let wr = window_runtime.clone();
    window.on_update_settings_terminal_multi_line_paste(move |index| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr
                .borrow_mut()
                .update_settings_terminal_multi_line_paste(index);
        }
    });
    let wr = window_runtime.clone();
    window.on_update_settings_terminal_auto_copy(move |enabled| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().update_settings_terminal_auto_copy(enabled);
        }
    });
    let wr = window_runtime.clone();
    window.on_update_scrollback_lines(move |value: SharedString| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr
                .borrow_mut()
                .update_settings_scrollback_lines(value.as_ref());
        }
    });
    let wr = window_runtime.clone();
    window.on_update_scrollback_max_cells(move |value: SharedString| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr
                .borrow_mut()
                .update_settings_scrollback_max_cells(value.as_ref());
        }
    });
    let wr = window_runtime.clone();
    window.on_reset_terminal_defaults(move || {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().reset_settings_terminal_defaults();
        }
    });

    let wr = window_runtime.clone();
    window.on_update_settings_sftp_show_hidden(move |show_hidden| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr
                .borrow_mut()
                .update_settings_sftp_show_hidden(show_hidden);
        }
    });
    let wr = window_runtime.clone();
    window.on_update_settings_sftp_conflict_action(move |index| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().update_settings_sftp_conflict_action(index);
        }
    });
    let wr = window_runtime.clone();
    window.on_update_settings_sftp_external_editor(move |index| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().update_settings_sftp_external_editor(index);
        }
    });
    let wr = window_runtime.clone();
    window.on_update_settings_sftp_external_editor_path(move |value: SharedString| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr
                .borrow_mut()
                .update_settings_sftp_external_editor_path(value.as_ref());
        }
    });
    // rfd runs on a worker thread (blocking on the UI thread freezes rendering);
    // the picked path comes back through the app-level dialog channel.
    let host_browse = host.clone();
    window.on_browse_settings_sftp_external_editor(move || {
        let Some(host) = host_browse.upgrade() else {
            return;
        };
        let manager = host.borrow().dialogs.manager.upgrade();
        if let Some(manager) = manager {
            manager.borrow().browse_settings_sftp_editor();
        }
    });

    let wr = window_runtime.clone();
    window.on_open_master_password_dialog(move |kind: SharedString| {
        if let Some(wr) = wr.as_ref() {
            let _ = wr.borrow_mut().open_master_password_dialog(kind.as_ref());
        }
    });

    let wr = window_runtime.clone();
    window.on_open_config_dir(move || {
        if let Some(wr) = wr.as_ref() {
            let dir = wr.borrow().config_dir.display().to_string();
            crate::bootstrap::spawn_open_path(dir);
        }
    });

    let wr = window_runtime.clone();
    let host_save = host.clone();
    window.on_save_settings(move || {
        let Some(wr) = wr.as_ref() else {
            return;
        };
        let result = wr.borrow_mut().save_settings_all();
        match result {
            Ok(projection) => {
                if projection.settings_appearance_visible {
                    // A section failed validation: keep the dialog open, the
                    // runtime already projected that section's inline status.
                    return;
                }
                // Save = save and close. The runtime clears the projection flag,
                // but the local modal flag would keep the aux window open on the
                // next sync tick — mirror the Cancel path here.
                wr.borrow_mut().mark_ui_dirty();
                if let Some(host) = host_save.upgrade() {
                    let mut host = host.borrow_mut();
                    host.dialogs.local.settings = false;
                    if host.dialogs.settings_shown {
                        if let Some(settings) = host.dialogs.settings.as_ref() {
                            let _ = settings.hide();
                        }
                        host.dialogs.settings_shown = false;
                        host.dialogs.end_modal();
                    }
                }
            }
            Err(error) => {
                tracing::warn!(
                    target: "yshell::app",
                    "could not save the settings: {error}"
                );
            }
        }
    });

    // --- General / Advanced / MCP: existing per-section setters -------------
    let wr = window_runtime.clone();
    window.on_set_language(move |value: SharedString| {
        if let Some(wr) = wr.as_ref() {
            let result = wr.borrow_mut().set_ui_language(value.as_ref());
            match result {
                Ok(projection) => {
                    crate::bootstrap::apply_language(projection.settings_language_text.as_str())
                }
                Err(error) => tracing::warn!(
                    target: "yshell::app",
                    "could not save the language setting: {error}"
                ),
            }
        }
    });
    let wr = window_runtime.clone();
    window.on_set_debug_logging(move |enabled| {
        if let Some(wr) = wr.as_ref() {
            if let Err(error) = wr.borrow_mut().set_debug_logging(enabled) {
                tracing::warn!(
                    target: "yshell::app",
                    "could not save the debug logging setting: {error}"
                );
            }
        }
    });
    let wr = window_runtime.clone();
    window.on_open_debug_log_dir(move || {
        if let Some(wr) = wr.as_ref() {
            if let Ok(dir) = wr.borrow().ensure_debug_log_dir() {
                crate::bootstrap::spawn_open_path(dir.display().to_string());
            }
        }
    });

    // --- Reduce motion：配置是唯一来源，开关即时应用并落盘 -------------------
    let weak = window.as_weak();
    let wr = window_runtime.clone();
    let set_host = host.clone();
    window.on_set_reduce_motion(move |enabled| {
        if let Some(wr) = wr.as_ref() {
            if let Err(error) = wr.borrow_mut().set_reduce_motion(enabled) {
                tracing::warn!(
                    target: "yshell::app",
                    "could not save the reduce-motion setting: {error}"
                );
            }
        }
        if let Some(window) = weak.upgrade() {
            crate::bootstrap::apply_reduce_motion(&window.global::<Theme>(), enabled);
        }
        if let Some(host) = set_host.upgrade() {
            let manager = host.borrow().dialogs.manager.upgrade();
            if let Some(manager) = manager {
                manager.borrow().apply_reduce_motion(enabled);
            }
        }
    });
}

fn sync_settings_from(
    settings: &SettingsWindow,
    projection: &AppProjection,
    main: Option<&MainWindow>,
    reduce_motion: bool,
) {
    settings.set_config_dir_text(projection.config_dir_text.clone().into());
    settings.set_scrollback_lines_text(projection.settings_scrollback_lines_text.clone().into());
    settings.set_scrollback_max_cells_text(
        projection.settings_scrollback_max_cells_text.clone().into(),
    );
    settings.set_settings_sftp_max_parallel_transfers_text(
        projection
            .settings_sftp_max_parallel_transfers_text
            .clone()
            .into(),
    );
    settings.set_language_text(projection.settings_language_text.clone().into());
    settings.set_debug_logging_enabled(projection.settings_debug_logging_enabled);
    settings.set_debug_log_dir_text(projection.settings_debug_log_dir_text.clone().into());
    settings.set_confirm_close_session_enabled(projection.settings_confirm_close_session);
    settings.set_confirm_quit_app_enabled(projection.settings_confirm_quit_app);
    settings.set_mcp_enabled(projection.settings_mcp_enabled);
    settings.set_mcp_auto_approve_control_enabled(projection.settings_mcp_auto_approve_control);
    settings.set_mcp_status_text(projection.settings_mcp_status_text.clone().into());
    settings.set_mcp_boundaries_text(projection.settings_mcp_boundaries_text.clone().into());
    settings.set_mcp_host_config_text(projection.settings_mcp_host_config_text.clone().into());
    settings.set_appearance_color_scheme_name_text(
        projection
            .settings_appearance_color_scheme_name_text
            .clone()
            .into(),
    );
    settings
        .set_appearance_color_scheme_builtin(projection.settings_appearance_color_scheme_builtin);
    settings.set_appearance_font_index(projection.settings_appearance_font_index);
    settings.set_appearance_font_size_text(
        projection.settings_appearance_font_size_text.clone().into(),
    );
    settings.set_appearance_fallback_fonts_text(
        projection
            .settings_appearance_backup_font_text
            .clone()
            .into(),
    );
    settings.set_appearance_status_text(projection.settings_appearance_status_text.clone().into());
    set_settings_swatches(
        settings,
        &projection.settings_appearance_palette_swatches,
        &projection.settings_appearance_accent_light_swatches,
        &projection.settings_appearance_accent_dark_swatches,
    );
    set_settings_options(
        settings,
        &projection.settings_appearance_font_options,
        &projection.settings_appearance_style_options,
    );
    if let Some(main) = main {
        settings.set_appearance_preview(main.get_settings_appearance_preview());
        settings
            .set_appearance_preview_background(main.get_settings_appearance_preview_background());
    }

    // --- settings-security ---
    settings.set_settings_appearance_mode_index(projection.settings_appearance_mode_index);
    settings.set_settings_appearance_accent_index(projection.settings_appearance_accent_index);
    settings.set_settings_appearance_style_index(projection.settings_appearance_style_index);
    settings
        .set_settings_terminal_middle_click_index(projection.settings_terminal_middle_click_index);
    settings
        .set_settings_terminal_right_click_index(projection.settings_terminal_right_click_index);
    settings.set_settings_terminal_multi_line_paste_index(
        projection.settings_terminal_multi_line_paste_index,
    );
    settings.set_settings_terminal_auto_copy(projection.settings_terminal_auto_copy);
    settings.set_settings_terminal_status_kind_text(
        projection.settings_terminal_status_kind_text.clone().into(),
    );
    settings.set_settings_terminal_status_field_text(
        projection
            .settings_terminal_status_field_text
            .clone()
            .into(),
    );
    settings.set_settings_terminal_status_value_text(
        projection
            .settings_terminal_status_value_text
            .clone()
            .into(),
    );
    settings.set_settings_terminal_status_limit_text(
        projection
            .settings_terminal_status_limit_text
            .clone()
            .into(),
    );
    settings.set_settings_sftp_show_hidden(projection.settings_sftp_show_hidden);
    settings.set_settings_sftp_conflict_index(projection.settings_sftp_conflict_index);
    settings.set_settings_sftp_editor_index(projection.settings_sftp_editor_index);
    settings
        .set_settings_sftp_editor_custom_visible(projection.settings_sftp_editor_custom_visible);
    settings.set_settings_sftp_editor_path_text(
        projection.settings_sftp_editor_path_text.clone().into(),
    );
    settings.set_settings_sftp_status_kind_text(
        projection.settings_sftp_status_kind_text.clone().into(),
    );
    settings.set_settings_master_password_set(projection.settings_master_password_set);
    settings.set_settings_master_password_locked(projection.settings_master_password_locked);
    settings
        .set_settings_security_known_hosts_count(projection.settings_security_known_hosts_count);
    settings
        .set_settings_security_private_keys_count(projection.settings_security_private_keys_count);
    settings.set_settings_security_proxies_count(projection.settings_security_proxies_count);
    if !string_model_matches(
        &settings.get_settings_sftp_editor_options(),
        &projection.settings_sftp_editor_options,
    ) {
        settings.set_settings_sftp_editor_options(option_model(
            &projection.settings_sftp_editor_options,
        ));
    }
    // D36: the Settings window itself follows the App theme instant preview.
    crate::bootstrap::apply_app_theme_preview(&settings.global::<Theme>(), projection);
    // Settings → General → Motion：`[ui] reduce_motion` 是唯一来源（无环境变量）。
    crate::bootstrap::apply_reduce_motion(&settings.global::<Theme>(), reduce_motion);
}

/// `[color]` models are rebuilt only when the palette actually changed: the sync
/// runs on every UI tick while the window is open.
fn set_settings_swatches(
    settings: &SettingsWindow,
    palette: &[[u8; 3]],
    accent_light: &[[u8; 3]],
    accent_dark: &[[u8; 3]],
) {
    if !color_model_matches(&settings.get_appearance_palette_swatches(), palette) {
        settings.set_appearance_palette_swatches(swatch_model(palette));
    }
    if !color_model_matches(
        &settings.get_settings_appearance_accent_light_swatches(),
        accent_light,
    ) {
        settings.set_settings_appearance_accent_light_swatches(swatch_model(accent_light));
    }
    if !color_model_matches(
        &settings.get_settings_appearance_accent_dark_swatches(),
        accent_dark,
    ) {
        settings.set_settings_appearance_accent_dark_swatches(swatch_model(accent_dark));
    }
}

fn set_settings_options(settings: &SettingsWindow, fonts: &[String], styles: &[String]) {
    if !string_model_matches(&settings.get_appearance_font_options(), fonts) {
        settings.set_appearance_font_options(option_model(fonts));
    }
    if !string_model_matches(&settings.get_settings_appearance_style_options(), styles) {
        settings.set_settings_appearance_style_options(option_model(styles));
    }
}

fn swatch_model(swatches: &[[u8; 3]]) -> ModelRc<slint::Color> {
    ModelRc::new(VecModel::from(
        swatches
            .iter()
            .map(|[red, green, blue]| slint::Color::from_rgb_u8(*red, *green, *blue))
            .collect::<Vec<_>>(),
    ))
}

fn option_model(values: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values
            .iter()
            .map(|value| SharedString::from(value.as_str()))
            .collect::<Vec<_>>(),
    ))
}

fn color_model_matches(model: &ModelRc<slint::Color>, expected: &[[u8; 3]]) -> bool {
    model.row_count() == expected.len()
        && model
            .iter()
            .zip(expected.iter())
            .all(|(color, [red, green, blue])| {
                color == slint::Color::from_rgb_u8(*red, *green, *blue)
            })
}

fn string_model_matches(model: &ModelRc<SharedString>, expected: &[String]) -> bool {
    model.row_count() == expected.len()
        && model
            .iter()
            .zip(expected.iter())
            .all(|(item, value)| item.as_str() == value.as_str())
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
