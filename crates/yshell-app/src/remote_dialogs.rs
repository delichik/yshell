//! OS dialogs for the out-of-process Shell (D38).
//!
//! Host is the only disk writer. Shell shows tool windows from `AppProjection`
//! flags / Shell-local open signals and sends UiCalls through
//! [`AppRuntimeClient::Remote`].
//!
//! About, New Folder, the SFTP entry dialog, and quit / close-window /
//! delete-session / clear-scrollback / MCP-consent confirms are Shell windows
//! too. While any aux window is shown the main window input is disabled, and
//! the last hide re-enables it (same modal depth as `DialogWindowManager`).
//!
//! Settings L-tier fields stay on the Settings window until Save, which batches
//! them into Host and then calls `save_settings_all`. Cancel only calls
//! `close_settings_appearance` (no L-tier flush). Mode and accent preview on
//! that window and the main window via [`apply_app_theme_indices`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use serde_json::json;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use crate::bootstrap::generated_ui::{
    AboutWindow, AuthPromptWindow, CompactConfirmWindow, FolderEditorWindow, HostKeyWindow,
    HostKeysWindow, KnownHostAddWindow, KnownHostExportWindow, KnownHostImportWindow,
    LoggingWindow, LoginStepKeysWindow, LoginStepWaitWindow, MainWindow,
    MasterPasswordUnlockWindow, MasterPasswordWindow, NewFolderWindow, PasswordPromptWindow,
    PrivateKeyDeployWindow, PrivateKeyExportWindow, PrivateKeyGenerateWindow,
    PrivateKeyImportWindow, PrivateKeysWindow, ProxiesWindow, SessionEditorWindow, SettingsWindow,
    SftpChmodWindow, SftpConflictWindow, SftpEntryWindow, SftpPropertiesWindow, Theme,
    ThemeManagerWindow, TunnelEditorWindow,
};
use crate::bootstrap::{
    apply_app_theme_indices, apply_reduce_motion, apply_tool_window_theme, dialog_kind_name,
    DialogKind, FileDialogOutcome,
};
use crate::dialog_windows::{
    about_licenses_preview, open_full_open_source_licenses, set_settings_options, sync_auth,
    sync_folder_editor, sync_host_key_prompt, sync_host_keys, sync_known_host_add,
    sync_known_host_export, sync_known_host_import, sync_logging, sync_login_step_keys,
    sync_login_step_wait, sync_master_password_props, sync_master_password_unlock_props,
    sync_password, sync_private_key_deploy, sync_private_key_export, sync_private_key_generate,
    sync_private_key_import, sync_private_keys, sync_proxies, sync_session_editor,
    sync_settings_from, sync_sftp_chmod, sync_sftp_conflict, sync_sftp_properties,
    sync_theme_manager, sync_tunnel_editor,
};
use crate::error::{AppError, AppResult};
use crate::host::shell_ipc::{projection_from_json, HostShellClient};
use crate::host::AppRuntimeClient;
use crate::remote_dialog_wire::{self, FileTx};
use crate::runtime::AppProjection;

const CONFIRM_KEYS_REMOVE: u8 = 1;
const CONFIRM_PROXIES_REMOVE: u8 = 2;
const CONFIRM_THEME_REMOVE: u8 = 3;
const CONFIRM_HOST_KEYS_CLEAR: u8 = 4;
const CONFIRM_HOST_KEYS_REMOVE: u8 = 5;
const CONFIRM_EDITOR_DISCARD: u8 = 6;
const CONFIRM_CLOSE_TABS: u8 = 7;
const CONFIRM_CLEAR_SCROLLBACK: u8 = 8;
const CONFIRM_DELETE_SESSION: u8 = 9;
const CONFIRM_QUIT: u8 = 10;
const CONFIRM_CLOSE_WINDOW: u8 = 11;
const CONFIRM_MCP_CONSENT: u8 = 12;

struct Slot<W> {
    window: Option<W>,
    shown: bool,
}

impl<W> Default for Slot<W> {
    fn default() -> Self {
        Self {
            window: None,
            shown: false,
        }
    }
}

/// Shell-side dialog state synced from Host projections.
pub struct RemoteDialogHost {
    unlock: Slot<MasterPasswordUnlockWindow>,
    master_password: Slot<MasterPasswordWindow>,
    password: Slot<PasswordPromptWindow>,
    host_key: Slot<HostKeyWindow>,
    auth: Slot<AuthPromptWindow>,
    settings: Slot<SettingsWindow>,
    session_editor: Slot<SessionEditorWindow>,
    login_step_keys: Slot<LoginStepKeysWindow>,
    login_step_wait: Slot<LoginStepWaitWindow>,
    tunnel_editor: Slot<TunnelEditorWindow>,
    folder_editor: Slot<FolderEditorWindow>,
    private_keys: Slot<PrivateKeysWindow>,
    key_generate: Slot<PrivateKeyGenerateWindow>,
    key_import: Slot<PrivateKeyImportWindow>,
    key_export: Slot<PrivateKeyExportWindow>,
    key_deploy: Slot<PrivateKeyDeployWindow>,
    proxies: Slot<ProxiesWindow>,
    theme_manager: Slot<ThemeManagerWindow>,
    host_keys: Slot<HostKeysWindow>,
    known_host_add: Slot<KnownHostAddWindow>,
    known_host_import: Slot<KnownHostImportWindow>,
    known_host_export: Slot<KnownHostExportWindow>,
    logging: Slot<LoggingWindow>,
    sftp_conflict: Slot<SftpConflictWindow>,
    sftp_properties: Slot<SftpPropertiesWindow>,
    sftp_chmod: Slot<SftpChmodWindow>,
    about: Slot<AboutWindow>,
    new_folder: Slot<NewFolderWindow>,
    sftp_entry: Slot<SftpEntryWindow>,
    confirm: Option<CompactConfirmWindow>,
    confirm_shown: bool,
    confirm_kind: Rc<Cell<u8>>,
    host_keys_clear: Rc<Cell<bool>>,
    host_keys_remove: Rc<Cell<bool>>,
    about_open: Rc<Cell<bool>>,
    clear_scrollback: Rc<Cell<bool>>,
    delete_session: Rc<Cell<bool>>,
    quit: Rc<Cell<bool>>,
    new_folder_open: Rc<Cell<bool>>,
    sftp_dialog_kind: Rc<RefCell<String>>,
    /// Shell should hide the main window and detach after quit / close confirm.
    pending_exit: Rc<Cell<bool>>,
    /// Local dialog latch changed; sync even when the Host projection is clean.
    needs_refresh: Rc<Cell<bool>>,
    modal_depth: Cell<u32>,
    file_tx: FileTx,
    file_rx: Receiver<FileDialogOutcome>,
    main: Option<Weak<MainWindow>>,
    reduce_motion: Rc<Cell<bool>>,
    reduce_motion_known: Cell<bool>,
}

impl Default for RemoteDialogHost {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteDialogHost {
    #[must_use]
    pub fn new() -> Self {
        let (file_tx, file_rx) = std::sync::mpsc::channel();
        Self {
            unlock: Slot::default(),
            master_password: Slot::default(),
            password: Slot::default(),
            host_key: Slot::default(),
            auth: Slot::default(),
            settings: Slot::default(),
            session_editor: Slot::default(),
            login_step_keys: Slot::default(),
            login_step_wait: Slot::default(),
            tunnel_editor: Slot::default(),
            folder_editor: Slot::default(),
            private_keys: Slot::default(),
            key_generate: Slot::default(),
            key_import: Slot::default(),
            key_export: Slot::default(),
            key_deploy: Slot::default(),
            proxies: Slot::default(),
            theme_manager: Slot::default(),
            host_keys: Slot::default(),
            known_host_add: Slot::default(),
            known_host_import: Slot::default(),
            known_host_export: Slot::default(),
            logging: Slot::default(),
            sftp_conflict: Slot::default(),
            sftp_properties: Slot::default(),
            sftp_chmod: Slot::default(),
            about: Slot::default(),
            new_folder: Slot::default(),
            sftp_entry: Slot::default(),
            confirm: None,
            confirm_shown: false,
            confirm_kind: Rc::new(Cell::new(0)),
            host_keys_clear: Rc::new(Cell::new(false)),
            host_keys_remove: Rc::new(Cell::new(false)),
            about_open: Rc::new(Cell::new(false)),
            clear_scrollback: Rc::new(Cell::new(false)),
            delete_session: Rc::new(Cell::new(false)),
            quit: Rc::new(Cell::new(false)),
            new_folder_open: Rc::new(Cell::new(false)),
            sftp_dialog_kind: Rc::new(RefCell::new("none".into())),
            pending_exit: Rc::new(Cell::new(false)),
            needs_refresh: Rc::new(Cell::new(false)),
            modal_depth: Cell::new(0),
            file_tx,
            file_rx,
            main: None,
            reduce_motion: Rc::new(Cell::new(false)),
            reduce_motion_known: Cell::new(false),
        }
    }

    pub fn attach_main(&mut self, window: &MainWindow) {
        self.main = Some(window.as_weak());
    }

    /// True after quit / close-window confirm — Shell should hide and detach.
    #[must_use]
    pub fn take_pending_exit(&self) -> bool {
        self.pending_exit.replace(false)
    }

    /// Local About / confirm / folder latch changed since the last sync.
    #[must_use]
    pub fn take_needs_refresh(&self) -> bool {
        self.needs_refresh.take()
    }

    /// Sender for Shell-local native picker results (rfd).
    ///
    /// MainWindow Browse buttons reuse the dialog host's channel so every pick
    /// (dialog or main-window) is applied through [`Self::poll_local`]: pickers
    /// with a Host-side effect go through `apply_file_dialog_picked`, the
    /// Settings external-editor path stays in the local draft.
    #[must_use]
    pub fn file_tx(&self) -> FileTx {
        self.file_tx.clone()
    }

    /// Main window still has a Shell-local dialog request the next sync should ingest.
    #[must_use]
    pub fn main_requests_dialog(&self) -> bool {
        let Some(main) = self.main.as_ref().and_then(Weak::upgrade) else {
            return false;
        };
        main.get_about_dialog_visible()
            || main.get_clear_scrollback_dialog_visible()
            || main.get_delete_session_dialog_visible()
            || main.get_quit_dialog_visible()
            || main.get_new_folder_dialog_visible()
            || main.get_host_keys_clear_dialog_visible()
            || main.get_host_keys_remove_dialog_visible()
            || main.get_sftp_dialog_kind() != "none"
            || self.about_open.get()
            || self.clear_scrollback.get()
            || self.delete_session.get()
            || self.quit.get()
            || self.new_folder_open.get()
            || self.host_keys_clear.get()
            || self.host_keys_remove.get()
            || self.sftp_dialog_kind.borrow().as_str() != "none"
            || self.about.shown
            || self.new_folder.shown
            || self.sftp_entry.shown
            || self.confirm_shown
    }

    /// Raise Shell-local quit confirm (File → Exit / last-window close).
    pub fn request_quit_confirm(&self) {
        self.quit.set(true);
        self.needs_refresh.set(true);
    }

    fn ingest_local(&mut self) {
        let Some(main) = self.main.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        if main.get_about_dialog_visible() {
            self.about_open.set(true);
            main.set_about_dialog_visible(false);
        }
        if main.get_clear_scrollback_dialog_visible() {
            self.clear_scrollback.set(true);
            main.set_clear_scrollback_dialog_visible(false);
        }
        if main.get_delete_session_dialog_visible() {
            self.delete_session.set(true);
            main.set_delete_session_dialog_visible(false);
        }
        if main.get_quit_dialog_visible() {
            self.quit.set(true);
            main.set_quit_dialog_visible(false);
        }
        if main.get_new_folder_dialog_visible() {
            self.new_folder_open.set(true);
            main.set_new_folder_dialog_visible(false);
        }
        if main.get_host_keys_clear_dialog_visible() {
            self.host_keys_clear.set(true);
            main.set_host_keys_clear_dialog_visible(false);
        }
        if main.get_host_keys_remove_dialog_visible() {
            self.host_keys_remove.set(true);
            main.set_host_keys_remove_dialog_visible(false);
        }
        let kind = main.get_sftp_dialog_kind().to_string();
        if kind != "none" {
            *self.sftp_dialog_kind.borrow_mut() = kind;
            main.set_sftp_dialog_kind("none".into());
        }
    }

    fn begin_modal(&self, main: Option<&MainWindow>) {
        begin_modal_depth(&self.modal_depth, main);
    }

    fn end_modal(&self, main: Option<&MainWindow>) {
        end_modal_depth(&self.modal_depth, main);
    }

    /// Apply Shell-local file picks even when the Host projection is not dirty.
    pub fn poll_local(&mut self, client: &Arc<HostShellClient>) {
        let rt = AppRuntimeClient::remote(Arc::clone(client));
        while let Ok(outcome) = self.file_rx.try_recv() {
            if let FileDialogOutcome::Picked { kind, path } = outcome {
                // SFTP editor Browse is part of the Settings L draft.
                if kind == DialogKind::SettingsSftpEditor {
                    if let Some(window) = self.settings.window.as_ref() {
                        window.set_settings_sftp_editor_path_text(path.into());
                    }
                    continue;
                }
                let _ = rt.call_value(
                    "apply_file_dialog_picked",
                    json!([dialog_kind_name(kind), path]),
                );
            }
        }
    }

    pub fn sync(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
    ) -> AppResult<()> {
        self.poll_local(client);
        self.ingest_local();
        self.ensure_reduce_motion(client);
        let main = self.main.as_ref().and_then(|weak| weak.upgrade());
        let main_ref = main.as_ref();
        let reduce = self.reduce_motion.get();
        self.sync_about(projection, main_ref)?;
        self.sync_unlock(client, projection, main_ref)?;
        self.sync_master_password(client, projection, main_ref)?;
        self.sync_password(client, projection, main_ref)?;
        self.sync_host_key(client, projection, main_ref)?;
        self.sync_auth(client, projection, main_ref)?;
        self.sync_settings(client, projection, main_ref, reduce)?;
        self.sync_session_editor(client, projection, main_ref)?;
        self.sync_login_steps(client, projection, main_ref)?;
        self.sync_folder_editor(client, projection, main_ref)?;
        self.sync_private_keys(client, projection, main_ref)?;
        self.sync_proxies(client, projection, main_ref)?;
        self.sync_theme_manager(client, projection, main_ref)?;
        self.sync_host_keys(client, projection, main_ref)?;
        self.sync_logging(client, projection, main_ref)?;
        self.sync_sftp(client, projection, main_ref)?;
        self.sync_new_folder(client, projection, main_ref)?;
        self.sync_sftp_entry(client, projection, main_ref)?;
        self.sync_confirm(client, projection, main_ref)?;
        Ok(())
    }

    /// `reduce_motion` is not a Host getter. Sample the main window once;
    /// Settings → Motion writes the cell via `set_reduce_motion`.
    fn ensure_reduce_motion(&mut self, _client: &Arc<HostShellClient>) {
        if self.reduce_motion_known.get() {
            return;
        }
        if let Some(main) = self.main.as_ref().and_then(|weak| weak.upgrade()) {
            self.reduce_motion
                .set(main.global::<Theme>().get_reduce_motion());
        }
        self.reduce_motion_known.set(true);
    }

    #[allow(clippy::too_many_arguments)]
    fn sync_tool<W, FNew, FWire, FSync>(
        want: bool,
        slot: &mut Slot<W>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
        modal_depth: &Cell<u32>,
        create: FNew,
        wire: FWire,
        sync_props: FSync,
    ) -> AppResult<()>
    where
        W: ComponentHandle + 'static,
        for<'a> Theme<'a>: slint::Global<'a, W>,
        FNew: FnOnce() -> AppResult<W>,
        FWire: FnOnce(&W),
        FSync: FnOnce(&W, &AppProjection),
    {
        if want {
            if slot.window.is_none() {
                let window = create()?;
                wire(&window);
                slot.window = Some(window);
            }
            if let Some(window) = slot.window.as_ref() {
                apply_tool_window_theme(&window.global::<Theme>(), projection);
                if let Some(main) = main {
                    apply_reduce_motion(
                        &window.global::<Theme>(),
                        main.global::<Theme>().get_reduce_motion(),
                    );
                }
                sync_props(window, projection);
                if !slot.shown {
                    begin_modal_depth(modal_depth, main);
                    window.show().map_err(AppError::from_error)?;
                    crate::win_chrome::prepare_aux_window(window.window());
                    if let Some(main) = main {
                        crate::win_chrome::attach_owned_popup(window.window(), main.window());
                        crate::win_chrome::center_aux_window(window.window(), main.window());
                        crate::win_chrome::finish_owned_aux_window_when_ready(
                            window.as_weak(),
                            main.as_weak(),
                            8,
                        );
                    } else {
                        crate::win_chrome::prepare_aux_window_when_ready(window.as_weak(), 8);
                    }
                    slot.shown = true;
                }
            }
        } else if slot.shown {
            if let Some(window) = slot.window.as_ref() {
                let _ = window.hide();
            }
            slot.shown = false;
            end_modal_depth(modal_depth, main);
        }
        Ok(())
    }

    fn sync_unlock(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client = Arc::clone(client);
        Self::sync_tool(
            projection.settings_master_password_unlock_visible,
            &mut self.unlock,
            projection,
            main,
            &self.modal_depth,
            || MasterPasswordUnlockWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_unlock(w, client),
            |w, p| sync_master_password_unlock_props(w, p, None),
        )
    }

    fn sync_master_password(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client = Arc::clone(client);
        Self::sync_tool(
            projection.settings_master_password_dialog_visible,
            &mut self.master_password,
            projection,
            main,
            &self.modal_depth,
            || MasterPasswordWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_master_password(w, client),
            |w, p| sync_master_password_props(w, p, None),
        )
    }

    fn sync_password(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client = Arc::clone(client);
        Self::sync_tool(
            projection.password_prompt_visible,
            &mut self.password,
            projection,
            main,
            &self.modal_depth,
            || PasswordPromptWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_password(w, client),
            |w, p| {
                if main.is_some() {
                    sync_password(w, p, main);
                } else {
                    w.set_title_text(p.password_prompt_host_text.clone().into());
                    w.set_message_text(p.password_prompt_host_text.clone().into());
                }
            },
        )
    }

    fn sync_host_key(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client = Arc::clone(client);
        Self::sync_tool(
            projection.host_key_prompt_visible,
            &mut self.host_key,
            projection,
            main,
            &self.modal_depth,
            || HostKeyWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_host_key(w, client),
            sync_host_key_prompt,
        )
    }

    fn sync_auth(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client = Arc::clone(client);
        let tx = self.file_tx.clone();
        Self::sync_tool(
            projection.auth_prompt_visible,
            &mut self.auth,
            projection,
            main,
            &self.modal_depth,
            || AuthPromptWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_auth(w, client, tx),
            |w, p| sync_auth(w, p, None),
        )
    }

    fn sync_settings(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
        reduce: bool,
    ) -> AppResult<()> {
        let want = projection.settings_appearance_visible;
        if want && self.settings.window.is_none() {
            let window = SettingsWindow::new().map_err(AppError::from_error)?;
            remote_dialog_wire::wire_settings(
                &window,
                Arc::clone(client),
                self.file_tx.clone(),
                Rc::clone(&self.reduce_motion),
                main.map(|window| window.as_weak()),
            );
            self.settings.window = Some(window);
        }
        if want {
            // Seed L-tier fields once per open. Later ticks refresh I-tier
            // state, then put the local Mode/Accent back on this window and
            // the main window (Host ticks still carry the saved theme).
            let seed = !self.settings.shown;
            let mut opened = false;
            if let Some(window) = self.settings.window.as_ref() {
                if seed {
                    apply_tool_window_theme(&window.global::<Theme>(), projection);
                    if let Some(main) = main {
                        apply_reduce_motion(
                            &window.global::<Theme>(),
                            main.global::<Theme>().get_reduce_motion(),
                        );
                    }
                    sync_settings_from(window, projection, main, reduce);
                    window.show().map_err(AppError::from_error)?;
                    begin_modal_depth(&self.modal_depth, main);
                    crate::win_chrome::prepare_aux_window(window.window());
                    if let Some(main) = main {
                        crate::win_chrome::attach_owned_popup(window.window(), main.window());
                        crate::win_chrome::center_aux_window(window.window(), main.window());
                        crate::win_chrome::finish_owned_aux_window_when_ready(
                            window.as_weak(),
                            main.as_weak(),
                            8,
                        );
                    } else {
                        crate::win_chrome::prepare_aux_window_when_ready(window.as_weak(), 8);
                    }
                    opened = true;
                } else {
                    apply_reduce_motion(&window.global::<Theme>(), reduce);
                    sync_settings_host_live(window, projection);
                    let mode = window.get_settings_appearance_mode_index();
                    let accent = window.get_settings_appearance_accent_index();
                    apply_app_theme_indices(&window.global::<Theme>(), mode, accent);
                    if let Some(main) = main {
                        apply_app_theme_indices(&main.global::<Theme>(), mode, accent);
                    }
                }
            }
            if opened {
                self.settings.shown = true;
            }
        } else if self.settings.shown {
            if let Some(window) = self.settings.window.as_ref() {
                let _ = window.hide();
            }
            self.settings.shown = false;
            end_modal_depth(&self.modal_depth, main);
        }
        Ok(())
    }

    fn sync_session_editor(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client = Arc::clone(client);
        let tx = self.file_tx.clone();
        Self::sync_tool(
            projection.editor_modal_visible,
            &mut self.session_editor,
            projection,
            main,
            &self.modal_depth,
            || SessionEditorWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_session_editor(w, client, tx),
            |w, p| sync_session_editor(w, p, main),
        )
    }

    fn sync_login_steps(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let keys_client = Arc::clone(client);
        Self::sync_tool(
            projection.editor_login_step_dialog_visible
                && projection.editor_login_step_kind_text == "keys",
            &mut self.login_step_keys,
            projection,
            main,
            &self.modal_depth,
            || LoginStepKeysWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_login_step_keys(w, keys_client),
            |w, p| sync_login_step_keys(w, p, None),
        )?;
        let wait_client = Arc::clone(client);
        Self::sync_tool(
            projection.editor_login_step_dialog_visible
                && projection.editor_login_step_kind_text == "wait",
            &mut self.login_step_wait,
            projection,
            main,
            &self.modal_depth,
            || LoginStepWaitWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_login_step_wait(w, wait_client),
            |w, p| sync_login_step_wait(w, p, None),
        )?;
        let tunnel_client = Arc::clone(client);
        Self::sync_tool(
            projection.editor_tunnel_dialog_visible,
            &mut self.tunnel_editor,
            projection,
            main,
            &self.modal_depth,
            || TunnelEditorWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_tunnel_editor(w, tunnel_client),
            |w, p| sync_tunnel_editor(w, p, None),
        )
    }

    fn sync_folder_editor(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client = Arc::clone(client);
        Self::sync_tool(
            projection.folder_editor_visible,
            &mut self.folder_editor,
            projection,
            main,
            &self.modal_depth,
            || FolderEditorWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_folder_editor(w, client),
            |w, p| sync_folder_editor(w, p, None),
        )
    }

    fn sync_private_keys(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client_main = Arc::clone(client);
        Self::sync_tool(
            projection.private_keys_modal_visible,
            &mut self.private_keys,
            projection,
            main,
            &self.modal_depth,
            || PrivateKeysWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_private_keys(w, client_main),
            |w, p| sync_private_keys(w, p, None),
        )?;
        let client_gen = Arc::clone(client);
        Self::sync_tool(
            projection.keys_generate_visible,
            &mut self.key_generate,
            projection,
            main,
            &self.modal_depth,
            || PrivateKeyGenerateWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_private_key_generate(w, client_gen),
            |w, p| sync_private_key_generate(w, p, None),
        )?;
        let client_import = Arc::clone(client);
        let tx = self.file_tx.clone();
        Self::sync_tool(
            projection.keys_import_visible,
            &mut self.key_import,
            projection,
            main,
            &self.modal_depth,
            || PrivateKeyImportWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_private_key_import(w, client_import, tx),
            |w, p| sync_private_key_import(w, p, None),
        )?;
        let client_export = Arc::clone(client);
        let tx = self.file_tx.clone();
        Self::sync_tool(
            projection.keys_export_visible,
            &mut self.key_export,
            projection,
            main,
            &self.modal_depth,
            || PrivateKeyExportWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_private_key_export(w, client_export, tx),
            |w, p| sync_private_key_export(w, p, None),
        )?;
        let client_deploy = Arc::clone(client);
        Self::sync_tool(
            projection.private_keys_deploy_confirm_visible,
            &mut self.key_deploy,
            projection,
            main,
            &self.modal_depth,
            || PrivateKeyDeployWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_private_key_deploy(w, client_deploy),
            |w, p| sync_private_key_deploy(w, p, None),
        )
    }

    fn sync_proxies(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client = Arc::clone(client);
        Self::sync_tool(
            projection.proxies_modal_visible,
            &mut self.proxies,
            projection,
            main,
            &self.modal_depth,
            || ProxiesWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_proxies(w, client),
            |w, p| sync_proxies(w, p, None),
        )
    }

    fn sync_theme_manager(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client = Arc::clone(client);
        let tx = self.file_tx.clone();
        Self::sync_tool(
            projection.theme_manager_visible,
            &mut self.theme_manager,
            projection,
            main,
            &self.modal_depth,
            || ThemeManagerWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_theme_manager(w, client, tx),
            |w, p| sync_theme_manager(w, p, None),
        )
    }

    fn sync_host_keys(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client_main = Arc::clone(client);
        let clear = Rc::clone(&self.host_keys_clear);
        let remove = Rc::clone(&self.host_keys_remove);
        Self::sync_tool(
            projection.host_keys_modal_visible,
            &mut self.host_keys,
            projection,
            main,
            &self.modal_depth,
            || HostKeysWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_host_keys(w, client_main, clear, remove),
            |w, p| sync_host_keys(w, p, None),
        )?;
        let client_add = Arc::clone(client);
        Self::sync_tool(
            projection.hosts_add_visible,
            &mut self.known_host_add,
            projection,
            main,
            &self.modal_depth,
            || KnownHostAddWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_known_host_add(w, client_add),
            |w, p| sync_known_host_add(w, p, None),
        )?;
        let client_import = Arc::clone(client);
        let tx = self.file_tx.clone();
        Self::sync_tool(
            projection.host_keys_import_visible,
            &mut self.known_host_import,
            projection,
            main,
            &self.modal_depth,
            || KnownHostImportWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_known_host_import(w, client_import, tx),
            |w, p| sync_known_host_import(w, p, None),
        )?;
        let client_export = Arc::clone(client);
        let tx = self.file_tx.clone();
        Self::sync_tool(
            projection.hosts_export_visible,
            &mut self.known_host_export,
            projection,
            main,
            &self.modal_depth,
            || KnownHostExportWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_known_host_export(w, client_export, tx),
            |w, p| sync_known_host_export(w, p, None),
        )
    }

    fn sync_logging(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client = Arc::clone(client);
        let tx = self.file_tx.clone();
        Self::sync_tool(
            projection.logging_dialog_visible,
            &mut self.logging,
            projection,
            main,
            &self.modal_depth,
            || LoggingWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_logging(w, client, tx),
            |w, p| sync_logging(w, p, None),
        )
    }

    fn sync_sftp(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let client_conflict = Arc::clone(client);
        Self::sync_tool(
            projection.sftp_conflict_visible,
            &mut self.sftp_conflict,
            projection,
            main,
            &self.modal_depth,
            || SftpConflictWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_sftp_conflict(w, client_conflict),
            sync_sftp_conflict,
        )?;
        let client_props = Arc::clone(client);
        Self::sync_tool(
            projection.sftp_properties_visible,
            &mut self.sftp_properties,
            projection,
            main,
            &self.modal_depth,
            || SftpPropertiesWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_sftp_properties(w, client_props),
            sync_sftp_properties,
        )?;
        let client_chmod = Arc::clone(client);
        Self::sync_tool(
            projection.sftp_chmod_visible,
            &mut self.sftp_chmod,
            projection,
            main,
            &self.modal_depth,
            || SftpChmodWindow::new().map_err(AppError::from_error),
            move |w| remote_dialog_wire::wire_sftp_chmod(w, client_chmod),
            sync_sftp_chmod,
        )
    }

    fn sync_about(
        &mut self,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let want = self.about_open.get();
        if want {
            if self.about.window.is_none() {
                let window = AboutWindow::new().map_err(AppError::from_error)?;
                window.set_native_title_drag(cfg!(windows));
                let preview = about_licenses_preview();
                window.set_licenses_text(preview.text);
                window.set_licenses_truncated(preview.truncated);
                window.set_licenses_preview_lines(preview.lines);
                let weak = window.as_weak();
                window.on_title_drag_requested(move || {
                    if let Some(window) = weak.upgrade() {
                        crate::win_chrome::start_native_window_move(window.window());
                        crate::win_chrome::reset_pointer_state_after_native_move(window.as_weak());
                    }
                });
                window.on_open_full_licenses(|| {
                    open_full_open_source_licenses();
                });
                let flag = Rc::clone(&self.about_open);
                let refresh = Rc::clone(&self.needs_refresh);
                window.on_dismiss({
                    let flag = Rc::clone(&flag);
                    let refresh = Rc::clone(&refresh);
                    move || {
                        flag.set(false);
                        refresh.set(true);
                    }
                });
                window.window().on_close_requested({
                    move || {
                        flag.set(false);
                        refresh.set(true);
                        slint::CloseRequestResponse::HideWindow
                    }
                });
                self.about.window = Some(window);
            }
            if let Some(window) = self.about.window.as_ref() {
                window.set_version_text(projection.app_version_text.clone().into());
                window.set_config_dir_text(projection.config_dir_text.clone().into());
                apply_tool_window_theme(&window.global::<Theme>(), projection);
                if let Some(main) = main {
                    apply_reduce_motion(
                        &window.global::<Theme>(),
                        main.global::<Theme>().get_reduce_motion(),
                    );
                }
                if !self.about.shown {
                    self.begin_modal(main);
                    window.show().map_err(AppError::from_error)?;
                    crate::win_chrome::prepare_aux_window(window.window());
                    if let Some(main) = main {
                        crate::win_chrome::attach_owned_popup(window.window(), main.window());
                        crate::win_chrome::center_aux_window(window.window(), main.window());
                        crate::win_chrome::finish_owned_aux_window_when_ready(
                            window.as_weak(),
                            main.as_weak(),
                            8,
                        );
                    }
                    self.about.shown = true;
                }
            }
        } else if self.about.shown {
            if let Some(window) = self.about.window.as_ref() {
                let _ = window.hide();
            }
            self.about.shown = false;
            self.end_modal(main);
        }
        Ok(())
    }

    fn sync_new_folder(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let want = self.new_folder_open.get();
        if want {
            if self.new_folder.window.is_none() {
                let window = NewFolderWindow::new().map_err(AppError::from_error)?;
                remote_dialog_wire::wire_new_folder(
                    &window,
                    Arc::clone(client),
                    Rc::clone(&self.new_folder_open),
                    Rc::clone(&self.needs_refresh),
                );
                self.new_folder.window = Some(window);
            }
            if let Some(window) = self.new_folder.window.as_ref() {
                apply_tool_window_theme(&window.global::<Theme>(), projection);
                if let Some(main) = main {
                    apply_reduce_motion(
                        &window.global::<Theme>(),
                        main.global::<Theme>().get_reduce_motion(),
                    );
                }
                if !self.new_folder.shown {
                    if let Some(main) = main {
                        window.set_name_text(main.get_new_root_folder_name_text());
                    }
                    self.begin_modal(main);
                    window.show().map_err(AppError::from_error)?;
                    crate::win_chrome::prepare_aux_window(window.window());
                    if let Some(main) = main {
                        crate::win_chrome::attach_owned_popup(window.window(), main.window());
                        crate::win_chrome::center_aux_window(window.window(), main.window());
                        crate::win_chrome::finish_owned_aux_window_when_ready(
                            window.as_weak(),
                            main.as_weak(),
                            8,
                        );
                    }
                    self.new_folder.shown = true;
                }
            }
        } else if self.new_folder.shown {
            if let Some(window) = self.new_folder.window.as_ref() {
                let _ = window.hide();
            }
            self.new_folder.shown = false;
            self.end_modal(main);
        }
        Ok(())
    }

    fn sync_sftp_entry(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let kind = self.sftp_dialog_kind.borrow().clone();
        let want = kind != "none";
        if want {
            if self.sftp_entry.window.is_none() {
                let window = SftpEntryWindow::new().map_err(AppError::from_error)?;
                remote_dialog_wire::wire_sftp_entry(
                    &window,
                    Arc::clone(client),
                    Rc::clone(&self.sftp_dialog_kind),
                    Rc::clone(&self.needs_refresh),
                );
                self.sftp_entry.window = Some(window);
            }
            if let Some(window) = self.sftp_entry.window.as_ref() {
                apply_tool_window_theme(&window.global::<Theme>(), projection);
                if let Some(main) = main {
                    apply_reduce_motion(
                        &window.global::<Theme>(),
                        main.global::<Theme>().get_reduce_motion(),
                    );
                }
                window.set_kind(kind.into());
                if let Some(main) = main {
                    window.set_selected_name_text(main.get_sftp_selected_name_text());
                    window.set_selected_path_text(main.get_sftp_selected_path_text());
                    window.set_selected_count(main.get_sftp_selected_count());
                    if !self.sftp_entry.shown {
                        window.set_name_text(main.get_sftp_dialog_name_text());
                        window.set_permissions_text(main.get_sftp_dialog_permissions_text());
                        window.set_local_path_text(main.get_sftp_dialog_local_text());
                    }
                }
                if !self.sftp_entry.shown {
                    self.begin_modal(main);
                    window.show().map_err(AppError::from_error)?;
                    crate::win_chrome::prepare_aux_window(window.window());
                    if let Some(main) = main {
                        crate::win_chrome::attach_owned_popup(window.window(), main.window());
                        crate::win_chrome::center_aux_window(window.window(), main.window());
                        crate::win_chrome::finish_owned_aux_window_when_ready(
                            window.as_weak(),
                            main.as_weak(),
                            8,
                        );
                    }
                    self.sftp_entry.shown = true;
                }
            }
        } else if self.sftp_entry.shown {
            if let Some(window) = self.sftp_entry.window.as_ref() {
                let _ = window.hide();
            }
            self.sftp_entry.shown = false;
            self.end_modal(main);
        }
        Ok(())
    }

    fn sync_confirm(
        &mut self,
        client: &Arc<HostShellClient>,
        projection: &AppProjection,
        main: Option<&MainWindow>,
    ) -> AppResult<()> {
        let spec = active_confirm(
            projection,
            main,
            self.host_keys_clear.get(),
            self.host_keys_remove.get(),
            self.clear_scrollback.get(),
            self.delete_session.get(),
            self.quit.get(),
        );
        let Some(spec) = spec else {
            if self.confirm_shown {
                if let Some(window) = self.confirm.as_ref() {
                    let _ = window.hide();
                }
                self.confirm_shown = false;
                self.confirm_kind.set(0);
                self.end_modal(main);
            }
            return Ok(());
        };
        if self.confirm.is_none() {
            let window = CompactConfirmWindow::new().map_err(AppError::from_error)?;
            window.set_native_title_drag(cfg!(windows));
            let weak = window.as_weak();
            window.on_title_drag_requested(move || {
                if let Some(window) = weak.upgrade() {
                    crate::win_chrome::start_native_window_move(window.window());
                    crate::win_chrome::reset_pointer_state_after_native_move(window.as_weak());
                }
            });
            let kind_cell = Rc::clone(&self.confirm_kind);
            let clear = Rc::clone(&self.host_keys_clear);
            let remove = Rc::clone(&self.host_keys_remove);
            let clear_scroll = Rc::clone(&self.clear_scrollback);
            let delete = Rc::clone(&self.delete_session);
            let quit = Rc::clone(&self.quit);
            let pending_exit = Rc::clone(&self.pending_exit);
            let refresh = Rc::clone(&self.needs_refresh);
            let rt = AppRuntimeClient::remote(Arc::clone(client));
            let weak = window.as_weak();
            window.on_confirmed({
                let kind_cell = Rc::clone(&kind_cell);
                let clear = Rc::clone(&clear);
                let remove = Rc::clone(&remove);
                let clear_scroll = Rc::clone(&clear_scroll);
                let delete = Rc::clone(&delete);
                let quit = Rc::clone(&quit);
                let pending_exit = Rc::clone(&pending_exit);
                let refresh = Rc::clone(&refresh);
                let rt = rt.clone();
                let weak = weak.clone();
                move || {
                    let dont_ask = weak
                        .upgrade()
                        .is_some_and(|window| window.get_dont_ask_again());
                    dispatch_remote_confirm(
                        &rt,
                        kind_cell.get(),
                        true,
                        &clear,
                        &remove,
                        &clear_scroll,
                        &delete,
                        &quit,
                        &pending_exit,
                        &refresh,
                        dont_ask,
                    );
                }
            });
            window.on_canceled({
                let kind_cell = Rc::clone(&kind_cell);
                let clear = Rc::clone(&clear);
                let remove = Rc::clone(&remove);
                let clear_scroll = Rc::clone(&clear_scroll);
                let delete = Rc::clone(&delete);
                let quit = Rc::clone(&quit);
                let pending_exit = Rc::clone(&pending_exit);
                let refresh = Rc::clone(&refresh);
                let rt = rt.clone();
                move || {
                    dispatch_remote_confirm(
                        &rt,
                        kind_cell.get(),
                        false,
                        &clear,
                        &remove,
                        &clear_scroll,
                        &delete,
                        &quit,
                        &pending_exit,
                        &refresh,
                        false,
                    );
                }
            });
            window.window().on_close_requested({
                let kind_cell = Rc::clone(&self.confirm_kind);
                let clear = Rc::clone(&self.host_keys_clear);
                let remove = Rc::clone(&self.host_keys_remove);
                let clear_scroll = Rc::clone(&self.clear_scrollback);
                let delete = Rc::clone(&self.delete_session);
                let quit = Rc::clone(&self.quit);
                let pending_exit = Rc::clone(&self.pending_exit);
                let refresh = Rc::clone(&self.needs_refresh);
                let rt = AppRuntimeClient::remote(Arc::clone(client));
                move || {
                    dispatch_remote_confirm(
                        &rt,
                        kind_cell.get(),
                        false,
                        &clear,
                        &remove,
                        &clear_scroll,
                        &delete,
                        &quit,
                        &pending_exit,
                        &refresh,
                        false,
                    );
                    slint::CloseRequestResponse::HideWindow
                }
            });
            self.confirm = Some(window);
        }
        let window = self.confirm.as_ref().expect("confirm window");
        apply_tool_window_theme(&window.global::<Theme>(), projection);
        if let Some(main) = main {
            apply_reduce_motion(
                &window.global::<Theme>(),
                main.global::<Theme>().get_reduce_motion(),
            );
        }
        window.set_title_text(spec.title.into());
        window.set_message_text(spec.message.into());
        window.set_confirm_label(spec.label.into());
        let close_tabs = spec.kind == CONFIRM_CLOSE_TABS;
        let quit = spec.kind == CONFIRM_QUIT;
        let close_window = spec.kind == CONFIRM_CLOSE_WINDOW;
        let mcp = spec.kind == CONFIRM_MCP_CONSENT;
        window.set_kind(
            match spec.kind {
                CONFIRM_EDITOR_DISCARD => "editor-discard",
                CONFIRM_CLOSE_TABS => "close-tabs",
                _ => "",
            }
            .into(),
        );
        window.set_close_tabs_single(projection.close_tabs_confirm_single);
        window.set_close_tabs_name(projection.close_tabs_confirm_name_text.clone().into());
        window.set_close_tabs_count(projection.close_tabs_confirm_count);
        window.set_close_tabs_active_count(projection.close_tabs_confirm_active_count);
        let note = if close_tabs {
            projection.close_tabs_confirm_note_text.clone()
        } else if quit {
            projection.quit_confirm_note_text.clone()
        } else {
            String::new()
        };
        window.set_note_text(note.into());
        let detail_lines = if close_tabs {
            projection
                .close_tabs_confirm_details
                .iter()
                .map(|line| SharedString::from(line.as_str()))
                .collect::<Vec<_>>()
        } else if quit {
            projection
                .quit_confirm_details
                .iter()
                .map(|line| SharedString::from(line.as_str()))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        window.set_details(ModelRc::new(VecModel::from(detail_lines)));
        window.set_destructive(!mcp);
        window.set_show_dont_ask_again(close_tabs || quit || close_window || mcp);
        window.set_dont_ask_again(match spec.kind {
            CONFIRM_CLOSE_TABS => main
                .map(|window| window.get_close_confirm_dont_ask_again())
                .unwrap_or(false),
            CONFIRM_QUIT => main
                .map(|window| window.get_quit_confirm_dont_ask_again())
                .unwrap_or(false),
            CONFIRM_CLOSE_WINDOW => main
                .map(|window| window.get_close_window_confirm_dont_ask_again())
                .unwrap_or(false),
            CONFIRM_MCP_CONSENT => projection.settings_mcp_auto_approve_control,
            _ => false,
        });
        self.confirm_kind.set(spec.kind);
        if !self.confirm_shown {
            self.begin_modal(main);
            window.show().map_err(AppError::from_error)?;
            crate::win_chrome::prepare_aux_window(window.window());
            if let Some(main) = main {
                crate::win_chrome::finish_owned_aux_window_when_ready(
                    window.as_weak(),
                    main.as_weak(),
                    8,
                );
            }
            self.confirm_shown = true;
        }
        Ok(())
    }
}

struct ConfirmView {
    kind: u8,
    title: String,
    message: String,
    label: &'static str,
}

fn active_confirm(
    projection: &AppProjection,
    main: Option<&MainWindow>,
    host_keys_clear: bool,
    host_keys_remove: bool,
    clear_scrollback: bool,
    delete_session: bool,
    quit: bool,
) -> Option<ConfirmView> {
    if projection.private_keys_remove_confirm_visible {
        return Some(ConfirmView {
            kind: CONFIRM_KEYS_REMOVE,
            title: "Remove Managed Key".into(),
            message: projection.private_keys_remove_warning_text.clone(),
            label: "Remove",
        });
    }
    if projection.proxies_remove_confirm_visible {
        return Some(ConfirmView {
            kind: CONFIRM_PROXIES_REMOVE,
            title: "Delete Proxy Profile".into(),
            message: "This removes the managed proxy profile and its saved password from the secret store."
                .into(),
            label: "Delete",
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
        return Some(ConfirmView {
            kind: CONFIRM_THEME_REMOVE,
            title: "Delete Theme".into(),
            message,
            label: "Delete",
        });
    }
    if host_keys_clear {
        return Some(ConfirmView {
            kind: CONFIRM_HOST_KEYS_CLEAR,
            title: "Clear All Host Keys".into(),
            message: "Remove every saved host key from disk? This cannot be undone.".into(),
            label: "Clear All",
        });
    }
    if host_keys_remove {
        return Some(ConfirmView {
            kind: CONFIRM_HOST_KEYS_REMOVE,
            title: "Remove Host Keys".into(),
            message: "Remove all saved keys for the selected host:port? This deletes the entire group, not just the selected key.".into(),
            label: "Remove",
        });
    }
    if projection.editor_discard_dialog_visible {
        return Some(ConfirmView {
            kind: CONFIRM_EDITOR_DISCARD,
            title: "Discard Changes".into(),
            message: "Discard unsaved changes to this session?".into(),
            label: "Discard",
        });
    }
    if projection.close_tabs_confirm_visible {
        let (title, message) = if projection.close_tabs_confirm_single {
            (
                "Close Session".to_owned(),
                format!(
                    "Close the tab for session “{}”?",
                    projection.close_tabs_confirm_name_text
                ),
            )
        } else {
            (
                "Close Tabs".to_owned(),
                format!("Close {} tabs?", projection.close_tabs_confirm_count),
            )
        };
        return Some(ConfirmView {
            kind: CONFIRM_CLOSE_TABS,
            title,
            message,
            label: "Close",
        });
    }
    if projection.close_window_confirm_visible {
        let message = if projection.close_window_confirm_active_count == 1 {
            "Close this window? 1 active session will be disconnected.".to_owned()
        } else {
            format!(
                "Close this window? {} active sessions will be disconnected.",
                projection.close_window_confirm_active_count
            )
        };
        return Some(ConfirmView {
            kind: CONFIRM_CLOSE_WINDOW,
            title: "Close Window".into(),
            message,
            label: "Close",
        });
    }
    if clear_scrollback {
        return Some(ConfirmView {
            kind: CONFIRM_CLEAR_SCROLLBACK,
            title: "Clear Scrollback".into(),
            message: "Clear the terminal scrollback buffer? The live screen stays as-is.".into(),
            label: "Clear Scrollback",
        });
    }
    if delete_session {
        return Some(ConfirmView {
            kind: CONFIRM_DELETE_SESSION,
            title: "Delete Saved Session".into(),
            message: "This permanently removes the selected saved session from disk.".into(),
            label: "Delete",
        });
    }
    if quit {
        let message = if projection.quit_confirm_details.is_empty() {
            "Quit YShell now? Active runtime sessions will be disconnected.".to_owned()
        } else {
            "Quit the application while sessions are still active?".to_owned()
        };
        let _ = main;
        return Some(ConfirmView {
            kind: CONFIRM_QUIT,
            title: "Quit YShell".into(),
            message,
            label: "Quit",
        });
    }
    if projection.mcp_consent_visible {
        return Some(ConfirmView {
            kind: CONFIRM_MCP_CONSENT,
            title: "Allow AI control of terminals".into(),
            message: "An AI agent wants control of YShell terminal sessions: run commands, read the screen, and open or close tabs. It cannot fill passwords or accept host keys — those stay in this app. Allow?".into(),
            label: "Allow",
        });
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn dispatch_remote_confirm(
    rt: &AppRuntimeClient,
    kind: u8,
    confirmed: bool,
    clear: &Cell<bool>,
    remove: &Cell<bool>,
    clear_scroll: &Cell<bool>,
    delete: &Cell<bool>,
    quit: &Cell<bool>,
    pending_exit: &Cell<bool>,
    refresh: &Cell<bool>,
    dont_ask: bool,
) {
    refresh.set(true);
    match kind {
        CONFIRM_CLEAR_SCROLLBACK => clear_scroll.set(false),
        CONFIRM_DELETE_SESSION => delete.set(false),
        CONFIRM_QUIT => quit.set(false),
        _ => {}
    }

    if kind == CONFIRM_QUIT {
        if confirmed {
            if dont_ask {
                let _ = rt.call_value("set_confirm_quit_app", json!([false]));
            }
            pending_exit.set(true);
        }
        let _ = rt.call_value("mark_ui_dirty", json!([]));
        return;
    }
    if kind == CONFIRM_CLOSE_WINDOW {
        if confirmed {
            let _ = rt.call_value("confirm_close_window", json!([0u64, dont_ask]));
            pending_exit.set(true);
        } else {
            let _ = rt.call_value("cancel_close_window", json!([0u64]));
        }
        let _ = rt.call_value("mark_ui_dirty", json!([]));
        return;
    }
    if kind == CONFIRM_MCP_CONSENT {
        let _ = rt.call_value("resolve_mcp_consent", json!([confirmed, dont_ask]));
        let _ = rt.call_value("mark_ui_dirty", json!([]));
        return;
    }
    if kind == CONFIRM_CLOSE_TABS {
        if confirmed {
            let _ = rt.call_value("confirm_close_tabs_with", json!([dont_ask]));
        } else {
            let _ = rt.call_value("cancel_close_tabs", json!([]));
        }
        let _ = rt.call_value("mark_ui_dirty", json!([]));
        return;
    }
    let method = match (kind, confirmed) {
        (CONFIRM_KEYS_REMOVE, true) => "confirm_private_key_remove",
        (CONFIRM_KEYS_REMOVE, false) => "cancel_private_key_remove",
        (CONFIRM_PROXIES_REMOVE, true) => "confirm_proxy_remove",
        (CONFIRM_PROXIES_REMOVE, false) => "cancel_proxy_remove",
        (CONFIRM_THEME_REMOVE, true) => "confirm_delete_color_theme",
        (CONFIRM_THEME_REMOVE, false) => "cancel_delete_color_theme",
        (CONFIRM_HOST_KEYS_CLEAR, true) => {
            clear.set(false);
            "clear_all_host_keys"
        }
        (CONFIRM_HOST_KEYS_CLEAR, false) => {
            clear.set(false);
            ""
        }
        (CONFIRM_HOST_KEYS_REMOVE, true) => {
            remove.set(false);
            "remove_selected_host_key"
        }
        (CONFIRM_HOST_KEYS_REMOVE, false) => {
            remove.set(false);
            ""
        }
        (CONFIRM_EDITOR_DISCARD, true) => "confirm_discard_session_editor",
        (CONFIRM_EDITOR_DISCARD, false) => "cancel_discard_session_editor",
        (CONFIRM_CLEAR_SCROLLBACK, true) => "clear_active_terminal_scrollback",
        (CONFIRM_DELETE_SESSION, true) => "delete_selected_saved_session",
        _ => "",
    };
    if !method.is_empty() {
        let _ = rt.call_value(method, json!([]));
    }
    let _ = rt.call_value("mark_ui_dirty", json!([]));
}

/// I-tier and live counts. L-tier widget fields stay as the Shell draft.
fn sync_settings_host_live(settings: &SettingsWindow, projection: &AppProjection) {
    settings.set_config_dir_text(projection.config_dir_text.clone().into());
    settings.set_language_text(projection.settings_language_text.clone().into());
    settings.set_debug_logging_enabled(projection.settings_debug_logging_enabled);
    settings.set_debug_log_dir_text(projection.settings_debug_log_dir_text.clone().into());
    settings.set_confirm_close_session_enabled(projection.settings_confirm_close_session);
    settings.set_confirm_quit_app_enabled(projection.settings_confirm_quit_app);
    settings.set_mcp_enabled(projection.settings_mcp_enabled);
    settings.set_mcp_port_text(projection.settings_mcp_port_text.clone().into());
    settings.set_mcp_listen_lan(projection.settings_mcp_listen_lan);
    settings.set_mcp_api_key_text(projection.settings_mcp_api_key_text.clone().into());
    settings.set_mcp_view_user_connections(projection.settings_mcp_view_user_connections);
    settings.set_mcp_control_user_connections(projection.settings_mcp_control_user_connections);
    settings.set_mcp_auto_approve_control_enabled(projection.settings_mcp_auto_approve_control);
    settings.set_mcp_status_text(projection.settings_mcp_status_text.clone().into());
    settings.set_mcp_boundaries_text(projection.settings_mcp_boundaries_text.clone().into());
    settings.set_mcp_host_config_text(projection.settings_mcp_host_config_text.clone().into());
    settings.set_settings_master_password_set(projection.settings_master_password_set);
    settings.set_settings_master_password_locked(projection.settings_master_password_locked);
    settings
        .set_settings_security_known_hosts_count(projection.settings_security_known_hosts_count);
    settings
        .set_settings_security_private_keys_count(projection.settings_security_private_keys_count);
    settings.set_settings_security_proxies_count(projection.settings_security_proxies_count);
    set_settings_options(
        settings,
        &projection.settings_appearance_font_options,
        &projection.settings_appearance_style_options,
    );
}

fn begin_modal_depth(depth: &Cell<u32>, main: Option<&MainWindow>) {
    if depth.get() == 0 {
        if let Some(main) = main {
            crate::win_chrome::set_window_enabled(main.window(), false);
        }
    }
    depth.set(depth.get().saturating_add(1));
}

fn end_modal_depth(depth: &Cell<u32>, main: Option<&MainWindow>) {
    let current = depth.get();
    if current == 0 {
        return;
    }
    let current = current - 1;
    depth.set(current);
    if current > 0 {
        return;
    }
    if let Some(main) = main {
        crate::win_chrome::set_window_enabled(main.window(), true);
        crate::win_chrome::focus_window(main.window());
        main.invoke_focus_global_shortcuts();
    }
}

/// Decode Host JSON and sync remote dialogs.
pub fn sync_from_value(
    host: &mut RemoteDialogHost,
    client: &Arc<HostShellClient>,
    value: serde_json::Value,
) {
    match projection_from_json(value) {
        Ok(projection) => {
            if let Err(error) = host.sync(client, &projection) {
                tracing::warn!(error = %error, "remote dialog sync failed");
            }
        }
        Err(error) => tracing::warn!(error = %error, "remote dialog projection decode failed"),
    }
}
