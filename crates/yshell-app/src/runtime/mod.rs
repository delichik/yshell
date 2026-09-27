//! Runtime composition layer that bridges config, core commands, and UI callbacks.
//!
//! The runtime state machine lives in submodules by feature domain; items are
//! re-exported here, so `crate::runtime::*` import paths stay stable.
//!
//! Submodule map (each file is a separate single-writer unit):
//!
//! * `tabs` - terminal tabs, close scopes, session polling cursor
//! * `menus` - menu flags and tab context-menu state
//! * `sessions` - saved-session table, recent sessions, sidebar tree
//! * `connection` - backend selection, connect/reconnect/disconnect, prompts
//! * `ssh_config` - SSH/proxy/tunnel config assembly and secret resolution
//! * `sftp` - SFTP browsing state; `sftp_ops` - SFTP mutations
//! * `transfer` - SFTP transfer queue
//! * `logging` - global logging settings and per-session logging
//! * `auth` - known-hosts manager and host-key policy helpers
//! * `editor` - session editor draft and persistence
//! * `panels` - panel visibility and the settings (scrollback) form
//! * `keys` - active terminal input, keys, clipboard, search, selection
//! * `projection` - `AppProjection` assembly and sub-projections

mod auth;
mod clipboard;
mod connection;
mod editor;
mod keys;
mod local_pane;
mod logging;
mod menus;
mod panels;
mod projection;
mod quick_connect;
mod sessions;
mod sftp;
mod sftp_ops;
mod ssh_config;
mod tabs;
mod transfer;

#[cfg(test)]
mod tests;

pub(crate) use auth::config_host_key_policy_label;
pub(crate) use auth::config_host_key_policy_to_runtime;
pub(crate) use auth::runtime_host_key_policy_to_config;
// N1 Phase 2：本地栏 / 队列抽屉 / 冲突策略的运行时接口。
pub(crate) use clipboard::{ClipboardSide, FileClipboard, MoveCleanup};
pub(crate) use connection::HostKeyPromptMode;
pub(crate) use connection::PendingHostKeyPrompt;
#[cfg(test)]
pub(crate) use connection::PendingPasswordAuthMethod;
pub(crate) use connection::PendingPasswordPrompt;
pub(crate) use local_pane::LocalPaneState;
pub(crate) use sessions::collect_session_inventory_lines;
pub(crate) use sessions::find_session_folder_id;
pub use sessions::SessionTreeRow;
pub(crate) use sessions::SAVED_SESSIONS_FOLDER_ID;
pub(crate) use sessions::SAVED_SESSIONS_FOLDER_NAME;
pub(crate) use sftp::join_remote_path;
pub(crate) use sftp::normalize_remote_path;
#[cfg(test)]
pub(crate) use sftp::parent_remote_path;
pub(crate) use sftp::parse_sftp_permissions;
pub use sftp::SftpCrumbData;
pub(crate) use sftp::SftpListingState;
pub use sftp::SftpRowData;
pub(crate) use sftp::SftpSessionLifecycle;
pub(crate) use sftp::SftpUnavailableReason;
pub(crate) use ssh_config::proxy_protocol_label;
pub(crate) use ssh_config::tunnel_config_summary;
pub(crate) use ssh_config::tunnel_forward_summary;
pub(crate) use ssh_config::tunnel_kind_label;
pub(crate) use tabs::PendingCloseTabs;
pub(crate) use transfer::{
    apply_sftp_tree_report, overwrite_policy_from_id, overwrite_policy_id, overwrite_policy_label,
    tree_transfer_status_text, update_sftp_transfer_progress,
};
pub use transfer::{SftpConflictPrompt, TransferQueueCounts, TransferRowData};
pub use tabs::TabData;
pub(crate) use tabs::TabEntry;
#[cfg(test)]
pub(crate) use tabs::CLOSE_SCOPE_ALL;
#[cfg(test)]
pub(crate) use tabs::CLOSE_SCOPE_DISCONNECTED;
// N4 Phase 2：密钥管理页 / 认证弹窗的投影数据类型（bootstrap/projection 消费）。
pub(crate) use auth::HostKeyEntryData;
pub(crate) use auth::HostKeyGroupData;
pub(crate) use auth::PrivateKeyEntryInfo;
pub(crate) use auth::PrivateKeyRowData;
pub(crate) use connection::AuthKeyOptionData;
pub(crate) use connection::AuthPromptMethod;
pub(crate) use connection::AuthPromptQuestionData;
pub(crate) use connection::PendingAuthPrompt;
pub(crate) use editor::parse_port_field;
pub(crate) use editor::EditorAuthMethod;
pub(crate) use editor::EditorAuthTestStatus;
pub(crate) use editor::EditorProxyMode;
pub(crate) use editor::EditorSection;
pub(crate) use editor::SessionEditorDraft;
// N6 Phase 2：终端日志弹窗状态（mod.rs 字段 / bootstrap 消费）。
pub(crate) use logging::LoggingDialogState;
pub(crate) use panels::SettingsTerminalStatus;
pub use projection::AppProjection;
pub use quick_connect::QuickConnectRowData;
pub use quick_connect::QuickLinkRowData;

use crate::{
    error::AppError, error::AppResult, session_runtime::SessionRuntime, sftp_view::SftpSortColumn,
};
use std::{
    collections::BTreeMap, collections::BTreeSet, env, fmt, fs, path::Path, path::PathBuf,
    sync::Arc,
};
use yshell_config::{ConfigDocument, ConfigStore, LoadOutcome, QuickLink};
use yshell_core::CoreCommandDispatcher;
#[cfg(test)]
use yshell_core::SessionEvent;
use yshell_secret::{FileKeychain, Keychain, OsKeychain};
use yshell_sftp::{FsEntry, RemoteEditSession, TransferQueue};
use yshell_ssh::{HostKeyPolicy, KnownHosts, TransportBackend};
use yshell_terminal::SearchMatch;

impl AppRuntime {
    pub fn new(config_dir: PathBuf) -> AppResult<Self> {
        let selection = default_runtime_keychain(&config_dir)?;
        Self::new_with_keychain_and_secret_store_path(
            config_dir,
            selection.keychain,
            selection.secret_store_path,
            selection.kind,
        )
    }

    pub(crate) fn new_with_keychain_and_secret_store_path(
        config_dir: PathBuf,
        keychain: Option<Arc<dyn Keychain>>,
        secret_store_path: Option<PathBuf>,
        secret_store_kind: SecretStoreKind,
    ) -> AppResult<Self> {
        let config_store = ConfigStore::new(config_dir.clone());
        let LoadOutcome {
            document,
            recovered_from_backup,
        } = config_store
            .load_or_recover()
            .map_err(AppError::from_error)?;
        let terminal = document.terminal.clone();
        let persistent_known_hosts = config_store
            .load_known_hosts()
            .map_err(AppError::from_error)?;
        let mut runtime = Self {
            config_dir: config_dir.clone(),
            config_store,
            config_document: document,
            dispatcher: CoreCommandDispatcher::new(),
            sessions: BTreeMap::new(),
            active_session_id: None,
            tabs: Vec::new(),
            active_tab_id: None,
            terminal_poll_cursor: 0,
            pending_close_tabs: None,
            tab_menu_tab_id: None,
            selected_saved_session_id: None,
            selected_saved_folder_id: None,
            collapsed_saved_folders: BTreeSet::new(),
            recent_session_ids: Vec::new(),
            session_search_query: String::new(),
            sftp_visible: true,
            tunnels_visible: true,
            commands_visible: true,
            secret_store_path,
            secret_store_kind,
            secret_reset_confirmation: String::new(),
            editor: SessionEditorDraft::default(),
            new_folder_name: String::new(),
            editor_auth_test_status: EditorAuthTestStatus::Hint,
            status_text: String::new(),
            status_kind: String::new(),
            status_param_1: String::new(),
            status_param_2: String::new(),
            status_kind_source: String::new(),
            settings_scrollback_lines_text: terminal.scrollback_lines.to_string(),
            settings_scrollback_max_cells_text: terminal.scrollback_max_cells.to_string(),
            settings_terminal_status: SettingsTerminalStatus::Hint,
            terminal_clipboard: String::new(),
            terminal_search_query: String::new(),
            terminal_search_matches: Vec::new(),
            terminal_search_current_index: None,
            sftp_path: "/".to_owned(),
            sftp_listing: SftpListingState::ConnectReal,
            sftp_local_path: config_dir.join("sftp-transfer.txt").display().to_string(),
            sftp_remote_target: String::new(),
            sftp_secondary_target: String::new(),
            sftp_permissions: "644".to_owned(),
            sftp_entries: Vec::new(),
            sftp_selected_index: None,
            sftp_sort_column: SftpSortColumn::Name,
            sftp_sort_ascending: true,
            sftp_show_hidden: false,
            sftp_session: SftpSessionLifecycle::Disconnected { session_key: None },
            remote_edit_session: None,
            transfer_queue: TransferQueue::default(),
            next_transfer_ordinal: 1,
            local_pane: LocalPaneState::default(),
            sftp_selection: BTreeSet::new(),
            sftp_selection_anchor: None,
            transfer_drawer_expanded: false,
            pending_sftp_conflict: None,
            sftp_properties_open: false,
            file_clipboard: None,
            pending_move_cleanup: BTreeMap::new(),
            sftp_upload_dir_override: None,
            sftp_jobs: None,
            persistent_known_hosts,
            temporary_known_hosts: KnownHosts::new(),
            known_hosts_modal_visible: false,
            known_hosts_selected_key: None,
            known_hosts_clear_confirmation: String::new(),
            pending_host_key_prompt: None,
            host_key_replace_confirmation: String::new(),
            pending_password_prompt: None,
            transport_backend: TransportBackend::Fake,
            editor_modal_visible: false,
            editor_section: EditorSection::General,
            host_key_policy_override: None,
            recovered_from_backup,
            next_runtime_ordinal: 1,
            keychain: keychain.map(RuntimeKeychain),
            logging_dialog: LoggingDialogState::default(),
            last_logging_directory: None,
            quick_connect_input: String::new(),
            quick_connect_error_text: String::new(),
            quick_connect_last_target: String::new(),
            pending_quick_connect_targets: BTreeMap::new(),
            recent_saved_usage: Vec::new(),
            private_keys_modal_visible: false,
            private_keys_selected_id: None,
            private_keys_import_label: String::new(),
            private_keys_import_path: String::new(),
            private_keys_import_passphrase: String::new(),
            private_keys_remember_import_passphrase: false,
            private_keys_passphrase_input: String::new(),
            private_keys_remove_confirm_visible: false,
            private_keys_deploy_confirm_visible: false,
            private_keys_status_text: String::new(),
            private_keys_test_result_text: String::new(),
            host_keys_modal_visible: false,
            host_keys_selected_group: None,
            host_keys_selected_entry: None,
            host_keys_import_visible: false,
            host_keys_import_text: String::new(),
            host_keys_status_text: String::new(),
            pending_auth_prompt: None,
            fake_auth_scenario: env::var("YSHELL_FAKE_AUTH_FAILURE")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            fake_auth_failure_injected: false,
        };
        runtime.status_text = runtime.startup_status();
        runtime.hydrate_saved_sessions();
        Ok(runtime)
    }

    pub fn update_secret_reset_confirmation(&mut self, confirmation: &str) -> AppProjection {
        self.secret_reset_confirmation = confirmation.to_owned();
        self.projection()
    }

    pub fn reset_secret_store(&mut self) -> AppResult<AppProjection> {
        if self.secret_reset_confirmation.trim() != "RESET SECRETS" {
            return Err(AppError::new(
                "type `RESET SECRETS` exactly before resetting the local secret store",
            ));
        }
        let Some(path) = self.secret_store_path.clone() else {
            return Err(AppError::new(
                "secret reset is only available for the local file-backed secret store",
            ));
        };
        if path.exists() {
            fs::remove_file(&path).map_err(AppError::from_error)?;
        }
        let Some(master_password) = env::var("YSHELL_MASTER_PASSWORD").ok() else {
            self.keychain = None;
            self.secret_store_kind = SecretStoreKind::Disabled;
            self.secret_reset_confirmation.clear();
            let path_text = path.display().to_string();
            self.set_status_kind(
                "secrets-reset",
                format!(
                    "Deleted `{path_text}` and left the secret store disabled because YSHELL_MASTER_PASSWORD is not set."
                ),
                path_text,
                "master-password-missing".to_owned(),
            );
            return Ok(self.projection());
        };
        if master_password.trim().is_empty() {
            self.keychain = None;
            self.secret_store_kind = SecretStoreKind::Disabled;
            self.secret_reset_confirmation.clear();
            let path_text = path.display().to_string();
            self.set_status_kind(
                "secrets-reset",
                format!(
                    "Deleted `{path_text}` and left the secret store disabled because YSHELL_MASTER_PASSWORD is empty."
                ),
                path_text,
                "master-password-empty".to_owned(),
            );
            return Ok(self.projection());
        }
        let keychain =
            FileKeychain::open_or_create(&path, master_password).map_err(AppError::from_error)?;
        self.keychain = Some(RuntimeKeychain(Arc::new(keychain)));
        self.secret_store_kind = SecretStoreKind::File;
        self.secret_reset_confirmation.clear();
        let path_text = path.display().to_string();
        self.set_status_kind(
            "secrets-reset",
            format!("Reset the local secret store at `{path_text}`."),
            path_text,
            String::new(),
        );
        Ok(self.projection())
    }

    pub(crate) fn status_i18n_parts(&self) -> (String, String, String) {
        if self.status_kind.is_empty() || self.status_text != self.status_kind_source {
            return (String::new(), String::new(), String::new());
        }
        (
            self.status_kind.clone(),
            self.status_param_1.clone(),
            self.status_param_2.clone(),
        )
    }

    pub(crate) fn set_status_kind(
        &mut self,
        kind: &str,
        text: String,
        param_1: String,
        param_2: String,
    ) {
        self.status_text = text;
        self.status_kind = kind.to_owned();
        self.status_param_1 = param_1;
        self.status_param_2 = param_2;
        self.status_kind_source = self.status_text.clone();
    }

    pub(crate) fn allocate_runtime_ordinal(&mut self) -> usize {
        let ordinal = self.next_runtime_ordinal;
        self.next_runtime_ordinal += 1;
        ordinal
    }

    pub(crate) fn active_session_key(&self) -> AppResult<String> {
        self.active_session_id
            .clone()
            .ok_or_else(|| AppError::new("no active runtime session"))
    }
}

pub struct AppRuntime {
    pub(crate) config_dir: PathBuf,
    pub(crate) config_store: ConfigStore,
    pub(crate) config_document: ConfigDocument,
    pub(crate) dispatcher: CoreCommandDispatcher,
    pub(crate) sessions: BTreeMap<String, SessionRuntime>,
    pub(crate) active_session_id: Option<String>,
    /// N0：标签条顺序 = 显示顺序；`active_tab_id` 必须存在于 `tabs`。
    pub(crate) tabs: Vec<TabEntry>,
    pub(crate) active_tab_id: Option<String>,
    /// N0 轮询游标：非活动标签轮转的起点（对 `tabs` 顺序取模）。
    pub(crate) terminal_poll_cursor: usize,
    /// N0：等待确认的关闭请求（单关/批量复用同一个确认弹窗）。
    pub(crate) pending_close_tabs: Option<PendingCloseTabs>,
    /// N0：右键菜单针对的标签 id（`prepare_tab_context_menu` 设置）。
    pub(crate) tab_menu_tab_id: Option<String>,
    pub(crate) selected_saved_session_id: Option<String>,
    /// Selected folder in the sidebar tree (folder ids are not profile ids, so
    /// the tree keeps a separate selection slot). Selecting a folder clears the
    /// session selection and vice versa.
    pub(crate) selected_saved_folder_id: Option<String>,
    /// Collapsed folders in the sidebar tree (absence = expanded, the default).
    pub(crate) collapsed_saved_folders: BTreeSet<String>,
    pub(crate) recent_session_ids: Vec<String>,
    pub(crate) session_search_query: String,
    pub(crate) sftp_visible: bool,
    pub(crate) tunnels_visible: bool,
    pub(crate) commands_visible: bool,
    pub(crate) secret_store_path: Option<PathBuf>,
    pub(crate) secret_store_kind: SecretStoreKind,
    pub(crate) secret_reset_confirmation: String,
    pub(crate) editor: SessionEditorDraft,
    pub(crate) new_folder_name: String,
    pub(crate) editor_auth_test_status: EditorAuthTestStatus,
    pub(crate) status_text: String,
    /// `status_text` 的 i18n 模板 id 与参数（仅覆盖的生产者设置）。
    pub(crate) status_kind: String,
    pub(crate) status_param_1: String,
    pub(crate) status_param_2: String,
    /// 设置 kind 时的 `status_text` 快照：文本被其它路径改写后 kind 失效。
    pub(crate) status_kind_source: String,
    pub(crate) settings_scrollback_lines_text: String,
    pub(crate) settings_scrollback_max_cells_text: String,
    pub(crate) settings_terminal_status: SettingsTerminalStatus,
    pub(crate) terminal_clipboard: String,
    pub(crate) terminal_search_query: String,
    pub(crate) terminal_search_matches: Vec<SearchMatch>,
    pub(crate) terminal_search_current_index: Option<usize>,
    pub(crate) sftp_path: String,
    pub(crate) sftp_listing: SftpListingState,
    pub(crate) sftp_local_path: String,
    pub(crate) sftp_remote_target: String,
    pub(crate) sftp_secondary_target: String,
    pub(crate) sftp_permissions: String,
    pub(crate) sftp_entries: Vec<FsEntry>,
    pub(crate) sftp_selected_index: Option<usize>,
    pub(crate) sftp_sort_column: SftpSortColumn,
    pub(crate) sftp_sort_ascending: bool,
    pub(crate) sftp_show_hidden: bool,
    pub(crate) sftp_session: SftpSessionLifecycle,
    pub(crate) remote_edit_session: Option<RemoteEditSession>,
    pub(crate) transfer_queue: TransferQueue,
    pub(crate) next_transfer_ordinal: usize,
    // --- N1 Phase 2：本地栏 / 远端多选 / 队列抽屉 / 冲突与属性 / 内部剪贴板 ------
    pub(crate) local_pane: LocalPaneState,
    /// Remote multi-selection (paths; a single click keeps one entry here).
    pub(crate) sftp_selection: BTreeSet<String>,
    /// Shift-range anchor for the remote pane (path, so sorting/refresh keeps it).
    pub(crate) sftp_selection_anchor: Option<String>,
    pub(crate) transfer_drawer_expanded: bool,
    pub(crate) pending_sftp_conflict: Option<SftpConflictPrompt>,
    pub(crate) sftp_properties_open: bool,
    pub(crate) file_clipboard: Option<FileClipboard>,
    /// "Move" gestures: source paths to delete once the copy job succeeds,
    /// keyed by transfer id.
    pub(crate) pending_move_cleanup: BTreeMap<String, MoveCleanup>,
    /// "Upload Here..." target captured by the context menu (one-shot).
    pub(crate) sftp_upload_dir_override: Option<String>,
    /// SFTP transfer worker handle (injected by `bootstrap`; tests inject a
    /// channel-backed handle to assert submitted jobs).
    pub(crate) sftp_jobs: Option<crate::sftp_jobs::SftpJobHandle>,
    pub(crate) persistent_known_hosts: KnownHosts,
    pub(crate) temporary_known_hosts: KnownHosts,
    pub(crate) known_hosts_modal_visible: bool,
    pub(crate) known_hosts_selected_key: Option<String>,
    pub(crate) known_hosts_clear_confirmation: String,
    pub(crate) pending_host_key_prompt: Option<PendingHostKeyPrompt>,
    pub(crate) host_key_replace_confirmation: String,
    /// W5：密码型认证缺少密钥时的挂起目标（仅内存；重试连接用）。
    pub(crate) pending_password_prompt: Option<PendingPasswordPrompt>,
    pub(crate) transport_backend: TransportBackend,
    pub(crate) editor_modal_visible: bool,
    pub(crate) editor_section: EditorSection,
    pub(crate) host_key_policy_override: Option<HostKeyPolicy>,
    pub(crate) recovered_from_backup: Option<PathBuf>,
    pub(crate) next_runtime_ordinal: usize,
    pub(crate) keychain: Option<RuntimeKeychain>,
    /// N6：终端日志弹窗状态（打开时针对活动会话）。
    pub(crate) logging_dialog: LoggingDialogState,
    /// N6："上次使用目录"：手动日志开启成功后记住，作为下次弹窗默认保存位置。
    pub(crate) last_logging_directory: Option<PathBuf>,
    /// N2：Quick Connect 输入框内容（宿主与页面双向绑定）。
    pub(crate) quick_connect_input: String,
    /// N2：非法输入的内联提示（空 = 无错误）。
    pub(crate) quick_connect_error_text: String,
    /// N2：最近一次连接成功的目标（"保存为会话…"入口）。
    pub(crate) quick_connect_last_target: String,
    /// N2：QC 直连的挂起目标（session key → canonical），连接成功后落历史。
    pub(crate) pending_quick_connect_targets: BTreeMap<String, String>,
    /// N2：最近使用的已保存会话（profile_id, 时间戳），"最近连接"里的 saved 行。
    pub(crate) recent_saved_usage: Vec<(String, i64)>,
    // --- N4：密钥管理页（私钥 / 主机密钥）与认证弹窗 -------------------------
    /// 私钥管理页可见性。
    pub(crate) private_keys_modal_visible: bool,
    /// 私钥页当前选中的 key id。
    pub(crate) private_keys_selected_id: Option<String>,
    /// 导入表单镜像（页面 in-out → 宿主）。
    pub(crate) private_keys_import_label: String,
    pub(crate) private_keys_import_path: String,
    pub(crate) private_keys_import_passphrase: String,
    pub(crate) private_keys_remember_import_passphrase: bool,
    /// 选中密钥的口令输入（"记住口令"用）。
    pub(crate) private_keys_passphrase_input: String,
    pub(crate) private_keys_remove_confirm_visible: bool,
    pub(crate) private_keys_deploy_confirm_visible: bool,
    pub(crate) private_keys_status_text: String,
    pub(crate) private_keys_test_result_text: String,
    /// 主机密钥页可见性。
    pub(crate) host_keys_modal_visible: bool,
    pub(crate) host_keys_selected_group: Option<usize>,
    pub(crate) host_keys_selected_entry: Option<usize>,
    pub(crate) host_keys_import_visible: bool,
    pub(crate) host_keys_import_text: String,
    pub(crate) host_keys_status_text: String,
    /// 认证弹窗挂起状态（服务端驱动的认证失败；Phase 2 接线）。
    pub(crate) pending_auth_prompt: Option<PendingAuthPrompt>,
    /// e2e/dev：`YSHELL_FAKE_AUTH_FAILURE` 场景（fake 后端首次开 shell 注入认证失败）。
    pub(crate) fake_auth_scenario: Option<String>,
    /// 上述场景是否已注入（重试放行）。
    pub(crate) fake_auth_failure_injected: bool,
}

#[derive(Clone)]
pub(crate) struct RuntimeKeychain(Arc<dyn Keychain>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecretStoreKind {
    Disabled,
    File,
    Os,
    #[cfg(test)]
    Injected,
}

impl SecretStoreKind {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::File => "file",
            Self::Os => "os",
            #[cfg(test)]
            Self::Injected => "injected",
        }
    }
}

impl fmt::Debug for RuntimeKeychain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RuntimeKeychain(..)")
    }
}

impl fmt::Debug for AppRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppRuntime")
            .field("config_dir", &self.config_dir)
            .field("config_store", &self.config_store)
            .field("config_document", &self.config_document)
            .field("dispatcher", &self.dispatcher)
            .field("sessions", &self.sessions)
            .field("active_session_id", &self.active_session_id)
            .field("selected_saved_session_id", &self.selected_saved_session_id)
            .field("recent_session_ids", &self.recent_session_ids)
            .field("session_search_query", &self.session_search_query)
            .field("sftp_visible", &self.sftp_visible)
            .field("tunnels_visible", &self.tunnels_visible)
            .field("commands_visible", &self.commands_visible)
            .field("secret_store_path", &self.secret_store_path)
            .field("secret_reset_confirmation", &self.secret_reset_confirmation)
            .field("editor", &self.editor)
            .field("status_text", &self.status_text)
            .field(
                "settings_scrollback_lines_text",
                &self.settings_scrollback_lines_text,
            )
            .field(
                "settings_scrollback_max_cells_text",
                &self.settings_scrollback_max_cells_text,
            )
            .field("settings_terminal_status", &self.settings_terminal_status)
            .field("terminal_clipboard", &self.terminal_clipboard)
            .field("terminal_search_query", &self.terminal_search_query)
            .field("terminal_search_matches", &self.terminal_search_matches)
            .field(
                "terminal_search_current_index",
                &self.terminal_search_current_index,
            )
            .field("sftp_path", &self.sftp_path)
            .field("sftp_listing", &self.sftp_listing)
            .field("sftp_local_path", &self.sftp_local_path)
            .field("sftp_remote_target", &self.sftp_remote_target)
            .field("sftp_secondary_target", &self.sftp_secondary_target)
            .field("sftp_permissions", &self.sftp_permissions)
            .field("sftp_entries", &self.sftp_entries)
            .field("sftp_selected_index", &self.sftp_selected_index)
            .field("sftp_sort_column", &self.sftp_sort_column)
            .field("sftp_sort_ascending", &self.sftp_sort_ascending)
            .field("sftp_show_hidden", &self.sftp_show_hidden)
            .field("sftp_session", &self.sftp_session)
            .field("remote_edit_session", &self.remote_edit_session)
            .field("transfer_queue", &self.transfer_queue)
            .field("next_transfer_ordinal", &self.next_transfer_ordinal)
            .field("transport_backend", &self.transport_backend)
            .field("host_key_policy_override", &self.host_key_policy_override)
            .field("recovered_from_backup", &self.recovered_from_backup)
            .field("next_runtime_ordinal", &self.next_runtime_ordinal)
            .field("keychain", &self.keychain)
            .field("logging_dialog", &self.logging_dialog)
            .field("last_logging_directory", &self.last_logging_directory)
            .field("quick_connect_input", &self.quick_connect_input)
            .field("quick_connect_error_text", &self.quick_connect_error_text)
            .field("quick_connect_last_target", &self.quick_connect_last_target)
            .field(
                "pending_quick_connect_targets",
                &self.pending_quick_connect_targets,
            )
            .field("recent_saved_usage", &self.recent_saved_usage)
            .field(
                "private_keys_modal_visible",
                &self.private_keys_modal_visible,
            )
            .field("private_keys_selected_id", &self.private_keys_selected_id)
            .field("host_keys_modal_visible", &self.host_keys_modal_visible)
            .field("pending_auth_prompt", &self.pending_auth_prompt)
            .finish()
    }
}

pub(crate) struct DefaultRuntimeKeychainSelection {
    pub(crate) keychain: Option<Arc<dyn Keychain>>,
    pub(crate) secret_store_path: Option<PathBuf>,
    pub(crate) kind: SecretStoreKind,
}

pub(crate) fn default_runtime_keychain(
    config_dir: &Path,
) -> AppResult<DefaultRuntimeKeychainSelection> {
    let path = config_dir.join("secret-store.toml");
    if let Ok(master_password) = env::var("YSHELL_MASTER_PASSWORD") {
        if master_password.trim().is_empty() {
            return Ok(DefaultRuntimeKeychainSelection {
                keychain: None,
                secret_store_path: Some(path),
                kind: SecretStoreKind::Disabled,
            });
        }
        let keychain = FileKeychain::open_or_create(path.clone(), master_password)
            .map_err(AppError::from_error)?;
        return Ok(DefaultRuntimeKeychainSelection {
            keychain: Some(Arc::new(keychain)),
            secret_store_path: Some(path),
            kind: SecretStoreKind::File,
        });
    }
    Ok(DefaultRuntimeKeychainSelection {
        keychain: Some(Arc::new(OsKeychain::new("yshell"))),
        secret_store_path: None,
        kind: SecretStoreKind::Os,
    })
}

impl AppRuntime {
    #[cfg(test)]
    pub fn new_with_keychain(
        config_dir: PathBuf,
        keychain: Option<Arc<dyn Keychain>>,
    ) -> AppResult<Self> {
        let kind = if keychain.is_some() {
            SecretStoreKind::Injected
        } else {
            SecretStoreKind::Disabled
        };
        Self::new_with_keychain_and_secret_store_path(config_dir, keychain, None, kind)
    }

    #[cfg(test)]
    pub fn emitted_events(&self) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        for session in self.sessions.values() {
            events.push(SessionEvent::StateChanged {
                session_id: session.session_id().clone(),
                state: session.state,
            });
        }
        events
    }
}
