//! Runtime composition layer that bridges config, core commands, and UI callbacks.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fmt,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use yshell_config::{
    parse_quick_connect, AuthMethod as ConfigAuthMethod, ConfigDocument, ConfigStore,
    FolderProfile, HostKeyPolicy as ConfigHostKeyPolicy, LoadOutcome, LoggingProfile, ProxyProfile,
    ProxyProtocol, ResolvedSessionProfile, SessionProfile, TerminalProfile, TunnelForward,
    TunnelForwardKind, TunnelProfile,
};
use yshell_core::{
    CommandDispatcher, CoreCommandDispatcher, SessionCommand, SessionEvent, SessionState,
};
use yshell_secret::{FileKeychain, Keychain, OsKeychain, SecretRef};
use yshell_sftp::{
    prepare_remote_edit_session, DirectoryListing, FsEntry, FsEntryKind, RemoteEditSession,
    SftpClient, TransferDirection, TransferQueue, TransferStatus, TransferTask,
};
use yshell_ssh::{
    AuthMethod, ForwardingKind, HostKeyFingerprint, HostKeyPolicy, HostKeyProblem, KnownHosts,
    ProxyConfig, ShellClient, SshConnectionConfig, TransportBackend, TunnelConfig,
};
use yshell_terminal::{
    GridPoint, SearchMatch, TerminalSnapshot, DEFAULT_SCROLLBACK_LINES,
    DEFAULT_SCROLLBACK_MAX_CELLS,
};

use crate::{
    error::{AppError, AppResult},
    session_runtime::{SessionRuntime, SessionSource, TerminalViewportMetrics},
    sftp_view::{
        format_sftp_modified, format_sftp_permissions, format_sftp_permissions_octal,
        format_sftp_size, sftp_kind_text, visible_entries, SftpSortColumn,
    },
};

/// One visible SFTP file-list row, ready for the Slint model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpRowData {
    pub name: String,
    pub kind_text: String,
    pub size_text: String,
    pub modified_text: String,
    pub permissions_text: String,
    pub is_dir: bool,
}

/// One clickable breadcrumb of the current remote path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpCrumbData {
    pub label: String,
    pub path: String,
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

/// Why the SFTP lifecycle is unavailable (enum id; Slint renders the sentence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SftpUnavailableReason {
    SelectNativeSsh,
    FakeBackendSelected,
    DesktopStartupFake,
}

impl SftpUnavailableReason {
    const fn id(self) -> &'static str {
        match self {
            Self::SelectNativeSsh => "select-native-ssh",
            Self::FakeBackendSelected => "fake-backend",
            Self::DesktopStartupFake => "desktop-startup-fake",
        }
    }

    const fn legacy_text(self) -> &'static str {
        match self {
            Self::SelectNativeSsh => "select the native-ssh backend before using SFTP",
            Self::FakeBackendSelected => "fake transport backend is selected",
            Self::DesktopStartupFake => "desktop startup uses the fake backend",
        }
    }
}

/// Value-only SFTP listing placeholder state (the row listing itself is data).
#[derive(Debug, Clone, PartialEq, Eq)]
enum SftpListingState {
    ConnectReal,
    DraftDisconnected,
    RefreshNeedsSession,
    DesktopStartup,
    FakeBackendSelected,
    NativeSshSelected,
    EmptyDirectory,
    Rows(String),
    ReconnectFailed,
    SyncFakeBackend,
    SyncMissingSession,
    SyncDisconnected,
    SyncReconnectNative,
    SyncFailed {
        session_key: String,
        path: String,
        error: String,
    },
}

impl SftpListingState {
    const fn kind_id(&self) -> &'static str {
        match self {
            Self::ConnectReal => "connect-real",
            Self::DraftDisconnected => "draft-disconnected",
            Self::RefreshNeedsSession => "refresh-needs-session",
            Self::DesktopStartup => "desktop-startup",
            Self::FakeBackendSelected => "fake-backend-selected",
            Self::NativeSshSelected => "native-ssh-selected",
            Self::EmptyDirectory => "empty-directory",
            Self::Rows(_) => "rows",
            Self::ReconnectFailed => "reconnect-failed",
            Self::SyncFakeBackend => "sync-fake-backend",
            Self::SyncMissingSession => "sync-missing-session",
            Self::SyncDisconnected => "sync-disconnected",
            Self::SyncReconnectNative => "sync-reconnect-native",
            Self::SyncFailed { .. } => "sync-failed",
        }
    }

    fn rows(&self) -> &str {
        match self {
            Self::Rows(rows) => rows,
            _ => "",
        }
    }

    fn session_key(&self) -> &str {
        match self {
            Self::SyncFailed { session_key, .. } => session_key,
            _ => "",
        }
    }

    fn path(&self) -> &str {
        match self {
            Self::SyncFailed { path, .. } => path,
            _ => "",
        }
    }

    fn error(&self) -> &str {
        match self {
            Self::SyncFailed { error, .. } => error,
            _ => "",
        }
    }
}

/// Settings-dialog field ids for terminal validation messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTerminalField {
    ScrollbackLines,
    ScrollbackMaxCells,
}

impl SettingsTerminalField {
    const fn id(self) -> &'static str {
        match self {
            Self::ScrollbackLines => "scrollback-lines",
            Self::ScrollbackMaxCells => "scrollback-max-cells",
        }
    }

    const fn legacy_label(self) -> &'static str {
        match self {
            Self::ScrollbackLines => "Scrollback lines",
            Self::ScrollbackMaxCells => "Scrollback memory cap",
        }
    }
}

/// Value-only status shown under the terminal settings form.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SettingsTerminalStatus {
    Hint,
    DefaultsLoaded,
    Empty {
        field: SettingsTerminalField,
    },
    NotNumber {
        field: SettingsTerminalField,
    },
    OutOfRange {
        field: SettingsTerminalField,
        minimum: usize,
        maximum: usize,
    },
    Saved {
        lines: usize,
        max_cells: usize,
    },
}

impl SettingsTerminalStatus {
    const fn kind_id(&self) -> &'static str {
        match self {
            Self::Hint => "hint",
            Self::DefaultsLoaded => "defaults-loaded",
            Self::Empty { .. } => "empty",
            Self::NotNumber { .. } => "not-number",
            Self::OutOfRange { .. } => "out-of-range",
            Self::Saved { .. } => "saved",
        }
    }

    fn field_id(&self) -> &'static str {
        match self {
            Self::Empty { field } | Self::NotNumber { field } | Self::OutOfRange { field, .. } => {
                field.id()
            }
            Self::Hint | Self::DefaultsLoaded | Self::Saved { .. } => "",
        }
    }

    fn value_text(&self) -> String {
        match self {
            Self::OutOfRange { minimum, .. } => minimum.to_string(),
            Self::Saved { lines, .. } => lines.to_string(),
            Self::Hint | Self::DefaultsLoaded | Self::Empty { .. } | Self::NotNumber { .. } => {
                String::new()
            }
        }
    }

    fn limit_text(&self) -> String {
        match self {
            Self::OutOfRange { maximum, .. } => maximum.to_string(),
            Self::Saved { max_cells, .. } => max_cells.to_string(),
            Self::Hint | Self::DefaultsLoaded | Self::Empty { .. } | Self::NotNumber { .. } => {
                String::new()
            }
        }
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    fn legacy_text(&self) -> String {
        match self {
            Self::Hint => {
                "Save terminal settings to apply the scrollback limits to existing sessions."
                    .to_owned()
            }
            Self::DefaultsLoaded => "Defaults loaded. Click Save to apply.".to_owned(),
            Self::Empty { field } => format!("{} must not be empty.", field.legacy_label()),
            Self::NotNumber { field } => {
                format!("{} must be a whole number.", field.legacy_label())
            }
            Self::OutOfRange {
                field,
                minimum,
                maximum,
            } => format!(
                "{} must be between {minimum} and {maximum}.",
                field.legacy_label()
            ),
            Self::Saved { lines, max_cells } => {
                format!("Saved terminal settings: scrollback={lines} lines, cap={max_cells} cells.")
            }
        }
    }
}

/// Value-only auth-test result for the session editor.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EditorAuthTestStatus {
    /// Nothing tested yet (fresh editor).
    Hint,
    /// A saved session was loaded into the editor.
    LoadedHint,
    Success {
        host_label: String,
        backend: String,
        startup_snippet: String,
    },
    Failed {
        error: String,
    },
}

impl EditorAuthTestStatus {
    const fn kind_id(&self) -> &'static str {
        match self {
            Self::Hint => "hint",
            Self::LoadedHint => "loaded",
            Self::Success { .. } => "success",
            Self::Failed { .. } => "failed",
        }
    }

    fn host_label(&self) -> &str {
        match self {
            Self::Success { host_label, .. } => host_label,
            _ => "",
        }
    }

    fn backend(&self) -> &str {
        match self {
            Self::Success { backend, .. } => backend,
            _ => "",
        }
    }

    fn startup_snippet(&self) -> &str {
        match self {
            Self::Success {
                startup_snippet, ..
            } => startup_snippet,
            _ => "",
        }
    }

    fn error(&self) -> &str {
        match self {
            Self::Failed { error } => error,
            _ => "",
        }
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    fn legacy_text(&self) -> String {
        match self {
            Self::Hint => {
                "Run Auth Test to validate the current editor fields without saving.".to_owned()
            }
            Self::LoadedHint => {
                "Loaded saved session into the editor. Run Auth Test before saving if you want to validate the current form."
                    .to_owned()
            }
            Self::Success {
                host_label,
                backend,
                startup_snippet,
            } => {
                if startup_snippet.is_empty() {
                    format!("Auth test succeeded for {host_label} using the `{backend}` backend.")
                } else {
                    format!(
                        "Auth test succeeded for {host_label} using the `{backend}` backend. Startup: {startup_snippet}"
                    )
                }
            }
            Self::Failed { error } => format!("Auth test failed: {error}"),
        }
    }
}

/// Value-only projection for the window. Natural-language templates live in
/// `ui/main_window.slint` (`@tr`), so every field here is either a raw value
/// (host/path/count/enum id) or machine data assembled from values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppProjection {
    pub config_dir_text: String,
    pub secret_store_kind_text: String,
    pub secret_store_path_text: String,
    pub secret_reset_confirmation_text: String,
    pub editor_folder_label_text: String,
    pub editor_folder_id_text: String,
    pub editor_folder_known: bool,
    pub new_folder_name_text: String,
    pub editor_name_text: String,
    pub editor_host_text: String,
    pub editor_port_text: String,
    pub editor_username_text: String,
    pub editor_auth_method_text: String,
    pub editor_host_key_policy_text: String,
    pub editor_password_text: String,
    pub editor_key_path_text: String,
    pub editor_passphrase_text: String,
    pub editor_target_session_id_text: String,
    pub editor_target_editing: bool,
    pub editor_auth_test_kind_text: String,
    pub editor_auth_test_host_text: String,
    pub editor_auth_test_backend_text: String,
    pub editor_auth_test_startup_text: String,
    pub editor_auth_test_error_text: String,
    pub editor_proxy_mode_text: String,
    pub editor_proxy_protocol_text: String,
    pub editor_proxy_host_text: String,
    pub editor_proxy_port_text: String,
    pub editor_proxy_username_text: String,
    pub editor_proxy_password_text: String,
    pub editor_proxy_dns_by_proxy: bool,
    pub editor_proxy_summary_kind_text: String,
    pub editor_proxy_summary_address_text: String,
    pub editor_proxy_summary_user_text: String,
    pub editor_tunnel_kind_text: String,
    pub editor_tunnel_bind_host_text: String,
    pub editor_tunnel_bind_port_text: String,
    pub editor_tunnel_target_host_text: String,
    pub editor_tunnel_target_port_text: String,
    pub editor_tunnel_summary_kind_text: String,
    pub editor_tunnel_summary_rows_text: String,
    pub editor_tunnel_summary_count: i32,
    pub logging_enabled_text: String,
    pub logging_format_text: String,
    pub logging_directory_text: String,
    pub logging_directory_display_text: String,
    pub settings_scrollback_lines_text: String,
    pub settings_scrollback_max_cells_text: String,
    pub settings_terminal_status_kind_text: String,
    pub settings_terminal_status_field_text: String,
    pub settings_terminal_status_value_text: String,
    pub settings_terminal_status_limit_text: String,
    pub editor_modal_visible: bool,
    pub editor_section_text: String,
    pub known_hosts_modal_visible: bool,
    pub known_hosts_inventory_rows_text: String,
    pub known_hosts_inventory_empty: bool,
    pub known_hosts_selection_kind_text: String,
    pub known_hosts_selection_host_text: String,
    pub known_hosts_selection_port_text: String,
    pub known_hosts_selection_index: i32,
    pub known_hosts_selection_total: i32,
    pub known_hosts_details_text: String,
    pub known_hosts_path_text: String,
    pub known_hosts_clear_confirmation_text: String,
    pub active_session_kind_text: String,
    pub active_session_name_text: String,
    pub active_session_state_text: String,
    /// W5-A3：当前是否存在活动运行时会话（菜单/按钮状态用，纯布尔量）。
    pub has_active_session: bool,
    /// W5-A3：活动运行时会话是否处于 `connected` 状态。
    pub active_session_connected: bool,
    /// W5-A3：是否有选中的已保存会话（Edit/Update/Delete 菜单项状态）。
    pub has_saved_selection: bool,
    pub saved_session_count: i32,
    pub session_summary_rows_text: String,
    pub session_summary_count: i32,
    pub session_summary_hidden_count: i32,
    pub session_search_text: String,
    pub saved_session_selection_kind_text: String,
    pub saved_session_selection_name_text: String,
    pub saved_session_selection_host_text: String,
    pub saved_session_selection_id_text: String,
    pub saved_session_inventory_rows_text: String,
    pub saved_session_inventory_query_text: String,
    pub saved_session_inventory_empty: bool,
    /// Flattened sidebar session-tree rows in display order (the legacy text
    /// projections above are kept for Rust bindings but no longer rendered).
    pub session_tree_rows: Vec<SessionTreeRow>,
    pub recent_sessions_rows_text: String,
    pub recent_sessions_empty: bool,
    pub host_key_prompt_visible: bool,
    pub host_key_prompt_text: String,
    pub host_key_prompt_mode_text: String,
    pub host_key_prompt_confirmation_text: String,
    /// W5：连接需要密码而密钥库无法提供时挂起输入（仅本次连接使用，不落盘）。
    pub password_prompt_visible: bool,
    /// 密码弹窗文案里的 `user@host:port`（由挂起目标组装）。
    pub password_prompt_host_text: String,
    pub tab_name_text: String,
    pub tab_state_text: String,
    pub tab_has_session: bool,
    pub terminal_title_name_text: String,
    pub terminal_title_has_session: bool,
    pub terminal_body_text: String,
    pub terminal_body_kind_text: String,
    pub terminal_visible_lines: Vec<String>,
    pub terminal_cursor_column: i32,
    pub terminal_cursor_row: i32,
    pub terminal_frame_id: u64,
    pub terminal_scroll_offset: u64,
    /// 终端缓冲区总行数（scrollback 行数 + 当前视口行数）：滚动条几何的唯一总量口径。
    pub terminal_scrollback_lines: i32,
    /// 当前视口行数（取自 `active_terminal_viewport_metrics`）：滚动条滑块比例与可见性用。
    pub terminal_viewport_rows: i32,
    pub terminal_selection_active: bool,
    /// W5-A3：活动终端是否存在可复制选区（Edit/Copy 菜单项状态，与
    /// `terminal_selection_active` 同源；后者继续供终端视图渲染选区浮层）。
    pub terminal_has_selection: bool,
    pub terminal_search_query_text: String,
    pub terminal_search_kind_text: String,
    pub terminal_search_match_count: i32,
    pub terminal_search_current_index: i32,
    pub sftp_path_text: String,
    pub sftp_listing_kind_text: String,
    pub sftp_listing_rows_text: String,
    pub sftp_listing_session_key_text: String,
    pub sftp_listing_path_text: String,
    pub sftp_listing_error_text: String,
    pub sftp_local_path_text: String,
    pub sftp_remote_target_text: String,
    pub sftp_secondary_target_text: String,
    pub sftp_permissions_text: String,
    pub sftp_session_status_kind_text: String,
    pub sftp_session_status_reason_text: String,
    pub sftp_session_status_detail_text: String,
    pub sftp_session_status_session_key_text: String,
    pub sftp_remote_edit_remote_path_text: String,
    pub sftp_remote_edit_local_path_text: String,
    pub sftp_rows: Vec<SftpRowData>,
    pub sftp_crumbs: Vec<SftpCrumbData>,
    pub sftp_selected_index: i32,
    pub sftp_sort_column_text: String,
    pub sftp_sort_ascending: bool,
    pub sftp_show_hidden: bool,
    pub sftp_item_summary_text: String,
    /// SFTP 汇总行计数（Slint 侧用 @tr 复数模板组装句子）。
    pub sftp_item_count: i32,
    pub sftp_dir_count: i32,
    pub sftp_empty_text: String,
    pub sftp_selected_name_text: String,
    pub sftp_selected_path_text: String,
    pub sftp_selected_permissions_text: String,
    pub sftp_remote_edit_active: bool,
    /// W5-A3：SFTP 生命周期是否就绪（`ready`）；SFTP 操作菜单项据此禁用。
    pub sftp_available: bool,
    pub transfer_queue_rows_text: String,
    pub transfer_queue_empty: bool,
    pub tunnels_summary_kind_text: String,
    pub tunnels_summary_rows_text: String,
    pub status_text: String,
    /// 状态栏 i18n 模板：覆盖的生产者填 kind + 参数，其余路径保持空 kind，
    /// Slint 端回退英文 `status_text` 原文。
    pub status_kind: String,
    pub status_param_1: String,
    pub status_param_2: String,
    pub transport_backend_text: String,
    pub sftp_visible: bool,
    pub tunnels_visible: bool,
    pub commands_visible: bool,
    pub app_version_text: String,
}

pub struct AppRuntime {
    config_dir: PathBuf,
    config_store: ConfigStore,
    config_document: ConfigDocument,
    dispatcher: CoreCommandDispatcher,
    sessions: BTreeMap<String, SessionRuntime>,
    active_session_id: Option<String>,
    selected_saved_session_id: Option<String>,
    /// Selected folder in the sidebar tree (folder ids are not profile ids, so
    /// the tree keeps a separate selection slot). Selecting a folder clears the
    /// session selection and vice versa.
    selected_saved_folder_id: Option<String>,
    /// Collapsed folders in the sidebar tree (absence = expanded, the default).
    collapsed_saved_folders: BTreeSet<String>,
    recent_session_ids: Vec<String>,
    session_search_query: String,
    sftp_visible: bool,
    tunnels_visible: bool,
    commands_visible: bool,
    secret_store_path: Option<PathBuf>,
    secret_store_kind: SecretStoreKind,
    secret_reset_confirmation: String,
    editor: SessionEditorDraft,
    new_folder_name: String,
    editor_auth_test_status: EditorAuthTestStatus,
    status_text: String,
    /// `status_text` 的 i18n 模板 id 与参数（仅覆盖的生产者设置）。
    status_kind: String,
    status_param_1: String,
    status_param_2: String,
    /// 设置 kind 时的 `status_text` 快照：文本被其它路径改写后 kind 失效。
    status_kind_source: String,
    settings_scrollback_lines_text: String,
    settings_scrollback_max_cells_text: String,
    settings_terminal_status: SettingsTerminalStatus,
    terminal_clipboard: String,
    terminal_search_query: String,
    terminal_search_matches: Vec<SearchMatch>,
    terminal_search_current_index: Option<usize>,
    sftp_path: String,
    sftp_listing: SftpListingState,
    sftp_local_path: String,
    sftp_remote_target: String,
    sftp_secondary_target: String,
    sftp_permissions: String,
    sftp_entries: Vec<FsEntry>,
    sftp_selected_index: Option<usize>,
    sftp_sort_column: SftpSortColumn,
    sftp_sort_ascending: bool,
    sftp_show_hidden: bool,
    sftp_session: SftpSessionLifecycle,
    remote_edit_session: Option<RemoteEditSession>,
    transfer_queue: TransferQueue,
    next_transfer_ordinal: usize,
    persistent_known_hosts: KnownHosts,
    temporary_known_hosts: KnownHosts,
    known_hosts_modal_visible: bool,
    known_hosts_selected_key: Option<String>,
    known_hosts_clear_confirmation: String,
    pending_host_key_prompt: Option<PendingHostKeyPrompt>,
    host_key_replace_confirmation: String,
    /// W5：密码型认证缺少密钥时的挂起目标（仅内存；重试连接用）。
    pending_password_prompt: Option<PendingPasswordPrompt>,
    transport_backend: TransportBackend,
    editor_modal_visible: bool,
    editor_section: EditorSection,
    host_key_policy_override: Option<HostKeyPolicy>,
    recovered_from_backup: Option<PathBuf>,
    next_runtime_ordinal: usize,
    keychain: Option<RuntimeKeychain>,
}

#[derive(Clone)]
struct RuntimeKeychain(Arc<dyn Keychain>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecretStoreKind {
    Disabled,
    File,
    Os,
    #[cfg(test)]
    Injected,
}

impl SecretStoreKind {
    const fn id(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::File => "file",
            Self::Os => "os",
            #[cfg(test)]
            Self::Injected => "injected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SftpSessionLifecycle {
    Unavailable {
        reason: SftpUnavailableReason,
    },
    Disconnected {
        session_key: Option<String>,
    },
    Ready {
        session_key: String,
    },
    Failed {
        session_key: Option<String>,
        reason: String,
    },
}

impl SftpSessionLifecycle {
    const fn status_kind(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "unavailable",
            Self::Disconnected { .. } => "disconnected",
            Self::Ready { .. } => "ready",
            Self::Failed { .. } => "failed",
        }
    }

    fn status_reason(&self) -> &'static str {
        match self {
            Self::Unavailable { reason } => reason.id(),
            Self::Disconnected { .. } | Self::Ready { .. } | Self::Failed { .. } => "",
        }
    }

    fn status_detail(&self) -> &str {
        match self {
            Self::Failed { reason, .. } => reason,
            Self::Unavailable { .. } | Self::Disconnected { .. } | Self::Ready { .. } => "",
        }
    }

    fn status_session_key(&self) -> &str {
        match self {
            Self::Disconnected {
                session_key: Some(session_key),
            }
            | Self::Ready { session_key } => session_key,
            Self::Failed {
                session_key: Some(session_key),
                ..
            } => session_key,
            Self::Unavailable { .. }
            | Self::Disconnected { session_key: None }
            | Self::Failed {
                session_key: None, ..
            } => "",
        }
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    fn legacy_status_text(&self) -> String {
        match self {
            Self::Unavailable { reason } => {
                format!("SFTP unavailable: {}", reason.legacy_text())
            }
            Self::Disconnected { session_key } => match session_key {
                Some(session_key) => {
                    format!("SFTP session disconnected for runtime session `{session_key}`.")
                }
                None => "SFTP session disconnected. Connect a runtime session first.".to_owned(),
            },
            Self::Ready { session_key } => {
                format!("SFTP session ready for runtime session `{session_key}`.")
            }
            Self::Failed {
                session_key,
                reason,
            } => match session_key {
                Some(session_key) => {
                    format!("SFTP session failed for runtime session `{session_key}`: {reason}")
                }
                None => format!("SFTP session failed: {reason}"),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorAuthMethod {
    Agent,
    Password,
    KeyboardInteractive,
    PrivateKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorProxyMode {
    None,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorSection {
    General,
    Authentication,
    Terminal,
    Sftp,
    Tunnels,
    Proxy,
    Logging,
    Advanced,
    Appearance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KnownHostEntryInfo {
    key: String,
    host: String,
    port: u16,
    algorithm: String,
    fingerprint: String,
}

impl EditorProxyMode {
    const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Custom => "custom",
        }
    }
}

impl EditorSection {
    const fn label(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Authentication => "authentication",
            Self::Terminal => "terminal",
            Self::Sftp => "sftp",
            Self::Tunnels => "tunnels",
            Self::Proxy => "proxy",
            Self::Logging => "logging",
            Self::Advanced => "advanced",
            Self::Appearance => "appearance",
        }
    }
}

impl EditorAuthMethod {
    const fn label(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Password => "password",
            Self::KeyboardInteractive => "keyboard_interactive",
            Self::PrivateKey => "private_key",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionEditorDraft {
    target_session_id: Option<String>,
    target_folder_id: String,
    name: String,
    host: String,
    port_text: String,
    username: String,
    auth_method: EditorAuthMethod,
    host_key_policy: ConfigHostKeyPolicy,
    password: String,
    key_path: String,
    passphrase: String,
    proxy_mode: EditorProxyMode,
    proxy_protocol: ProxyProtocol,
    proxy_host: String,
    proxy_port_text: String,
    proxy_username: String,
    proxy_password: String,
    proxy_dns_by_proxy: bool,
    tunnel_kind: TunnelForwardKind,
    tunnel_bind_host: String,
    tunnel_bind_port_text: String,
    tunnel_target_host: String,
    tunnel_target_port_text: String,
    tunnels: Vec<TunnelForward>,
}

impl Default for SessionEditorDraft {
    fn default() -> Self {
        Self {
            target_session_id: None,
            target_folder_id: SAVED_SESSIONS_FOLDER_ID.to_owned(),
            name: String::new(),
            host: String::new(),
            port_text: "22".to_owned(),
            username: String::new(),
            auth_method: EditorAuthMethod::Agent,
            host_key_policy: ConfigHostKeyPolicy::Strict,
            password: String::new(),
            key_path: String::new(),
            passphrase: String::new(),
            proxy_mode: EditorProxyMode::None,
            proxy_protocol: ProxyProtocol::Socks5,
            proxy_host: String::new(),
            proxy_port_text: String::new(),
            proxy_username: String::new(),
            proxy_password: String::new(),
            proxy_dns_by_proxy: true,
            tunnel_kind: TunnelForwardKind::Local,
            tunnel_bind_host: "127.0.0.1".to_owned(),
            tunnel_bind_port_text: String::new(),
            tunnel_target_host: String::new(),
            tunnel_target_port_text: String::new(),
            tunnels: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditorFolderChoice {
    id: String,
    path_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingHostKeyPrompt {
    session_key: String,
    host: String,
    port: u16,
    username: String,
    presented: HostKeyFingerprint,
    expected: Option<HostKeyFingerprint>,
    known_hosts_path: PathBuf,
}

/// W5：密码弹窗挂起的目标（仅内存，重试连接时据此重建运行时会话）。
///
/// 保存的是"最小上下文"：已保存会话 id + host/port/user + 认证方式；密码本身
/// 只在 `submit_password` 的一次调用里流转，不进入任何持久化结构。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingPasswordPrompt {
    profile_id: String,
    host: String,
    port: u16,
    username: String,
    auth_method: PendingPasswordAuthMethod,
}

impl PendingPasswordPrompt {
    /// 弹窗文案里的 `user@host:port`（Slint 侧套 `@tr("Enter the password for {0} ...")`）。
    fn host_text(&self) -> String {
        format!("{}@{}:{}", self.username, self.host, self.port)
    }
}

/// 需要用户输入密码型密钥的认证方式（决定重试时构造哪种 `AuthMethod`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingPasswordAuthMethod {
    Password,
    KeyboardInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostKeyPromptMode {
    FirstTrust,
    Changed,
}

impl PendingHostKeyPrompt {
    fn mode(&self) -> HostKeyPromptMode {
        if self.expected.is_some() {
            HostKeyPromptMode::Changed
        } else {
            HostKeyPromptMode::FirstTrust
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
            .finish()
    }
}

const SAVED_SESSIONS_FOLDER_ID: &str = "saved-sessions";
const SAVED_SESSIONS_FOLDER_NAME: &str = "Saved Sessions";
const SETTINGS_SCROLLBACK_LINES_MIN: usize = 100;
const SETTINGS_SCROLLBACK_LINES_MAX: usize = 1_000_000;
const SETTINGS_SCROLLBACK_MAX_CELLS_MIN: usize = 100_000;
const SETTINGS_SCROLLBACK_MAX_CELLS_MAX: usize = 100_000_000;

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

    fn new_with_keychain_and_secret_store_path(
        config_dir: PathBuf,
        keychain: Option<Arc<dyn Keychain>>,
        secret_store_path: Option<PathBuf>,
        secret_store_kind: SecretStoreKind,
    ) -> AppResult<Self> {
        let config_store = ConfigStore::new(config_dir.clone());
        let LoadOutcome {
            document,
            recovered_from_backup,
        } = config_store.load_or_recover().map_err(AppError::from_error)?;
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
        };
        runtime.status_text = runtime.startup_status();
        runtime.hydrate_saved_sessions();
        Ok(runtime)
    }

    pub fn projection(&self) -> AppProjection {
        let (editor_folder_label_text, editor_folder_id_text, editor_folder_known) =
            self.editor_folder_parts();
        let (editor_target_editing, editor_target_session_id_text) = self.editor_target_parts();
        let (proxy_summary_kind_text, proxy_summary_address_text, proxy_summary_user_text) =
            self.editor_proxy_summary_parts();
        let (tunnels_summary_kind_text, tunnels_summary_rows_text) = self.tunnels_summary_parts();
        let (session_summary_rows_text, session_summary_hidden_count) =
            self.session_summary_parts();
        let (selection_kind_text, selection_name_text, selection_host_text, selection_id_text) =
            self.saved_session_selection_parts();
        let (inventory_rows_text, inventory_empty) = self.saved_session_inventory_parts();
        let session_tree_rows = self.session_tree_rows();
        let (recent_rows_text, recent_empty) = self.recent_sessions_parts();
        let (active_kind_text, active_name_text, active_state_text) = self.active_session_parts();
        let (tab_has_session, tab_name_text, tab_state_text) = self.tab_parts();
        let (terminal_title_has_session, terminal_title_name_text) = self.terminal_title_parts();
        let (terminal_body_kind_text, terminal_body_text) = self.terminal_body_parts();
        let (terminal_search_kind_text, terminal_search_match_count, terminal_search_current_index) =
            self.terminal_search_summary_parts();
        let (sftp_remote_edit_remote_path_text, sftp_remote_edit_local_path_text) =
            self.sftp_remote_edit_parts();
        let (transfer_queue_rows_text, transfer_queue_empty) = self.transfer_queue_parts();
        let (sftp_item_count, sftp_dir_count) = self.sftp_visible_counts();
        let (status_kind, status_param_1, status_param_2) = self.status_i18n_parts();
        let (terminal_scrollback_lines, terminal_viewport_rows) =
            self.active_terminal_scroll_geometry();

        AppProjection {
            config_dir_text: self.config_dir.display().to_string(),
            secret_store_kind_text: self.secret_store_kind.id().to_owned(),
            secret_store_path_text: self
                .secret_store_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            secret_reset_confirmation_text: self.secret_reset_confirmation.clone(),
            editor_folder_label_text,
            editor_folder_id_text,
            editor_folder_known,
            new_folder_name_text: self.new_folder_name.clone(),
            editor_name_text: self.editor.name.clone(),
            editor_host_text: self.editor.host.clone(),
            editor_port_text: self.editor.port_text.clone(),
            editor_username_text: self.editor.username.clone(),
            editor_auth_method_text: self.editor.auth_method.label().to_owned(),
            editor_host_key_policy_text: config_host_key_policy_label(self.editor.host_key_policy)
                .to_owned(),
            editor_password_text: self.editor.password.clone(),
            editor_key_path_text: self.editor.key_path.clone(),
            editor_passphrase_text: self.editor.passphrase.clone(),
            editor_target_session_id_text,
            editor_target_editing,
            editor_auth_test_kind_text: self.editor_auth_test_status.kind_id().to_owned(),
            editor_auth_test_host_text: self.editor_auth_test_status.host_label().to_owned(),
            editor_auth_test_backend_text: self.editor_auth_test_status.backend().to_owned(),
            editor_auth_test_startup_text: self
                .editor_auth_test_status
                .startup_snippet()
                .to_owned(),
            editor_auth_test_error_text: self.editor_auth_test_status.error().to_owned(),
            editor_proxy_mode_text: self.editor.proxy_mode.label().to_owned(),
            editor_proxy_protocol_text: proxy_protocol_label(self.editor.proxy_protocol).to_owned(),
            editor_proxy_host_text: self.editor.proxy_host.clone(),
            editor_proxy_port_text: self.editor.proxy_port_text.clone(),
            editor_proxy_username_text: self.editor.proxy_username.clone(),
            editor_proxy_password_text: self.editor.proxy_password.clone(),
            editor_proxy_dns_by_proxy: self.editor.proxy_dns_by_proxy,
            editor_proxy_summary_kind_text: proxy_summary_kind_text,
            editor_proxy_summary_address_text: proxy_summary_address_text,
            editor_proxy_summary_user_text: proxy_summary_user_text,
            editor_tunnel_kind_text: tunnel_kind_label(self.editor.tunnel_kind).to_owned(),
            editor_tunnel_bind_host_text: self.editor.tunnel_bind_host.clone(),
            editor_tunnel_bind_port_text: self.editor.tunnel_bind_port_text.clone(),
            editor_tunnel_target_host_text: self.editor.tunnel_target_host.clone(),
            editor_tunnel_target_port_text: self.editor.tunnel_target_port_text.clone(),
            editor_tunnel_summary_kind_text: if self.editor.tunnels.is_empty() {
                "none".to_owned()
            } else {
                "rows".to_owned()
            },
            editor_tunnel_summary_rows_text: self.editor_tunnel_summary_rows_text(),
            editor_tunnel_summary_count: i32::try_from(self.editor.tunnels.len())
                .unwrap_or(i32::MAX),
            logging_enabled_text: self.logging_enabled_text(),
            logging_format_text: self.logging_format_text(),
            logging_directory_text: self.logging_directory_text(),
            logging_directory_display_text: self.logging_directory_display_text(),
            settings_scrollback_lines_text: self.settings_scrollback_lines_text.clone(),
            settings_scrollback_max_cells_text: self.settings_scrollback_max_cells_text.clone(),
            settings_terminal_status_kind_text: self.settings_terminal_status.kind_id().to_owned(),
            settings_terminal_status_field_text: self
                .settings_terminal_status
                .field_id()
                .to_owned(),
            settings_terminal_status_value_text: self.settings_terminal_status.value_text(),
            settings_terminal_status_limit_text: self.settings_terminal_status.limit_text(),
            editor_modal_visible: self.editor_modal_visible,
            editor_section_text: self.editor_section.label().to_owned(),
            known_hosts_modal_visible: self.known_hosts_modal_visible,
            known_hosts_inventory_rows_text: self.known_hosts_inventory_rows_text(),
            known_hosts_inventory_empty: self.known_hosts_inventory_empty(),
            known_hosts_selection_kind_text: self.known_hosts_selection_kind().to_owned(),
            known_hosts_selection_host_text: self.known_hosts_selection_host_text(),
            known_hosts_selection_port_text: self.known_hosts_selection_port_text(),
            known_hosts_selection_index: self.known_hosts_selection_index(),
            known_hosts_selection_total: self.known_hosts_selection_total(),
            known_hosts_details_text: self.known_hosts_details_text(),
            known_hosts_path_text: self.config_store.known_hosts_file().display().to_string(),
            known_hosts_clear_confirmation_text: self.known_hosts_clear_confirmation.clone(),
            active_session_kind_text: active_kind_text.to_owned(),
            active_session_name_text: active_name_text,
            active_session_state_text: active_state_text,
            has_active_session: self.active_terminal_runtime().is_some(),
            active_session_connected: self
                .active_terminal_runtime()
                .is_some_and(|session| session.state == SessionState::Connected),
            has_saved_selection: self.selected_saved_session_id.is_some(),
            saved_session_count: i32::try_from(self.saved_session_count()).unwrap_or(i32::MAX),
            session_summary_rows_text,
            session_summary_count: i32::try_from(self.sessions.len()).unwrap_or(i32::MAX),
            session_summary_hidden_count: i32::try_from(session_summary_hidden_count)
                .unwrap_or(i32::MAX),
            session_search_text: self.session_search_query.clone(),
            saved_session_selection_kind_text: selection_kind_text,
            saved_session_selection_name_text: selection_name_text,
            saved_session_selection_host_text: selection_host_text,
            saved_session_selection_id_text: selection_id_text,
            saved_session_inventory_rows_text: inventory_rows_text,
            saved_session_inventory_query_text: self.session_search_query.trim().to_owned(),
            saved_session_inventory_empty: inventory_empty,
            session_tree_rows,
            recent_sessions_rows_text: recent_rows_text,
            recent_sessions_empty: recent_empty,
            host_key_prompt_visible: self.pending_host_key_prompt.is_some(),
            host_key_prompt_text: self.host_key_prompt_text(),
            host_key_prompt_mode_text: self.host_key_prompt_mode_text().to_owned(),
            host_key_prompt_confirmation_text: self.host_key_replace_confirmation.clone(),
            password_prompt_visible: self.pending_password_prompt.is_some(),
            password_prompt_host_text: self.password_prompt_host_text(),
            tab_name_text,
            tab_state_text,
            tab_has_session,
            terminal_title_name_text,
            terminal_title_has_session,
            terminal_body_text,
            terminal_body_kind_text: terminal_body_kind_text.to_owned(),
            terminal_visible_lines: self.terminal_visible_lines(),
            terminal_cursor_column: self.terminal_cursor_column(),
            terminal_cursor_row: self.terminal_cursor_row(),
            terminal_frame_id: self.active_terminal_frame_id(),
            terminal_scroll_offset: self.active_terminal_scroll_offset(),
            terminal_scrollback_lines: i32::try_from(terminal_scrollback_lines).unwrap_or(i32::MAX),
            terminal_viewport_rows: i32::try_from(terminal_viewport_rows).unwrap_or(i32::MAX),
            terminal_selection_active: self.active_terminal_selection_active(),
            terminal_has_selection: self.active_terminal_selection_active(),
            terminal_search_query_text: self.terminal_search_query.clone(),
            terminal_search_kind_text: terminal_search_kind_text.to_owned(),
            terminal_search_match_count,
            terminal_search_current_index,
            sftp_path_text: self.sftp_path.clone(),
            sftp_listing_kind_text: self.sftp_listing.kind_id().to_owned(),
            sftp_listing_rows_text: self.sftp_listing.rows().to_owned(),
            sftp_listing_session_key_text: self.sftp_listing.session_key().to_owned(),
            sftp_listing_path_text: self.sftp_listing.path().to_owned(),
            sftp_listing_error_text: self.sftp_listing.error().to_owned(),
            sftp_local_path_text: self.sftp_local_path.clone(),
            sftp_remote_target_text: self.sftp_remote_target.clone(),
            sftp_secondary_target_text: self.sftp_secondary_target.clone(),
            sftp_permissions_text: self.sftp_permissions.clone(),
            sftp_session_status_kind_text: self.sftp_session.status_kind().to_owned(),
            sftp_session_status_reason_text: self.sftp_session.status_reason().to_owned(),
            sftp_session_status_detail_text: self.sftp_session.status_detail().to_owned(),
            sftp_session_status_session_key_text: self.sftp_session.status_session_key().to_owned(),
            sftp_remote_edit_remote_path_text,
            sftp_remote_edit_local_path_text,
            sftp_rows: self.sftp_rows(),
            sftp_crumbs: self.sftp_crumbs(),
            sftp_selected_index: self
                .sftp_selected_index
                .map(|index| i32::try_from(index).unwrap_or(i32::MAX))
                .unwrap_or(-1),
            sftp_sort_column_text: self.sftp_sort_column.label().to_ascii_lowercase(),
            sftp_sort_ascending: self.sftp_sort_ascending,
            sftp_show_hidden: self.sftp_show_hidden,
            sftp_item_summary_text: self.sftp_item_summary_text(),
            sftp_item_count: i32::try_from(sftp_item_count).unwrap_or(i32::MAX),
            sftp_dir_count: i32::try_from(sftp_dir_count).unwrap_or(i32::MAX),
            sftp_empty_text: self.sftp_empty_text(),
            sftp_selected_name_text: self.sftp_selected_name_text(),
            sftp_selected_path_text: self.sftp_selected_path_text(),
            sftp_selected_permissions_text: self.sftp_selected_permissions_text(),
            sftp_remote_edit_active: self.remote_edit_session.is_some(),
            sftp_available: self.sftp_ready_session_key().is_some(),
            transfer_queue_rows_text,
            transfer_queue_empty,
            tunnels_summary_kind_text: tunnels_summary_kind_text.to_owned(),
            tunnels_summary_rows_text,
            status_text: self.status_text.clone(),
            status_kind,
            status_param_1,
            status_param_2,
            transport_backend_text: self.transport_backend.label().to_owned(),
            sftp_visible: self.sftp_visible,
            tunnels_visible: self.tunnels_visible,
            commands_visible: self.commands_visible,
            app_version_text: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    pub fn handle_quick_connect(&mut self, input: &str) -> AppResult<AppProjection> {
        let target = parse_quick_connect(input).map_err(AppError::from_error)?;
        let runtime = SessionRuntime::from_quick_connect(target, self.allocate_runtime_ordinal());
        self.activate_runtime_session(runtime)
    }

    pub fn handle_new_session(&mut self) -> AppProjection {
        let mut runtime = SessionRuntime::draft(self.allocate_runtime_ordinal());
        let session_id = runtime.session_id().as_str().to_owned();
        self.configure_runtime_logging(&mut runtime);
        self.configure_runtime_terminal_limits(&mut runtime);
        runtime.append_status_line("This draft is runtime-owned but not yet connected.");
        self.active_session_id = Some(session_id.clone());
        self.sftp_session = SftpSessionLifecycle::Disconnected {
            session_key: Some(session_id.clone()),
        };
        self.sftp_listing = SftpListingState::DraftDisconnected;
        self.status_text =
            "Created a runtime-backed session draft. Next step is wiring this draft into saved-session editing and real connect.".to_owned();
        if let Some(notice) = runtime.take_logging_notice() {
            self.fold_logging_notice_into_status(notice);
        }
        self.sessions.insert(session_id, runtime);
        self.projection()
    }

    pub fn select_fake_transport_backend(&mut self) -> AppProjection {
        self.select_transport_backend(TransportBackend::Fake)
    }

    pub fn select_native_ssh_transport_backend(&mut self) -> AppProjection {
        self.select_transport_backend(TransportBackend::Real)
    }

    pub fn prepare_desktop_startup_projection(&mut self) -> AppProjection {
        self.transport_backend = TransportBackend::Fake;
        self.sftp_listing = SftpListingState::DesktopStartup;
        self.sftp_session = SftpSessionLifecycle::Unavailable {
            reason: SftpUnavailableReason::DesktopStartupFake,
        };
        if self.status_text.is_empty() {
            self.status_text =
                "Desktop startup stays on `fake`. `native-ssh` now runs the embedded SSH shell and SFTP paths."
                    .to_owned();
        } else {
            self.status_text = format!(
                "{} Desktop startup stays on `fake`. `native-ssh` now runs the embedded SSH shell and SFTP paths.",
                self.status_text
            );
        }
        self.projection()
    }

    pub fn update_secret_reset_confirmation(&mut self, confirmation: &str) -> AppProjection {
        self.secret_reset_confirmation = confirmation.to_owned();
        self.projection()
    }

    pub fn update_host_key_replace_confirmation(&mut self, confirmation: &str) -> AppProjection {
        self.host_key_replace_confirmation = confirmation.to_owned();
        self.projection()
    }

    pub fn cancel_host_key_prompt(&mut self) -> AppProjection {
        self.pending_host_key_prompt = None;
        self.host_key_replace_confirmation.clear();
        self.status_text = "Canceled host key confirmation. Connection remains blocked.".to_owned();
        self.projection()
    }

    /// W5：用用户输入的密码重试刚才挂起的连接（已保存会话）。
    ///
    /// 密码只用于这一次连接尝试：不写密钥库、不改配置、不落盘。无论重试成败都会
    /// 清掉挂起状态（失败写入 `status_text`，投影里 `password_prompt_visible = false`），
    /// 需要时用户可以再次点"连接"重新弹窗。
    pub fn submit_password(&mut self, password: &str) -> AppResult<AppProjection> {
        let prompt = self
            .pending_password_prompt
            .clone()
            .ok_or_else(|| AppError::new("no password prompt is pending"))?;
        if password.trim().is_empty() {
            return Err(AppError::new("password must not be empty"));
        }
        let result = self.retry_pending_password_connection(&prompt, password);
        self.pending_password_prompt = None;
        match result {
            Ok(projection) => Ok(projection),
            Err(error) => {
                self.status_text = format!("Password connection failed: {error}");
                Ok(self.projection())
            }
        }
    }

    /// W5：取消密码弹窗，清掉挂起状态（不改动已有会话）。
    pub fn cancel_password_prompt(&mut self) -> AppProjection {
        self.pending_password_prompt = None;
        self.status_text =
            "Canceled the password prompt. The connection was not started.".to_owned();
        self.projection()
    }

    pub fn trust_host_key_once(&mut self) -> AppResult<AppProjection> {
        let prompt = self
            .pending_host_key_prompt
            .clone()
            .ok_or_else(|| AppError::new("no host key confirmation is pending"))?;
        if prompt.mode() != HostKeyPromptMode::FirstTrust {
            return Err(AppError::new(
                "trust once is only available for a first-seen host key",
            ));
        }
        self.temporary_known_hosts
            .pin(&prompt.host, prompt.port, prompt.presented.clone());
        self.pending_host_key_prompt = None;
        self.host_key_replace_confirmation.clear();
        self.status_text = format!(
            "Trusted the presented host key for {}:{} once. Reconnecting.",
            prompt.host, prompt.port
        );
        self.reconnect_session_by_key(&prompt.session_key)
    }

    pub fn trust_host_key_and_save(&mut self) -> AppResult<AppProjection> {
        let prompt = self
            .pending_host_key_prompt
            .clone()
            .ok_or_else(|| AppError::new("no host key confirmation is pending"))?;
        if prompt.mode() != HostKeyPromptMode::FirstTrust {
            return Err(AppError::new(
                "trust and save is only available for a first-seen host key",
            ));
        }
        self.persistent_known_hosts
            .pin(&prompt.host, prompt.port, prompt.presented.clone());
        self.config_store
            .save_known_hosts(&self.persistent_known_hosts)
            .map_err(AppError::from_error)?;
        self.pending_host_key_prompt = None;
        self.host_key_replace_confirmation.clear();
        self.status_text = format!(
            "Saved the presented host key for {}:{} to `{}` and reconnecting.",
            prompt.host,
            prompt.port,
            prompt.known_hosts_path.display()
        );
        self.reconnect_session_by_key(&prompt.session_key)
    }

    pub fn replace_host_key_and_connect(&mut self) -> AppResult<AppProjection> {
        let prompt = self
            .pending_host_key_prompt
            .clone()
            .ok_or_else(|| AppError::new("no host key confirmation is pending"))?;
        if prompt.mode() != HostKeyPromptMode::Changed {
            return Err(AppError::new(
                "replace key is only available for a changed host key",
            ));
        }
        if self.host_key_replace_confirmation.trim() != "REPLACE" {
            return Err(AppError::new(
                "type REPLACE before replacing the stored host key",
            ));
        }
        self.persistent_known_hosts
            .replace(&prompt.host, prompt.port, prompt.presented.clone());
        self.config_store
            .save_known_hosts(&self.persistent_known_hosts)
            .map_err(AppError::from_error)?;
        self.pending_host_key_prompt = None;
        self.host_key_replace_confirmation.clear();
        self.status_text = format!(
            "Replaced the stored host key for {}:{} in `{}` and reconnecting.",
            prompt.host,
            prompt.port,
            prompt.known_hosts_path.display()
        );
        self.reconnect_session_by_key(&prompt.session_key)
    }

    pub fn start_new_saved_session_editor(&mut self) -> AppProjection {
        self.editor = SessionEditorDraft::default();
        self.new_folder_name.clear();
        self.editor_modal_visible = true;
        self.editor_section = EditorSection::General;
        self.editor_auth_test_status = EditorAuthTestStatus::Hint;
        self.status_text =
            "Session editor cleared. Fill in the fields to create a saved session.".to_owned();
        self.projection()
    }

    pub fn load_selected_saved_session_into_editor(&mut self) -> AppResult<AppProjection> {
        let profile_id = self
            .selected_saved_session_id
            .clone()
            .ok_or_else(|| AppError::new("no saved session is currently selected"))?;
        let resolved = self
            .config_document
            .resolve_session(&profile_id)
            .ok_or_else(|| AppError::new(format!("saved session `{profile_id}` was not found")))?;
        self.editor = self.editor_from_resolved_session(&resolved);
        self.editor_modal_visible = true;
        self.editor_section = EditorSection::General;
        self.editor_auth_test_status = EditorAuthTestStatus::LoadedHint;
        self.status_text = format!("Loaded saved session `{profile_id}` into the editor.");
        Ok(self.projection())
    }

    pub fn close_session_editor_modal(&mut self) -> AppProjection {
        self.editor_modal_visible = false;
        self.status_text = "Closed Session Editor.".to_owned();
        self.projection()
    }

    pub fn open_known_hosts_manager(&mut self) -> AppProjection {
        self.known_hosts_modal_visible = true;
        self.ensure_known_hosts_selection();
        self.status_text = format!(
            "Known hosts manager opened with {} persisted entr{}.",
            self.known_host_entries().len(),
            if self.known_host_entries().len() == 1 { "y" } else { "ies" }
        );
        self.projection()
    }

    pub fn close_known_hosts_manager(&mut self) -> AppProjection {
        self.known_hosts_modal_visible = false;
        self.status_text = "Closed known hosts manager.".to_owned();
        self.projection()
    }

    pub fn select_previous_known_host(&mut self) -> AppProjection {
        let entries = self.known_host_entries();
        if entries.is_empty() {
            self.known_hosts_selected_key = None;
        } else if let Some(selected) = self.known_hosts_selected_key.as_ref() {
            let current = entries
                .iter()
                .position(|entry| &entry.key == selected)
                .unwrap_or(0);
            let next = if current == 0 { entries.len() - 1 } else { current - 1 };
            self.known_hosts_selected_key = Some(entries[next].key.clone());
        } else {
            self.known_hosts_selected_key = Some(entries[0].key.clone());
        }
        self.status_text = self.known_hosts_selection_legacy_text();
        self.projection()
    }

    pub fn select_next_known_host(&mut self) -> AppProjection {
        let entries = self.known_host_entries();
        if entries.is_empty() {
            self.known_hosts_selected_key = None;
        } else if let Some(selected) = self.known_hosts_selected_key.as_ref() {
            let current = entries
                .iter()
                .position(|entry| &entry.key == selected)
                .unwrap_or(0);
            let next = if current + 1 >= entries.len() { 0 } else { current + 1 };
            self.known_hosts_selected_key = Some(entries[next].key.clone());
        } else {
            self.known_hosts_selected_key = Some(entries[0].key.clone());
        }
        self.status_text = self.known_hosts_selection_legacy_text();
        self.projection()
    }

    pub fn remove_selected_known_host(&mut self) -> AppResult<AppProjection> {
        let selected = self
            .selected_known_host_entry()
            .ok_or_else(|| AppError::new("no known host entry is currently selected"))?;
        self.persistent_known_hosts.remove(&selected.host, selected.port);
        self.config_store
            .save_known_hosts(&self.persistent_known_hosts)
            .map_err(AppError::from_error)?;
        self.ensure_known_hosts_selection();
        self.status_text = format!(
            "Removed known host entry for {}:{} from `{}`.",
            selected.host,
            selected.port,
            self.config_store.known_hosts_file().display()
        );
        Ok(self.projection())
    }

    pub fn update_known_hosts_clear_confirmation(&mut self, value: &str) -> AppProjection {
        self.known_hosts_clear_confirmation = value.to_owned();
        self.projection()
    }

    pub fn clear_all_known_hosts(&mut self) -> AppResult<AppProjection> {
        if self.known_hosts_clear_confirmation.trim() != "CLEAR" {
            return Err(AppError::new("type CLEAR before removing all persisted known hosts"));
        }
        self.persistent_known_hosts = KnownHosts::new();
        self.config_store
            .save_known_hosts(&self.persistent_known_hosts)
            .map_err(AppError::from_error)?;
        self.known_hosts_selected_key = None;
        self.known_hosts_clear_confirmation.clear();
        self.status_text = format!(
            "Cleared all persisted known hosts from `{}`.",
            self.config_store.known_hosts_file().display()
        );
        Ok(self.projection())
    }

    pub fn select_session_editor_general(&mut self) -> AppProjection {
        self.editor_section = EditorSection::General;
        self.projection()
    }

    pub fn select_session_editor_authentication(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Authentication;
        self.projection()
    }

    pub fn select_session_editor_terminal(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Terminal;
        self.projection()
    }

    pub fn select_session_editor_sftp(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Sftp;
        self.projection()
    }

    pub fn select_session_editor_tunnels(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Tunnels;
        self.projection()
    }

    pub fn select_session_editor_proxy(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Proxy;
        self.projection()
    }

    pub fn select_session_editor_logging(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Logging;
        self.projection()
    }

    pub fn select_session_editor_advanced(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Advanced;
        self.projection()
    }

    pub fn select_session_editor_appearance(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Appearance;
        self.projection()
    }

    pub fn update_new_folder_name(&mut self, value: &str) -> AppProjection {
        self.new_folder_name = value.to_owned();
        self.projection()
    }

    pub fn select_next_editor_folder(&mut self) -> AppProjection {
        self.rotate_editor_folder(true);
        self.status_text = format!("Editor target folder: {}", self.editor_folder_legacy_text());
        self.projection()
    }

    pub fn select_previous_editor_folder(&mut self) -> AppProjection {
        self.rotate_editor_folder(false);
        self.status_text = format!("Editor target folder: {}", self.editor_folder_legacy_text());
        self.projection()
    }

    pub fn update_editor_name(&mut self, value: &str) -> AppProjection {
        self.editor.name = value.to_owned();
        self.projection()
    }

    pub fn update_editor_host(&mut self, value: &str) -> AppProjection {
        self.editor.host = value.to_owned();
        self.projection()
    }

    pub fn update_editor_port(&mut self, value: &str) -> AppProjection {
        self.editor.port_text = value.to_owned();
        self.projection()
    }

    pub fn update_editor_username(&mut self, value: &str) -> AppProjection {
        self.editor.username = value.to_owned();
        self.projection()
    }

    pub fn set_editor_auth_method_agent(&mut self) -> AppProjection {
        self.editor.auth_method = EditorAuthMethod::Agent;
        self.projection()
    }

    pub fn set_editor_auth_method_password(&mut self) -> AppProjection {
        self.editor.auth_method = EditorAuthMethod::Password;
        self.projection()
    }

    pub fn set_editor_auth_method_keyboard_interactive(&mut self) -> AppProjection {
        self.editor.auth_method = EditorAuthMethod::KeyboardInteractive;
        self.projection()
    }

    pub fn set_editor_auth_method_private_key(&mut self) -> AppProjection {
        self.editor.auth_method = EditorAuthMethod::PrivateKey;
        self.projection()
    }

    pub fn set_editor_host_key_policy_strict(&mut self) -> AppProjection {
        self.editor.host_key_policy = ConfigHostKeyPolicy::Strict;
        self.projection()
    }

    pub fn set_editor_host_key_policy_trust_on_first_use(&mut self) -> AppProjection {
        self.editor.host_key_policy = ConfigHostKeyPolicy::TrustOnFirstUse;
        self.projection()
    }

    pub fn set_editor_host_key_policy_accept_any_for_testing(&mut self) -> AppProjection {
        self.editor.host_key_policy = ConfigHostKeyPolicy::AcceptAnyForTesting;
        self.projection()
    }

    pub fn update_editor_password(&mut self, value: &str) -> AppProjection {
        self.editor.password = value.to_owned();
        self.projection()
    }

    pub fn update_editor_key_path(&mut self, value: &str) -> AppProjection {
        self.editor.key_path = value.to_owned();
        self.projection()
    }

    pub fn update_editor_passphrase(&mut self, value: &str) -> AppProjection {
        self.editor.passphrase = value.to_owned();
        self.projection()
    }

    pub fn set_editor_proxy_mode_none(&mut self) -> AppProjection {
        self.editor.proxy_mode = EditorProxyMode::None;
        self.projection()
    }

    pub fn set_editor_proxy_mode_custom(&mut self) -> AppProjection {
        self.editor.proxy_mode = EditorProxyMode::Custom;
        self.projection()
    }

    pub fn set_editor_proxy_protocol_socks4(&mut self) -> AppProjection {
        self.editor.proxy_protocol = ProxyProtocol::Socks4;
        self.projection()
    }

    pub fn set_editor_proxy_protocol_socks4a(&mut self) -> AppProjection {
        self.editor.proxy_protocol = ProxyProtocol::Socks4a;
        self.projection()
    }

    pub fn set_editor_proxy_protocol_socks5(&mut self) -> AppProjection {
        self.editor.proxy_protocol = ProxyProtocol::Socks5;
        self.projection()
    }

    pub fn set_editor_proxy_protocol_http_connect(&mut self) -> AppProjection {
        self.editor.proxy_protocol = ProxyProtocol::HttpConnect;
        self.projection()
    }

    pub fn update_editor_proxy_host(&mut self, value: &str) -> AppProjection {
        self.editor.proxy_host = value.to_owned();
        self.projection()
    }

    pub fn update_editor_proxy_port(&mut self, value: &str) -> AppProjection {
        self.editor.proxy_port_text = value.to_owned();
        self.projection()
    }

    pub fn update_editor_proxy_username(&mut self, value: &str) -> AppProjection {
        self.editor.proxy_username = value.to_owned();
        self.projection()
    }

    pub fn update_editor_proxy_password(&mut self, value: &str) -> AppProjection {
        self.editor.proxy_password = value.to_owned();
        self.projection()
    }

    pub fn toggle_editor_proxy_dns_by_proxy(&mut self) -> AppProjection {
        self.editor.proxy_dns_by_proxy = !self.editor.proxy_dns_by_proxy;
        self.projection()
    }

    pub fn set_editor_tunnel_kind_local(&mut self) -> AppProjection {
        self.editor.tunnel_kind = TunnelForwardKind::Local;
        self.projection()
    }

    pub fn set_editor_tunnel_kind_remote(&mut self) -> AppProjection {
        self.editor.tunnel_kind = TunnelForwardKind::Remote;
        self.projection()
    }

    pub fn set_editor_tunnel_kind_dynamic(&mut self) -> AppProjection {
        self.editor.tunnel_kind = TunnelForwardKind::Dynamic;
        self.projection()
    }

    pub fn update_editor_tunnel_bind_host(&mut self, value: &str) -> AppProjection {
        self.editor.tunnel_bind_host = value.to_owned();
        self.projection()
    }

    pub fn update_editor_tunnel_bind_port(&mut self, value: &str) -> AppProjection {
        self.editor.tunnel_bind_port_text = value.to_owned();
        self.projection()
    }

    pub fn update_editor_tunnel_target_host(&mut self, value: &str) -> AppProjection {
        self.editor.tunnel_target_host = value.to_owned();
        self.projection()
    }

    pub fn update_editor_tunnel_target_port(&mut self, value: &str) -> AppProjection {
        self.editor.tunnel_target_port_text = value.to_owned();
        self.projection()
    }

    pub fn add_editor_tunnel_forward(&mut self) -> AppResult<AppProjection> {
        let forward = self.build_editor_tunnel_forward()?;
        self.editor.tunnels.push(forward.clone());
        self.editor.tunnel_bind_port_text.clear();
        self.editor.tunnel_target_host.clear();
        self.editor.tunnel_target_port_text.clear();
        self.status_text = format!(
            "Added {} tunnel {}.",
            tunnel_kind_label(forward.kind),
            tunnel_forward_summary(&forward)
        );
        Ok(self.projection())
    }

    pub fn clear_editor_tunnels(&mut self) -> AppProjection {
        self.editor.tunnels.clear();
        self.status_text = "Cleared all editor tunnel forwards.".to_owned();
        self.projection()
    }

    pub fn test_editor_auth(&mut self) -> AppProjection {
        let status = match self.run_editor_auth_test() {
            Ok(status) => status,
            Err(error) => EditorAuthTestStatus::Failed {
                error: error.to_string(),
            },
        };
        self.status_text = status.legacy_text();
        self.editor_auth_test_status = status;
        self.projection()
    }

    pub fn toggle_global_logging_enabled(&mut self) -> AppProjection {
        self.config_document.logging.enabled = !self.config_document.logging.enabled;
        self.status_text = format!(
            "Global logging is now {}. Save logging settings to persist this policy.",
            if self.config_document.logging.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        self.projection()
    }

    pub fn set_global_logging_format_raw(&mut self) -> AppProjection {
        self.config_document.logging.format = "raw".to_owned();
        self.status_text =
            "Global logging format set to raw transcript. Save logging settings to persist."
                .to_owned();
        self.projection()
    }

    pub fn set_global_logging_format_sanitized(&mut self) -> AppProjection {
        self.config_document.logging.format = "sanitized".to_owned();
        self.status_text =
            "Global logging format set to sanitized text. Save logging settings to persist."
                .to_owned();
        self.projection()
    }

    pub fn update_global_logging_directory(&mut self, value: &str) -> AppProjection {
        let directory = value.trim();
        self.config_document.logging.directory = if directory.is_empty() {
            None
        } else {
            Some(directory.to_owned())
        };
        self.projection()
    }

    pub fn save_global_logging_settings(&mut self) -> AppResult<AppProjection> {
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        let summary = self.logging_summary_legacy_text();
        self.set_status_kind(
            "logging-saved",
            format!("Saved global logging policy: {summary}."),
            summary,
            String::new(),
        );
        Ok(self.projection())
    }

    pub fn update_settings_scrollback_lines(&mut self, value: &str) -> AppProjection {
        self.settings_scrollback_lines_text = value.to_owned();
        self.projection()
    }

    pub fn update_settings_scrollback_max_cells(&mut self, value: &str) -> AppProjection {
        self.settings_scrollback_max_cells_text = value.to_owned();
        self.projection()
    }

    pub fn reset_settings_terminal_defaults(&mut self) -> AppProjection {
        self.settings_scrollback_lines_text = DEFAULT_SCROLLBACK_LINES.to_string();
        self.settings_scrollback_max_cells_text = DEFAULT_SCROLLBACK_MAX_CELLS.to_string();
        self.settings_terminal_status = SettingsTerminalStatus::DefaultsLoaded;
        self.projection()
    }

    pub fn save_settings_terminal(&mut self) -> AppResult<AppProjection> {
        let scrollback_lines = match parse_settings_usize(
            SettingsTerminalField::ScrollbackLines,
            &self.settings_scrollback_lines_text,
            SETTINGS_SCROLLBACK_LINES_MIN,
            SETTINGS_SCROLLBACK_LINES_MAX,
        ) {
            Ok(value) => value,
            Err(status) => {
                self.settings_terminal_status = status;
                return Ok(self.projection());
            }
        };
        let scrollback_max_cells = match parse_settings_usize(
            SettingsTerminalField::ScrollbackMaxCells,
            &self.settings_scrollback_max_cells_text,
            SETTINGS_SCROLLBACK_MAX_CELLS_MIN,
            SETTINGS_SCROLLBACK_MAX_CELLS_MAX,
        ) {
            Ok(value) => value,
            Err(status) => {
                self.settings_terminal_status = status;
                return Ok(self.projection());
            }
        };

        let terminal = TerminalProfile {
            scrollback_lines,
            scrollback_max_cells,
        };
        self.config_document.terminal = terminal.clone();
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        for runtime in self.sessions.values_mut() {
            runtime.configure_terminal_limits(&terminal);
        }
        self.settings_scrollback_lines_text = scrollback_lines.to_string();
        self.settings_scrollback_max_cells_text = scrollback_max_cells.to_string();
        self.settings_terminal_status = SettingsTerminalStatus::Saved {
            lines: scrollback_lines,
            max_cells: scrollback_max_cells,
        };
        self.status_text = self.settings_terminal_status.legacy_text();
        Ok(self.projection())
    }

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

    pub fn save_editor_to_saved_session(&mut self) -> AppResult<AppProjection> {
        let session_id = self.persist_editor_to_saved_session()?;
        self.editor_modal_visible = false;
        self.status_text = format!("Saved session editor changes into `{session_id}`.");
        Ok(self.projection())
    }

    pub fn save_editor_and_connect(&mut self) -> AppResult<AppProjection> {
        let session_id = self.persist_editor_to_saved_session()?;
        self.editor_modal_visible = false;
        let projection = self.open_saved_session(&session_id)?;
        Ok(projection)
    }

    fn persist_editor_to_saved_session(&mut self) -> AppResult<String> {
        let port = self
            .editor
            .port_text
            .trim()
            .parse::<u16>()
            .map_err(|_| AppError::new("editor port must be a valid integer between 1 and 65535"))?;
        if port == 0 {
            return Err(AppError::new("editor port must be greater than zero"));
        }
        if self.editor.name.trim().is_empty() {
            return Err(AppError::new("editor session name must not be empty"));
        }
        if self.editor.host.trim().is_empty() {
            return Err(AppError::new("editor host must not be empty"));
        }

        let session_id = self
            .editor
            .target_session_id
            .clone()
            .unwrap_or_else(|| self.next_saved_profile_id(&self.editor.host));
        let auth_profile_id = format!("{session_id}-auth");
        let auth_profile = self.build_auth_profile_for_editor(&auth_profile_id)?;
        let proxy_profile_id = format!("{session_id}-proxy");
        let proxy_profile = self.build_proxy_profile_for_editor(&proxy_profile_id)?;
        let target_folder_id = self.editor.target_folder_id.trim().to_owned();
        if target_folder_id.is_empty() {
            return Err(AppError::new("editor target folder must not be empty"));
        }

        let mut profile = SessionProfile::new(
            session_id.clone(),
            self.editor.name.trim().to_owned(),
            self.editor.host.trim().to_owned(),
        );
        profile.port = port;
        profile.username = if self.editor.username.trim().is_empty() {
            None
        } else {
            Some(self.editor.username.trim().to_owned())
        };
        profile.host_key_policy = Some(self.editor.host_key_policy);
        profile.auth_profile_id = Some(auth_profile_id.clone());
        profile.proxy_profile_id = proxy_profile.as_ref().map(|_| proxy_profile_id.clone());
        profile.tunnel = if self.editor.tunnels.is_empty() {
            None
        } else {
            Some(TunnelProfile {
                forwards: self.editor.tunnels.clone(),
            })
        };

        self.config_document
            .auth_profiles
            .insert(auth_profile_id, auth_profile);
        match proxy_profile {
            Some(profile) => {
                self.config_document
                    .proxy_profiles
                    .insert(proxy_profile_id, profile);
            }
            None => {
                self.config_document.proxy_profiles.remove(&proxy_profile_id);
            }
        }
        let existing_folder_id = find_session_folder_id(&self.config_document.folders, &session_id);
        if existing_folder_id.as_deref() == Some(target_folder_id.as_str()) {
            if let Some(existing) = self.config_document.find_session_mut(&session_id) {
                *existing = profile;
            } else {
                self.target_saved_sessions_folder_mut(&target_folder_id)?
                    .sessions
                    .push(profile);
            }
        } else {
            let _ = self.config_document.remove_session(&session_id);
            self.target_saved_sessions_folder_mut(&target_folder_id)?
                .sessions
                .push(profile);
        }
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        self.selected_saved_session_id = Some(session_id.clone());
        self.editor.target_session_id = Some(session_id.clone());
        self.editor.target_folder_id = target_folder_id;
        Ok(session_id)
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
        self.status_text = format!(
            "Updated saved session `{profile_id}` from the active runtime session."
        );
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
        self.sessions.remove(&profile_id);
        self.recent_session_ids.retain(|id| id != &profile_id);
        if self.active_session_id.as_deref() == Some(profile_id.as_str()) {
            self.active_session_id = None;
        }
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

    /// Expands/collapses one folder row of the sidebar session tree.
    ///
    /// Expansion state is memory-only (it is not part of the config document).
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

    /// Selects one node of the sidebar session tree by id.
    ///
    /// Folder ids select the folder row and clear the session selection; saved
    /// profile ids select the session row and clear the folder selection. An
    /// unknown id leaves the current selection untouched.
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

    /// Activates one node of the sidebar session tree (double click / Enter):
    /// folders expand or collapse, saved sessions are opened by id.
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
            format!("Filtering saved sessions for `{}`.", self.session_search_query)
        };
        self.projection()
    }

    pub fn send_active_terminal_input(&mut self, input: &str) -> AppResult<AppProjection> {
        self.send_active_terminal_bytes(input.as_bytes())
    }

    /// Encode a key event from the terminal surface and write it to the shell.
    pub fn send_active_terminal_key(
        &mut self,
        text: &str,
        ctrl: bool,
        alt: bool,
        shift: bool,
        meta: bool,
    ) -> AppResult<AppProjection> {
        let bytes = yshell_terminal::encode_key(text, ctrl, alt, shift, meta);
        self.send_active_terminal_bytes(&bytes)
    }

    /// Write raw bytes to the active shell (paste, control characters, ...).
    pub fn send_active_terminal_bytes(&mut self, bytes: &[u8]) -> AppResult<AppProjection> {
        if bytes.is_empty() {
            return Ok(self.projection());
        }
        let session_key = self.active_session_key()?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        self.dispatcher
            .dispatch(SessionCommand::SendTerminalInput {
                session_id,
                bytes: bytes.to_vec(),
            })
            .map_err(AppError::from_error)?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let _ = runtime
            .write_terminal_input(bytes)
            .map_err(AppError::from_error)?;
        self.status_text = format!(
            "Sent {} bytes through the runtime terminal pipeline.",
            bytes.len()
        );
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub fn copy_active_terminal_visible_text(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let visible = self
            .sessions
            .get(&session_key)
            .map(SessionRuntime::visible_text)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        self.terminal_clipboard = visible;
        self.status_text = if self.terminal_clipboard.trim().is_empty() {
            "Copied visible terminal view, but it was empty.".to_owned()
        } else {
            format!(
                "Copied {} characters from the visible terminal view into the app clipboard.",
                self.terminal_clipboard.chars().count()
            )
        };
        Ok(self.projection())
    }

    pub fn paste_terminal_clipboard(&mut self) -> AppResult<AppProjection> {
        if self.terminal_clipboard.is_empty() {
            self.status_text = "Terminal clipboard is empty, so nothing was pasted.".to_owned();
            return Ok(self.projection());
        }
        let mut payload = self.terminal_clipboard.clone();
        if !payload.ends_with('\n') {
            payload.push('\n');
        }
        self.send_active_terminal_input(&payload)
    }

    pub fn paste_text_into_terminal(&mut self, text: &str) -> AppResult<AppProjection> {
        if text.is_empty() {
            self.status_text = "Clipboard is empty, so nothing was pasted.".to_owned();
            return Ok(self.projection());
        }
        self.terminal_clipboard = text.to_owned();
        // Paste sends the clipboard bytes verbatim; terminals do not append a
        // newline, so pasted text waits for the user to press Enter.
        self.send_active_terminal_bytes(text.as_bytes())
    }

    pub fn clear_active_terminal(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        runtime.clear_visible_terminal();
        self.terminal_search_query.clear();
        self.terminal_search_matches.clear();
        self.terminal_search_current_index = None;
        self.status_text = "Cleared the visible terminal view.".to_owned();
        Ok(self.projection())
    }

    pub fn find_in_active_terminal(&mut self, query: &str) -> AppResult<AppProjection> {
        self.terminal_search_query = query.to_owned();
        let session_key = self.active_session_key()?;
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let matches = runtime.find_visible_text(query);
        self.terminal_search_matches = matches;
        self.terminal_search_current_index = if self.terminal_search_matches.is_empty() {
            None
        } else {
            Some(0)
        };
        self.status_text = self.terminal_search_summary_legacy_text();
        Ok(self.projection())
    }

    pub fn select_next_terminal_match(&mut self) -> AppResult<AppProjection> {
        self.rotate_terminal_match(true);
        self.status_text = self.terminal_search_summary_legacy_text();
        Ok(self.projection())
    }

    pub fn select_previous_terminal_match(&mut self) -> AppResult<AppProjection> {
        self.rotate_terminal_match(false);
        self.status_text = self.terminal_search_summary_legacy_text();
        Ok(self.projection())
    }

    pub fn refresh_active_sftp_listing(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.sftp_listing = SftpListingState::RefreshNeedsSession;
            self.status_text = self.sftp_session.legacy_status_text();
            return Ok(self.projection());
        };
        let status_text = self.sync_sftp_lifecycle_for_session(&session_key, true);
        self.record_sftp_refresh_status(status_text);
        Ok(self.projection())
    }

    /// Records the i18n kind for the message produced by a SFTP refresh:
    /// ready listings carry path + entry count, failures carry session key +
    /// reason. Other lifecycle messages keep the English `status_text`.
    fn record_sftp_refresh_status(&mut self, text: String) {
        let ready = matches!(self.sftp_session, SftpSessionLifecycle::Ready { .. });
        let failed = matches!(self.sftp_session, SftpSessionLifecycle::Failed { .. });
        if ready {
            let path = self.sftp_path.clone();
            let count = self.sftp_entries.len().to_string();
            self.set_status_kind("sftp-ready", text, path, count);
        } else if failed {
            let session_key = self.sftp_session.status_session_key().to_owned();
            let reason = self.sftp_session.status_detail().to_owned();
            self.set_status_kind("sftp-failed", text, session_key, reason);
        } else {
            self.status_text = text;
        }
    }

    pub fn open_sftp_path(&mut self, path: &str) -> AppResult<AppProjection> {
        let path = normalize_remote_path(path)?;
        self.sftp_path = path;
        self.refresh_active_sftp_listing()
    }

    pub fn open_sftp_parent(&mut self) -> AppResult<AppProjection> {
        self.sftp_path = parent_remote_path(&self.sftp_path);
        self.refresh_active_sftp_listing()
    }

    // --- SFTP file manager (iteration 3) -----------------------------------

    /// Selects a visible row by list index; `-1` clears the selection.
    pub fn select_sftp_entry(&mut self, index: i32) -> AppProjection {
        let visible_len = self.visible_sftp_entries().len();
        self.sftp_selected_index = usize::try_from(index)
            .ok()
            .filter(|index| *index < visible_len);
        self.projection()
    }

    /// Activates the selected row: directories are opened, files start the
    /// remote edit flow.
    pub fn activate_sftp_entry(&mut self) -> AppResult<AppProjection> {
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP entry before opening it.".to_owned();
            return Ok(self.projection());
        };
        if matches!(entry.kind, FsEntryKind::Directory) {
            return self.open_sftp_path(&entry.path);
        }
        self.edit_sftp_selected()
    }

    /// Sorts by `column` (name/size/modified/permissions), flipping the
    /// direction when that column is already active.
    pub fn sort_sftp_by(&mut self, column: &str) -> AppProjection {
        let Some(column) = SftpSortColumn::from_label(column) else {
            self.status_text = format!("Unknown SFTP sort column `{}`.", column.trim());
            return self.projection();
        };
        let selected = self.selected_sftp_entry().map(|entry| entry.path);
        if self.sftp_sort_column == column {
            self.sftp_sort_ascending = !self.sftp_sort_ascending;
        } else {
            self.sftp_sort_column = column;
            self.sftp_sort_ascending = true;
        }
        self.reconcile_sftp_selection(selected.as_deref());
        let sort_text = self.sftp_sort_legacy_text();
        let column_id = self.sftp_sort_column.label().to_ascii_lowercase();
        let direction = if self.sftp_sort_ascending {
            "ascending"
        } else {
            "descending"
        };
        self.set_status_kind(
            "sftp-sorted",
            format!("Sorted SFTP entries by {sort_text}."),
            column_id,
            direction.to_owned(),
        );
        self.projection()
    }

    /// Toggles dotfile visibility and keeps the selection when possible.
    pub fn toggle_sftp_hidden_files(&mut self) -> AppProjection {
        let selected = self.selected_sftp_entry().map(|entry| entry.path);
        self.sftp_show_hidden = !self.sftp_show_hidden;
        self.reconcile_sftp_selection(selected.as_deref());
        self.status_text = if self.sftp_show_hidden {
            "SFTP hidden files are now shown.".to_owned()
        } else {
            "SFTP hidden files are now hidden.".to_owned()
        };
        self.projection()
    }

    /// Navigates to one of the current breadcrumbs.
    pub fn open_sftp_crumb(&mut self, index: i32) -> AppResult<AppProjection> {
        let crumbs = self.sftp_crumbs();
        let Some(crumb) = usize::try_from(index).ok().and_then(|index| crumbs.get(index)) else {
            self.status_text = "That SFTP breadcrumb is no longer available.".to_owned();
            return Ok(self.projection());
        };
        let path = crumb.path.clone();
        self.open_sftp_path(&path)
    }

    /// Creates a directory inside the current remote directory (absolute paths
    /// are used as given).
    pub fn create_sftp_folder_named(&mut self, name: &str) -> AppResult<AppProjection> {
        let name = name.trim();
        if name.is_empty() {
            self.status_text = "New SFTP folder name must not be empty.".to_owned();
            return Ok(self.projection());
        }
        let target = self.sftp_relative_or_absolute_path(name);
        self.sftp_secondary_target = name.to_owned();
        self.create_sftp_directory()?;
        self.reselect_sftp_path(&target);
        Ok(self.projection())
    }

    /// Renames the selected entry; relative names stay in the current directory.
    pub fn rename_sftp_selected(&mut self, new_name: &str) -> AppResult<AppProjection> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            self.status_text = "New SFTP name must not be empty.".to_owned();
            return Ok(self.projection());
        }
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP entry before renaming it.".to_owned();
            return Ok(self.projection());
        };
        let target = self.sftp_relative_or_absolute_path(new_name);
        self.sftp_remote_target = entry.path;
        self.sftp_secondary_target = new_name.to_owned();
        self.rename_sftp_path()?;
        self.reselect_sftp_path(&target);
        Ok(self.projection())
    }

    /// Deletes the selected entry after the UI confirmation.
    pub fn delete_sftp_selected(&mut self) -> AppResult<AppProjection> {
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP entry before deleting it.".to_owned();
            return Ok(self.projection());
        };
        let Some(session_key) = self.prepare_active_sftp_operation("deleting through SFTP")? else {
            return Ok(self.projection());
        };
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        client
            .delete(&entry.path)
            .map_err(AppError::from_error)?;
        self.sftp_remote_target = entry.path.clone();
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!("Deleted `{}` through SFTP.", entry.path);
        Ok(self.projection())
    }

    /// Applies octal permissions to the selected entry.
    pub fn chmod_sftp_selected(&mut self, permissions: &str) -> AppResult<AppProjection> {
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP entry before changing permissions.".to_owned();
            return Ok(self.projection());
        };
        self.sftp_remote_target = entry.path;
        self.sftp_permissions = permissions.trim().to_owned();
        self.chmod_sftp_path()
    }

    /// Uploads a local file into the current remote directory under its file name.
    pub fn upload_sftp_into_current(&mut self, local_path: &str) -> AppResult<AppProjection> {
        let local_path = local_path.trim();
        if local_path.is_empty() {
            self.status_text = "Local upload path must not be empty.".to_owned();
            return Ok(self.projection());
        }
        self.sftp_local_path = local_path.to_owned();
        self.sftp_remote_target = String::new();
        self.upload_sftp_file()?;
        let uploaded = self.sftp_remote_target.clone();
        self.reselect_sftp_path(&uploaded);
        Ok(self.projection())
    }

    /// Downloads the selected entry to `local_path` (defaults to the current
    /// local transfer path when empty).
    pub fn download_sftp_selected(&mut self, local_path: &str) -> AppResult<AppProjection> {
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP entry before downloading it.".to_owned();
            return Ok(self.projection());
        };
        let local_path = local_path.trim();
        if !local_path.is_empty() {
            self.sftp_local_path = local_path.to_owned();
        }
        if self.sftp_local_path.trim().is_empty() {
            self.status_text = "Local download path must not be empty.".to_owned();
            return Ok(self.projection());
        }
        self.sftp_remote_target = entry.path;
        self.download_sftp_file()
    }

    /// Starts the remote edit flow for the selected file.
    pub fn edit_sftp_selected(&mut self) -> AppResult<AppProjection> {
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP file before starting remote edit.".to_owned();
            return Ok(self.projection());
        };
        self.sftp_remote_target = entry.path;
        self.start_sftp_remote_edit()
    }

    #[cfg(test)]
    pub fn set_sftp_remote_target(&mut self, path: &str) -> AppProjection {
        self.sftp_remote_target = path.to_owned();
        self.projection()
    }

    pub fn set_sftp_operation_inputs(
        &mut self,
        local_path: &str,
        remote_target: &str,
        secondary_target: &str,
        permissions: &str,
    ) -> AppProjection {
        self.sftp_local_path = local_path.to_owned();
        self.sftp_remote_target = remote_target.to_owned();
        self.sftp_secondary_target = secondary_target.to_owned();
        self.sftp_permissions = permissions.to_owned();
        self.projection()
    }

    pub fn upload_sftp_file(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) =
            self.prepare_active_sftp_operation("uploading through SFTP")?
        else {
            return Ok(self.projection());
        };
        let local_path = PathBuf::from(self.sftp_local_path.trim());
        if self.sftp_local_path.trim().is_empty() {
            return Err(AppError::new("local upload path is empty"));
        }
        let remote_path = self.sftp_upload_target()?;
        let transfer_bytes = fs::metadata(&local_path)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        let transfer_id = self.enqueue_sftp_transfer(
            TransferDirection::Upload,
            local_path.display().to_string(),
            remote_path.clone(),
            Some(transfer_bytes),
        );
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        if let Err(error) = client.upload_file(&local_path, &remote_path) {
            let app_error = AppError::from_error(error);
            self.fail_sftp_transfer(&transfer_id, app_error.to_string());
            self.status_text = format!("SFTP upload error: {app_error}");
            return Ok(self.projection());
        }
        self.complete_sftp_transfer(&transfer_id, transfer_bytes);
        if let Some(runtime) = self.sessions.get_mut(&session_key) {
            runtime.record_transfer(
                "upload",
                &local_path,
                Path::new(&remote_path),
                transfer_bytes,
            );
        }
        self.sftp_remote_target = remote_path.clone();
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!(
            "Uploaded `{}` to `{}` through the live SFTP backend.",
            local_path.display(),
            remote_path
        );
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub fn download_sftp_file(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) =
            self.prepare_active_sftp_operation("downloading through SFTP")?
        else {
            return Ok(self.projection());
        };
        if self.sftp_local_path.trim().is_empty() {
            return Err(AppError::new("local download path is empty"));
        }
        let remote_path = self.sftp_download_target()?;
        let local_path = PathBuf::from(self.sftp_local_path.trim());
        let transfer_id = self.enqueue_sftp_transfer(
            TransferDirection::Download,
            remote_path.clone(),
            local_path.display().to_string(),
            None,
        );
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        if let Err(error) = client.download_file(&remote_path, &local_path) {
            let app_error = AppError::from_error(error);
            self.fail_sftp_transfer(&transfer_id, app_error.to_string());
            self.status_text = format!("SFTP download error: {app_error}");
            return Ok(self.projection());
        }
        let transfer_bytes = fs::metadata(&local_path)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        self.complete_sftp_transfer(&transfer_id, transfer_bytes);
        if let Some(runtime) = self.sessions.get_mut(&session_key) {
            runtime.record_transfer(
                "download",
                &local_path,
                Path::new(&remote_path),
                transfer_bytes,
            );
        }
        self.status_text = format!(
            "Downloaded `{}` to `{}` through the live SFTP backend.",
            remote_path,
            local_path.display()
        );
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub fn start_sftp_remote_edit(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) =
            self.prepare_active_sftp_operation("starting remote file edit")?
        else {
            return Ok(self.projection());
        };
        let remote_path = self.sftp_download_target()?;
        let temp_root = self.config_dir.join("remote-edit");
        let edit_session = prepare_remote_edit_session(&temp_root, &remote_path)
            .map_err(AppError::from_error)?;
        let local_path = PathBuf::from(&edit_session.local_temp_path);
        let transfer_id = self.enqueue_sftp_transfer(
            edit_session.download_direction(),
            remote_path.clone(),
            local_path.display().to_string(),
            None,
        );
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        if let Err(error) = client.download_file(&remote_path, &local_path) {
            let app_error = AppError::from_error(error);
            self.fail_sftp_transfer(&transfer_id, app_error.to_string());
            self.status_text = format!("SFTP remote edit download error: {app_error}");
            return Ok(self.projection());
        }
        let transfer_bytes = fs::metadata(&local_path)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        self.complete_sftp_transfer(&transfer_id, transfer_bytes);
        self.sftp_local_path = local_path.display().to_string();
        self.sftp_remote_target = remote_path.clone();
        self.remote_edit_session = Some(edit_session);
        if let Some(runtime) = self.sessions.get_mut(&session_key) {
            runtime.record_transfer(
                "remote-edit-download",
                &local_path,
                Path::new(&remote_path),
                transfer_bytes,
            );
        }
        self.status_text = format!(
            "Remote edit ready for `{remote_path}`. Edit local temp file `{}` and then save remote edit.",
            local_path.display()
        );
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub fn save_sftp_remote_edit(&mut self) -> AppResult<AppProjection> {
        let Some(edit_session) = self.remote_edit_session.clone() else {
            self.status_text =
                "No active remote edit session to save. Start remote edit first.".to_owned();
            return Ok(self.projection());
        };
        let Some(session_key) =
            self.prepare_active_sftp_operation("saving remote file edit")?
        else {
            return Ok(self.projection());
        };
        let local_path = PathBuf::from(&edit_session.local_temp_path);
        let transfer_bytes = fs::metadata(&local_path)
            .map(|metadata| metadata.len())
            .map_err(|error| {
                AppError::new(format!(
                    "remote edit temp file `{}` is not readable: {error}",
                    local_path.display()
                ))
            })?;
        let transfer_id = self.enqueue_sftp_transfer(
            edit_session.upload_direction(),
            local_path.display().to_string(),
            edit_session.remote_path.clone(),
            Some(transfer_bytes),
        );
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        if let Err(error) = client.upload_file(&local_path, &edit_session.remote_path) {
            let app_error = AppError::from_error(error);
            self.fail_sftp_transfer(&transfer_id, app_error.to_string());
            self.status_text = format!("SFTP remote edit upload error: {app_error}");
            return Ok(self.projection());
        }
        self.complete_sftp_transfer(&transfer_id, transfer_bytes);
        if let Some(runtime) = self.sessions.get_mut(&session_key) {
            runtime.record_transfer(
                "remote-edit-upload",
                &local_path,
                Path::new(&edit_session.remote_path),
                transfer_bytes,
            );
        }
        self.sftp_remote_target = edit_session.remote_path.clone();
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!(
            "Saved remote edit `{}` from local temp file `{}`.",
            edit_session.remote_path,
            local_path.display()
        );
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub fn cancel_sftp_remote_edit(&mut self) -> AppResult<AppProjection> {
        let Some(edit_session) = self.remote_edit_session.take() else {
            self.status_text =
                "No active remote edit session to cancel. Start remote edit first.".to_owned();
            return Ok(self.projection());
        };
        match fs::remove_file(&edit_session.local_temp_path) {
            Ok(()) => {
                self.status_text = format!(
                    "Canceled remote edit for `{}` and removed temp file `{}`.",
                    edit_session.remote_path, edit_session.local_temp_path
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.status_text = format!(
                    "Canceled remote edit for `{}`. Temp file was already gone.",
                    edit_session.remote_path
                );
            }
            Err(error) => {
                self.status_text = format!(
                    "Canceled remote edit for `{}`, but temp file `{}` could not be removed: {error}",
                    edit_session.remote_path, edit_session.local_temp_path
                );
            }
        }
        Ok(self.projection())
    }

    pub fn create_sftp_directory(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) =
            self.prepare_active_sftp_operation("creating directories through SFTP")?
        else {
            return Ok(self.projection());
        };
        let remote_path = self.sftp_secondary_target_path("new SFTP directory path is empty")?;
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        client.mkdir(&remote_path).map_err(AppError::from_error)?;
        self.sftp_remote_target = remote_path.clone();
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!("Created remote directory `{remote_path}` through SFTP.");
        Ok(self.projection())
    }

    pub fn rename_sftp_path(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) =
            self.prepare_active_sftp_operation("renaming paths through SFTP")?
        else {
            return Ok(self.projection());
        };
        let from_path = self.sftp_download_target()?;
        let to_path = self.sftp_secondary_target_path("new SFTP path is empty")?;
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        client
            .rename(&from_path, &to_path)
            .map_err(AppError::from_error)?;
        self.sftp_remote_target = to_path.clone();
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!("Renamed `{from_path}` to `{to_path}` through SFTP.");
        Ok(self.projection())
    }

    pub fn chmod_sftp_path(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) =
            self.prepare_active_sftp_operation("changing permissions through SFTP")?
        else {
            return Ok(self.projection());
        };
        let remote_path = self.sftp_download_target()?;
        let permissions = parse_sftp_permissions(&self.sftp_permissions)?;
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        client
            .chmod(&remote_path, permissions)
            .map_err(AppError::from_error)?;
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!(
            "Applied chmod {:03o} to `{remote_path}` through SFTP.",
            permissions
        );
        Ok(self.projection())
    }

    #[cfg(test)]
    pub fn poll_active_terminal_output(&mut self) -> AppResult<AppProjection> {
        if let Some(projection) = self.poll_active_terminal_output_passive()? {
            return Ok(projection);
        }
        self.status_text = "Polled the active terminal, but no new output was available.".to_owned();
        Ok(self.projection())
    }

    pub fn poll_active_terminal_output_passive(&mut self) -> AppResult<Option<AppProjection>> {
        let Some(session_key) = self.active_session_id.clone() else {
            return Ok(None);
        };
        let previous_terminal = self
            .sessions
            .get(&session_key)
            .map(SessionRuntime::visible_text)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let previous_state = self
            .sessions
            .get(&session_key)
            .map(|runtime| runtime.state)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let chunks = runtime
            .poll_shell_output()
            .map_err(AppError::from_error)?;
        let shell_connected = runtime.shell_is_connected().unwrap_or(false);
        let shell_closed = previous_state == yshell_core::SessionState::Connected && !shell_connected;
        if shell_closed {
            runtime.set_state(yshell_core::SessionState::Disconnected);
            runtime.append_status_line("Live shell closed while the app was polling for output.");
            self.status_text =
                "The active live shell closed while the terminal was refreshing.".to_owned();
        }
        let terminal_changed = runtime.visible_text() != previous_terminal;
        let state_changed = runtime.state != previous_state;
        if chunks.is_empty() && !terminal_changed && !state_changed {
            return Ok(None);
        }
        if shell_closed {
            let sftp_status = self.sync_sftp_lifecycle_for_session(&session_key, false);
            self.status_text = format!("{} {}", self.status_text, sftp_status);
        }
        self.fold_logging_notice_from_session(&session_key);
        Ok(Some(self.projection()))
    }

    pub fn terminal_clipboard_text(&self) -> &str {
        &self.terminal_clipboard
    }

    /// Frame revision of the active terminal (0 when no session is active).
    #[must_use]
    pub fn active_terminal_frame_id(&self) -> u64 {
        self.active_terminal_runtime()
            .map(SessionRuntime::frame_id)
            .unwrap_or(0)
    }

    #[must_use]
    pub fn active_terminal_scroll_offset(&self) -> u64 {
        self.active_terminal_runtime()
            .map(|runtime| runtime.viewport_offset() as u64)
            .unwrap_or(0)
    }

    /// Scrollbar geometry for the active terminal: (total lines, viewport rows).
    ///
    /// Total lines are the scrollback length plus the live viewport, i.e. every
    /// line the user can reach by scrolling; both counts are 0 without a session.
    #[must_use]
    pub fn active_terminal_scroll_geometry(&self) -> (usize, usize) {
        self.active_terminal_viewport_metrics()
            .map(|metrics| {
                // `top_absolute_row + viewport_offset` reconstructs the scrollback length
                // without reaching into the session runtime.
                let scrollback = metrics
                    .top_absolute_row
                    .saturating_add(metrics.viewport_offset);
                (
                    scrollback.saturating_add(usize::from(metrics.rows)),
                    usize::from(metrics.rows),
                )
            })
            .unwrap_or((0, 0))
    }

    #[must_use]
    pub fn active_terminal_selection_active(&self) -> bool {
        self.active_terminal_runtime()
            .map(SessionRuntime::terminal_selection_active)
            .unwrap_or(false)
    }

    #[must_use]
    pub fn active_terminal_selection_text(&self) -> String {
        self.active_terminal_runtime()
            .map(SessionRuntime::terminal_selection_text)
            .unwrap_or_default()
    }

    /// Grid geometry used by bootstrap to translate pixels into cells.
    #[must_use]
    pub fn active_terminal_viewport_metrics(&self) -> Option<TerminalViewportMetrics> {
        self.active_terminal_runtime()
            .map(SessionRuntime::terminal_viewport_metrics)
    }

    /// Renderer input for the active terminal (viewport lines + cursor + selection).
    #[must_use]
    pub fn active_terminal_render_snapshot(&self) -> Option<TerminalSnapshot<'_>> {
        self.active_terminal_runtime()
            .and_then(SessionRuntime::terminal_render_snapshot)
    }

    pub fn scroll_active_terminal(&mut self, delta: i32) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            self.status_text = "No active terminal to scroll.".to_owned();
            return Ok(self.projection());
        };
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let changed = runtime.scroll_terminal(delta);
        self.status_text = if runtime.viewport_offset() == 0 {
            "Terminal scrolled to the live bottom.".to_owned()
        } else if changed {
            format!(
                "Terminal scrolled back {} line(s) into scrollback.",
                runtime.viewport_offset()
            )
        } else {
            "Terminal is already at the top of the scrollback buffer.".to_owned()
        };
        Ok(self.projection())
    }

    /// Move the active terminal viewport so that line `line_from_top` (0 = the
    /// oldest line in the buffer) is the first visible row.
    ///
    /// Slint's scrollbar works in "lines from the top of the buffer" instead of
    /// scroll offsets; the value is clamped to the reachable range, so 0 pins the
    /// viewport to the top and anything at or past the last page lands on the
    /// live bottom.
    pub fn scroll_active_terminal_to_line(
        &mut self,
        line_from_top: i32,
    ) -> AppResult<AppProjection> {
        let Some(current_top) = self
            .active_terminal_viewport_metrics()
            .map(|metrics| metrics.top_absolute_row)
        else {
            self.status_text = "No active terminal to scroll.".to_owned();
            return Ok(self.projection());
        };
        let target = usize::try_from(line_from_top.max(0)).unwrap_or(usize::MAX);
        // The viewport top is `scrollback_len - viewport_offset`, so the required
        // delta is simply the distance to the requested absolute line; the
        // session runtime clamps the resulting offset to `[0, scrollback_len]`.
        let delta = if target >= current_top {
            -i32::try_from(target - current_top).unwrap_or(i32::MAX)
        } else {
            i32::try_from(current_top - target).unwrap_or(i32::MAX)
        };
        self.scroll_active_terminal(delta)
    }

    pub fn scroll_active_terminal_to_bottom(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            self.status_text = "No active terminal to scroll.".to_owned();
            return Ok(self.projection());
        };
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        if runtime.scroll_terminal_to_bottom() {
            self.status_text = "Terminal scrolled to the live bottom.".to_owned();
        }
        Ok(self.projection())
    }

    pub fn begin_active_terminal_selection(
        &mut self,
        column: u16,
        absolute_row: u16,
    ) -> AppResult<AppProjection> {
        self.update_active_terminal_selection_with(column, absolute_row, |runtime, point| {
            runtime.begin_terminal_selection(point)
        })
    }

    pub fn update_active_terminal_selection(
        &mut self,
        column: u16,
        absolute_row: u16,
    ) -> AppResult<AppProjection> {
        self.update_active_terminal_selection_with(column, absolute_row, |runtime, point| {
            runtime.update_terminal_selection(point)
        })
    }

    pub fn select_word_in_active_terminal(
        &mut self,
        column: u16,
        absolute_row: u16,
    ) -> AppResult<AppProjection> {
        self.update_active_terminal_selection_with(column, absolute_row, |runtime, point| {
            runtime.select_word_at(point)
        })
    }

    pub fn select_all_active_terminal(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            self.status_text = "No active terminal to select.".to_owned();
            return Ok(self.projection());
        };
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        if runtime.select_all_terminal() {
            self.status_text = format!(
                "Selected {} characters from the terminal.",
                runtime.terminal_selection_text().chars().count()
            );
        }
        Ok(self.projection())
    }

    /// Copy the active selection into the app clipboard (the shell-independent
    /// buffer the UI already used for the visible-view copy).
    pub fn copy_active_terminal_selection(&mut self) -> AppResult<AppProjection> {
        self.active_session_key()?;
        let text = self.active_terminal_selection_text();
        if text.is_empty() {
            self.status_text = "No terminal selection to copy.".to_owned();
            return Ok(self.projection());
        }
        self.terminal_clipboard = text;
        self.status_text = format!(
            "Copied {} characters from the terminal selection.",
            self.terminal_clipboard.chars().count()
        );
        Ok(self.projection())
    }

    fn update_active_terminal_selection_with(
        &mut self,
        column: u16,
        absolute_row: u16,
        update: impl FnOnce(&mut SessionRuntime, GridPoint) -> bool,
    ) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            return Ok(self.projection());
        };
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let _ = update(
            runtime,
            GridPoint {
                column,
                row: absolute_row,
            },
        );
        Ok(self.projection())
    }

    fn active_terminal_runtime(&self) -> Option<&SessionRuntime> {
        self.active_session_id
            .as_ref()
            .and_then(|session_key| self.sessions.get(session_key))
    }

    pub fn resize_active_terminal(&mut self, columns: u16, rows: u16) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        self.dispatcher
            .dispatch(SessionCommand::ResizeTerminal {
                session_id,
                columns,
                rows,
            })
            .map_err(AppError::from_error)?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let _ = runtime
            .resize_shell_pty(columns, rows)
            .map_err(AppError::from_error)?;
        // 只有已连接的会话才用"已调整终端尺寸"更新状态栏：断开/失败的会话
        // （例如密码错误的连接失败）需要把失败原因留在状态栏直到用户下一步操作。
        let session_connected = self
            .sessions
            .get(&session_key)
            .is_some_and(|session| session.state == yshell_core::SessionState::Connected);
        if session_connected {
            self.status_text = format!("Resized runtime terminal to {}x{}.", columns, rows);
        }
        Ok(self.projection())
    }

    pub fn sync_active_terminal_size_passive(
        &mut self,
        columns: u16,
        rows: u16,
    ) -> AppResult<Option<AppProjection>> {
        let Some(session_key) = self.active_session_id.clone() else {
            return Ok(None);
        };
        let current_size = self
            .sessions
            .get(&session_key)
            .map(SessionRuntime::current_pty_size)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        if current_size.columns == columns && current_size.rows == rows {
            return Ok(None);
        }
        self.resize_active_terminal(columns, rows).map(Some)
    }

    pub fn disconnect_active_session(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            runtime.disconnect_shell().map_err(AppError::from_error)?;
        }
        let events = self
            .dispatcher
            .dispatch(SessionCommand::SetSessionState {
                session_id: session_id.clone(),
                state: yshell_core::SessionState::Disconnected,
            })
            .map_err(AppError::from_error)?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        Self::apply_session_events(&session_id, runtime, events);
        let sftp_status = self.sync_sftp_lifecycle_for_session(&session_key, false);
        self.status_text = format!("Disconnected the active runtime session. {sftp_status}");
        Ok(self.projection())
    }

    pub fn reconnect_active_session(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let ssh_config = self
            .sessions
            .get(&session_key)
            .map(|session| session.ssh_config.clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;

        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            runtime.append_status_line("Reconnecting runtime shell session.");
            runtime.transport_backend = self.transport_backend;
        }
        let connecting_events = self
            .dispatcher
            .dispatch(SessionCommand::SetSessionState {
                session_id: session_id.clone(),
                state: yshell_core::SessionState::Connecting,
            })
            .map_err(AppError::from_error)?;
        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            Self::apply_session_events(&session_id, runtime, connecting_events);
        }

        let shell_session = match self.open_shell_for_runtime(&ssh_config) {
            Ok(shell_session) => shell_session,
            Err(error) => {
                return self.handle_runtime_shell_open_error(&session_key, &session_id, error);
            }
        };
        let shell_connected = shell_session.is_connected();
        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            runtime.attach_shell_session(shell_session);
                let _ = runtime.poll_shell_output().map_err(AppError::from_error)?;
                if !shell_connected {
                    runtime.append_status_line(
                        "Shell runtime reopened a non-live backend path. Either later SSH stages are still scaffolded, or the live shell has already exited.",
                    );
                }
                if let Some(notice) = runtime.take_logging_notice() {
                    self.fold_logging_notice_into_status(notice);
                }
            }
        if shell_connected {
            let connected_events = self
                .dispatcher
                .dispatch(SessionCommand::SetSessionState {
                    session_id: session_id.clone(),
                    state: yshell_core::SessionState::Connected,
                })
                .map_err(AppError::from_error)?;
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            Self::apply_session_events(&session_id, runtime, connected_events);
        }
        self.status_text = format!(
            "{} the active runtime session through the `{}` shell backend.",
            if shell_connected {
                "Reconnected"
            } else {
                "Reopened"
            },
            self.transport_backend.label()
        );
        let sftp_status = self.sync_sftp_lifecycle_for_session(&session_key, shell_connected);
        self.status_text = format!("{} {}", self.status_text, sftp_status);
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub fn toggle_sftp(&mut self) -> AppProjection {
        self.sftp_visible = !self.sftp_visible;
        self.status_text = if self.sftp_visible {
            "SFTP panel shown".to_owned()
        } else {
            "SFTP panel hidden".to_owned()
        };
        self.projection()
    }

    pub fn toggle_tunnels(&mut self) -> AppProjection {
        self.tunnels_visible = !self.tunnels_visible;
        self.status_text = if self.tunnels_visible {
            "Tunnels panel shown".to_owned()
        } else {
            "Tunnels panel hidden".to_owned()
        };
        self.projection()
    }

    pub fn toggle_commands(&mut self) -> AppProjection {
        self.commands_visible = !self.commands_visible;
        self.status_text = if self.commands_visible {
            "Quick Commands panel shown".to_owned()
        } else {
            "Quick Commands panel hidden".to_owned()
        };
        self.projection()
    }

    pub fn saved_session_profiles(&self) -> Vec<SessionProfile> {
        let mut profiles = Vec::new();
        collect_session_profiles(&self.config_document.folders, &mut profiles);
        profiles
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

    fn startup_status(&self) -> String {
        let saved_count = self.saved_session_count();
        let keychain_status = if self.keychain.is_some() {
            "secret store: enabled"
        } else {
            "secret store: disabled"
        };
        match &self.recovered_from_backup {
            Some(path) => format!(
                "Recovered configuration from backup. Saved sessions: {}. {}. Backup: {}",
                saved_count,
                keychain_status,
                path.display()
            ),
            None => format!(
                "Runtime initialized. Config: {}. Saved sessions discovered: {}. {}.",
                self.config_dir.display(),
                saved_count,
                keychain_status
            ),
        }
    }

    fn select_transport_backend(&mut self, backend: TransportBackend) -> AppProjection {
        self.transport_backend = backend;
        self.status_text = match backend {
            TransportBackend::Fake => {
                self.sftp_session = SftpSessionLifecycle::Unavailable {
                    reason: SftpUnavailableReason::FakeBackendSelected,
                };
                self.sftp_listing = SftpListingState::FakeBackendSelected;
                "Transport backend set to `fake`. Quick Connect will use the deterministic in-process shell adapter.".to_owned()
            }
            TransportBackend::Real => {
                if let Some(session_key) = self.active_session_id.clone() {
                    let _ = self.sync_sftp_lifecycle_for_session(&session_key, false);
                } else {
                    self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
                    self.sftp_listing = SftpListingState::NativeSshSelected;
                }
                "Transport backend set to `native-ssh`. Quick Connect will use the embedded ssh2 shell path, and the SFTP panel will use the embedded ssh2 SFTP path.".to_owned()
            }
        };
        self.status_text = format!(
            "{} {}",
            self.status_text,
            self.sftp_session.legacy_status_text()
        );
        self.projection()
    }

    fn active_session_parts(&self) -> (&'static str, String, String) {
        match self
            .active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
        {
            Some(session) => (
                "session",
                session.display_name.clone(),
                session.state_label().to_owned(),
            ),
            None if self.saved_session_count() == 0 => ("welcome", String::new(), String::new()),
            None => ("saved-sessions", String::new(), String::new()),
        }
    }

    fn session_summary_parts(&self) -> (String, usize) {
        let lines = self
            .sessions
            .values()
            .take(6)
            .map(SessionRuntime::sidebar_summary)
            .collect::<Vec<_>>();
        let hidden = self.sessions.len().saturating_sub(lines.len());
        (lines.join("\n"), hidden)
    }

    fn saved_session_selection_parts(&self) -> (String, String, String, String) {
        match &self.selected_saved_session_id {
            Some(profile_id) => self
                .config_document
                .find_session(profile_id)
                .map(|profile| {
                    (
                        "profile".to_owned(),
                        profile.name.clone(),
                        profile.host.clone(),
                        String::new(),
                    )
                })
                .unwrap_or_else(|| {
                    (
                        "missing".to_owned(),
                        String::new(),
                        String::new(),
                        profile_id.clone(),
                    )
                }),
            None => (
                "none".to_owned(),
                String::new(),
                String::new(),
                String::new(),
            ),
        }
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    fn saved_session_selection_legacy_text(&self) -> String {
        let (kind, name, host, id) = self.saved_session_selection_parts();
        match kind.as_str() {
            "profile" => format!("{name} ({host})"),
            "missing" => format!("{id} (missing)"),
            _ => "No saved session selected".to_owned(),
        }
    }

    fn saved_session_inventory_parts(&self) -> (String, bool) {
        let mut lines = Vec::new();
        collect_session_inventory_lines(
            &self.config_document.folders,
            0,
            self.selected_saved_session_id.as_deref(),
            self.session_search_query.trim(),
            &mut lines,
        );
        if lines.is_empty() {
            return (String::new(), true);
        }
        (
            lines.into_iter().take(12).collect::<Vec<_>>().join("\n"),
            false,
        )
    }

    /// Flattens the saved-session tree into the rows the sidebar renders.
    ///
    /// Search behaves like the legacy inventory: a matching folder shows its
    /// whole subtree, a matching session keeps its ancestor folder chain.
    fn session_tree_rows(&self) -> Vec<SessionTreeRow> {
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

    fn filtered_saved_session_profiles(&self) -> Vec<SessionProfile> {
        let mut profiles = Vec::new();
        collect_filtered_session_profiles(
            &self.config_document.folders,
            self.session_search_query.trim(),
            &mut profiles,
        );
        profiles
    }

    fn recent_sessions_parts(&self) -> (String, bool) {
        if self.recent_session_ids.is_empty() {
            return (String::new(), true);
        }
        let rows = self
            .recent_session_ids
            .iter()
            .take(5)
            .filter_map(|session_id| self.sessions.get(session_id))
            .map(SessionRuntime::sidebar_summary)
            .collect::<Vec<_>>()
            .join("\n");
        (rows, false)
    }

    fn known_host_entries(&self) -> Vec<KnownHostEntryInfo> {
        let mut entries = self
            .persistent_known_hosts
            .snapshot()
            .into_iter()
            .filter_map(|(key, fingerprint)| {
                let (host, port_text) = key.rsplit_once(':')?;
                let host = host.to_owned();
                let port = port_text.parse::<u16>().ok()?;
                Some(KnownHostEntryInfo {
                    key,
                    host,
                    port,
                    algorithm: fingerprint.algorithm,
                    fingerprint: fingerprint.fingerprint,
                })
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.key.cmp(&right.key));
        entries
    }

    fn ensure_known_hosts_selection(&mut self) {
        let entries = self.known_host_entries();
        if entries.is_empty() {
            self.known_hosts_selected_key = None;
            return;
        }
        let still_valid = self
            .known_hosts_selected_key
            .as_ref()
            .is_some_and(|selected| entries.iter().any(|entry| &entry.key == selected));
        if !still_valid {
            self.known_hosts_selected_key = Some(entries[0].key.clone());
        }
    }

    fn selected_known_host_entry(&self) -> Option<KnownHostEntryInfo> {
        let selected = self.known_hosts_selected_key.as_ref()?;
        self.known_host_entries()
            .into_iter()
            .find(|entry| &entry.key == selected)
    }

    fn known_hosts_inventory_rows_text(&self) -> String {
        let entries = self.known_host_entries();
        entries
            .iter()
            .map(|entry| {
                let marker = if self
                    .known_hosts_selected_key
                    .as_ref()
                    .is_some_and(|selected| selected == &entry.key)
                {
                    ">"
                } else {
                    " "
                };
                format!(
                    "{marker} {}:{} [{}]",
                    entry.host, entry.port, entry.algorithm
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn known_hosts_inventory_empty(&self) -> bool {
        self.known_host_entries().is_empty()
    }

    fn known_hosts_selection_kind(&self) -> &'static str {
        let count = self.known_host_entries().len();
        match self.selected_known_host_entry() {
            Some(_) => "selected",
            None if count == 0 => "empty",
            None => "loaded",
        }
    }

    fn known_hosts_selection_host_text(&self) -> String {
        self.selected_known_host_entry()
            .map(|entry| entry.host)
            .unwrap_or_default()
    }

    fn known_hosts_selection_port_text(&self) -> String {
        self.selected_known_host_entry()
            .map(|entry| entry.port.to_string())
            .unwrap_or_default()
    }

    fn known_hosts_selection_index(&self) -> i32 {
        let Some(entry) = self.selected_known_host_entry() else {
            return 0;
        };
        i32::try_from(
            self.known_host_entries()
                .iter()
                .position(|candidate| candidate.key == entry.key)
                .map(|index| index + 1)
                .unwrap_or(1),
        )
        .unwrap_or(i32::MAX)
    }

    fn known_hosts_selection_total(&self) -> i32 {
        i32::try_from(self.known_host_entries().len()).unwrap_or(i32::MAX)
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    fn known_hosts_selection_legacy_text(&self) -> String {
        match self.known_hosts_selection_kind() {
            "selected" => format!(
                "Selected known host {}:{} ({} of {}).",
                self.known_hosts_selection_host_text(),
                self.known_hosts_selection_port_text(),
                self.known_hosts_selection_index(),
                self.known_hosts_selection_total()
            ),
            "empty" => "No persisted known hosts are available.".to_owned(),
            _ => format!(
                "Known hosts manager loaded {} entries.",
                self.known_hosts_selection_total()
            ),
        }
    }

    fn known_hosts_details_text(&self) -> String {
        match self.selected_known_host_entry() {
            Some(entry) => format!(
                "Host: {}\nPort: {}\nAlgorithm: {}\nFingerprint: {}",
                entry.host, entry.port, entry.algorithm, entry.fingerprint
            ),
            None => format!(
                "Known hosts path: {}\nTemporary trust entries are not persisted here.",
                self.config_store.known_hosts_file().display()
            ),
        }
    }

    fn editor_target_parts(&self) -> (bool, String) {
        match &self.editor.target_session_id {
            Some(id) => (true, id.clone()),
            None => (false, String::new()),
        }
    }

    fn editor_tunnel_summary_rows_text(&self) -> String {
        self.editor
            .tunnels
            .iter()
            .take(4)
            .map(tunnel_forward_summary)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn logging_enabled_text(&self) -> String {
        if self.config_document.logging.enabled {
            "enabled".to_owned()
        } else {
            "disabled".to_owned()
        }
    }

    fn logging_format_text(&self) -> String {
        if self.config_document.logging.format.eq_ignore_ascii_case("raw") {
            "raw".to_owned()
        } else {
            "sanitized".to_owned()
        }
    }

    fn logging_directory_text(&self) -> String {
        self.config_document
            .logging
            .directory
            .clone()
            .unwrap_or_default()
    }

    fn logging_directory_display_text(&self) -> String {
        self.config_document
            .logging
            .directory
            .as_deref()
            .filter(|directory| !directory.trim().is_empty())
            .unwrap_or("logs")
            .to_owned()
    }

    fn editor_proxy_summary_parts(&self) -> (String, String, String) {
        match self.editor.proxy_mode {
            EditorProxyMode::None => ("none".to_owned(), String::new(), String::new()),
            EditorProxyMode::Custom => {
                let user = if self.editor.proxy_username.trim().is_empty() {
                    "<none>".to_owned()
                } else {
                    self.editor.proxy_username.trim().to_owned()
                };
                if self.editor.proxy_host.trim().is_empty()
                    || self.editor.proxy_port_text.trim().is_empty()
                {
                    ("incomplete".to_owned(), String::new(), user)
                } else {
                    (
                        "custom".to_owned(),
                        format!(
                            "{}:{}",
                            self.editor.proxy_host.trim(),
                            self.editor.proxy_port_text.trim()
                        ),
                        user,
                    )
                }
            }
        }
    }

    fn editor_folder_parts(&self) -> (String, String, bool) {
        self.editor_folder_choices()
            .into_iter()
            .find(|choice| choice.id == self.editor.target_folder_id)
            .map(|choice| (choice.path_label, choice.id, true))
            .unwrap_or_else(|| (String::new(), self.editor.target_folder_id.clone(), false))
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    fn editor_folder_legacy_text(&self) -> String {
        let (label, id, known) = self.editor_folder_parts();
        if known {
            format!("{label} ({id})")
        } else {
            format!("Unknown folder ({id})")
        }
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    fn logging_summary_legacy_text(&self) -> String {
        format!(
            "global={} format={} directory={}",
            self.logging_enabled_text(),
            self.logging_format_text(),
            self.logging_directory_display_text()
        )
    }

    fn editor_from_resolved_session(&self, resolved: &ResolvedSessionProfile) -> SessionEditorDraft {
        let mut draft = SessionEditorDraft {
            target_session_id: Some(resolved.session.id.clone()),
            target_folder_id: find_session_folder_id(&self.config_document.folders, &resolved.session.id)
                .unwrap_or_else(|| SAVED_SESSIONS_FOLDER_ID.to_owned()),
            name: resolved.session.name.clone(),
            host: resolved.session.host.clone(),
            port_text: resolved.session.port.to_string(),
            username: resolved.session.username.clone().unwrap_or_default(),
            auth_method: EditorAuthMethod::Agent,
            host_key_policy: resolved
                .session
                .host_key_policy
                .unwrap_or(ConfigHostKeyPolicy::Strict),
            password: String::new(),
            key_path: String::new(),
            passphrase: String::new(),
            proxy_mode: EditorProxyMode::None,
            proxy_protocol: ProxyProtocol::Socks5,
            proxy_host: String::new(),
            proxy_port_text: String::new(),
            proxy_username: String::new(),
            proxy_password: String::new(),
            proxy_dns_by_proxy: true,
            tunnel_kind: TunnelForwardKind::Local,
            tunnel_bind_host: "127.0.0.1".to_owned(),
            tunnel_bind_port_text: String::new(),
            tunnel_target_host: String::new(),
            tunnel_target_port_text: String::new(),
            tunnels: resolved.tunnel.forwards.clone(),
        };
        if let Some(auth) = &resolved.auth {
            match &auth.method {
                ConfigAuthMethod::Agent => {
                    draft.auth_method = EditorAuthMethod::Agent;
                }
                ConfigAuthMethod::Password { .. } => {
                    draft.auth_method = EditorAuthMethod::Password;
                }
                ConfigAuthMethod::KeyboardInteractive { .. } => {
                    draft.auth_method = EditorAuthMethod::KeyboardInteractive;
                }
                ConfigAuthMethod::PrivateKey { path, .. } => {
                    draft.auth_method = EditorAuthMethod::PrivateKey;
                    draft.key_path = path.clone();
                }
            }
        }
        if let Some(proxy) = &resolved.proxy {
            draft.proxy_mode = EditorProxyMode::Custom;
            draft.proxy_protocol = proxy.protocol;
            draft.proxy_host = proxy.host.clone();
            draft.proxy_port_text = proxy.port.to_string();
            draft.proxy_username = proxy.username.clone().unwrap_or_default();
            draft.proxy_dns_by_proxy = proxy.resolve_dns_by_proxy;
        }
        draft
    }

    fn tunnels_summary_parts(&self) -> (&'static str, String) {
        match self
            .active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
        {
            Some(session) => {
                if session.ssh_config.tunnels.is_empty() {
                    ("none", String::new())
                } else {
                    let rows = session
                        .ssh_config
                        .tunnels
                        .iter()
                        .take(4)
                        .map(tunnel_config_summary)
                        .collect::<Vec<_>>()
                        .join("\n");
                    ("rows", rows)
                }
            }
            None => ("no-active", String::new()),
        }
    }

    fn tab_parts(&self) -> (bool, String, String) {
        match self
            .active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
        {
            Some(session) => (
                true,
                session.display_name.clone(),
                session.state_label().to_owned(),
            ),
            None => (false, String::new(), "idle".to_owned()),
        }
    }

    fn terminal_title_parts(&self) -> (bool, String) {
        match self
            .active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
        {
            Some(session) => (true, session.display_name.clone()),
            None => (false, String::new()),
        }
    }

    fn terminal_body_parts(&self) -> (&'static str, String) {
        match self
            .active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
        {
            Some(session) => {
                let visible = session.visible_text();
                if visible.trim().is_empty() {
                    ("no-output", String::new())
                } else {
                    ("data", visible)
                }
            }
            None => ("ready", String::new()),
        }
    }

    fn terminal_visible_lines(&self) -> Vec<String> {
        self.active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
            .map(|session| {
                let lines = session.visible_lines();
                if lines.iter().all(|line| line.is_empty()) {
                    vec!["Terminal runtime is initialized but no output is available yet.".to_owned()]
                } else {
                    lines
                }
            })
            .unwrap_or_else(|| {
                vec!["Runtime is ready. Open a Quick Connect target or create a draft session.".to_owned()]
            })
    }

    fn terminal_cursor_column(&self) -> i32 {
        self.active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
            .map(|session| i32::from(session.cursor_position().0))
            .unwrap_or(0)
    }

    fn terminal_cursor_row(&self) -> i32 {
        self.active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
            .map(|session| i32::from(session.cursor_position().1))
            .unwrap_or(0)
    }

    fn host_key_prompt_text(&self) -> String {
        let Some(prompt) = &self.pending_host_key_prompt else {
            return String::new();
        };
        match &prompt.expected {
            Some(expected) => format!(
                "Host key changed for {}@{}:{}.\nKnown hosts: {}\nExpected: {} {}\nPresented: {} {}\nType REPLACE to enable replacement.",
                prompt.username,
                prompt.host,
                prompt.port,
                prompt.known_hosts_path.display(),
                expected.algorithm,
                expected.fingerprint,
                prompt.presented.algorithm,
                prompt.presented.fingerprint
            ),
            None => format!(
                "First-time host key for {}@{}:{}.\nKnown hosts: {}\nPresented: {} {}\nChoose Trust Once or Trust and Save.",
                prompt.username,
                prompt.host,
                prompt.port,
                prompt.known_hosts_path.display(),
                prompt.presented.algorithm,
                prompt.presented.fingerprint
            ),
        }
    }

    fn host_key_prompt_mode_text(&self) -> &'static str {
        match self.pending_host_key_prompt.as_ref().map(PendingHostKeyPrompt::mode) {
            Some(HostKeyPromptMode::FirstTrust) => "first-trust",
            Some(HostKeyPromptMode::Changed) => "changed",
            None => "",
        }
    }

    /// Visible rows of the last listing (hidden filter + directory-first sort).
    fn visible_sftp_entries(&self) -> Vec<FsEntry> {
        visible_entries(
            &self.sftp_entries,
            self.sftp_show_hidden,
            self.sftp_sort_column,
            self.sftp_sort_ascending,
        )
    }

    /// Selected row resolved against the visible list.
    fn selected_sftp_entry(&self) -> Option<FsEntry> {
        let index = self.sftp_selected_index?;
        self.visible_sftp_entries().into_iter().nth(index)
    }

    /// Re-resolves the selection by path (used after sorting/filtering).
    fn reconcile_sftp_selection(&mut self, preferred_path: Option<&str>) {
        let visible = self.visible_sftp_entries();
        self.sftp_selected_index =
            preferred_path.and_then(|path| visible.iter().position(|entry| entry.path == path));
    }

    /// Selects `path` when it is part of the visible list, keeping the current
    /// selection otherwise.
    fn reselect_sftp_path(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        if let Some(index) = self
            .visible_sftp_entries()
            .iter()
            .position(|entry| entry.path == path)
        {
            self.sftp_selected_index = Some(index);
        }
    }

    /// Resolves a user supplied remote path against the current directory.
    fn sftp_relative_or_absolute_path(&self, target: &str) -> String {
        if target.starts_with('/') {
            normalize_remote_path(target).unwrap_or_else(|_| target.to_owned())
        } else {
            join_remote_path(&self.sftp_path, target)
        }
    }

    fn sftp_rows(&self) -> Vec<SftpRowData> {
        self.visible_sftp_entries()
            .iter()
            .map(|entry| {
                let is_dir = matches!(entry.kind, FsEntryKind::Directory);
                SftpRowData {
                    name: entry.name.clone(),
                    kind_text: sftp_kind_text(entry.kind).to_owned(),
                    size_text: if is_dir {
                        String::new()
                    } else {
                        format_sftp_size(entry.size_bytes)
                    },
                    modified_text: format_sftp_modified(entry.modified),
                    permissions_text: format_sftp_permissions(entry.kind, entry.permissions),
                    is_dir,
                }
            })
            .collect()
    }

    fn sftp_crumbs(&self) -> Vec<SftpCrumbData> {
        let mut crumbs = vec![SftpCrumbData {
            label: "/".to_owned(),
            path: "/".to_owned(),
        }];
        let normalized = normalize_remote_path(&self.sftp_path).unwrap_or_else(|_| "/".to_owned());
        let mut path = String::new();
        for part in normalized.split('/').filter(|part| !part.is_empty()) {
            path.push('/');
            path.push_str(part);
            crumbs.push(SftpCrumbData {
                label: part.to_owned(),
                path: path.clone(),
            });
        }
        crumbs
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    fn sftp_sort_legacy_text(&self) -> String {
        let arrow = if self.sftp_sort_ascending {
            "↑"
        } else {
            "↓"
        };
        format!("{} {arrow}", self.sftp_sort_column.label())
    }

    fn sftp_item_summary_text(&self) -> String {
        let (items, directories) = self.sftp_visible_counts();
        format!(
            "{items} {} · {directories} {}",
            if items == 1 { "item" } else { "items" },
            if directories == 1 { "dir" } else { "dirs" }
        )
    }

    /// Visible entry/directory counts backing the SFTP summary row.
    /// Slint renders these through `@tr` plural templates (`sftp_panel.slint`).
    fn sftp_visible_counts(&self) -> (usize, usize) {
        let visible = self.visible_sftp_entries();
        let directories = visible
            .iter()
            .filter(|entry| matches!(entry.kind, FsEntryKind::Directory))
            .count();
        (visible.len(), directories)
    }

    /// Status bar tail for Slint: covered producers record `kind` + params, and
    /// any later mutation of `status_text` (append/error paths) invalidates the
    /// snapshot so the UI falls back to the English `status_text`.
    fn status_i18n_parts(&self) -> (String, String, String) {
        if self.status_kind.is_empty() || self.status_text != self.status_kind_source {
            return (String::new(), String::new(), String::new());
        }
        (
            self.status_kind.clone(),
            self.status_param_1.clone(),
            self.status_param_2.clone(),
        )
    }

    /// Records an i18n status bar message: `text` stays the English fallback
    /// (CLI/tests/append channel), while Slint re-renders `kind` + params.
    fn set_status_kind(&mut self, kind: &str, text: String, param_1: String, param_2: String) {
        self.status_text = text;
        self.status_kind = kind.to_owned();
        self.status_param_1 = param_1;
        self.status_param_2 = param_2;
        self.status_kind_source = self.status_text.clone();
    }

    /// Message shown inside the file list when it has no rows to draw.
    fn sftp_empty_text(&self) -> String {
        if !self.visible_sftp_entries().is_empty() {
            return String::new();
        }
        if self.sftp_entries.is_empty() {
            // Slint falls back to the translated listing state for this case.
            return String::new();
        }
        if !self.sftp_show_hidden {
            return "No visible files. Enable Hidden to include dotfiles.".to_owned();
        }
        "This directory is empty.".to_owned()
    }

    fn sftp_selected_name_text(&self) -> String {
        self.selected_sftp_entry()
            .map(|entry| entry.name)
            .unwrap_or_default()
    }

    fn sftp_selected_path_text(&self) -> String {
        self.selected_sftp_entry()
            .map(|entry| entry.path)
            .unwrap_or_default()
    }

    fn sftp_selected_permissions_text(&self) -> String {
        self.selected_sftp_entry()
            .map(|entry| format_sftp_permissions_octal(entry.permissions))
            .unwrap_or_default()
    }

    fn apply_sftp_listing(&mut self, listing: &DirectoryListing) {
        self.sftp_path = listing.path.clone();
        let selected = self.selected_sftp_entry().map(|entry| entry.path);
        self.sftp_entries = listing.entries.clone();
        self.reconcile_sftp_selection(selected.as_deref());
        if listing.entries.is_empty() {
            self.sftp_listing = SftpListingState::EmptyDirectory;
            return;
        }
        if self.sftp_remote_target.trim().is_empty() {
            if let Some(first_file) = listing
                .entries
                .iter()
                .find(|entry| matches!(entry.kind, FsEntryKind::File))
            {
                self.sftp_remote_target = first_file.path.clone();
            }
        }
        self.sftp_listing = SftpListingState::Rows(
            listing
                .entries
                .iter()
                .map(|entry| {
                    let prefix = match entry.kind {
                        FsEntryKind::Directory => "[dir]",
                        FsEntryKind::File => "[file]",
                        FsEntryKind::Symlink => "[link]",
                        FsEntryKind::Other => "[other]",
                    };
                    if matches!(entry.kind, FsEntryKind::File) {
                        format!("{prefix} {} ({} B)", entry.name, entry.size_bytes)
                    } else {
                        format!("{prefix} {}", entry.name)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    fn sftp_upload_target(&self) -> AppResult<String> {
        if !self.sftp_remote_target.trim().is_empty() {
            return normalize_remote_path(&self.sftp_remote_target);
        }
        let filename = Path::new(self.sftp_local_path.trim())
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| AppError::new("could not infer remote upload target from local file name"))?;
        Ok(join_remote_path(&self.sftp_path, filename))
    }

    fn sftp_download_target(&self) -> AppResult<String> {
        if self.sftp_remote_target.trim().is_empty() {
            return Err(AppError::new(
                "remote SFTP target is empty; refresh a directory or type a remote file path first",
            ));
        }
        normalize_remote_path(&self.sftp_remote_target)
    }

    fn sftp_secondary_target_path(&self, empty_message: &str) -> AppResult<String> {
        let target = self.sftp_secondary_target.trim();
        if target.is_empty() {
            return Err(AppError::new(empty_message));
        }
        if target.starts_with('/') {
            normalize_remote_path(target)
        } else {
            Ok(join_remote_path(&self.sftp_path, target))
        }
    }

    fn hydrate_saved_sessions(&mut self) {
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

    fn allocate_runtime_ordinal(&mut self) -> usize {
        let ordinal = self.next_runtime_ordinal;
        self.next_runtime_ordinal += 1;
        ordinal
    }

    fn active_session_key(&self) -> AppResult<String> {
        self.active_session_id
            .clone()
            .ok_or_else(|| AppError::new("no active runtime session"))
    }

    fn open_shell_for_runtime(
        &self,
        ssh_config: &SshConnectionConfig,
    ) -> Result<Box<dyn yshell_ssh::ShellSession>, yshell_ssh::SshError> {
        let effective_config = self.effective_ssh_config(ssh_config);
        match self.transport_backend {
            TransportBackend::Fake => {
                ShellClient::with_fake_backend().open_shell_boxed(&effective_config)
            }
            TransportBackend::Real => {
                ShellClient::with_real_backend().open_shell_boxed(&effective_config)
            }
        }
    }

    fn effective_ssh_config(
        &self,
        ssh_config: &SshConnectionConfig,
    ) -> SshConnectionConfig {
        let mut effective_config = ssh_config.clone();
        let mut known_hosts = self.persistent_known_hosts.clone();
        known_hosts.merge(&self.temporary_known_hosts);
        effective_config.known_hosts = known_hosts;
        if let Some(policy) = &self.host_key_policy_override {
            effective_config.host_key_policy = policy.clone();
        }
        effective_config
    }

    fn reconnect_session_by_key(&mut self, session_key: &str) -> AppResult<AppProjection> {
        self.active_session_id = Some(session_key.to_owned());
        self.reconnect_active_session()
    }

    fn handle_runtime_shell_open_error(
        &mut self,
        session_key: &str,
        session_id: &yshell_core::SessionId,
        error: yshell_ssh::SshError,
    ) -> AppResult<AppProjection> {
        let handled = self.apply_host_key_prompt_from_error(
            session_key,
            self.sessions
                .get(session_key)
                .map(|session| session.username_label().to_owned())
                .unwrap_or_else(|| "user".to_owned()),
            &error,
        );
        if let Some(runtime) = self.sessions.get_mut(session_key) {
            runtime.append_status_line(&format!(
                "Shell runtime failed while reconnecting: {error}"
            ));
        }
        let failed_events = self
            .dispatcher
            .dispatch(SessionCommand::SetSessionState {
                session_id: session_id.clone(),
                state: yshell_core::SessionState::Failed,
            })
            .map_err(AppError::from_error)?;
        if let Some(runtime) = self.sessions.get_mut(session_key) {
            Self::apply_session_events(session_id, runtime, failed_events);
        }
        self.status_text = if handled {
            "Host key confirmation is required before reconnecting the native SSH session.".to_owned()
        } else {
            format!("Reconnect failed: {error}")
        };
        self.sftp_session = SftpSessionLifecycle::Failed {
            session_key: Some(session_key.to_owned()),
            reason: "shell reconnect failed before SFTP could attach".to_owned(),
        };
        self.sftp_listing = SftpListingState::ReconnectFailed;
        self.status_text = format!(
            "{} {}",
            self.status_text,
            self.sftp_session.legacy_status_text()
        );
        Ok(self.projection())
    }

    fn apply_host_key_prompt_from_error(
        &mut self,
        session_key: &str,
        username: String,
        error: &yshell_ssh::SshError,
    ) -> bool {
        let Some(problem) = &error.host_key_problem else {
            return false;
        };
        let prompt = match problem.as_ref() {
            HostKeyProblem::Unknown {
                host,
                port,
                presented,
            } => PendingHostKeyPrompt {
                session_key: session_key.to_owned(),
                host: host.clone(),
                port: *port,
                username,
                presented: presented.clone(),
                expected: None,
                known_hosts_path: self.config_store.known_hosts_file(),
            },
            HostKeyProblem::Changed {
                host,
                port,
                presented,
                expected,
            } => PendingHostKeyPrompt {
                session_key: session_key.to_owned(),
                host: host.clone(),
                port: *port,
                username,
                presented: presented.clone(),
                expected: Some(expected.clone()),
                known_hosts_path: self.config_store.known_hosts_file(),
            },
        };
        self.pending_host_key_prompt = Some(prompt);
        self.host_key_replace_confirmation.clear();
        true
    }

    /// W5：已保存会话的密码型认证在密钥库里没有可用密码时，返回挂起目标（该弹窗）。
    ///
    /// 只覆盖 password / keyboard-interactive 的**认证密钥**：私钥口令、代理密码等
    /// 其它缺失仍走原有错误路径（弹窗文案只讲"密码"）。
    fn pending_password_prompt_for_runtime(
        &self,
        runtime: &SessionRuntime,
    ) -> Option<PendingPasswordPrompt> {
        let SessionSource::SavedSession { profile_id } = &runtime.source else {
            return None;
        };
        let resolved = self.config_document.resolve_session(profile_id)?;
        let auth = resolved.auth.as_ref()?;
        let (secret_key, auth_method) = password_prompt_auth_method(auth)?;
        // 密钥库里有可用密码（或没有密钥库）都返回 None；后者只有真的取不到密钥时才挂起。
        if self.resolve_secret_value(secret_key).is_ok() {
            return None;
        }
        Some(PendingPasswordPrompt {
            profile_id: profile_id.clone(),
            host: resolved.session.host.clone(),
            port: resolved.session.port,
            username: resolved
                .session
                .username
                .clone()
                .unwrap_or_else(|| "user".to_owned()),
            auth_method,
        })
    }

    /// W5：用弹窗里的密码重建目标会话并连接（仅本次使用，不写密钥库）。
    fn retry_pending_password_connection(
        &mut self,
        prompt: &PendingPasswordPrompt,
        password: &str,
    ) -> AppResult<AppProjection> {
        let profile = self
            .config_document
            .find_session(&prompt.profile_id)
            .cloned()
            .ok_or_else(|| {
                AppError::new(format!(
                    "saved session `{}` was not found",
                    prompt.profile_id
                ))
            })?;
        let resolved = self
            .config_document
            .resolve_session(&prompt.profile_id)
            .ok_or_else(|| {
                AppError::new(format!(
                    "saved session `{}` could not be resolved from config",
                    prompt.profile_id
                ))
            })?;
        // 挂起后配置可能被改动：认证方式不一致就不要再把密码发给新目标。
        let current_method = resolved
            .auth
            .as_ref()
            .and_then(password_prompt_auth_method)
            .map(|(_, method)| method)
            .ok_or_else(|| {
                AppError::new(format!(
                    "saved session `{}` no longer uses password authentication; connect again",
                    prompt.profile_id
                ))
            })?;
        if current_method != prompt.auth_method {
            return Err(AppError::new(format!(
                "saved session `{}` changed its authentication method; connect again",
                prompt.profile_id
            )));
        }
        let ssh_config =
            self.build_ssh_config_from_resolved_session_with_password(&resolved, password)?;
        let runtime = SessionRuntime::from_profile(&profile, self.allocate_runtime_ordinal());
        // 先清挂起状态再连接：连接路径返回的投影必须显示弹窗已关闭。
        self.pending_password_prompt = None;
        self.activate_runtime_session_after_password(runtime, ssh_config)
    }

    /// 密码弹窗文案里的 `user@host:port`（无挂起目标时为空串）。
    fn password_prompt_host_text(&self) -> String {
        self.pending_password_prompt
            .as_ref()
            .map(PendingPasswordPrompt::host_text)
            .unwrap_or_default()
    }

    fn resolve_runtime_ssh_config(&self, runtime: &SessionRuntime) -> AppResult<SshConnectionConfig> {
        match &runtime.source {
            SessionSource::SavedSession { profile_id } => {
                let resolved = self
                    .config_document
                    .resolve_session(profile_id)
                    .ok_or_else(|| {
                        AppError::new(format!(
                            "saved session `{profile_id}` could not be resolved from config"
                        ))
                    })?;
                self.build_ssh_config_from_resolved_session(&resolved)
            }
            SessionSource::QuickConnect | SessionSource::Draft => Ok(runtime.ssh_config.clone()),
        }
    }

    fn build_ssh_config_from_resolved_session(
        &self,
        resolved: &ResolvedSessionProfile,
    ) -> AppResult<SshConnectionConfig> {
        self.build_ssh_config_from_resolved_session_inner(resolved, None)
    }

    /// W5：`password` 非空时替代密钥库里的密码型密钥（仅本次连接使用，不写盘）。
    fn build_ssh_config_from_resolved_session_with_password(
        &self,
        resolved: &ResolvedSessionProfile,
        password: &str,
    ) -> AppResult<SshConnectionConfig> {
        self.build_ssh_config_from_resolved_session_inner(resolved, Some(password))
    }

    fn build_ssh_config_from_resolved_session_inner(
        &self,
        resolved: &ResolvedSessionProfile,
        password_override: Option<&str>,
    ) -> AppResult<SshConnectionConfig> {
        let username = resolved
            .session
            .username
            .clone()
            .unwrap_or_else(|| "user".to_owned());
        let auth = match resolved.auth.as_ref().map(|profile| &profile.method) {
            Some(ConfigAuthMethod::Password { secret_key }) => AuthMethod::Password {
                username,
                password: self.resolve_secret_value_with_override(secret_key, password_override)?,
            },
            Some(ConfigAuthMethod::KeyboardInteractive { secret_key }) => {
                AuthMethod::KeyboardInteractive {
                    username,
                    secret: self
                        .resolve_secret_value_with_override(secret_key, password_override)?,
                }
            }
            Some(ConfigAuthMethod::PrivateKey {
                path,
                passphrase_secret_key,
            }) => AuthMethod::PrivateKey {
                username,
                key_path: path.clone(),
                passphrase: passphrase_secret_key
                    .as_ref()
                    .map(|secret_key| self.resolve_secret_value(secret_key))
                    .transpose()?,
            },
            Some(ConfigAuthMethod::Agent) | None => AuthMethod::Agent { username },
        };
        let mut config = SshConnectionConfig::new(
            resolved.session.host.clone(),
            resolved.session.port,
            auth,
        );
        config.host_key_policy = resolved
            .session
            .host_key_policy
            .map(config_host_key_policy_to_runtime)
            .unwrap_or(HostKeyPolicy::Strict);
        config.proxy = self.build_proxy_config_from_resolved_session(resolved)?;
        config.tunnels = resolved
            .tunnel
            .forwards
            .iter()
            .cloned()
            .map(tunnel_forward_to_config)
            .collect();
        Ok(config)
    }

    fn build_ssh_config_from_editor(&self) -> AppResult<SshConnectionConfig> {
        let host = self.editor.host.trim();
        if host.is_empty() {
            return Err(AppError::new("editor host must not be empty"));
        }
        let port = self
            .editor
            .port_text
            .trim()
            .parse::<u16>()
            .map_err(|_| AppError::new("editor port must be a valid integer between 1 and 65535"))?;
        if port == 0 {
            return Err(AppError::new("editor port must be greater than zero"));
        }
        let username = if self.editor.username.trim().is_empty() {
            "user".to_owned()
        } else {
            self.editor.username.trim().to_owned()
        };
        let auth = match self.editor.auth_method {
            EditorAuthMethod::Agent => AuthMethod::Agent { username },
            EditorAuthMethod::Password => {
                if self.editor.password.trim().is_empty() {
                    return Err(AppError::new("password auth requires a password value"));
                }
                AuthMethod::Password {
                    username,
                    password: self.editor.password.trim().to_owned(),
                }
            }
            EditorAuthMethod::KeyboardInteractive => {
                if self.editor.password.trim().is_empty() {
                    return Err(AppError::new(
                        "keyboard-interactive auth requires a response value",
                    ));
                }
                AuthMethod::KeyboardInteractive {
                    username,
                    secret: self.editor.password.trim().to_owned(),
                }
            }
            EditorAuthMethod::PrivateKey => {
                if self.editor.key_path.trim().is_empty() {
                    return Err(AppError::new("private key auth requires a key path"));
                }
                AuthMethod::PrivateKey {
                    username,
                    key_path: self.editor.key_path.trim().to_owned(),
                    passphrase: if self.editor.passphrase.trim().is_empty() {
                        None
                    } else {
                        Some(self.editor.passphrase.trim().to_owned())
                    },
                }
            }
        };
        let mut config = SshConnectionConfig::new(
            host.to_owned(),
            port,
            auth,
        );
        config.host_key_policy = config_host_key_policy_to_runtime(self.editor.host_key_policy);
        config.proxy = self.build_proxy_config_from_editor()?;
        config.tunnels = self
            .editor
            .tunnels
            .iter()
            .cloned()
            .map(tunnel_forward_to_config)
            .collect();
        Ok(self.effective_ssh_config(&config))
    }

    fn build_proxy_config_from_resolved_session(
        &self,
        resolved: &ResolvedSessionProfile,
    ) -> AppResult<ProxyConfig> {
        let Some(proxy) = &resolved.proxy else {
            return Ok(ProxyConfig::None);
        };
        let address = format!("{}:{}", proxy.host, proxy.port);
        let username = proxy.username.clone();
        let password = proxy
            .password_secret_key
            .as_ref()
            .map(|secret_key| self.resolve_secret_value(secret_key))
            .transpose()?;
        Ok(match proxy.protocol {
            ProxyProtocol::Socks4 => ProxyConfig::Socks4 { address, username },
            ProxyProtocol::Socks4a => ProxyConfig::Socks4a { address, username },
            ProxyProtocol::Socks5 => ProxyConfig::Socks5 {
                address,
                username,
                password,
                resolve_dns_by_proxy: proxy.resolve_dns_by_proxy,
            },
            ProxyProtocol::HttpConnect => ProxyConfig::HttpConnect {
                address,
                username,
                password,
            },
        })
    }

    fn build_proxy_config_from_editor(&self) -> AppResult<ProxyConfig> {
        if self.editor.proxy_mode == EditorProxyMode::None {
            return Ok(ProxyConfig::None);
        }
        let host = self.editor.proxy_host.trim();
        if host.is_empty() {
            return Err(AppError::new("proxy host must not be empty"));
        }
        let port = parse_port_field(
            &self.editor.proxy_port_text,
            "proxy port must be a valid integer between 1 and 65535",
        )?;
        let address = format!("{host}:{port}");
        let username = if self.editor.proxy_username.trim().is_empty() {
            None
        } else {
            Some(self.editor.proxy_username.trim().to_owned())
        };
        let password = if self.editor.proxy_password.trim().is_empty() {
            None
        } else {
            Some(self.editor.proxy_password.trim().to_owned())
        };
        Ok(match self.editor.proxy_protocol {
            ProxyProtocol::Socks4 => ProxyConfig::Socks4 { address, username },
            ProxyProtocol::Socks4a => ProxyConfig::Socks4a { address, username },
            ProxyProtocol::Socks5 => ProxyConfig::Socks5 {
                address,
                username,
                password,
                resolve_dns_by_proxy: self.editor.proxy_dns_by_proxy,
            },
            ProxyProtocol::HttpConnect => ProxyConfig::HttpConnect {
                address,
                username,
                password,
            },
        })
    }

    fn resolve_secret_value(&self, secret_key: &str) -> AppResult<String> {
        let keychain = self.keychain.as_ref().ok_or_else(|| {
            AppError::new(format!(
                "saved session auth requires secret `{secret_key}`, but no keychain is configured"
            ))
        })?;
        keychain
            .0
            .get(&SecretRef::new(secret_key))
            .map(|secret| secret.expose_secret().to_owned())
            .map_err(AppError::from_error)
    }

    /// W5：密码弹窗重试用 —— 有用户输入的密码时优先，否则回退到密钥库。
    fn resolve_secret_value_with_override(
        &self,
        secret_key: &str,
        password_override: Option<&str>,
    ) -> AppResult<String> {
        match password_override {
            Some(password) => Ok(password.to_owned()),
            None => self.resolve_secret_value(secret_key),
        }
    }

    fn store_secret_value(&self, secret_key: &str, value: &str) -> AppResult<()> {
        let keychain = self.keychain.as_ref().ok_or_else(|| {
            AppError::new(format!(
                "saving secret `{secret_key}` requires an enabled secret store"
            ))
        })?;
        keychain
            .0
            .put(SecretRef::new(secret_key), value.into())
            .map_err(AppError::from_error)
    }

    fn build_auth_profile_for_editor(
        &self,
        auth_profile_id: &str,
    ) -> AppResult<yshell_config::AuthProfile> {
        let method = match self.editor.auth_method {
            EditorAuthMethod::Agent => ConfigAuthMethod::Agent,
            EditorAuthMethod::Password => {
                if self.editor.password.trim().is_empty() {
                    return Err(AppError::new("password auth requires a password value"));
                }
                let secret_key = format!("local://yshell/{auth_profile_id}/password");
                self.store_secret_value(&secret_key, self.editor.password.trim())?;
                ConfigAuthMethod::Password { secret_key }
            }
            EditorAuthMethod::KeyboardInteractive => {
                if self.editor.password.trim().is_empty() {
                    return Err(AppError::new(
                        "keyboard-interactive auth requires a response value",
                    ));
                }
                let secret_key =
                    format!("local://yshell/{auth_profile_id}/keyboard-interactive");
                self.store_secret_value(&secret_key, self.editor.password.trim())?;
                ConfigAuthMethod::KeyboardInteractive { secret_key }
            }
            EditorAuthMethod::PrivateKey => {
                if self.editor.key_path.trim().is_empty() {
                    return Err(AppError::new("private key auth requires a key path"));
                }
                let passphrase_secret_key = if self.editor.passphrase.trim().is_empty() {
                    None
                } else {
                    let secret_key = format!("local://yshell/{auth_profile_id}/passphrase");
                    self.store_secret_value(&secret_key, self.editor.passphrase.trim())?;
                    Some(secret_key)
                };
                ConfigAuthMethod::PrivateKey {
                    path: self.editor.key_path.trim().to_owned(),
                    passphrase_secret_key,
                }
            }
        };
        Ok(yshell_config::AuthProfile {
            id: auth_profile_id.to_owned(),
            name: format!("Auth for {}", self.editor.name.trim()),
            method,
        })
    }

    fn build_proxy_profile_for_editor(
        &self,
        proxy_profile_id: &str,
    ) -> AppResult<Option<ProxyProfile>> {
        if self.editor.proxy_mode == EditorProxyMode::None {
            return Ok(None);
        }
        let host = self.editor.proxy_host.trim();
        if host.is_empty() {
            return Err(AppError::new("proxy host must not be empty"));
        }
        let port = parse_port_field(
            &self.editor.proxy_port_text,
            "proxy port must be a valid integer between 1 and 65535",
        )?;
        let password_secret_key = if self.editor.proxy_password.trim().is_empty() {
            None
        } else {
            match self.editor.proxy_protocol {
                ProxyProtocol::Socks4 | ProxyProtocol::Socks4a => None,
                ProxyProtocol::Socks5 | ProxyProtocol::HttpConnect => {
                    let secret_key = format!("local://yshell/{proxy_profile_id}/password");
                    self.store_secret_value(&secret_key, self.editor.proxy_password.trim())?;
                    Some(secret_key)
                }
            }
        };
        Ok(Some(ProxyProfile {
            id: proxy_profile_id.to_owned(),
            name: format!("Proxy for {}", self.editor.name.trim()),
            protocol: self.editor.proxy_protocol,
            host: host.to_owned(),
            port,
            username: if self.editor.proxy_username.trim().is_empty() {
                None
            } else {
                Some(self.editor.proxy_username.trim().to_owned())
            },
            resolve_dns_by_proxy: self.editor.proxy_dns_by_proxy,
            password_secret_key,
        }))
    }

    fn activate_runtime_session(&mut self, runtime: SessionRuntime) -> AppResult<AppProjection> {
        if let SessionSource::SavedSession { profile_id } = &runtime.source {
            self.selected_saved_session_id = Some(profile_id.clone());
        }
        // W5：密码型认证在密钥库里找不到密码时不报错，改为挂起并让 UI 弹输入框；
        // 用户 `submit_password` 后走 `activate_runtime_session_with_ssh_config` 重试。
        if let Some(prompt) = self.pending_password_prompt_for_runtime(&runtime) {
            let host_text = prompt.host_text();
            self.pending_password_prompt = Some(prompt);
            self.status_text = format!(
                "Password required for {host_text}. The password is used for this connection only and is not saved."
            );
            return Ok(self.projection());
        }
        let ssh_config = self.resolve_runtime_ssh_config(&runtime)?;
        self.activate_runtime_session_with_ssh_config(runtime, ssh_config)
    }

    /// 已经解析/注入好 `ssh_config` 的会话激活路径。
    fn activate_runtime_session_with_ssh_config(
        &mut self,
        runtime: SessionRuntime,
        ssh_config: SshConnectionConfig,
    ) -> AppResult<AppProjection> {
        self.activate_runtime_session_inner(runtime, ssh_config, false)
    }

    /// W5：密码重试专用激活路径 —— shell 打开失败时把真实错误写进 `status_text`
    /// （普通路径保留原有的中性状态句，不影响既有行为）。
    fn activate_runtime_session_after_password(
        &mut self,
        runtime: SessionRuntime,
        ssh_config: SshConnectionConfig,
    ) -> AppResult<AppProjection> {
        self.activate_runtime_session_inner(runtime, ssh_config, true)
    }

    fn activate_runtime_session_inner(
        &mut self,
        mut runtime: SessionRuntime,
        ssh_config: SshConnectionConfig,
        password_retry: bool,
    ) -> AppResult<AppProjection> {
        let session_id = runtime.session_id().clone();
        if let SessionSource::SavedSession { profile_id } = &runtime.source {
            self.selected_saved_session_id = Some(profile_id.clone());
        }
        let tab_id = runtime.tab_id().clone();
        let open_events = self
            .dispatcher
            .dispatch(SessionCommand::OpenSession {
                tab_id,
                session_id: session_id.clone(),
            })
            .map_err(AppError::from_error)?;
        Self::apply_session_events(&session_id, &mut runtime, open_events);
        let state_events = self
            .dispatcher
            .dispatch(SessionCommand::SetSessionState {
                session_id: session_id.clone(),
                state: runtime.state,
            })
            .map_err(AppError::from_error)?;
        Self::apply_session_events(&session_id, &mut runtime, state_events);
        runtime.ssh_config = ssh_config;
        runtime.transport_backend = self.transport_backend;
        self.configure_runtime_logging(&mut runtime);
        self.configure_runtime_terminal_limits(&mut runtime);
        runtime.append_status_line("Core session entry created. Opening shell runtime boundary.");
        // W5：密码重试失败时把 shell 的真实错误带进状态栏（普通路径保持原中性文案）。
        let mut shell_open_error: Option<String> = None;
        match self.open_shell_for_runtime(&runtime.ssh_config) {
            Ok(shell_session) => {
                let shell_connected = shell_session.is_connected();
                runtime.attach_shell_session(shell_session);
                let _ = runtime.poll_shell_output().map_err(AppError::from_error)?;
                if shell_connected {
                    let connected_events = self
                        .dispatcher
                        .dispatch(SessionCommand::SetSessionState {
                            session_id: session_id.clone(),
                            state: yshell_core::SessionState::Connected,
                        })
                        .map_err(AppError::from_error)?;
                    Self::apply_session_events(&session_id, &mut runtime, connected_events);
                } else {
                    let connecting_events = self
                        .dispatcher
                        .dispatch(SessionCommand::SetSessionState {
                            session_id: session_id.clone(),
                            state: yshell_core::SessionState::Connecting,
                        })
                        .map_err(AppError::from_error)?;
                    Self::apply_session_events(&session_id, &mut runtime, connecting_events);
                    runtime.append_status_line(
                        "Shell runtime opened a non-live backend path. Either later SSH stages remain scaffolded, or the spawned shell is not yet connected.",
                    );
                }
            }
            Err(error) => {
                let handled = self.apply_host_key_prompt_from_error(
                    session_id.as_str(),
                    runtime.username_label().to_owned(),
                    &error,
                );
                runtime.append_status_line(&format!(
                    "Shell runtime failed during real shell startup: {error}"
                ));
                let failed_events = self
                    .dispatcher
                    .dispatch(SessionCommand::SetSessionState {
                        session_id: session_id.clone(),
                        state: yshell_core::SessionState::Failed,
                    })
                    .map_err(AppError::from_error)?;
                Self::apply_session_events(&session_id, &mut runtime, failed_events);
                if handled {
                    runtime.append_status_line("Host key confirmation is required before the native SSH session can continue.");
                } else {
                    // 主机密钥问题会走上面的 prompt，不算密码失败。
                    shell_open_error = Some(error.to_string());
                }
            }
        }

        let session_key = session_id.as_str().to_owned();
        self.active_session_id = Some(session_key.clone());
        self.record_recent_session(&session_key);
        self.status_text = match (&shell_open_error, password_retry) {
            (Some(error), true) => format!("Password connection failed: {error}"),
            _ => format!(
                "Runtime session created for {} (user={}) using the `{}` shell backend. {}",
                runtime.host_label(),
                runtime.username_label(),
                self.transport_backend.label(),
                if runtime.state == yshell_core::SessionState::Connected {
                    "The shell boundary is live."
                } else {
                    "The backend path exists, but a live shell was not reached."
                }
            ),
        };
        if let Some(notice) = runtime.take_logging_notice() {
            self.fold_logging_notice_into_status(notice);
        }
        self.sessions.insert(session_key.clone(), runtime);
        let sftp_status = self.sync_sftp_lifecycle_for_session(&session_key, true);
        self.status_text = format!("{} {}", self.status_text, sftp_status);
        Ok(self.projection())
    }

    fn resolved_logging_profile_for_runtime(&self, runtime: &SessionRuntime) -> LoggingProfile {
        match &runtime.source {
            SessionSource::SavedSession { profile_id } => self
                .config_document
                .resolve_session(profile_id)
                .map(|resolved| resolved.logging)
                .unwrap_or_else(|| self.config_document.logging.clone()),
            SessionSource::QuickConnect | SessionSource::Draft => self.config_document.logging.clone(),
        }
    }

    fn configure_runtime_logging(&self, runtime: &mut SessionRuntime) {
        let logging = self.resolved_logging_profile_for_runtime(runtime);
        runtime.configure_logging(&self.config_dir, &logging);
    }

    fn resolved_terminal_profile_for_runtime(&self, runtime: &SessionRuntime) -> TerminalProfile {
        match &runtime.source {
            SessionSource::SavedSession { profile_id } => self
                .config_document
                .resolve_session(profile_id)
                .map(|resolved| resolved.terminal)
                .unwrap_or_else(|| self.config_document.terminal.clone()),
            SessionSource::QuickConnect | SessionSource::Draft => {
                self.config_document.terminal.clone()
            }
        }
    }

    fn configure_runtime_terminal_limits(&self, runtime: &mut SessionRuntime) {
        let terminal = self.resolved_terminal_profile_for_runtime(runtime);
        runtime.configure_terminal_limits(&terminal);
    }

    fn fold_logging_notice_from_session(&mut self, session_key: &str) {
        let notice = self
            .sessions
            .get_mut(session_key)
            .and_then(SessionRuntime::take_logging_notice);
        if let Some(notice) = notice {
            self.fold_logging_notice_into_status(notice);
        }
    }

    fn fold_logging_notice_into_status(&mut self, notice: String) {
        if self.status_text.is_empty() {
            self.status_text = format!("Logging notice: {notice}");
        } else {
            self.status_text = format!("{} Logging notice: {}", self.status_text, notice);
        }
    }

    fn prepare_active_sftp_operation(&mut self, action: &str) -> AppResult<Option<String>> {
        let Some(session_key) = self.active_session_id.clone() else {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.status_text = format!(
                "{} Cannot continue {action} until a runtime session is connected.",
                self.sftp_session.legacy_status_text()
            );
            return Ok(None);
        };
        let message = self.sync_sftp_lifecycle_for_session(&session_key, false);
        if self.sftp_ready_session_key().is_some() {
            Ok(Some(session_key))
        } else {
            self.status_text = format!("{message} Cannot continue {action}.");
            Ok(None)
        }
    }

    fn sync_sftp_lifecycle_for_session(
        &mut self,
        session_key: &str,
        refresh_listing: bool,
    ) -> String {
        if self.transport_backend != TransportBackend::Real {
            self.sftp_session = SftpSessionLifecycle::Unavailable {
                reason: SftpUnavailableReason::SelectNativeSsh,
            };
            self.sftp_listing = SftpListingState::SyncFakeBackend;
            return self.sftp_session.legacy_status_text();
        }

        let Some((session_state, session_backend, ssh_config)) =
            self.sessions.get(session_key).map(|runtime| {
                (
                    runtime.state,
                    runtime.transport_backend,
                    runtime.ssh_config.clone(),
                )
            })
        else {
            self.sftp_session = SftpSessionLifecycle::Failed {
                session_key: Some(session_key.to_owned()),
                reason: "runtime session is missing".to_owned(),
            };
            self.sftp_listing = SftpListingState::SyncMissingSession;
            return self.sftp_session.legacy_status_text();
        };

        if session_state != yshell_core::SessionState::Connected {
            self.sftp_session = SftpSessionLifecycle::Disconnected {
                session_key: Some(session_key.to_owned()),
            };
            self.sftp_listing = SftpListingState::SyncDisconnected;
            return self.sftp_session.legacy_status_text();
        }

        if session_backend != TransportBackend::Real {
            self.sftp_session = SftpSessionLifecycle::Disconnected {
                session_key: Some(session_key.to_owned()),
            };
            self.sftp_listing = SftpListingState::SyncReconnectNative;
            return self.sftp_session.legacy_status_text();
        }

        self.sftp_session = SftpSessionLifecycle::Ready {
            session_key: session_key.to_owned(),
        };
        if !refresh_listing {
            return self.sftp_session.legacy_status_text();
        }

        let ssh_config = self.effective_ssh_config(&ssh_config);
        let client = SftpClient::with_real_backend(ssh_config);
        match client
            .list_dir(&self.sftp_path)
            .map_err(AppError::from_error)
        {
            Ok(listing) => {
                let entry_count = listing.entries.len();
                let listing_path = listing.path.clone();
                self.apply_sftp_listing(&listing);
                format!(
                    "SFTP session ready. Loaded {entry_count} SFTP entr{} from `{listing_path}`.",
                    if entry_count == 1 { "y" } else { "ies" }
                )
            }
            Err(error) => {
                self.sftp_session = SftpSessionLifecycle::Failed {
                    session_key: Some(session_key.to_owned()),
                    reason: error.to_string(),
                };
                self.sftp_listing = SftpListingState::SyncFailed {
                    session_key: session_key.to_owned(),
                    path: self.sftp_path.clone(),
                    error: error.to_string(),
                };
                self.sftp_session.legacy_status_text()
            }
        }
    }

    fn sftp_ready_session_key(&self) -> Option<&str> {
        match &self.sftp_session {
            SftpSessionLifecycle::Ready { session_key } => Some(session_key.as_str()),
            SftpSessionLifecycle::Unavailable { .. }
            | SftpSessionLifecycle::Disconnected { .. }
            | SftpSessionLifecycle::Failed { .. } => None,
        }
    }

    fn enqueue_sftp_transfer(
        &mut self,
        direction: TransferDirection,
        source_path: String,
        destination_path: String,
        total_bytes: Option<u64>,
    ) -> String {
        let transfer_id = format!("sftp-transfer-{}", self.next_transfer_ordinal);
        self.next_transfer_ordinal += 1;
        self.transfer_queue.enqueue(TransferTask::new(
            transfer_id.clone(),
            direction,
            source_path,
            destination_path,
            total_bytes,
        ));
        let _ = self.transfer_queue.start_next();
        transfer_id
    }

    fn complete_sftp_transfer(&mut self, transfer_id: &str, bytes_done: u64) {
        let _ = self.transfer_queue.complete(transfer_id, bytes_done);
    }

    fn fail_sftp_transfer(&mut self, transfer_id: &str, reason: String) {
        let _ = self.transfer_queue.fail(transfer_id, reason);
    }

    fn transfer_queue_parts(&self) -> (String, bool) {
        if self.transfer_queue.tasks.is_empty() {
            return (String::new(), true);
        }
        let rows = self
            .transfer_queue
            .tasks
            .iter()
            .rev()
            .take(5)
            .map(|task| {
                let progress = task
                    .progress_percent()
                    .map(|percent| format!("{percent}%"))
                    .unwrap_or_else(|| format!("{} B", task.bytes_done));
                let error = task
                    .last_error
                    .as_ref()
                    .map(|error| format!(" error={error}"))
                    .unwrap_or_default();
                format!(
                    "{} {} {} {} {} -> {}{}",
                    task.id,
                    transfer_direction_label(task.direction),
                    transfer_status_label(task.status),
                    progress,
                    task.source_path,
                    task.destination_path,
                    error
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        (rows, false)
    }

    fn sftp_remote_edit_parts(&self) -> (String, String) {
        match &self.remote_edit_session {
            Some(session) => (session.remote_path.clone(), session.local_temp_path.clone()),
            None => (String::new(), String::new()),
        }
    }

    fn rotate_saved_session_selection(&mut self, forward: bool) {
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

    fn record_recent_session(&mut self, session_id: &str) {
        self.recent_session_ids.retain(|existing| existing != session_id);
        self.recent_session_ids.insert(0, session_id.to_owned());
        self.recent_session_ids.truncate(8);
    }

    fn next_saved_profile_id(&self, host: &str) -> String {
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

    fn next_folder_id(&self, name: &str) -> String {
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

    fn saved_session_count(&self) -> usize {
        count_sessions(&self.config_document.folders)
    }

    fn first_saved_session_id(&self) -> Option<String> {
        first_session_id(&self.config_document.folders)
    }

    fn ensure_saved_sessions_folder(&mut self) -> &mut FolderProfile {
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

    fn target_saved_sessions_folder_mut(
        &mut self,
        folder_id: &str,
    ) -> AppResult<&mut FolderProfile> {
        if folder_id == SAVED_SESSIONS_FOLDER_ID {
            return Ok(self.ensure_saved_sessions_folder());
        }
        self.config_document.find_folder_mut(folder_id).ok_or_else(|| {
            AppError::new(format!(
                "editor target folder `{folder_id}` does not exist in the saved-session tree"
            ))
        })
    }

    fn editor_folder_choices(&self) -> Vec<EditorFolderChoice> {
        let mut choices = Vec::new();
        collect_editor_folder_choices(&self.config_document.folders, &mut Vec::new(), &mut choices);
        if !choices.iter().any(|choice| choice.id == SAVED_SESSIONS_FOLDER_ID) {
            choices.insert(
                0,
                EditorFolderChoice {
                    id: SAVED_SESSIONS_FOLDER_ID.to_owned(),
                    path_label: SAVED_SESSIONS_FOLDER_NAME.to_owned(),
                },
            );
        }
        if choices.is_empty() {
            choices.push(EditorFolderChoice {
                id: SAVED_SESSIONS_FOLDER_ID.to_owned(),
                path_label: SAVED_SESSIONS_FOLDER_NAME.to_owned(),
            });
        }
        choices
    }

    fn rotate_editor_folder(&mut self, forward: bool) {
        let choices = self.editor_folder_choices();
        if choices.is_empty() {
            self.editor.target_folder_id = SAVED_SESSIONS_FOLDER_ID.to_owned();
            return;
        }
        let current_index = choices
            .iter()
            .position(|choice| choice.id == self.editor.target_folder_id)
            .unwrap_or(0);
        let len = choices.len();
        let next_index = if forward {
            (current_index + 1) % len
        } else if current_index == 0 {
            len - 1
        } else {
            current_index - 1
        };
        self.editor.target_folder_id = choices[next_index].id.clone();
    }

    fn run_editor_auth_test(&mut self) -> AppResult<EditorAuthTestStatus> {
        let ssh_config = self.build_ssh_config_from_editor()?;
        let host_label = format!(
            "{}@{}:{}",
            ssh_config.username(),
            ssh_config.host,
            ssh_config.port
        );
        let mut shell = self
            .open_shell_for_runtime(&ssh_config)
            .map_err(AppError::from_error)?;
        let connected = shell.is_connected();
        let startup_output = shell.poll_output().map_err(AppError::from_error)?;
        shell.disconnect().map_err(AppError::from_error)?;
        let startup_snippet = String::from_utf8_lossy(&startup_output)
            .split_whitespace()
            .take(18)
            .collect::<Vec<_>>()
            .join(" ");
        if connected {
            Ok(EditorAuthTestStatus::Success {
                host_label,
                backend: self.transport_backend.label().to_owned(),
                startup_snippet,
            })
        } else {
            Err(AppError::new(if startup_snippet.is_empty() {
                format!(
                    "auth test reached the `{}` backend path for {host_label}, but a live shell was not established",
                    self.transport_backend.label()
                )
            } else {
                format!(
                    "auth test reached the `{}` backend path for {host_label}, but a live shell was not established. Startup: {}",
                    self.transport_backend.label(),
                    startup_snippet
                )
            }))
        }
    }

    fn build_editor_tunnel_forward(&self) -> AppResult<TunnelForward> {
        let bind_host = self.editor.tunnel_bind_host.trim();
        if bind_host.is_empty() {
            return Err(AppError::new("tunnel listen host must not be empty"));
        }
        let bind_port = parse_port_field(
            &self.editor.tunnel_bind_port_text,
            "tunnel listen port must be a valid integer between 1 and 65535",
        )?;
        let (target_host, target_port) = if self.editor.tunnel_kind == TunnelForwardKind::Dynamic {
            (String::new(), 0)
        } else {
            let host = self.editor.tunnel_target_host.trim();
            if host.is_empty() {
                return Err(AppError::new("tunnel target host must not be empty"));
            }
            let port = parse_port_field(
                &self.editor.tunnel_target_port_text,
                "tunnel target port must be a valid integer between 1 and 65535",
            )?;
            (host.to_owned(), port)
        };
        Ok(TunnelForward {
            kind: self.editor.tunnel_kind,
            bind_host: bind_host.to_owned(),
            bind_port,
            target_host,
            target_port,
        })
    }

    fn rotate_terminal_match(&mut self, forward: bool) {
        if self.terminal_search_matches.is_empty() {
            return;
        }
        let len = self.terminal_search_matches.len();
        let current = self.terminal_search_current_index.unwrap_or(0);
        let next = if forward {
            (current + 1) % len
        } else if current == 0 {
            len - 1
        } else {
            current - 1
        };
        self.terminal_search_current_index = Some(next);
    }

    fn terminal_search_summary_parts(&self) -> (&'static str, i32, i32) {
        if self.terminal_search_query.trim().is_empty() {
            return ("prompt", 0, 0);
        }
        if self.terminal_search_matches.is_empty() {
            return ("empty", 0, 0);
        }
        let count = i32::try_from(self.terminal_search_matches.len()).unwrap_or(i32::MAX);
        let current =
            i32::try_from(self.terminal_search_current_index.unwrap_or(0) + 1).unwrap_or(i32::MAX);
        ("found", count, current)
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    fn terminal_search_summary_legacy_text(&self) -> String {
        match self.terminal_search_summary_parts().0 {
            "prompt" => "Find in terminal".to_owned(),
            "empty" => format!(
                "No matches for `{}` in the visible terminal.",
                self.terminal_search_query
            ),
            _ => format!(
                "Found {} match(es) for `{}` in the visible terminal. Showing {}/{}.",
                self.terminal_search_matches.len(),
                self.terminal_search_query,
                self.terminal_search_current_index.unwrap_or(0) + 1,
                self.terminal_search_matches.len()
            ),
        }
    }

    fn apply_session_events(
        expected_session_id: &yshell_core::SessionId,
        runtime: &mut SessionRuntime,
        events: Vec<SessionEvent>,
    ) {
        for event in events {
            match event {
                SessionEvent::TabOpened { session_id, .. } if &session_id == expected_session_id => {
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
                    runtime.terminal_parser.advance(&mut runtime.terminal_grid, &bytes);
                }
                _ => {}
            }
        }
    }

    #[cfg(test)]
    fn set_host_key_policy_override_for_testing(&mut self, policy: HostKeyPolicy) {
        self.host_key_policy_override = Some(policy);
    }
}

fn collect_session_profiles(folders: &[FolderProfile], out: &mut Vec<SessionProfile>) {
    for folder in folders {
        out.extend(folder.sessions.iter().cloned());
        collect_session_profiles(&folder.folders, out);
    }
}

fn collect_editor_folder_choices(
    folders: &[FolderProfile],
    ancestors: &mut Vec<String>,
    out: &mut Vec<EditorFolderChoice>,
) {
    for folder in folders {
        ancestors.push(folder.name.clone());
        out.push(EditorFolderChoice {
            id: folder.id.clone(),
            path_label: ancestors.join(" / "),
        });
        collect_editor_folder_choices(&folder.folders, ancestors, out);
        let _ = ancestors.pop();
    }
}

fn collect_filtered_session_profiles(
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

fn collect_session_inventory_lines(
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
struct SavedTreeProjection<'a> {
    query: &'a str,
    selected_session_id: Option<&'a str>,
    selected_folder_id: Option<&'a str>,
    collapsed_folders: &'a BTreeSet<String>,
}

/// Appends the visible tree rows of `folders` in display order (folder row
/// first, then its visible sessions, then nested folders) and returns how many
/// sessions are visible below them.
///
/// `show_all` is `true` when an ancestor folder matched the query: the whole
/// subtree is then visible regardless of its own matches.
fn collect_session_tree_rows(
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
fn session_user_host(session: &SessionProfile) -> String {
    match session.username.as_deref() {
        Some(username) if !username.is_empty() => format!("{username}@{}", session.host),
        _ => session.host.clone(),
    }
}

fn session_matches_query(session: &SessionProfile, query: &str) -> bool {
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

fn folder_matches_query(folder: &FolderProfile, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    folder.name.to_ascii_lowercase().contains(&query.to_ascii_lowercase())
}

fn count_sessions(folders: &[FolderProfile]) -> usize {
    folders
        .iter()
        .map(|folder| folder.sessions.len() + count_sessions(&folder.folders))
        .sum()
}

fn first_session_id(folders: &[FolderProfile]) -> Option<String> {
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

fn find_session_folder_id(folders: &[FolderProfile], session_id: &str) -> Option<String> {
    for folder in folders {
        if folder.sessions.iter().any(|session| session.id == session_id) {
            return Some(folder.id.clone());
        }
        if let Some(nested) = find_session_folder_id(&folder.folders, session_id) {
            return Some(nested);
        }
    }
    None
}

fn normalize_remote_path(path: &str) -> AppResult<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppError::new("remote path must not be empty"));
    }
    if !trimmed.starts_with('/') {
        return Err(AppError::new(format!(
            "remote path `{trimmed}` must start with `/`"
        )));
    }
    let normalized = if trimmed == "/" {
        "/".to_owned()
    } else {
        format!("/{}", trimmed.trim_matches('/'))
    };
    Ok(normalized)
}

fn parent_remote_path(path: &str) -> String {
    let normalized = normalize_remote_path(path).unwrap_or_else(|_| "/".to_owned());
    if normalized == "/" {
        return normalized;
    }
    let trimmed = normalized.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", _)) | None => "/".to_owned(),
        Some((parent, _)) => parent.to_owned(),
    }
}

fn join_remote_path(base: &str, name: &str) -> String {
    let base = normalize_remote_path(base).unwrap_or_else(|_| "/".to_owned());
    if base == "/" {
        format!("/{}", name.trim_start_matches('/'))
    } else {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            name.trim_start_matches('/')
        )
    }
}

fn parse_sftp_permissions(value: &str) -> AppResult<u32> {
    let trimmed = value.trim();
    let octal = trimmed.strip_prefix("0o").unwrap_or(trimmed);
    if octal.len() != 3 || !octal.chars().all(|digit| matches!(digit, '0'..='7')) {
        return Err(AppError::new(
            "SFTP permissions must be a three-digit octal value such as 644 or 755",
        ));
    }
    u32::from_str_radix(octal, 8)
        .map_err(|_| AppError::new("SFTP permissions must be valid octal digits"))
}

fn transfer_direction_label(direction: TransferDirection) -> &'static str {
    match direction {
        TransferDirection::Upload => "upload",
        TransferDirection::Download => "download",
    }
}

fn transfer_status_label(status: TransferStatus) -> &'static str {
    match status {
        TransferStatus::Queued => "queued",
        TransferStatus::Running => "running",
        TransferStatus::Completed => "completed",
        TransferStatus::Failed => "failed",
        TransferStatus::Cancelled => "cancelled",
    }
}

fn parse_port_field(value: &str, invalid_message: &str) -> AppResult<u16> {
    let port = value
        .trim()
        .parse::<u16>()
        .map_err(|_| AppError::new(invalid_message))?;
    if port == 0 {
        return Err(AppError::new("port must be greater than zero"));
    }
    Ok(port)
}

/// Validates a terminal settings text field against an inclusive range.
fn parse_settings_usize(
    field: SettingsTerminalField,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<usize, SettingsTerminalStatus> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SettingsTerminalStatus::Empty { field });
    }
    let parsed = trimmed
        .parse::<usize>()
        .map_err(|_| SettingsTerminalStatus::NotNumber { field })?;
    if parsed < minimum || parsed > maximum {
        return Err(SettingsTerminalStatus::OutOfRange {
            field,
            minimum,
            maximum,
        });
    }
    Ok(parsed)
}

fn tunnel_kind_label(kind: TunnelForwardKind) -> &'static str {
    match kind {
        TunnelForwardKind::Local => "local",
        TunnelForwardKind::Remote => "remote",
        TunnelForwardKind::Dynamic => "dynamic",
    }
}

fn proxy_protocol_label(protocol: ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Socks4 => "socks4",
        ProxyProtocol::Socks4a => "socks4a",
        ProxyProtocol::Socks5 => "socks5",
        ProxyProtocol::HttpConnect => "http-connect",
    }
}

fn config_host_key_policy_label(policy: ConfigHostKeyPolicy) -> &'static str {
    match policy {
        ConfigHostKeyPolicy::Strict => "strict",
        ConfigHostKeyPolicy::TrustOnFirstUse => "trust-on-first-use",
        ConfigHostKeyPolicy::AcceptAnyForTesting => "accept-any-for-testing",
    }
}

/// W5：认证档案里"需要用户输入的密码型密钥"（secret key + 认证方式）。
///
/// 私钥口令与代理密码不在此列：弹窗文案与重试路径只处理 `Password` /
/// `KeyboardInteractive` 这两种认证密钥。
fn password_prompt_auth_method(
    auth: &yshell_config::AuthProfile,
) -> Option<(&str, PendingPasswordAuthMethod)> {
    match &auth.method {
        ConfigAuthMethod::Password { secret_key } => {
            Some((secret_key.as_str(), PendingPasswordAuthMethod::Password))
        }
        ConfigAuthMethod::KeyboardInteractive { secret_key } => Some((
            secret_key.as_str(),
            PendingPasswordAuthMethod::KeyboardInteractive,
        )),
        ConfigAuthMethod::PrivateKey { .. } | ConfigAuthMethod::Agent => None,
    }
}

fn config_host_key_policy_to_runtime(policy: ConfigHostKeyPolicy) -> HostKeyPolicy {
    match policy {
        ConfigHostKeyPolicy::Strict => HostKeyPolicy::Strict,
        ConfigHostKeyPolicy::TrustOnFirstUse => HostKeyPolicy::TrustOnFirstUse,
        ConfigHostKeyPolicy::AcceptAnyForTesting => HostKeyPolicy::AcceptAnyForTesting,
    }
}

pub(crate) fn runtime_host_key_policy_to_config(policy: &HostKeyPolicy) -> ConfigHostKeyPolicy {
    match policy {
        HostKeyPolicy::Strict => ConfigHostKeyPolicy::Strict,
        HostKeyPolicy::TrustOnFirstUse => ConfigHostKeyPolicy::TrustOnFirstUse,
        HostKeyPolicy::AcceptAnyForTesting => ConfigHostKeyPolicy::AcceptAnyForTesting,
    }
}

fn tunnel_forward_summary(forward: &TunnelForward) -> String {
    match forward.kind {
        TunnelForwardKind::Dynamic => format!(
            "{} {}:{}",
            tunnel_kind_label(forward.kind),
            forward.bind_host,
            forward.bind_port
        ),
        TunnelForwardKind::Local | TunnelForwardKind::Remote => format!(
            "{} {}:{} -> {}:{}",
            tunnel_kind_label(forward.kind),
            forward.bind_host,
            forward.bind_port,
            forward.target_host,
            forward.target_port
        ),
    }
}

fn tunnel_forward_to_config(forward: TunnelForward) -> TunnelConfig {
    TunnelConfig {
        id: format!(
            "{}-{}-{}",
            tunnel_kind_label(forward.kind),
            forward.bind_host.replace(':', "-"),
            forward.bind_port
        ),
        kind: match forward.kind {
            TunnelForwardKind::Local => ForwardingKind::Local,
            TunnelForwardKind::Remote => ForwardingKind::Remote,
            TunnelForwardKind::Dynamic => ForwardingKind::Dynamic,
        },
        listen_host: forward.bind_host,
        listen_port: forward.bind_port,
        target_host: forward.target_host,
        target_port: forward.target_port,
    }
}

fn tunnel_config_summary(config: &TunnelConfig) -> String {
    match config.kind {
        ForwardingKind::Dynamic => {
            format!("dynamic {}:{}", config.listen_host, config.listen_port)
        }
        ForwardingKind::Local => format!(
            "local {}:{} -> {}:{}",
            config.listen_host, config.listen_port, config.target_host, config.target_port
        ),
        ForwardingKind::Remote => format!(
            "remote {}:{} -> {}:{}",
            config.listen_host, config.listen_port, config.target_host, config.target_port
        ),
    }
}

struct DefaultRuntimeKeychainSelection {
    keychain: Option<Arc<dyn Keychain>>,
    secret_store_path: Option<PathBuf>,
    kind: SecretStoreKind,
}

fn default_runtime_keychain(config_dir: &Path) -> AppResult<DefaultRuntimeKeychainSelection> {
    let path = config_dir.join("secret-store.toml");
    if let Ok(master_password) = env::var("YSHELL_MASTER_PASSWORD") {
        if master_password.trim().is_empty() {
            return Ok(DefaultRuntimeKeychainSelection {
                keychain: None,
                secret_store_path: Some(path),
                kind: SecretStoreKind::Disabled,
            });
        }
        let keychain =
            FileKeychain::open_or_create(path.clone(), master_password).map_err(AppError::from_error)?;
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

#[cfg(test)]
mod tests {
    use std::{fs, net::TcpListener, sync::{Arc, Mutex, OnceLock}};
    use std::thread;
    use std::time::Duration;

    use super::*;
    use tempfile::tempdir;
    use yshell_config::{
        AuthMethod as ConfigAuthMethod, AuthProfile, ConfigDocument, FolderProfile,
        ProxyProfile, ProxyProtocol, QuickConnectTarget, TunnelForward, TunnelForwardKind,
        TunnelProfile,
    };
    use yshell_secret::{FakeKeychain, FileKeychain, Keychain, SecretRef, SecretString};
    use yshell_ssh::{
        AuthMethod as SshAuthMethod, ForwardingKind, HostKeyFingerprint, HostKeyProblem,
        ProxyConfig, SshError, SshErrorKind,
    };

    #[test]
    fn runtime_boots_with_startup_projection() {
        let temp = tempdir().expect("tempdir");
        let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let projection = runtime.projection();

        assert!(projection.status_text.contains("Saved sessions discovered"));
        assert_eq!(projection.active_session_kind_text, "welcome");
        assert_eq!(projection.saved_session_count, 0);
        assert!(projection.sftp_visible);
    }

    #[test]
    fn runtime_new_defaults_to_operating_system_keychain_without_master_password_env() {
        let _guard = lock_env();
        std::env::remove_var("YSHELL_MASTER_PASSWORD");
        let temp = tempdir().expect("tempdir");
        let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        assert_eq!(runtime.secret_store_kind.id(), "os");
        assert!(runtime.secret_store_path.is_none());
    }

    #[test]
    fn unknown_host_key_error_surfaces_first_trust_prompt() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        let session_key = runtime.active_session_key().expect("active session");
        let error = SshError::new(
            SshErrorKind::HostKeyRejected,
            "no known host key".to_owned(),
        )
        .with_host_key_problem(HostKeyProblem::Unknown {
            host: "example.com".to_owned(),
            port: 2200,
            presented: HostKeyFingerprint {
                algorithm: "ssh-ed25519".to_owned(),
                fingerprint: "sha256:test".to_owned(),
            },
        });

        assert!(runtime.apply_host_key_prompt_from_error(&session_key, "alice".to_owned(), &error));
        let projection = runtime.projection();

        assert!(projection.host_key_prompt_visible);
        assert_eq!(projection.host_key_prompt_mode_text, "first-trust");
        assert!(projection.host_key_prompt_text.contains("Trust Once"));
    }

    #[test]
    fn trust_host_key_and_save_persists_known_hosts_and_reconnects() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        let session_key = runtime.active_session_key().expect("active session");
        runtime.pending_host_key_prompt = Some(PendingHostKeyPrompt {
            session_key: session_key.clone(),
            host: "example.com".to_owned(),
            port: 2200,
            username: "alice".to_owned(),
            presented: HostKeyFingerprint {
                algorithm: "ssh-ed25519".to_owned(),
                fingerprint: "sha256:test".to_owned(),
            },
            expected: None,
            known_hosts_path: runtime.config_store.known_hosts_file(),
        });

        let projection = runtime
            .trust_host_key_and_save()
            .expect("trust host key and save");

        assert!(!projection.host_key_prompt_visible);
        assert_eq!(projection.tab_state_text, "connected");
        assert!(projection.tab_has_session);
        let persisted = runtime
            .config_store
            .load_known_hosts()
            .expect("load persisted known_hosts");
        assert!(persisted.get("example.com", 2200).is_some());
    }

    #[test]
    fn replace_host_key_requires_confirmation_text() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        let session_key = runtime.active_session_key().expect("active session");
        runtime.pending_host_key_prompt = Some(PendingHostKeyPrompt {
            session_key,
            host: "example.com".to_owned(),
            port: 2200,
            username: "alice".to_owned(),
            presented: HostKeyFingerprint {
                algorithm: "ssh-ed25519".to_owned(),
                fingerprint: "sha256:new".to_owned(),
            },
            expected: Some(HostKeyFingerprint {
                algorithm: "ssh-ed25519".to_owned(),
                fingerprint: "sha256:old".to_owned(),
            }),
            known_hosts_path: runtime.config_store.known_hosts_file(),
        });

        let error = runtime
            .replace_host_key_and_connect()
            .expect_err("replace should require confirmation");

        assert!(error.message.contains("type REPLACE"));
        assert!(runtime.pending_host_key_prompt.is_some());
    }

    #[test]
    fn known_hosts_manager_lists_and_removes_persisted_entries() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut known_hosts = KnownHosts::new();
        known_hosts.pin(
            "alpha.example.test",
            22,
            HostKeyFingerprint {
                algorithm: "ssh-ed25519".to_owned(),
                fingerprint: "sha256:alpha".to_owned(),
            },
        );
        known_hosts.pin(
            "beta.example.test",
            2200,
            HostKeyFingerprint {
                algorithm: "ssh-rsa".to_owned(),
                fingerprint: "sha256:beta".to_owned(),
            },
        );
        store.save_known_hosts(&known_hosts).expect("save known_hosts");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let opened = runtime.open_known_hosts_manager();
        assert!(opened.known_hosts_modal_visible);
        assert!(opened
            .known_hosts_inventory_rows_text
            .contains("alpha.example.test:22"));
        assert!(opened
            .known_hosts_inventory_rows_text
            .contains("beta.example.test:2200"));
        assert!(!opened.known_hosts_inventory_empty);
        assert!(opened.known_hosts_details_text.contains("Fingerprint:"));

        let _ = runtime.select_next_known_host();
        let removed = runtime
            .remove_selected_known_host()
            .expect("remove selected known host");
        assert!(removed.status_text.contains("Removed known host entry"));
        let persisted = runtime
            .config_store
            .load_known_hosts()
            .expect("reload known hosts");
        assert!(persisted.snapshot().len() == 1);
    }

    #[test]
    fn known_hosts_manager_clear_requires_confirmation_and_persists_empty_store() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut known_hosts = KnownHosts::new();
        known_hosts.pin(
            "gamma.example.test",
            2022,
            HostKeyFingerprint {
                algorithm: "ssh-ed25519".to_owned(),
                fingerprint: "sha256:gamma".to_owned(),
            },
        );
        store.save_known_hosts(&known_hosts).expect("save known_hosts");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let error = runtime
            .clear_all_known_hosts()
            .expect_err("clear should require confirmation");
        assert!(error.message.contains("type CLEAR"));

        let _ = runtime.update_known_hosts_clear_confirmation("CLEAR");
        let cleared = runtime
            .clear_all_known_hosts()
            .expect("clear persisted known hosts");
        assert!(cleared.known_hosts_inventory_empty);
        assert_eq!(cleared.known_hosts_inventory_rows_text, "");
        assert_eq!(cleared.known_hosts_clear_confirmation_text, "");
        assert!(
            runtime
                .config_store
                .load_known_hosts()
                .expect("reload known hosts")
                .snapshot()
                .is_empty()
        );
    }

    #[test]
    fn runtime_new_uses_default_file_keychain_when_master_password_env_is_set() {
        let _guard = lock_env();
        let temp = tempdir().expect("tempdir");
        std::env::set_var("YSHELL_MASTER_PASSWORD", "env-master");

        let secret_store_path = temp.path().join("secret-store.toml");
        let keychain = FileKeychain::open_or_create(&secret_store_path, "env-master")
            .expect("file keychain");
        let secret_ref = SecretRef::new("file://saved/password");
        keychain
            .put(secret_ref.clone(), SecretString::from("env-secret"))
            .expect("put secret");

        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        document.auth_profiles.insert(
            "auth-password".to_owned(),
            AuthProfile {
                id: "auth-password".to_owned(),
                name: "Password".to_owned(),
                method: ConfigAuthMethod::Password {
                    secret_key: secret_ref.as_str().to_owned(),
                },
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("root".to_owned()),
            host: "password.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("saved-password");
        profile.auth_profile_id = Some("auth-password".to_owned());
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime
            .open_saved_session("saved-password")
            .expect("open saved session");

        let session_key = runtime.active_session_key().expect("active session");
        let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
        assert!(matches!(
            runtime_session.ssh_config.auth,
            SshAuthMethod::Password {
                ref username,
                ref password
            } if username == "root" && password == "env-secret"
        ));

        std::env::remove_var("YSHELL_MASTER_PASSWORD");
    }

    #[test]
    fn reset_secret_store_requires_exact_confirmation_phrase() {
        let _guard = lock_env();
        let temp = tempdir().expect("tempdir");
        std::env::set_var("YSHELL_MASTER_PASSWORD", "env-master");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime.update_secret_reset_confirmation("almost");
        let error = runtime
            .reset_secret_store()
            .expect_err("confirmation should be required");
        assert!(error
            .to_string()
            .contains("type `RESET SECRETS` exactly"));

        std::env::remove_var("YSHELL_MASTER_PASSWORD");
    }

    #[test]
    fn reset_secret_store_deletes_local_file_and_clears_confirmation() {
        let _guard = lock_env();
        let temp = tempdir().expect("tempdir");
        std::env::set_var("YSHELL_MASTER_PASSWORD", "env-master");

        let secret_store_path = temp.path().join("secret-store.toml");
        let keychain = FileKeychain::open_or_create(&secret_store_path, "env-master")
            .expect("file keychain");
        keychain
            .put(
                SecretRef::new("file://saved/password"),
                SecretString::from("env-secret"),
            )
            .expect("put secret");
        assert!(secret_store_path.exists());

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime.update_secret_reset_confirmation("RESET SECRETS");
        let projection = runtime.reset_secret_store().expect("reset secret store");

        if secret_store_path.exists() {
            let on_disk = fs::read_to_string(&secret_store_path).expect("read store");
            assert!(!on_disk.contains("env-secret"));
            assert!(!on_disk.contains("file://saved/password"));
        }
        assert_eq!(projection.secret_reset_confirmation_text, "");
        assert!(projection
            .status_text
            .contains("Reset the local secret store"));
        assert_eq!(projection.status_kind, "secrets-reset");
        assert!(projection.status_param_1.ends_with("secret-store.toml"));
        assert_eq!(projection.status_param_2, "");

        std::env::remove_var("YSHELL_MASTER_PASSWORD");
    }

    #[test]
    fn quick_connect_enters_runtime_pipeline() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");

        let projection = runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        assert_eq!(projection.active_session_kind_text, "session");
        assert!(projection
            .active_session_name_text
            .contains("alice@example.com:2200"));
        assert!(projection
            .status_text
            .contains("The shell boundary is live"));
        assert_eq!(projection.tab_state_text, "connected");
        assert!(projection.tab_name_text.contains("alice@example.com:2200"));
        assert!(projection
            .terminal_body_text
            .contains("Fake shell established"));
        assert_eq!(runtime.emitted_events().len(), 1);
    }

    #[test]
    fn hydrates_saved_sessions_from_config() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(
            QuickConnectTarget {
                username: Some("ops".to_owned()),
                host: "saved.example.test".to_owned(),
                port: 22,
            }
            .into_session_profile("saved-session-1"),
        );
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        assert_eq!(runtime.saved_session_profiles().len(), 1);
        let projection = runtime.projection();
        assert_eq!(projection.active_session_kind_text, "saved-sessions");
        assert_eq!(projection.saved_session_count, 1);
        assert!(projection
            .saved_session_selection_name_text
            .contains("saved.example.test"));
        assert!(projection
            .saved_session_selection_kind_text
            .contains("profile"));
        assert!(projection
            .saved_session_inventory_rows_text
            .contains("[folder] Saved Sessions"));
    }

    #[test]
    fn saved_session_agent_auth_profile_is_applied_to_runtime_shell_config() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        document.auth_profiles.insert(
            "auth-agent".to_owned(),
            AuthProfile {
                id: "auth-agent".to_owned(),
                name: "Agent".to_owned(),
                method: ConfigAuthMethod::Agent,
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("ops".to_owned()),
            host: "agent.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("saved-agent");
        profile.auth_profile_id = Some("auth-agent".to_owned());
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime
            .open_saved_session("saved-agent")
            .expect("open saved session");

        let session_key = runtime.active_session_key().expect("active session");
        let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
        assert!(matches!(
            runtime_session.ssh_config.auth,
            SshAuthMethod::Agent { ref username } if username == "ops"
        ));
        assert_eq!(
            runtime_session.ssh_config.host_key_policy,
            HostKeyPolicy::Strict
        );
    }

    #[test]
    fn saved_session_host_key_policy_is_applied_to_runtime_shell_config() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        let mut profile = QuickConnectTarget {
            username: Some("ops".to_owned()),
            host: "host-key.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("saved-host-key");
        profile.host_key_policy = Some(ConfigHostKeyPolicy::TrustOnFirstUse);
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime
            .open_saved_session("saved-host-key")
            .expect("open saved session");

        let session_key = runtime.active_session_key().expect("active session");
        let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
        assert_eq!(
            runtime_session.ssh_config.host_key_policy,
            HostKeyPolicy::TrustOnFirstUse
        );
    }

    #[test]
    fn saved_session_password_auth_profile_uses_secret_from_keychain() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let secret_ref = SecretRef::new("fake://yshell/password");
        let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("master"));
        keychain
            .put(secret_ref.clone(), SecretString::from("hunter2"))
            .expect("put secret");

        let mut document = ConfigDocument::default();
        document.auth_profiles.insert(
            "auth-password".to_owned(),
            AuthProfile {
                id: "auth-password".to_owned(),
                name: "Password".to_owned(),
                method: ConfigAuthMethod::Password {
                    secret_key: secret_ref.as_str().to_owned(),
                },
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("root".to_owned()),
            host: "password.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("saved-password");
        profile.auth_profile_id = Some("auth-password".to_owned());
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain))
            .expect("runtime");
        let _ = runtime
            .open_saved_session("saved-password")
            .expect("open saved session");

        let session_key = runtime.active_session_key().expect("active session");
        let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
        assert!(matches!(
            runtime_session.ssh_config.auth,
            SshAuthMethod::Password {
                ref username,
                ref password
            } if username == "root" && password == "hunter2"
        ));
    }

    #[test]
    fn saved_session_keyboard_interactive_auth_profile_uses_secret_from_keychain() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let secret_ref = SecretRef::new("fake://yshell/keyboard-interactive");
        let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("master"));
        keychain
            .put(secret_ref.clone(), SecretString::from("one-time-secret"))
            .expect("put secret");

        let mut document = ConfigDocument::default();
        document.auth_profiles.insert(
            "auth-kbdint".to_owned(),
            AuthProfile {
                id: "auth-kbdint".to_owned(),
                name: "Keyboard Interactive".to_owned(),
                method: ConfigAuthMethod::KeyboardInteractive {
                    secret_key: secret_ref.as_str().to_owned(),
                },
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("root".to_owned()),
            host: "kbdint.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("saved-kbdint");
        profile.auth_profile_id = Some("auth-kbdint".to_owned());
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain))
            .expect("runtime");
        let _ = runtime
            .open_saved_session("saved-kbdint")
            .expect("open saved session");

        let session_key = runtime.active_session_key().expect("active session");
        let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
        assert!(matches!(
            runtime_session.ssh_config.auth,
            SshAuthMethod::KeyboardInteractive {
                ref username,
                ref secret
            } if username == "root" && secret == "one-time-secret"
        ));
    }

    #[test]
    fn saved_session_private_key_auth_profile_resolves_passphrase_secret() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let secret_ref = SecretRef::new("fake://yshell/passphrase");
        let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("master"));
        keychain
            .put(secret_ref.clone(), SecretString::from("open-sesame"))
            .expect("put secret");

        let mut document = ConfigDocument::default();
        document.auth_profiles.insert(
            "auth-key".to_owned(),
            AuthProfile {
                id: "auth-key".to_owned(),
                name: "Private Key".to_owned(),
                method: ConfigAuthMethod::PrivateKey {
                    path: "/tmp/id_ed25519".to_owned(),
                    passphrase_secret_key: Some(secret_ref.as_str().to_owned()),
                },
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("deploy".to_owned()),
            host: "key.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("saved-key");
        profile.auth_profile_id = Some("auth-key".to_owned());
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain))
            .expect("runtime");
        let _ = runtime
            .open_saved_session("saved-key")
            .expect("open saved session");

        let session_key = runtime.active_session_key().expect("active session");
        let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
        assert!(matches!(
            runtime_session.ssh_config.auth,
            SshAuthMethod::PrivateKey {
                ref username,
                ref key_path,
                ref passphrase
            } if username == "deploy"
                && key_path == "/tmp/id_ed25519"
                && passphrase.as_deref() == Some("open-sesame")
        ));
    }

    #[test]
    fn saved_session_password_auth_profile_prompts_without_keychain() {
        let _guard = lock_env();
        std::env::remove_var("YSHELL_MASTER_PASSWORD");
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        document.auth_profiles.insert(
            "auth-password".to_owned(),
            AuthProfile {
                id: "auth-password".to_owned(),
                name: "Password".to_owned(),
                method: ConfigAuthMethod::Password {
                    secret_key: "fake://yshell/missing".to_owned(),
                },
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("root".to_owned()),
            host: "password.example.test".to_owned(),
            port: 2200,
        }
        .into_session_profile("saved-password");
        profile.auth_profile_id = Some("auth-password".to_owned());
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");
        let projection = runtime
            .open_saved_session("saved-password")
            .expect("missing password should suspend the connection, not error");

        // 投影：弹窗可见 + user@host:port 文案；连接尚未创建。
        assert!(projection.password_prompt_visible);
        assert_eq!(
            projection.password_prompt_host_text,
            "root@password.example.test:2200"
        );
        assert!(projection.status_text.contains("Password required"));
        assert!(runtime.active_session_id.is_none());

        // 挂起目标保存了重试所需的最小上下文。
        let pending = runtime
            .pending_password_prompt
            .as_ref()
            .expect("pending password prompt");
        assert_eq!(
            pending,
            &PendingPasswordPrompt {
                profile_id: "saved-password".to_owned(),
                host: "password.example.test".to_owned(),
                port: 2200,
                username: "root".to_owned(),
                auth_method: PendingPasswordAuthMethod::Password,
            }
        );
    }

    #[test]
    fn submit_password_retries_saved_session_and_connects() {
        let _guard = lock_env();
        std::env::remove_var("YSHELL_MASTER_PASSWORD");
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        document.auth_profiles.insert(
            "auth-password".to_owned(),
            AuthProfile {
                id: "auth-password".to_owned(),
                name: "Password".to_owned(),
                method: ConfigAuthMethod::Password {
                    secret_key: "fake://yshell/missing".to_owned(),
                },
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("root".to_owned()),
            host: "password.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("saved-password");
        profile.auth_profile_id = Some("auth-password".to_owned());
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");
        let config_before = fs::read(temp.path().join("config.toml")).expect("read config");

        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");
        let prompted = runtime
            .open_saved_session("saved-password")
            .expect("open saved session");
        assert!(prompted.password_prompt_visible);

        // fake 后端：带密码重试后进入 connected。
        // 说明：fake 后端不校验密码正确性，"密码错误 → 认证失败"只能在
        // native-ssh + 真实服务器上验证；这里断言的是"挂起 → 重试 → 连接成功"
        // 这条状态机，以及密码不写密钥库/配置文件。
        let projection = runtime
            .submit_password("hunter2")
            .expect("submit password should retry the connection");
        assert!(!projection.password_prompt_visible);
        assert!(projection.has_active_session);
        assert!(projection.active_session_connected);
        assert_eq!(projection.active_session_state_text, "connected");
        assert!(runtime.pending_password_prompt.is_none());

        // 密码只存在于本次运行时的 ssh 配置里；密钥库与配置文件都不应被写入。
        let session_key = runtime.active_session_key().expect("active session");
        let session = runtime.sessions.get(&session_key).expect("runtime session");
        assert!(matches!(
            session.ssh_config.auth,
            SshAuthMethod::Password {
                ref username,
                ref password
            } if username == "root" && password == "hunter2"
        ));
        assert!(runtime.keychain.is_none());
        let config_after = fs::read(temp.path().join("config.toml")).expect("read config");
        assert_eq!(
            config_before, config_after,
            "password retry must not write config"
        );
        assert!(
            !fs::read_dir(temp.path())
                .expect("temp dir")
                .flatten()
                .any(|entry| entry.file_name() == "secret-store.toml"),
            "password retry must not create a secret store"
        );
    }

    #[test]
    fn submit_password_reports_failure_and_closes_prompt_when_target_is_gone() {
        let _guard = lock_env();
        std::env::remove_var("YSHELL_MASTER_PASSWORD");
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        document.auth_profiles.insert(
            "auth-password".to_owned(),
            AuthProfile {
                id: "auth-password".to_owned(),
                name: "Password".to_owned(),
                method: ConfigAuthMethod::Password {
                    secret_key: "fake://yshell/missing".to_owned(),
                },
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("root".to_owned()),
            host: "password.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("saved-password");
        profile.auth_profile_id = Some("auth-password".to_owned());
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");
        let prompted = runtime
            .open_saved_session("saved-password")
            .expect("open saved session");
        assert!(prompted.password_prompt_visible);

        // 弹窗挂起后目标被删掉：重试失败写进 status_text，弹窗关闭。
        let deleted = runtime
            .delete_selected_saved_session()
            .expect("delete the prompted saved session");
        assert!(!deleted.has_saved_selection);

        let projection = runtime
            .submit_password("hunter2")
            .expect("failure is reported through the projection");
        assert!(!projection.password_prompt_visible);
        assert!(!projection.has_active_session);
        assert!(projection
            .status_text
            .contains("Password connection failed"));
        assert!(runtime.pending_password_prompt.is_none());
    }

    #[test]
    fn submit_password_reports_auth_failure_from_native_backend() {
        // native-ssh + 本地 TCP 探针：握手必失败，用来验证"密码重试失败"会写进
        // status_text（fake 后端不会失败，覆盖不到这条路径）。
        let (port, accept_handle) = start_tcp_probe_target();
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        document.auth_profiles.insert(
            "auth-password".to_owned(),
            AuthProfile {
                id: "auth-password".to_owned(),
                name: "Password".to_owned(),
                method: ConfigAuthMethod::Password {
                    secret_key: "fake://yshell/missing".to_owned(),
                },
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("alice".to_owned()),
            host: "127.0.0.1".to_owned(),
            port,
        }
        .into_session_profile("saved-password");
        profile.auth_profile_id = Some("auth-password".to_owned());
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");
        let _ = runtime.select_native_ssh_transport_backend();
        let prompted = runtime
            .open_saved_session("saved-password")
            .expect("open saved session");
        assert!(prompted.password_prompt_visible);

        let projection = runtime
            .submit_password("wrong-password")
            .expect("retry reports failure through the projection");
        accept_handle.join().expect("accept thread");

        assert!(!projection.password_prompt_visible);
        assert_eq!(projection.tab_state_text, "failed");
        assert!(projection
            .status_text
            .contains("Password connection failed"));
        assert!(projection
            .terminal_body_text
            .contains("Shell runtime failed"));
        assert!(runtime.pending_password_prompt.is_none());
    }

    #[test]
    fn cancel_password_prompt_clears_pending_state() {
        let temp = tempdir().expect("tempdir");
        let mut runtime =
            AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
        runtime.pending_password_prompt = Some(PendingPasswordPrompt {
            profile_id: "saved-password".to_owned(),
            host: "password.example.test".to_owned(),
            port: 22,
            username: "root".to_owned(),
            auth_method: PendingPasswordAuthMethod::KeyboardInteractive,
        });
        assert!(runtime.projection().password_prompt_visible);
        assert_eq!(
            runtime.projection().password_prompt_host_text,
            "root@password.example.test:22"
        );

        let projection = runtime.cancel_password_prompt();
        assert!(!projection.password_prompt_visible);
        assert!(projection.password_prompt_host_text.is_empty());
        assert!(runtime.pending_password_prompt.is_none());

        // 没有挂起目标时提交密码是显式错误（UI 层不应触发）。
        let error = runtime
            .submit_password("hunter2")
            .expect_err("submit without a pending prompt must fail");
        assert!(error.to_string().contains("no password prompt is pending"));
    }

    #[test]
    fn saved_session_keyboard_interactive_auth_prompts_without_keychain() {
        let _guard = lock_env();
        std::env::remove_var("YSHELL_MASTER_PASSWORD");
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        document.auth_profiles.insert(
            "auth-kbdint".to_owned(),
            AuthProfile {
                id: "auth-kbdint".to_owned(),
                name: "Keyboard Interactive".to_owned(),
                method: ConfigAuthMethod::KeyboardInteractive {
                    secret_key: "fake://yshell/missing-kbdint".to_owned(),
                },
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("ops".to_owned()),
            host: "kbdint.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("saved-kbdint");
        profile.auth_profile_id = Some("auth-kbdint".to_owned());
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");
        let projection = runtime
            .open_saved_session("saved-kbdint")
            .expect("keyboard-interactive should prompt too");
        assert!(projection.password_prompt_visible);
        assert_eq!(
            runtime
                .pending_password_prompt
                .as_ref()
                .map(|prompt| prompt.auth_method),
            Some(PendingPasswordAuthMethod::KeyboardInteractive)
        );
    }

    #[test]
    fn selected_saved_session_can_be_loaded_into_editor() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        document.auth_profiles.insert(
            "auth-agent".to_owned(),
            AuthProfile {
                id: "auth-agent".to_owned(),
                name: "Agent".to_owned(),
                method: ConfigAuthMethod::Agent,
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("ops".to_owned()),
            host: "editor.example.test".to_owned(),
            port: 2202,
        }
        .into_session_profile("saved-editor");
        profile.name = "Editor Session".to_owned();
        profile.auth_profile_id = Some("auth-agent".to_owned());
        profile.host_key_policy = Some(ConfigHostKeyPolicy::AcceptAnyForTesting);
        let mut folder = FolderProfile::new("folder-ops", "Ops");
        let mut nested = FolderProfile::new("folder-prod", "Prod");
        nested.sessions.push(profile);
        folder.folders.push(nested);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime.load_selected_saved_session_into_editor().expect("load editor");

        assert_eq!(runtime.editor.name, "Editor Session");
        assert_eq!(runtime.editor.host, "editor.example.test");
        assert_eq!(runtime.editor.port_text, "2202");
        assert_eq!(runtime.editor.username, "ops");
        assert_eq!(runtime.editor.auth_method, EditorAuthMethod::Agent);
        assert_eq!(
            runtime.editor.host_key_policy,
            ConfigHostKeyPolicy::AcceptAnyForTesting
        );
        assert_eq!(runtime.editor.target_folder_id, "folder-prod");
        let (folder_label, folder_id, folder_known) = runtime.editor_folder_parts();
        assert!(folder_label.contains("Ops / Prod"));
        assert_eq!(folder_id, "folder-prod");
        assert!(folder_known);
    }

    #[test]
    fn session_editor_modal_visibility_tracks_open_close_save_flows() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let opened = runtime.start_new_saved_session_editor();
        assert!(opened.editor_modal_visible);
        assert_eq!(opened.editor_section_text, "general");

        let closed = runtime.close_session_editor_modal();
        assert!(!closed.editor_modal_visible);

        let _ = runtime.start_new_saved_session_editor();
        let _ = runtime.update_editor_name("Modal Session");
        let _ = runtime.update_editor_host("modal.example.test");
        let saved = runtime
            .save_editor_to_saved_session()
            .expect("save editor session");
        assert!(!saved.editor_modal_visible);
    }

    #[test]
    fn session_editor_section_selection_updates_projection() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");
        let _ = runtime.start_new_saved_session_editor();

        assert_eq!(
            runtime.select_session_editor_authentication().editor_section_text,
            "authentication"
        );
        assert_eq!(runtime.select_session_editor_terminal().editor_section_text, "terminal");
        assert_eq!(runtime.select_session_editor_sftp().editor_section_text, "sftp");
        assert_eq!(runtime.select_session_editor_tunnels().editor_section_text, "tunnels");
        assert_eq!(runtime.select_session_editor_proxy().editor_section_text, "proxy");
        assert_eq!(runtime.select_session_editor_logging().editor_section_text, "logging");
        assert_eq!(runtime.select_session_editor_advanced().editor_section_text, "advanced");
        assert_eq!(
            runtime.select_session_editor_appearance().editor_section_text,
            "appearance"
        );
        assert_eq!(runtime.select_session_editor_general().editor_section_text, "general");
    }

    #[test]
    fn editor_folder_selection_controls_persistence_target() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        let mut parent = FolderProfile::new("folder-servers", "Servers");
        parent.folders.push(FolderProfile::new("folder-prod", "Prod"));
        document.folders.push(parent);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime.start_new_saved_session_editor();
        while runtime.editor.target_folder_id != "folder-prod" {
            let _ = runtime.select_next_editor_folder();
        }
        let _ = runtime.update_editor_name("Prod Session");
        let _ = runtime.update_editor_host("prod.example.test");
        let _ = runtime.update_editor_username("ops");

        let projection = runtime
            .save_editor_to_saved_session()
            .expect("save editor into nested folder");

        assert!(projection.editor_folder_known);
        assert!(projection
            .editor_folder_label_text
            .contains("Servers / Prod"));
        assert!(runtime
            .config_document
            .find_folder("folder-prod")
            .expect("prod folder")
            .sessions
            .iter()
            .any(|session| session.name == "Prod Session"));
        assert!(runtime
            .saved_session_inventory_parts()
            .0
            .contains("[folder] Servers"));
        assert!(runtime
            .saved_session_inventory_parts()
            .0
            .contains("[folder] Prod"));
    }

    #[test]
    fn editor_can_create_new_folder_under_current_target() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        let mut parent = FolderProfile::new("folder-servers", "Servers");
        parent.folders.push(FolderProfile::new("folder-prod", "Prod"));
        document.folders.push(parent);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        while runtime.editor.target_folder_id != "folder-prod" {
            let _ = runtime.select_next_editor_folder();
        }
        let _ = runtime.update_new_folder_name("Blue");

        let projection = runtime
            .create_folder_under_editor_target()
            .expect("create nested folder");

        assert!(projection.status_text.contains("Created folder `Blue`"));
        assert!(projection
            .editor_folder_label_text
            .contains("Servers / Prod / Blue"));
        assert_eq!(runtime.new_folder_name, "");
        let prod = runtime
            .config_document
            .find_folder("folder-prod")
            .expect("prod folder");
        assert!(prod.folders.iter().any(|folder| folder.name == "Blue"));
        assert!(runtime
            .saved_session_inventory_parts()
            .0
            .contains("[folder] Blue"));
    }

    #[test]
    fn editor_rejects_duplicate_folder_names_under_same_parent() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        let mut parent = FolderProfile::new("folder-servers", "Servers");
        let mut prod = FolderProfile::new("folder-prod", "Prod");
        prod.folders.push(FolderProfile::new("folder-blue", "Blue"));
        parent.folders.push(prod);
        document.folders.push(parent);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        while runtime.editor.target_folder_id != "folder-prod" {
            let _ = runtime.select_next_editor_folder();
        }
        let _ = runtime.update_new_folder_name("blue");

        let error = runtime
            .create_folder_under_editor_target()
            .expect_err("duplicate folder name should fail");

        assert!(error.to_string().contains("already exists under `Prod`"));
    }

    #[test]
    fn editor_can_add_tunnel_forward_and_persist_it() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime.start_new_saved_session_editor();
        let _ = runtime.update_editor_name("Tunnel Session");
        let _ = runtime.update_editor_host("tunnel.example.test");
        let _ = runtime.update_editor_username("ops");
        let _ = runtime.set_editor_host_key_policy_trust_on_first_use();
        let _ = runtime.set_editor_tunnel_kind_local();
        let _ = runtime.update_editor_tunnel_bind_host("127.0.0.1");
        let _ = runtime.update_editor_tunnel_bind_port("15432");
        let _ = runtime.update_editor_tunnel_target_host("db.internal");
        let _ = runtime.update_editor_tunnel_target_port("5432");
        let added = runtime
            .add_editor_tunnel_forward()
            .expect("add tunnel forward");
        assert_eq!(added.editor_tunnel_summary_kind_text, "rows");
        assert_eq!(added.editor_tunnel_summary_count, 1);
        assert!(added
            .editor_tunnel_summary_rows_text
            .contains("local 127.0.0.1:15432 -> db.internal:5432"));

        let saved = runtime
            .save_editor_to_saved_session()
            .expect("save tunnel session");
        assert!(saved.status_text.contains("Saved session editor changes"));
        let profile = runtime.saved_session_profiles().into_iter().next().expect("saved profile");
        let tunnel = profile.tunnel.expect("saved tunnel profile");
        assert_eq!(tunnel.forwards.len(), 1);
        assert_eq!(
            profile.host_key_policy,
            Some(ConfigHostKeyPolicy::TrustOnFirstUse)
        );
        assert_eq!(tunnel.forwards[0].kind, TunnelForwardKind::Local);
        assert_eq!(tunnel.forwards[0].bind_port, 15432);
        assert_eq!(tunnel.forwards[0].target_host, "db.internal");
        assert_eq!(tunnel.forwards[0].target_port, 5432);
    }

    #[test]
    fn global_logging_settings_are_visible_and_persisted() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let enabled = runtime.toggle_global_logging_enabled();
        assert_eq!(enabled.logging_enabled_text, "enabled");
        let raw = runtime.set_global_logging_format_raw();
        assert_eq!(raw.logging_format_text, "raw");
        let directory = runtime.update_global_logging_directory("audit/logs");
        assert_eq!(directory.logging_directory_text, "audit/logs");

        let saved = runtime
            .save_global_logging_settings()
            .expect("save logging settings");
        assert!(saved.status_text.contains("Saved global logging policy"));
        assert_eq!(saved.status_kind, "logging-saved");
        assert!(saved.status_param_1.contains("global=enabled"));
        assert_eq!(saved.status_param_2, "");

        let persisted = ConfigStore::new(temp.path())
            .load_or_recover()
            .expect("reload config")
            .document
            .logging;
        assert!(persisted.enabled);
        assert_eq!(persisted.format, "raw");
        assert_eq!(persisted.directory.as_deref(), Some("audit/logs"));
    }

    #[test]
    fn status_kind_expires_when_text_is_overwritten() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        runtime.set_status_kind(
            "session-deleted",
            "Deleted saved session `demo` and persisted the updated config.".to_owned(),
            "demo".to_owned(),
            String::new(),
        );
        let covered = runtime.projection();
        assert_eq!(covered.status_kind, "session-deleted");
        assert_eq!(covered.status_param_1, "demo");

        // Any mutation of the append channel after the kind snapshot invalidates
        // it, so Slint falls back to the English status_text.
        runtime.status_text = format!("{} Logging notice: rotated", runtime.status_text);
        let stale = runtime.projection();
        assert_eq!(stale.status_kind, "");
        assert_eq!(stale.status_param_1, "");
        assert_eq!(stale.status_param_2, "");
    }

    #[test]
    fn settings_terminal_save_persists_scrollback_limits() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let updated_lines = runtime.update_settings_scrollback_lines("25000");
        assert_eq!(updated_lines.settings_scrollback_lines_text, "25000");
        let updated_cells = runtime.update_settings_scrollback_max_cells("3000000");
        assert_eq!(updated_cells.settings_scrollback_max_cells_text, "3000000");

        let saved = runtime
            .save_settings_terminal()
            .expect("save terminal settings");

        assert_eq!(saved.settings_terminal_status_kind_text, "saved");
        assert_eq!(saved.settings_terminal_status_value_text, "25000");
        assert_eq!(saved.settings_terminal_status_limit_text, "3000000");
        let persisted = ConfigStore::new(temp.path())
            .load_or_recover()
            .expect("reload config")
            .document
            .terminal;
        assert_eq!(persisted.scrollback_lines, 25_000);
        assert_eq!(persisted.scrollback_max_cells, 3_000_000);
    }

    #[test]
    fn settings_terminal_save_applies_to_existing_sessions() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        let session_key = runtime.active_session_key().expect("active session");
        {
            let session = runtime
                .sessions
                .get_mut(&session_key)
                .expect("runtime session");
            assert_eq!(
                session.terminal_grid.scrollback_limits(),
                (DEFAULT_SCROLLBACK_LINES, DEFAULT_SCROLLBACK_MAX_CELLS)
            );
            for index in 0..200 {
                session.append_status_line(&format!("scrollback sample line {index}"));
            }
        }

        let _ = runtime.update_settings_scrollback_lines("120");
        let _ = runtime.update_settings_scrollback_max_cells("100000");
        runtime
            .save_settings_terminal()
            .expect("save terminal settings");

        let session = runtime.sessions.get(&session_key).expect("runtime session");
        assert_eq!(session.terminal_grid.scrollback_limits(), (120, 100_000));
        assert!(
            session.terminal_grid.scrollback_len() <= 120,
            "saving smaller limits must evict overflow"
        );
    }

    #[test]
    fn settings_terminal_rejects_invalid_input_without_persisting() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        for value in ["", "   ", "abc", "-5", "99", "1000001"] {
            let _ = runtime.update_settings_scrollback_lines(value);
            let rejected = runtime
                .save_settings_terminal()
                .expect("validation failure is not fatal");
            assert_ne!(
                rejected.settings_terminal_status_kind_text, "saved",
                "lines value {value:?} should be rejected"
            );
            assert!(!rejected.settings_terminal_status_kind_text.is_empty());
            assert_eq!(runtime.config_document.terminal, TerminalProfile::default());
        }

        let _ = runtime.update_settings_scrollback_lines("10000");
        for value in ["", "abc", "99999", "100000001"] {
            let _ = runtime.update_settings_scrollback_max_cells(value);
            let rejected = runtime
                .save_settings_terminal()
                .expect("validation failure is not fatal");
            assert_ne!(
                rejected.settings_terminal_status_kind_text, "saved",
                "cap value {value:?} should be rejected"
            );
            assert_eq!(runtime.config_document.terminal, TerminalProfile::default());
        }

        let persisted = ConfigStore::new(temp.path())
            .load_or_recover()
            .expect("reload config")
            .document
            .terminal;
        assert_eq!(persisted, TerminalProfile::default());
    }

    #[test]
    fn settings_terminal_reset_defaults_fills_inputs_without_persisting() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime.update_settings_scrollback_lines("500");
        let _ = runtime.update_settings_scrollback_max_cells("150000");
        runtime
            .save_settings_terminal()
            .expect("save custom limits");

        let projection = runtime.reset_settings_terminal_defaults();

        assert_eq!(projection.settings_scrollback_lines_text, "10000");
        assert_eq!(projection.settings_scrollback_max_cells_text, "2000000");
        assert_eq!(
            projection.settings_terminal_status_kind_text,
            "defaults-loaded"
        );
        assert_eq!(runtime.config_document.terminal.scrollback_lines, 500);
        assert_eq!(
            runtime.config_document.terminal.scrollback_max_cells,
            150_000
        );
    }

    #[test]
    fn editor_can_create_proxy_profile_and_persist_secret() {
        let _guard = lock_env();
        std::env::set_var("YSHELL_MASTER_PASSWORD", "proxy-master");
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime.start_new_saved_session_editor();
        let _ = runtime.update_editor_name("Proxy Session");
        let _ = runtime.update_editor_host("proxy-session.example.test");
        let _ = runtime.set_editor_proxy_mode_custom();
        let _ = runtime.set_editor_proxy_protocol_socks5();
        let _ = runtime.update_editor_proxy_host("127.0.0.1");
        let _ = runtime.update_editor_proxy_port("1080");
        let _ = runtime.update_editor_proxy_username("proxy-user");
        let _ = runtime.update_editor_proxy_password("proxy-secret");

        let projection = runtime
            .save_editor_to_saved_session()
            .expect("save proxy editor");

        assert_eq!(projection.editor_proxy_summary_kind_text, "custom");
        assert_eq!(projection.editor_proxy_protocol_text, "socks5");
        assert_eq!(
            projection.editor_proxy_summary_address_text,
            "127.0.0.1:1080"
        );
        assert_eq!(projection.editor_proxy_summary_user_text, "proxy-user");
        let saved = runtime.saved_session_profiles();
        let saved_session = &saved[0];
        let proxy = runtime
            .config_document
            .proxy_profiles
            .get(saved_session.proxy_profile_id.as_deref().expect("proxy id"))
            .expect("proxy profile");
        assert_eq!(proxy.protocol, ProxyProtocol::Socks5);
        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 1080);
        assert_eq!(proxy.username.as_deref(), Some("proxy-user"));
        let secret_key = proxy
            .password_secret_key
            .as_deref()
            .expect("proxy password secret key");
        let store_contents =
            fs::read_to_string(temp.path().join("secret-store.toml")).expect("read store");
        assert!(store_contents.contains(secret_key));
        assert!(!store_contents.contains("proxy-secret"));

        std::env::remove_var("YSHELL_MASTER_PASSWORD");
    }

    #[test]
    fn saved_session_proxy_profile_is_applied_to_runtime_shell_config() {
        let temp = tempdir().expect("tempdir");
        let keychain = Arc::new(FakeKeychain::new("proxy-master"));
        keychain
            .put(
                SecretRef::new("local://yshell/proxy-password"),
                SecretString::from("proxy-secret"),
            )
            .expect("seed proxy password");
        let keychain: Arc<dyn Keychain> = keychain;
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        document.proxy_profiles.insert(
            "proxy-1".to_owned(),
            ProxyProfile {
                id: "proxy-1".to_owned(),
                name: "SOCKS".to_owned(),
                protocol: ProxyProtocol::Socks5,
                host: "127.0.0.1".to_owned(),
                port: 1080,
                username: Some("proxy-user".to_owned()),
                resolve_dns_by_proxy: true,
                password_secret_key: Some("local://yshell/proxy-password".to_owned()),
            },
        );
        let mut profile = QuickConnectTarget {
            username: Some("ops".to_owned()),
            host: "proxy-runtime.example.test".to_owned(),
            port: 2200,
        }
        .into_session_profile("saved-proxy");
        profile.name = "Proxy Runtime".to_owned();
        profile.proxy_profile_id = Some("proxy-1".to_owned());
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain))
            .expect("runtime");
        runtime
            .open_saved_session("saved-proxy")
            .expect("open proxy session");

        let session_key = runtime.active_session_id.clone().expect("active session");
        let session = runtime.sessions.get(&session_key).expect("runtime session");
        match &session.ssh_config.proxy {
            ProxyConfig::Socks5 {
                address,
                username,
                password,
                resolve_dns_by_proxy,
            } => {
                assert_eq!(address, "127.0.0.1:1080");
                assert_eq!(username.as_deref(), Some("proxy-user"));
                assert_eq!(password.as_deref(), Some("proxy-secret"));
                assert!(*resolve_dns_by_proxy);
            }
            other => panic!("expected socks5 proxy, got {other:?}"),
        }
    }

    #[test]
    fn saved_session_tunnel_profile_is_applied_to_runtime_shell_config() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        let mut profile = QuickConnectTarget {
            username: Some("ops".to_owned()),
            host: "tunnel-runtime.example.test".to_owned(),
            port: 2222,
        }
        .into_session_profile("saved-tunnel");
        profile.name = "Tunnel Runtime".to_owned();
        profile.tunnel = Some(TunnelProfile {
            forwards: vec![
                TunnelForward {
                    kind: TunnelForwardKind::Local,
                    bind_host: "127.0.0.1".to_owned(),
                    bind_port: 18080,
                    target_host: "service.internal".to_owned(),
                    target_port: 8080,
                },
                TunnelForward {
                    kind: TunnelForwardKind::Dynamic,
                    bind_host: "127.0.0.1".to_owned(),
                    bind_port: 19050,
                    target_host: String::new(),
                    target_port: 0,
                },
            ],
        });
        let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
        folder.sessions.push(profile);
        document.folders.push(folder);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let projection = runtime
            .open_saved_session("saved-tunnel")
            .expect("open tunnel session");

        assert_eq!(projection.tunnels_summary_kind_text, "rows");
        assert!(projection
            .tunnels_summary_rows_text
            .contains("local 127.0.0.1:18080 -> service.internal:8080"));
        assert!(projection
            .tunnels_summary_rows_text
            .contains("dynamic 127.0.0.1:19050"));
        let session_key = runtime.active_session_id.clone().expect("active session");
        let session = runtime.sessions.get(&session_key).expect("runtime session");
        assert_eq!(session.ssh_config.tunnels.len(), 2);
        assert_eq!(session.ssh_config.tunnels[0].kind, ForwardingKind::Local);
        assert_eq!(session.ssh_config.tunnels[1].kind, ForwardingKind::Dynamic);
    }

    #[test]
    fn editor_can_create_password_saved_session_and_persist_secret() {
        let _guard = lock_env();
        let temp = tempdir().expect("tempdir");
        std::env::set_var("YSHELL_MASTER_PASSWORD", "editor-master");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime.start_new_saved_session_editor();
        let _ = runtime.update_editor_name("Password Session");
        let _ = runtime.update_editor_host("editor-password.example.test");
        let _ = runtime.update_editor_port("2222");
        let _ = runtime.update_editor_username("root");
        let _ = runtime.set_editor_auth_method_password();
        let _ = runtime.update_editor_password("super-secret");
        let projection = runtime
            .save_editor_to_saved_session()
            .expect("save editor session");

        assert!(projection.status_text.contains("Saved session editor changes"));
        let saved = runtime.saved_session_profiles();
        assert_eq!(saved.len(), 1);
        let saved_session = &saved[0];
        assert_eq!(saved_session.host, "editor-password.example.test");
        let auth = runtime
            .config_document
            .auth_profiles
            .get(saved_session.auth_profile_id.as_deref().expect("auth id"))
            .expect("auth profile");
        let secret_key = match &auth.method {
            ConfigAuthMethod::Password { secret_key } => secret_key.clone(),
            other => panic!("expected password auth, got {other:?}"),
        };
        let store_contents =
            fs::read_to_string(temp.path().join("secret-store.toml")).expect("read store");
        assert!(store_contents.contains(&secret_key));
        assert!(!store_contents.contains("super-secret"));

        std::env::remove_var("YSHELL_MASTER_PASSWORD");
    }

    #[test]
    fn editor_can_create_keyboard_interactive_saved_session_and_persist_secret() {
        let _guard = lock_env();
        let temp = tempdir().expect("tempdir");
        std::env::set_var("YSHELL_MASTER_PASSWORD", "editor-master");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime.start_new_saved_session_editor();
        let _ = runtime.update_editor_name("Keyboard Interactive Session");
        let _ = runtime.update_editor_host("editor-kbdint.example.test");
        let _ = runtime.update_editor_port("2222");
        let _ = runtime.update_editor_username("root");
        let _ = runtime.set_editor_auth_method_keyboard_interactive();
        let _ = runtime.update_editor_password("challenge-secret");
        let projection = runtime
            .save_editor_to_saved_session()
            .expect("save editor session");

        assert!(projection.status_text.contains("Saved session editor changes"));
        let saved = runtime.saved_session_profiles();
        assert_eq!(saved.len(), 1);
        let saved_session = &saved[0];
        let auth = runtime
            .config_document
            .auth_profiles
            .get(saved_session.auth_profile_id.as_deref().expect("auth id"))
            .expect("auth profile");
        let secret_key = match &auth.method {
            ConfigAuthMethod::KeyboardInteractive { secret_key } => secret_key.clone(),
            other => panic!("expected keyboard-interactive auth, got {other:?}"),
        };
        let store_contents =
            fs::read_to_string(temp.path().join("secret-store.toml")).expect("read store");
        assert!(store_contents.contains(&secret_key));
        assert!(!store_contents.contains("challenge-secret"));

        std::env::remove_var("YSHELL_MASTER_PASSWORD");
    }

    #[test]
    fn editor_password_save_requires_enabled_secret_store() {
        let _guard = lock_env();
        std::env::remove_var("YSHELL_MASTER_PASSWORD");
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");
        let _ = runtime.start_new_saved_session_editor();
        let _ = runtime.update_editor_name("Password Session");
        let _ = runtime.update_editor_host("editor-password.example.test");
        let _ = runtime.set_editor_auth_method_password();
        let _ = runtime.update_editor_password("super-secret");
        let error = runtime
            .save_editor_to_saved_session()
            .expect_err("password save should require secret store");

        assert!(error
            .to_string()
            .contains("requires an enabled secret store"));
    }

    #[test]
    fn fake_backend_editor_auth_test_succeeds_without_saving() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime.start_new_saved_session_editor();
        let _ = runtime.update_editor_name("Auth Test Session");
        let _ = runtime.update_editor_host("auth-test.example.test");
        let _ = runtime.update_editor_port("2222");
        let _ = runtime.update_editor_username("ops");

        let projection = runtime.test_editor_auth();

        assert_eq!(projection.editor_auth_test_kind_text, "success");
        assert!(projection
            .editor_auth_test_host_text
            .contains("auth-test.example.test:2222"));
        assert_eq!(projection.editor_auth_test_backend_text, "fake");
        assert_eq!(runtime.saved_session_profiles().len(), 0);
    }

    #[test]
    fn native_ssh_password_auth_test_runs_without_old_system_bridge_limitation() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime.select_native_ssh_transport_backend();
        let _ = runtime.start_new_saved_session_editor();
        let _ = runtime.update_editor_name("Password Auth Test");
        let _ = runtime.update_editor_host("password-auth.example.test");
        let _ = runtime.update_editor_username("root");
        let _ = runtime.set_editor_auth_method_password();
        let _ = runtime.update_editor_password("secret");

        let projection = runtime.test_editor_auth();

        assert!(
            matches!(
                projection.editor_auth_test_kind_text.as_str(),
                "success" | "failed"
            ),
            "auth test should still produce a structured status"
        );
    }

    #[test]
    fn editor_save_and_connect_opens_the_saved_session_runtime() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime.start_new_saved_session_editor();
        let _ = runtime.update_editor_name("Connect Session");
        let _ = runtime.update_editor_host("connect.example.test");
        let _ = runtime.update_editor_port("2224");
        let _ = runtime.update_editor_username("ops");
        let _ = runtime.set_editor_auth_method_agent();

        let projection = runtime.save_editor_and_connect().expect("save and connect");

        assert!(projection
            .active_session_name_text
            .contains("Connect Session"));
        assert_eq!(projection.tab_state_text, "connected");
        assert!(projection
            .terminal_body_text
            .contains("Fake shell established"));
        assert_eq!(runtime.saved_session_profiles().len(), 1);
    }

    #[test]
    fn session_search_filters_hierarchical_inventory() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        let mut parent = FolderProfile::new("folder-1", "Servers");
        let mut nested = FolderProfile::new("folder-2", "Prod");
        nested.sessions.push(
            QuickConnectTarget {
                username: Some("ops".to_owned()),
                host: "prod.example.test".to_owned(),
                port: 22,
            }
            .into_session_profile("session-prod"),
        );
        parent.sessions.push(
            QuickConnectTarget {
                username: Some("dev".to_owned()),
                host: "dev.example.test".to_owned(),
                port: 22,
            }
            .into_session_profile("session-dev"),
        );
        parent.folders.push(nested);
        document.folders.push(parent);
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let filtered = runtime.update_session_search("prod");

        assert_eq!(filtered.session_search_text, "prod");
        assert!(filtered
            .saved_session_inventory_rows_text
            .contains("[folder] Servers"));
        assert!(filtered
            .saved_session_inventory_rows_text
            .contains("[folder] Prod"));
        assert!(filtered
            .saved_session_inventory_rows_text
            .contains("prod.example.test"));
        assert!(!filtered
            .saved_session_inventory_rows_text
            .contains("dev.example.test"));
    }

    #[test]
    fn active_terminal_input_flows_through_shell_boundary() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let projection = runtime
            .send_active_terminal_input("pwd\n")
            .expect("send input");

        assert!(projection.status_text.contains("Sent"));
        assert!(projection.terminal_body_text.contains("fake-shell received input"));
    }

    #[test]
    fn terminal_projection_exposes_grid_lines_and_cursor_position() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let projection = runtime
            .send_active_terminal_input("echo grid\n")
            .expect("send input");

        assert!(projection
            .terminal_visible_lines
            .iter()
            .any(|line| line.contains("fake-shell received input")));
        assert!(projection.terminal_cursor_row >= 0);
        assert!(projection.terminal_cursor_column >= 0);
    }

    #[test]
    fn enabled_logging_writes_fake_terminal_transcript() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let document = ConfigDocument {
            logging: LoggingProfile {
                enabled: true,
                directory: Some("logs".to_owned()),
                format: "sanitized".to_owned(),
            },
            ..ConfigDocument::default()
        };
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let _ = runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let log_files = collect_log_files(&temp.path().join("logs"));
        assert!(!log_files.is_empty());
        let combined = log_files
            .iter()
            .filter_map(|path| fs::read_to_string(path).ok())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("Connecting to example.com:2200 as alice"));
        assert!(combined.contains("Fake shell established"));
    }

    #[test]
    fn active_terminal_resize_flows_through_shell_boundary() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let projection = runtime
            .resize_active_terminal(100, 40)
            .expect("resize terminal");

        assert!(projection.status_text.contains("100x40"));
        assert!(projection.terminal_body_text.contains("fake-shell resized"));
    }

    #[test]
    fn passive_terminal_resize_skips_duplicate_dimensions() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let first = runtime
            .sync_active_terminal_size_passive(100, 40)
            .expect("first passive resize");
        assert!(first.is_some());

        let second = runtime
            .sync_active_terminal_size_passive(100, 40)
            .expect("second passive resize");
        assert!(second.is_none());
    }

    #[test]
    fn terminal_key_input_flows_through_the_shell_boundary() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let typed = runtime
            .send_active_terminal_key("x", false, false, false, false)
            .expect("send key");
        assert!(typed.status_text.contains("Sent 1 bytes"));
        assert!(typed
            .terminal_body_text
            .contains("fake-shell received input: x"));

        let entered = runtime
            .send_active_terminal_key("\u{000a}", false, false, false, false)
            .expect("send enter");
        assert!(entered
            .terminal_body_text
            .contains("fake-shell received input: \\r"));

        let control = runtime
            .send_active_terminal_key("c", true, false, false, false)
            .expect("send ctrl+c");
        assert!(control.status_text.contains("Sent 1 bytes"));
    }

    #[test]
    fn active_terminal_resize_keeps_the_render_grid_in_sync() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let projection = runtime.resize_active_terminal(50, 12).expect("resize");

        assert_eq!(projection.terminal_visible_lines.len(), 12);
        let metrics = runtime
            .active_terminal_viewport_metrics()
            .expect("viewport metrics");
        assert_eq!(metrics.columns, 50);
        assert_eq!(metrics.rows, 12);
    }

    #[test]
    fn terminal_scroll_and_selection_round_trip_through_the_projection() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        let _ = runtime
            .resize_active_terminal(24, 4)
            .expect("narrow the terminal");
        let _ = runtime
            .send_active_terminal_input("one\ntwo\nthree\nfour\nfive\n")
            .expect("feed output");
        let frame_before = runtime.projection().terminal_frame_id;
        assert!(
            runtime
                .active_terminal_viewport_metrics()
                .expect("metrics")
                .top_absolute_row
                > 0
        );

        let top = runtime
            .active_terminal_viewport_metrics()
            .expect("metrics")
            .top_absolute_row;
        let _ = runtime
            .begin_active_terminal_selection(0, u16::try_from(top).expect("row fits"))
            .expect("begin selection");
        let selected = runtime
            .update_active_terminal_selection(2, u16::try_from(top).expect("row fits"))
            .expect("extend selection");
        assert!(selected.terminal_selection_active);
        assert!(selected.terminal_frame_id > frame_before);

        let copied = runtime
            .copy_active_terminal_selection()
            .expect("copy selection");
        assert!(copied.status_text.contains("Copied"));
        assert!(runtime.terminal_clipboard_text().chars().count() >= 3);

        let scrolled = runtime.scroll_active_terminal(1).expect("scroll back");
        assert_eq!(scrolled.terminal_scroll_offset, 1);

        let all = runtime
            .select_all_active_terminal()
            .expect("select all terminal");
        assert!(all.terminal_selection_active);
        assert!(runtime.active_terminal_selection_text().contains("one"));

        let bottom = runtime
            .scroll_active_terminal_to_bottom()
            .expect("scroll to bottom");
        assert_eq!(bottom.terminal_scroll_offset, 0);
    }

    #[test]
    fn terminal_scrollbar_projection_and_scroll_to_line_clamp_at_both_ends() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        let _ = runtime
            .resize_active_terminal(24, 4)
            .expect("narrow the terminal");
        let _ = runtime
            .send_active_terminal_input("one\ntwo\nthree\nfour\nfive\n")
            .expect("feed output");

        let projection = runtime.projection();
        let rows = projection.terminal_viewport_rows;
        let total = projection.terminal_scrollback_lines;
        assert_eq!(rows, 4);
        assert!(total > rows, "expected scrollback lines, got {total} lines");
        let max_offset = total - rows;

        // Line 0 pins the viewport to the oldest line: the scroll offset is at its max.
        let top = runtime.scroll_active_terminal_to_line(0).expect("scroll to top");
        assert_eq!(
            top.terminal_scroll_offset,
            u64::try_from(max_offset).expect("max offset fits")
        );
        assert_eq!(
            runtime
                .active_terminal_viewport_metrics()
                .expect("metrics")
                .top_absolute_row,
            0
        );

        // Past the top / past the bottom both clamp instead of wrapping around.
        let above_top = runtime
            .scroll_active_terminal_to_line(-5)
            .expect("clamp above the top");
        assert_eq!(above_top.terminal_scroll_offset, top.terminal_scroll_offset);
        let below_bottom = runtime
            .scroll_active_terminal_to_line(max_offset + 99)
            .expect("clamp below the bottom");
        assert_eq!(below_bottom.terminal_scroll_offset, 0);

        // The scrollbar's bottom-most line is the live view (offset 0).
        let last_line = runtime
            .scroll_active_terminal_to_line(max_offset)
            .expect("scroll to the last line");
        assert_eq!(last_line.terminal_scroll_offset, 0);

        // A line in the middle positions the viewport top exactly on it.
        let middle = max_offset / 2;
        let scrolled = runtime
            .scroll_active_terminal_to_line(middle)
            .expect("scroll to the middle line");
        assert_eq!(
            scrolled.terminal_scroll_offset,
            u64::try_from(max_offset - middle).expect("offset fits")
        );
        assert_eq!(
            runtime
                .active_terminal_viewport_metrics()
                .expect("metrics")
                .top_absolute_row,
            usize::try_from(middle).expect("line fits")
        );
        // The geometry projection keeps reporting the same content extent.
        assert_eq!(scrolled.terminal_scrollback_lines, total);
        assert_eq!(scrolled.terminal_viewport_rows, rows);
    }

    #[test]
    fn copy_paste_clear_and_find_work_on_visible_terminal_text() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        runtime
            .send_active_terminal_input("echo secret\n")
            .expect("send input");

        let copied = runtime
            .copy_active_terminal_visible_text()
            .expect("copy visible terminal");
        assert!(copied.status_text.contains("Copied"));

        let found = runtime
            .find_in_active_terminal("secret")
            .expect("find in terminal");
        assert_eq!(found.terminal_search_kind_text, "found");
        assert_eq!(found.terminal_search_match_count, 1);
        assert_eq!(found.terminal_search_current_index, 1);
        assert_eq!(found.terminal_search_query_text, "secret");

        let next = runtime.select_next_terminal_match().expect("next match");
        assert_eq!(next.terminal_search_kind_text, "found");
        assert_eq!(next.terminal_search_current_index, 1);

        let previous = runtime
            .select_previous_terminal_match()
            .expect("previous match");
        assert_eq!(previous.terminal_search_kind_text, "found");
        assert_eq!(previous.terminal_search_current_index, 1);

        let cleared = runtime.clear_active_terminal().expect("clear terminal");
        assert!(cleared.status_text.contains("Cleared"));
        assert!(cleared.terminal_body_text.contains("Terminal view cleared"));

        let pasted = runtime.paste_terminal_clipboard().expect("paste clipboard");
        assert!(pasted.status_text.contains("Sent"));
        assert!(pasted.terminal_body_text.contains("echo secret"));
    }

    #[test]
    fn fake_backend_sftp_refresh_explains_the_missing_native_ssh_path() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let projection = runtime.refresh_active_sftp_listing().expect("refresh sftp");

        assert!(projection.status_text.contains("SFTP unavailable"));
        assert_eq!(projection.sftp_listing_kind_text, "sync-fake-backend");
        assert_eq!(projection.sftp_session_status_kind_text, "unavailable");
        assert_eq!(
            projection.sftp_session_status_reason_text,
            "select-native-ssh"
        );
    }

    #[test]
    fn sftp_lifecycle_tracks_backend_and_runtime_session_state() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let startup = runtime.prepare_desktop_startup_projection();
        assert_eq!(startup.sftp_session_status_kind_text, "unavailable");
        assert_eq!(
            startup.sftp_session_status_reason_text,
            "desktop-startup-fake"
        );
        assert_eq!(startup.sftp_listing_kind_text, "desktop-startup");

        let connected = runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        assert_eq!(connected.sftp_session_status_kind_text, "unavailable");

        let native = runtime.select_native_ssh_transport_backend();
        assert_eq!(native.sftp_session_status_kind_text, "disconnected");
        assert_eq!(native.sftp_listing_kind_text, "sync-reconnect-native");

        let draft = runtime.handle_new_session();
        assert_eq!(draft.sftp_session_status_kind_text, "disconnected");
        assert_eq!(draft.sftp_listing_kind_text, "draft-disconnected");
    }

    #[test]
    fn sftp_path_helpers_normalize_and_join() {
        assert_eq!(normalize_remote_path("/srv//logs").unwrap(), "/srv//logs");
        assert_eq!(parent_remote_path("/srv/logs"), "/srv");
        assert_eq!(parent_remote_path("/srv"), "/");
        assert_eq!(join_remote_path("/srv", "file.txt"), "/srv/file.txt");
        assert_eq!(join_remote_path("/", "file.txt"), "/file.txt");
    }

    #[test]
    fn sftp_product_operation_inputs_are_projected() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let projection = runtime.set_sftp_operation_inputs(
            "C:/tmp/upload.txt",
            "/srv/source.txt",
            "renamed.txt",
            "755",
        );

        assert_eq!(projection.sftp_local_path_text, "C:/tmp/upload.txt");
        assert_eq!(projection.sftp_remote_target_text, "/srv/source.txt");
        assert_eq!(projection.sftp_secondary_target_text, "renamed.txt");
        assert_eq!(projection.sftp_permissions_text, "755");
    }

    #[test]
    fn sftp_summary_counts_and_sort_kind_are_projected() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime.sftp_entries = vec![
            FsEntry::directory("/srv/folder"),
            FsEntry::file("/srv/report.txt", 42),
        ];

        let projection = runtime.projection();
        assert_eq!(projection.sftp_item_count, 2);
        assert_eq!(projection.sftp_dir_count, 1);
        assert_eq!(projection.sftp_item_summary_text, "2 items · 1 dir");

        let sorted = runtime.sort_sftp_by("size");
        assert_eq!(sorted.status_kind, "sftp-sorted");
        assert_eq!(sorted.status_param_1, "size");
        assert_eq!(sorted.status_param_2, "ascending");
        assert!(sorted.status_text.contains("Sorted SFTP entries by Size ↑"));
    }

    #[test]
    fn sftp_transfer_queue_is_visible_in_projection() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let initial = runtime.projection();
        assert!(initial.transfer_queue_empty);
        assert_eq!(initial.transfer_queue_rows_text, "");

        let upload_id = runtime.enqueue_sftp_transfer(
            TransferDirection::Upload,
            "C:/tmp/upload.txt".to_owned(),
            "/srv/upload.txt".to_owned(),
            Some(10),
        );
        let running = runtime.projection();
        assert!(!running.transfer_queue_empty);
        assert!(running.transfer_queue_rows_text.contains(&upload_id));
        assert!(running.transfer_queue_rows_text.contains("upload running"));
        assert!(running.transfer_queue_rows_text.contains("0%"));

        runtime.complete_sftp_transfer(&upload_id, 10);
        let completed = runtime.projection();
        assert!(completed
            .transfer_queue_rows_text
            .contains("upload completed 100%"));

        let download_id = runtime.enqueue_sftp_transfer(
            TransferDirection::Download,
            "/srv/download.txt".to_owned(),
            "C:/tmp/download.txt".to_owned(),
            None,
        );
        runtime.fail_sftp_transfer(&download_id, "network".to_owned());
        let failed = runtime.projection();
        assert!(failed.transfer_queue_rows_text.contains("download failed"));
        assert!(failed.transfer_queue_rows_text.contains("error=network"));
    }

    #[test]
    fn fake_backend_gates_sftp_mutation_entry_points() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        runtime.set_sftp_operation_inputs("", "/srv/source.txt", "renamed.txt", "755");

        let mkdir = runtime.create_sftp_directory().expect("mkdir gate");
        assert!(mkdir.status_text.contains("native-ssh backend"));

        let rename = runtime.rename_sftp_path().expect("rename gate");
        assert!(rename.status_text.contains("native-ssh backend"));

        let chmod = runtime.chmod_sftp_path().expect("chmod gate");
        assert!(chmod.status_text.contains("native-ssh backend"));

        let remote_edit = runtime.start_sftp_remote_edit().expect("remote edit gate");
        assert!(remote_edit.status_text.contains("native-ssh backend"));
    }

    #[test]
    fn sftp_remote_edit_session_is_visible_and_cancelable() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let local_temp_path = temp.path().join("remote-edit.txt");
        fs::write(&local_temp_path, b"draft").expect("write temp edit");

        runtime.remote_edit_session = Some(RemoteEditSession::new(
            "/srv/remote-edit.txt",
            local_temp_path.display().to_string(),
        ));
        let active = runtime.projection();
        assert!(active.sftp_remote_edit_active);
        assert!(active
            .sftp_remote_edit_remote_path_text
            .contains("/srv/remote-edit.txt"));
        assert!(active
            .sftp_remote_edit_local_path_text
            .contains(&local_temp_path.display().to_string()));

        let canceled = runtime
            .cancel_sftp_remote_edit()
            .expect("cancel remote edit");
        assert!(!canceled.sftp_remote_edit_active);
        assert_eq!(canceled.sftp_remote_edit_remote_path_text, "");
        assert!(!local_temp_path.exists());
    }

    #[test]
    fn sftp_secondary_target_and_permissions_helpers_validate_inputs() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime.sftp_path = "/srv".to_owned();

        runtime.set_sftp_operation_inputs("", "", "logs", "644");
        assert_eq!(
            runtime
                .sftp_secondary_target_path("missing target")
                .expect("relative target"),
            "/srv/logs"
        );

        runtime.set_sftp_operation_inputs("", "", "/var/logs", "0o755");
        assert_eq!(
            runtime
                .sftp_secondary_target_path("missing target")
                .expect("absolute target"),
            "/var/logs"
        );
        assert_eq!(parse_sftp_permissions("644").expect("644"), 0o644);
        assert_eq!(parse_sftp_permissions("0o755").expect("755"), 0o755);
        assert!(parse_sftp_permissions("888").is_err());
        assert!(parse_sftp_permissions("64").is_err());
    }

    fn demo_file(path: &str, size_bytes: u64) -> FsEntry {
        let mut entry = FsEntry::file(path, size_bytes);
        entry.modified = Some(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        entry
    }

    #[test]
    fn sftp_selection_sorting_and_breadcrumbs_follow_visible_rows() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime.sftp_path = "/demo".to_owned();
        runtime.sftp_entries = vec![
            FsEntry::directory("/demo/sub"),
            FsEntry::directory("/demo/zeta"),
            demo_file("/demo/a.txt", 1_024),
            demo_file("/demo/b.txt", 4_096),
        ];

        let initial = runtime.projection();
        assert_eq!(initial.sftp_rows.len(), 4);
        assert!(initial.sftp_rows[0].is_dir);
        assert!(initial.sftp_rows[1].is_dir);
        assert_eq!(initial.sftp_selected_index, -1);
        assert_eq!(initial.sftp_item_summary_text, "4 items · 2 dirs");
        assert_eq!(initial.sftp_sort_column_text, "name");
        assert!(initial.sftp_sort_ascending);
        assert_eq!(initial.sftp_empty_text, "");
        assert_eq!(initial.sftp_rows[0].size_text, "");
        assert_eq!(initial.sftp_rows[0].permissions_text, "drwxr-xr-x");
        assert_eq!(initial.sftp_rows[2].kind_text, "File");
        assert_eq!(initial.sftp_rows[2].size_text, "1.0 KB");
        assert_eq!(initial.sftp_rows[2].modified_text, "2023-11-14 22:13");

        let selected = runtime.select_sftp_entry(1);
        assert_eq!(selected.sftp_selected_index, 1);
        assert_eq!(selected.sftp_selected_name_text, "zeta");
        assert_eq!(selected.sftp_selected_path_text, "/demo/zeta");
        assert_eq!(selected.sftp_selected_permissions_text, "755");

        // Flipping the direction keeps the same entry selected by path.
        let sorted = runtime.sort_sftp_by("name");
        assert_eq!(sorted.sftp_sort_column_text, "name");
        assert!(!sorted.sftp_sort_ascending);
        assert_eq!(sorted.sftp_selected_index, 0);
        assert_eq!(sorted.sftp_selected_name_text, "zeta");

        // Size sorting keeps directories first and preserves the selection.
        let by_size = runtime.sort_sftp_by("size");
        assert_eq!(by_size.sftp_sort_column_text, "size");
        assert!(by_size.sftp_sort_ascending);
        assert_eq!(
            by_size
                .sftp_rows
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["sub", "zeta", "a.txt", "b.txt"]
        );
        assert_eq!(by_size.sftp_selected_index, 1);
        assert_eq!(by_size.sftp_selected_name_text, "zeta");
        let bogus = runtime.sort_sftp_by("bogus").sftp_sort_column_text;
        assert_eq!(bogus, "size");
        assert!(runtime.status_text.contains("Unknown SFTP sort column"));

        // Breadcrumbs expose the clickable parents of the current path.
        let crumbs = runtime.projection().sftp_crumbs;
        assert_eq!(crumbs.len(), 2);
        assert_eq!(crumbs[0].label, "/");
        assert_eq!(crumbs[0].path, "/");
        assert_eq!(
            (crumbs[1].label.as_str(), crumbs[1].path.as_str()),
            ("demo", "/demo")
        );

        // Activating a directory navigates into it even without a live session.
        runtime.select_sftp_entry(0);
        let activated = runtime.activate_sftp_entry().expect("activate directory");
        assert_eq!(activated.sftp_path_text, "/demo/sub");
        assert_eq!(activated.sftp_crumbs.len(), 3);

        let navigated = runtime.open_sftp_crumb(2).expect("open crumb");
        assert_eq!(navigated.sftp_path_text, "/demo/sub");
        let root = runtime.open_sftp_crumb(0).expect("open root crumb");
        assert_eq!(root.sftp_path_text, "/");

        // Clearing the selection resets the selection-derived projection.
        let cleared = runtime.select_sftp_entry(-1);
        assert_eq!(cleared.sftp_selected_index, -1);
        assert_eq!(cleared.sftp_selected_name_text, "");
        assert_eq!(cleared.sftp_selected_path_text, "");
        assert_eq!(cleared.sftp_selected_permissions_text, "");
    }

    #[test]
    fn sftp_hidden_toggle_filters_dotfiles_and_reports_empty_state() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime.sftp_path = "/demo".to_owned();
        runtime.sftp_entries = vec![
            demo_file("/demo/.secret", 12),
            demo_file("/demo/readme.txt", 2_048),
        ];

        let hidden_off = runtime.projection();
        assert!(!hidden_off.sftp_show_hidden);
        assert_eq!(hidden_off.sftp_rows.len(), 1);
        assert_eq!(hidden_off.sftp_rows[0].name, "readme.txt");
        assert_eq!(hidden_off.sftp_item_summary_text, "1 item · 0 dirs");

        let shown = runtime.toggle_sftp_hidden_files();
        assert!(shown.sftp_show_hidden);
        assert!(shown.status_text.contains("now shown"));
        assert_eq!(shown.sftp_rows.len(), 2);
        assert_eq!(shown.sftp_rows[0].name, ".secret");

        // Selecting the visible dotfile and hiding hidden files clears it again.
        runtime.select_sftp_entry(0);
        assert_eq!(runtime.projection().sftp_selected_name_text, ".secret");
        let rehidden = runtime.toggle_sftp_hidden_files();
        assert!(!rehidden.sftp_show_hidden);
        assert!(rehidden.status_text.contains("now hidden"));
        assert_eq!(rehidden.sftp_selected_index, -1);
        assert_eq!(rehidden.sftp_rows.len(), 1);

        // A directory with only dotfiles explains why the list is empty.
        runtime.sftp_entries = vec![demo_file("/demo/.only", 1)];
        let empty = runtime.projection();
        assert!(empty.sftp_rows.is_empty());
        assert!(empty.sftp_empty_text.contains("Enable Hidden"));
        runtime.toggle_sftp_hidden_files();
        let revealed = runtime.projection();
        assert_eq!(revealed.sftp_rows.len(), 1);
        assert!(revealed.sftp_empty_text.is_empty());
    }

    #[test]
    fn sftp_selection_operations_assemble_existing_operation_inputs() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        runtime.sftp_path = "/demo".to_owned();
        runtime.sftp_entries = vec![
            FsEntry::directory("/demo/sub"),
            demo_file("/demo/readme.txt", 2_048),
        ];
        runtime.select_sftp_entry(1);

        let rename = runtime
            .rename_sftp_selected("renamed.txt")
            .expect("rename gate");
        assert_eq!(runtime.sftp_remote_target, "/demo/readme.txt");
        assert_eq!(runtime.sftp_secondary_target, "renamed.txt");
        assert!(rename.status_text.contains("native-ssh backend"));

        let chmod = runtime.chmod_sftp_selected("0o600").expect("chmod gate");
        assert_eq!(runtime.sftp_remote_target, "/demo/readme.txt");
        assert_eq!(runtime.sftp_permissions, "0o600");
        assert!(chmod.status_text.contains("native-ssh backend"));

        let delete = runtime.delete_sftp_selected().expect("delete gate");
        assert!(delete.status_text.contains("native-ssh backend"));

        let created = runtime
            .create_sftp_folder_named("logs")
            .expect("mkdir gate");
        assert_eq!(runtime.sftp_secondary_target, "logs");
        assert!(created.status_text.contains("native-ssh backend"));

        let upload = runtime
            .upload_sftp_into_current("C:/tmp/upload.txt")
            .expect("upload gate");
        assert_eq!(runtime.sftp_local_path, "C:/tmp/upload.txt");
        assert!(runtime.sftp_remote_target.is_empty());
        assert!(upload.status_text.contains("native-ssh backend"));

        let download = runtime
            .download_sftp_selected("C:/tmp/download.txt")
            .expect("download gate");
        assert_eq!(runtime.sftp_local_path, "C:/tmp/download.txt");
        assert!(download.status_text.contains("native-ssh backend"));

        let edit = runtime.edit_sftp_selected().expect("edit gate");
        assert!(edit.status_text.contains("native-ssh backend"));

        // Empty inputs and a cleared selection are rejected before any backend work.
        let empty_name = runtime.rename_sftp_selected("   ").expect("empty rename");
        assert!(empty_name.status_text.contains("must not be empty"));
        let empty_folder = runtime
            .create_sftp_folder_named("")
            .expect("empty folder name");
        assert!(empty_folder.status_text.contains("must not be empty"));
        let empty_upload = runtime
            .upload_sftp_into_current("")
            .expect("empty upload path");
        assert!(empty_upload.status_text.contains("must not be empty"));

        runtime.select_sftp_entry(-1);
        let no_selection = runtime.delete_sftp_selected().expect("no selection");
        assert!(no_selection.status_text.contains("Select an SFTP entry"));
        let no_crumb = runtime.open_sftp_crumb(9).expect("missing crumb");
        assert!(no_crumb.status_text.contains("no longer available"));
    }

    #[test]
    fn disconnect_and_reconnect_update_runtime_state() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let disconnected = runtime
            .disconnect_active_session()
            .expect("disconnect active session");
        assert_eq!(disconnected.tab_state_text, "disconnected");

        let reconnected = runtime
            .reconnect_active_session()
            .expect("reconnect active session");
        assert_eq!(reconnected.tab_state_text, "connected");
        assert!(reconnected
            .terminal_body_text
            .contains("Fake shell established"));
    }

    #[test]
    fn native_ssh_backend_marks_failed_when_non_ssh_target_rejects_native_handshake() {
        let (port, accept_handle) = start_tcp_probe_target();
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let projection = runtime.select_native_ssh_transport_backend();
        assert_eq!(projection.transport_backend_text, "native-ssh");

        let projection = runtime
            .handle_quick_connect(&format!("alice@127.0.0.1:{port}"))
            .expect("quick connect");
        accept_handle.join().expect("accept thread");

        assert_eq!(projection.tab_state_text, "failed");
        assert!(projection
            .status_text
            .contains("live shell was not reached"));
        assert!(projection
            .terminal_body_text
            .contains("Shell runtime failed"));
    }

    #[test]
    fn transport_backend_selection_is_runtime_visible() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let fake_projection = runtime.select_fake_transport_backend();
        assert_eq!(fake_projection.transport_backend_text, "fake");
        assert!(fake_projection.status_text.contains("deterministic in-process shell adapter"));

        let native_projection = runtime.select_native_ssh_transport_backend();
        assert_eq!(
            native_projection.transport_backend_text,
            "native-ssh"
        );
        assert!(native_projection
            .status_text
            .contains("embedded ssh2 shell path"));
    }

    #[test]
    fn desktop_startup_stays_fake_without_losing_startup_context() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let projection = runtime.prepare_desktop_startup_projection();

        assert_eq!(projection.transport_backend_text, "fake");
        assert!(projection.status_text.contains("Saved sessions discovered"));
        assert!(projection
            .status_text
            .contains("Desktop startup stays on `fake`"));
        assert_eq!(projection.sftp_listing_kind_text, "desktop-startup");
        assert_eq!(projection.sftp_session_status_kind_text, "unavailable");
        assert_eq!(
            projection.sftp_session_status_reason_text,
            "desktop-startup-fake"
        );
    }

    #[test]
    fn save_active_session_persists_minimal_profile() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let projection = runtime.save_active_session().expect("save active session");

        assert!(projection.status_text.contains("Saved active runtime session"));
        assert_eq!(runtime.saved_session_profiles().len(), 1);
        assert_eq!(runtime.saved_session_profiles()[0].host, "example.com");
        assert_eq!(runtime.saved_session_profiles()[0].port, 2200);
    }

    #[test]
    fn open_saved_session_reuses_saved_profile_path() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("ops@saved.example.test:2222")
            .expect("quick connect");
        runtime.save_active_session().expect("save active session");

        let profile_id = runtime.saved_session_profiles()[0].id.clone();
        let projection = runtime
            .open_saved_session(&profile_id)
            .expect("open saved session");

        assert!(projection
            .active_session_name_text
            .contains("saved.example.test"));
        assert_eq!(projection.active_session_kind_text, "session");
        assert!(projection
            .terminal_body_text
            .contains("Fake shell established"));
    }

    #[test]
    fn saved_session_selection_rotates_and_opens_selected_profile() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("ops@one.example.test:2222")
            .expect("quick connect");
        runtime.save_active_session().expect("save active session");
        runtime
            .handle_quick_connect("ops@two.example.test:2223")
            .expect("quick connect");
        runtime.save_active_session().expect("save active session");

        let initial = runtime.projection();
        assert_eq!(initial.saved_session_selection_kind_text, "profile");
        assert!(initial
            .saved_session_selection_name_text
            .contains("two.example.test"));

        let previous = runtime.select_previous_saved_session();
        assert!(previous
            .saved_session_selection_name_text
            .contains("one.example.test"));
        assert!(previous
            .saved_session_inventory_rows_text
            .contains("> ops@one.example.test:2222"));

        let opened = runtime
            .open_selected_saved_session()
            .expect("open selected saved session");
        assert!(opened.active_session_name_text.contains("one.example.test"));
        assert!(!opened.recent_sessions_empty);
        assert!(opened
            .recent_sessions_rows_text
            .contains("one.example.test"));
    }

    /// Config document shared by the sidebar session-tree tests:
    ///
    /// ```text
    /// Saved Sessions (root)
    ///   - One          (one.example.test)
    ///   - Prod (folder)
    ///       - Prod DB  (ops@db.example.test)
    ///       - Cache    (cache.example.test)
    /// ```
    fn saved_session_tree_store(config_dir: &Path) -> ConfigStore {
        let store = ConfigStore::new(config_dir);
        let mut document = ConfigDocument::default();
        let mut root = FolderProfile::new(SAVED_SESSIONS_FOLDER_ID, SAVED_SESSIONS_FOLDER_NAME);
        root.sessions
            .push(SessionProfile::new("saved-one", "One", "one.example.test"));
        let mut prod = FolderProfile::new("folder-prod", "Prod");
        let mut database = SessionProfile::new("saved-prod-db", "Prod DB", "db.example.test");
        database.username = Some("ops".to_owned());
        prod.sessions.push(database);
        prod.sessions
            .push(SessionProfile::new("saved-prod-cache", "Cache", "cache.example.test"));
        root.folders.push(prod);
        document.folders.push(root);
        store.save(&document).expect("save config");
        store
    }

    #[test]
    fn session_tree_flattens_folders_and_sessions_in_display_order() {
        let temp = tempdir().expect("tempdir");
        let _store = saved_session_tree_store(temp.path());
        let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let rows = runtime.projection().session_tree_rows;
        let outline = rows
            .iter()
            .map(|row| (row.kind.as_str(), row.depth, row.label.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            outline,
            vec![
                ("folder", 0, SAVED_SESSIONS_FOLDER_NAME),
                ("session", 1, "One"),
                ("folder", 1, "Prod"),
                ("session", 2, "Prod DB"),
                ("session", 2, "Cache"),
            ]
        );
        // 文件夹 detail = 子树内可见会话数；会话 detail = user@host。
        assert_eq!(rows[0].detail, "3");
        assert_eq!(rows[2].detail, "2");
        assert_eq!(rows[1].detail, "one.example.test");
        assert_eq!(rows[3].detail, "ops@db.example.test");
        assert!(rows
            .iter()
            .filter(|row| row.kind == "folder")
            .all(|row| row.expanded));
        assert!(rows
            .iter()
            .filter(|row| row.kind == "session")
            .all(|row| !row.expanded));
        // hydrate 默认选中第一个已保存会话。
        assert!(rows
            .iter()
            .any(|row| row.id == "saved-one" && row.selected));
    }

    #[test]
    fn session_tree_collapse_hides_children_and_expand_restores_them() {
        let temp = tempdir().expect("tempdir");
        let _store = saved_session_tree_store(temp.path());
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let collapsed = runtime
            .toggle_saved_folder("folder-prod")
            .expect("toggle prod folder");
        assert_eq!(collapsed.session_tree_rows.len(), 3);
        let prod = collapsed
            .session_tree_rows
            .iter()
            .find(|row| row.id == "folder-prod")
            .expect("prod row");
        assert!(!prod.expanded);
        assert!(!collapsed
            .session_tree_rows
            .iter()
            .any(|row| row.id == "saved-prod-db" || row.id == "saved-prod-cache"));

        let root_collapsed = runtime
            .toggle_saved_folder(SAVED_SESSIONS_FOLDER_ID)
            .expect("collapse root folder");
        assert_eq!(root_collapsed.session_tree_rows.len(), 1);
        assert_eq!(
            root_collapsed.session_tree_rows[0].id,
            SAVED_SESSIONS_FOLDER_ID
        );
        assert!(!root_collapsed.session_tree_rows[0].expanded);
        assert_eq!(root_collapsed.session_tree_rows[0].detail, "3");

        let root_expanded = runtime
            .toggle_saved_folder(SAVED_SESSIONS_FOLDER_ID)
            .expect("expand root folder");
        assert_eq!(root_expanded.session_tree_rows.len(), 3);
        let all_expanded = runtime
            .toggle_saved_folder("folder-prod")
            .expect("expand prod folder");
        assert_eq!(all_expanded.session_tree_rows.len(), 5);

        // 未知文件夹 id 报错，且不改变已有折叠状态。
        let missing = runtime
            .toggle_saved_folder("folder-missing")
            .expect_err("toggle missing folder");
        assert!(missing.message.contains("was not found"));

        // 折叠是内存态：不写回配置，重新加载后默认全部展开。
        let reloaded = AppRuntime::new(temp.path().to_path_buf()).expect("runtime reload");
        assert_eq!(reloaded.projection().session_tree_rows.len(), 5);
    }

    #[test]
    fn session_tree_search_keeps_hit_chains_and_folder_subtrees() {
        let temp = tempdir().expect("tempdir");
        let _store = saved_session_tree_store(temp.path());
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        // 命中会话：只保留它的祖先文件夹链。
        let session_hit = runtime.update_session_search("db.example");
        let outline = session_hit
            .session_tree_rows
            .iter()
            .map(|row| (row.kind.as_str(), row.id.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            outline,
            vec![
                ("folder", SAVED_SESSIONS_FOLDER_ID),
                ("folder", "folder-prod"),
                ("session", "saved-prod-db"),
            ]
        );
        assert_eq!(session_hit.session_tree_rows[0].detail, "1");

        // 命中文件夹：整棵子树（含不匹配查询的子项）都显示。
        let folder_hit = runtime.update_session_search("Prod");
        let labels = folder_hit
            .session_tree_rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec![SAVED_SESSIONS_FOLDER_NAME, "Prod", "Prod DB", "Cache"]
        );
        assert_eq!(folder_hit.session_tree_rows[0].detail, "2");

        // 进入搜索会展开全部：命中项不会藏在折叠的文件夹里。
        let collapsed = runtime
            .toggle_saved_folder(SAVED_SESSIONS_FOLDER_ID)
            .expect("collapse root folder");
        assert_eq!(collapsed.session_tree_rows.len(), 1);
        let _ = runtime.update_session_search("");
        let auto_expanded = runtime.update_session_search("Cache");
        assert!(auto_expanded
            .session_tree_rows
            .iter()
            .any(|row| row.id == "saved-prod-cache"));

        let cleared = runtime.update_session_search("");
        assert_eq!(cleared.session_tree_rows.len(), 5);
    }

    #[test]
    fn session_tree_selection_switches_between_folder_and_session() {
        let temp = tempdir().expect("tempdir");
        let _store = saved_session_tree_store(temp.path());
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let session_selected = runtime.select_saved_session_by_id("saved-prod-db");
        assert!(session_selected.has_saved_selection);
        assert!(session_selected
            .session_tree_rows
            .iter()
            .any(|row| row.id == "saved-prod-db" && row.selected));
        assert!(!session_selected
            .session_tree_rows
            .iter()
            .any(|row| row.kind == "folder" && row.selected));

        let folder_selected = runtime.select_saved_session_by_id("folder-prod");
        assert!(!folder_selected.has_saved_selection);
        assert!(folder_selected
            .session_tree_rows
            .iter()
            .any(|row| row.id == "folder-prod" && row.selected));
        assert!(!folder_selected
            .session_tree_rows
            .iter()
            .any(|row| row.kind == "session" && row.selected));

        // 未知 id 不改变选择。
        let unknown = runtime.select_saved_session_by_id("saved-missing");
        assert!(unknown
            .session_tree_rows
            .iter()
            .any(|row| row.id == "folder-prod" && row.selected));

        // 双击文件夹 = 折叠；双击会话 = 选中并打开。
        let toggled = runtime
            .activate_saved_session_tree_node("folder-prod")
            .expect("activate folder");
        assert!(!toggled
            .session_tree_rows
            .iter()
            .any(|row| row.id == "saved-prod-db"));

        let opened = runtime
            .activate_saved_session_tree_node("saved-one")
            .expect("activate session");
        assert_eq!(opened.active_session_name_text, "One");
        assert!(opened
            .session_tree_rows
            .iter()
            .any(|row| row.id == "saved-one" && row.selected));

        let missing = runtime
            .activate_saved_session_tree_node("saved-missing")
            .expect_err("activate missing session");
        assert!(missing.message.contains("was not found"));
    }

    #[test]
    fn selected_saved_session_can_be_updated_and_deleted() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("ops@one.example.test:2222")
            .expect("quick connect");
        let saved = runtime.save_active_session().expect("save active session");
        assert_eq!(saved.status_kind, "session-saved");
        assert!(saved.status_param_1.contains("one-example-test"));
        assert_eq!(saved.status_param_2, "1");
        runtime
            .handle_quick_connect("ops@two.example.test:2223")
            .expect("quick connect");
        runtime.save_active_session().expect("save active session");

        let _ = runtime.select_previous_saved_session();
        runtime
            .handle_quick_connect("ops@updated.example.test:2299")
            .expect("quick connect updated");

        let updated = runtime
            .update_selected_saved_session_from_active()
            .expect("update selected saved");
        assert!(updated.status_text.contains("Updated saved session"));
        assert!(runtime
            .saved_session_profiles()
            .iter()
            .any(|profile| profile.host == "updated.example.test" && profile.port == 2299));

        let deleted = runtime
            .delete_selected_saved_session()
            .expect("delete selected saved");
        assert!(deleted.status_text.contains("Deleted saved session"));
        assert_eq!(deleted.status_kind, "session-deleted");
        assert!(deleted.status_param_1.contains("one-example-test"));
        assert_eq!(runtime.saved_session_profiles().len(), 1);
    }

    #[test]
    fn open_first_saved_session_uses_minimal_ui_path() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("ops@saved.example.test:2222")
            .expect("quick connect");
        runtime.save_active_session().expect("save active session");

        let projection = runtime
            .open_first_saved_session()
            .expect("open first saved session");

        assert!(projection
            .active_session_name_text
            .contains("saved.example.test"));
        assert_eq!(projection.tab_state_text, "connected");
        assert!(!projection.recent_sessions_empty);
        assert!(projection
            .recent_sessions_rows_text
            .contains("saved.example.test"));
    }

    #[test]
    fn live_native_ssh_projection_smoke_when_env_target_is_set() {
        let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
            return;
        };
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime.select_native_ssh_transport_backend();
        runtime.set_host_key_policy_override_for_testing(HostKeyPolicy::AcceptAnyForTesting);

        let projection = runtime
            .handle_quick_connect(&target)
            .expect("live quick connect");
        assert_eq!(projection.tab_state_text, "connected");

        let marker = "__YSHELL_APP_LIVE__";
        let _ = runtime
            .send_active_terminal_input(&format!("printf '{marker}\\n'\n"))
            .expect("send live input");

        let mut final_projection = runtime.projection();
        for _ in 0..80 {
            if final_projection.terminal_body_text.contains(marker) {
                break;
            }
            thread::sleep(Duration::from_millis(50));
            final_projection = runtime
                .poll_active_terminal_output()
                .expect("poll live output");
        }

        assert!(final_projection.terminal_body_text.contains(marker));

        let resized = runtime
            .resize_active_terminal(90, 28)
            .expect("resize live terminal");
        assert!(resized.status_text.contains("90x28"));

        let _ = runtime
            .send_active_terminal_input("exit\n")
            .expect("send exit");
    }

    #[test]
    fn live_terminal_color_render_when_env_target_is_set() {
        let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
            return;
        };
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime.select_native_ssh_transport_backend();
        runtime.set_host_key_policy_override_for_testing(HostKeyPolicy::AcceptAnyForTesting);
        let projection = runtime
            .handle_quick_connect(&target)
            .expect("live quick connect");
        assert_eq!(projection.tab_state_text, "connected");

        let _ = runtime
            .send_active_terminal_input("printf '\\033[31mred\\033[0m plain\\n'\n")
            .expect("send ansi printf");
        let mut projection = runtime.projection();
        for _ in 0..80 {
            if projection.terminal_body_text.contains("red plain") {
                break;
            }
            thread::sleep(Duration::from_millis(50));
            projection = runtime
                .poll_active_terminal_output()
                .expect("poll live output");
        }
        assert!(projection.terminal_body_text.contains("red plain"));

        let mut renderer = yshell_terminal::TerminalRenderer::new();
        let snapshot = runtime
            .active_terminal_render_snapshot()
            .expect("render snapshot");
        let frame = renderer.render(&snapshot);
        let red_pixels = frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[0] > 150 && pixel[1] < 90 && pixel[2] < 90)
            .count();
        assert!(
            red_pixels > 40,
            "expected ANSI red pixels in the live frame, got {red_pixels}"
        );

        let _ = runtime.send_active_terminal_input("exit\n").expect("send exit");
    }

    #[test]
    fn live_native_sftp_projection_smoke_when_env_target_is_set() {
        let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
            return;
        };
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime.select_native_ssh_transport_backend();
        runtime.set_host_key_policy_override_for_testing(HostKeyPolicy::AcceptAnyForTesting);

        let _ = runtime
            .handle_quick_connect(&target)
            .expect("live quick connect");
        let projection = runtime
            .refresh_active_sftp_listing()
            .expect("refresh live sftp");

        assert_eq!(projection.sftp_path_text, "/");
        assert_eq!(projection.sftp_listing_kind_text, "rows");
        assert!(!projection.sftp_listing_rows_text.trim().is_empty());
        assert!(projection.status_text.contains("Loaded"));
    }

    #[test]
    fn live_native_sftp_transfer_smoke_when_env_target_is_set() {
        let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
            return;
        };
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime.select_native_ssh_transport_backend();
        runtime.set_host_key_policy_override_for_testing(HostKeyPolicy::AcceptAnyForTesting);
        let _ = runtime
            .handle_quick_connect(&target)
            .expect("live quick connect");

        let local_upload_path = temp.path().join("upload.txt");
        let local_download_path = temp.path().join("download.txt");
        fs::write(&local_upload_path, b"yshell app sftp").expect("write upload");
        let remote_root = "/tmp/yshell-app-sftp-smoke";
        let remote_file = format!("{remote_root}/upload.txt");

        runtime.sftp_path = remote_root.to_owned();
        runtime.sftp_local_path = local_upload_path.display().to_string();
        runtime.sftp_remote_target = remote_file.clone();
        let _ = runtime.refresh_active_sftp_listing().ok();
        runtime
            .set_sftp_remote_target(&remote_file);
        let _ = runtime.open_sftp_path("/tmp");

        let session_key = runtime.active_session_key().expect("active session");
        let runtime_session = runtime
            .sessions
            .get(&session_key)
            .expect("runtime session");
        let mut client = SftpClient::with_real_backend(
            runtime.effective_ssh_config(&runtime_session.ssh_config),
        );
        let _ = client.mkdir(remote_root);

        runtime.sftp_path = remote_root.to_owned();
        runtime.sftp_local_path = local_upload_path.display().to_string();
        runtime.sftp_remote_target = remote_file.clone();
        let uploaded = runtime.upload_sftp_file().expect("upload");
        assert!(uploaded.status_text.contains("Uploaded"));

        runtime.sftp_local_path = local_download_path.display().to_string();
        runtime.sftp_remote_target = remote_file.clone();
        let downloaded = runtime.download_sftp_file().expect("download");
        assert!(downloaded.status_text.contains("Downloaded"));
        assert_eq!(fs::read(&local_download_path).expect("read download"), b"yshell app sftp");

        let session_key = runtime.active_session_key().expect("active session");
        let runtime_session = runtime
            .sessions
            .get(&session_key)
            .expect("runtime session");
        let mut client = SftpClient::with_real_backend(
            runtime.effective_ssh_config(&runtime_session.ssh_config),
        );
        client.delete(&remote_file).expect("cleanup file");
        client.delete(remote_root).expect("cleanup dir");
    }

    #[test]
    fn live_native_logging_smoke_when_env_target_is_set() {
        let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
            return;
        };
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let document = ConfigDocument {
            logging: LoggingProfile {
                enabled: true,
                directory: Some("logs".to_owned()),
                format: "sanitized".to_owned(),
            },
            ..ConfigDocument::default()
        };
        store.save(&document).expect("save config");

        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime.select_native_ssh_transport_backend();
        runtime.set_host_key_policy_override_for_testing(HostKeyPolicy::AcceptAnyForTesting);
        let _ = runtime
            .handle_quick_connect(&target)
            .expect("live quick connect");

        let marker = "__YSHELL_APP_LOGGING__";
        let _ = runtime
            .send_active_terminal_input(&format!("printf '{marker}\\n'\n"))
            .expect("send live input");
        for _ in 0..80 {
            if runtime.projection().terminal_body_text.contains(marker) {
                break;
            }
            thread::sleep(Duration::from_millis(50));
            let _ = runtime
                .poll_active_terminal_output()
                .expect("poll live output");
        }

        let local_upload_path = temp.path().join("logging-upload.txt");
        let local_download_path = temp.path().join("logging-download.txt");
        fs::write(&local_upload_path, b"yshell logging smoke").expect("write upload");
        let remote_root = "/tmp/yshell-app-logging-smoke";
        let remote_file = format!("{remote_root}/logging-upload.txt");

        let session_key = runtime.active_session_key().expect("active session");
        let runtime_session = runtime
            .sessions
            .get(&session_key)
            .expect("runtime session");
        let mut client = SftpClient::with_real_backend(
            runtime.effective_ssh_config(&runtime_session.ssh_config),
        );
        let _ = client.mkdir(remote_root);

        runtime.sftp_path = remote_root.to_owned();
        runtime.sftp_local_path = local_upload_path.display().to_string();
        runtime.sftp_remote_target = remote_file.clone();
        runtime.upload_sftp_file().expect("upload");
        runtime.sftp_local_path = local_download_path.display().to_string();
        runtime.sftp_remote_target = remote_file.clone();
        runtime.download_sftp_file().expect("download");

        let _ = runtime
            .send_active_terminal_input("exit\n")
            .expect("send exit");

        client.delete(&remote_file).expect("cleanup file");
        client.delete(remote_root).expect("cleanup dir");

        let log_files = collect_log_files(&temp.path().join("logs"));
        let transcript = log_files
            .iter()
            .filter_map(|path| fs::read_to_string(path).ok())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(transcript.contains(marker));
        assert!(transcript.contains("operation=upload"));
        assert!(transcript.contains("operation=download"));
    }

    // --- W4 i18n：模板取值映射（句子在 Slint `@tr`，Rust 只测值）------------

    #[test]
    fn secret_store_status_projects_kind_and_path_values() {
        let temp = tempdir().expect("tempdir");
        let runtime =
            AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

        // No keychain injection -> the disabled kind; path stays empty.
        assert_eq!(runtime.secret_store_kind.id(), "disabled");
        let projection = runtime.projection();
        assert_eq!(projection.secret_store_kind_text, "disabled");
        assert_eq!(projection.secret_store_path_text, "");
    }

    #[test]
    fn settings_terminal_status_projects_kind_field_and_limits() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let hint = runtime.projection();
        assert_eq!(hint.settings_terminal_status_kind_text, "hint");
        assert_eq!(hint.settings_terminal_status_field_text, "");

        let _ = runtime.update_settings_scrollback_lines("");
        let empty = runtime.save_settings_terminal().expect("validation only");
        assert_eq!(empty.settings_terminal_status_kind_text, "empty");
        assert_eq!(
            empty.settings_terminal_status_field_text,
            "scrollback-lines"
        );

        let _ = runtime.update_settings_scrollback_lines("abc");
        let not_number = runtime.save_settings_terminal().expect("validation only");
        assert_eq!(not_number.settings_terminal_status_kind_text, "not-number");
        assert_eq!(
            not_number.settings_terminal_status_field_text,
            "scrollback-lines"
        );

        let _ = runtime.update_settings_scrollback_lines("99");
        let out_of_range = runtime.save_settings_terminal().expect("validation only");
        assert_eq!(
            out_of_range.settings_terminal_status_kind_text,
            "out-of-range"
        );
        assert_eq!(out_of_range.settings_terminal_status_value_text, "100");
        assert_eq!(out_of_range.settings_terminal_status_limit_text, "1000000");
        assert_eq!(
            runtime.settings_terminal_status.legacy_text(),
            "Scrollback lines must be between 100 and 1000000."
        );
    }

    #[test]
    fn editor_auth_test_status_projects_kind_and_params() {
        let hint = EditorAuthTestStatus::Hint;
        assert_eq!(hint.kind_id(), "hint");
        assert_eq!(hint.host_label(), "");
        assert!(hint.legacy_text().contains("Run Auth Test"));

        let success = EditorAuthTestStatus::Success {
            host_label: "ops@example.test:22".to_owned(),
            backend: "fake".to_owned(),
            startup_snippet: "welcome".to_owned(),
        };
        assert_eq!(success.kind_id(), "success");
        assert_eq!(success.backend(), "fake");
        assert_eq!(success.startup_snippet(), "welcome");
        assert_eq!(
            success.legacy_text(),
            "Auth test succeeded for ops@example.test:22 using the `fake` backend. Startup: welcome"
        );

        let failed = EditorAuthTestStatus::Failed {
            error: "boom".to_owned(),
        };
        assert_eq!(failed.kind_id(), "failed");
        assert_eq!(failed.error(), "boom");
        assert_eq!(failed.legacy_text(), "Auth test failed: boom");
    }

    #[test]
    fn sftp_session_lifecycle_projects_status_parts() {
        let unavailable = SftpSessionLifecycle::Unavailable {
            reason: SftpUnavailableReason::SelectNativeSsh,
        };
        assert_eq!(unavailable.status_kind(), "unavailable");
        assert_eq!(unavailable.status_reason(), "select-native-ssh");
        assert_eq!(
            unavailable.legacy_status_text(),
            "SFTP unavailable: select the native-ssh backend before using SFTP"
        );

        let disconnected = SftpSessionLifecycle::Disconnected {
            session_key: Some("runtime-1".to_owned()),
        };
        assert_eq!(disconnected.status_kind(), "disconnected");
        assert_eq!(disconnected.status_session_key(), "runtime-1");
        assert_eq!(disconnected.status_detail(), "");

        let ready = SftpSessionLifecycle::Ready {
            session_key: "runtime-2".to_owned(),
        };
        assert_eq!(ready.status_kind(), "ready");
        assert_eq!(ready.status_session_key(), "runtime-2");

        let failed = SftpSessionLifecycle::Failed {
            session_key: None,
            reason: "socket closed".to_owned(),
        };
        assert_eq!(failed.status_kind(), "failed");
        assert_eq!(failed.status_detail(), "socket closed");
        assert_eq!(
            failed.legacy_status_text(),
            "SFTP session failed: socket closed"
        );
    }

    #[test]
    fn sftp_listing_state_projects_kind_and_detail() {
        assert_eq!(SftpListingState::ConnectReal.kind_id(), "connect-real");
        assert_eq!(
            SftpListingState::EmptyDirectory.kind_id(),
            "empty-directory"
        );

        let rows = SftpListingState::Rows("[file] a.txt".to_owned());
        assert_eq!(rows.kind_id(), "rows");
        assert_eq!(rows.rows(), "[file] a.txt");

        let failed = SftpListingState::SyncFailed {
            session_key: "runtime-1".to_owned(),
            path: "/srv".to_owned(),
            error: "permission denied".to_owned(),
        };
        assert_eq!(failed.kind_id(), "sync-failed");
        assert_eq!(failed.session_key(), "runtime-1");
        assert_eq!(failed.path(), "/srv");
        assert_eq!(failed.error(), "permission denied");
    }

    #[test]
    fn sidebar_value_parts_project_without_sentences() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let welcome = runtime.projection();
        assert_eq!(welcome.active_session_kind_text, "welcome");
        assert_eq!(welcome.active_session_name_text, "");
        assert!(!welcome.tab_has_session);
        assert_eq!(welcome.tab_state_text, "idle");
        assert!(welcome.recent_sessions_empty);
        assert_eq!(welcome.recent_sessions_rows_text, "");

        let connected = runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        assert_eq!(connected.active_session_kind_text, "session");
        assert!(connected
            .active_session_name_text
            .contains("alice@example.com:2200"));
        assert!(connected.tab_has_session);
        assert_eq!(connected.tab_state_text, "connected");
        assert!(connected.terminal_title_has_session);
        assert!(connected
            .terminal_title_name_text
            .contains("alice@example.com:2200"));
        assert_eq!(connected.terminal_body_kind_text, "data");
        assert!(!connected.recent_sessions_empty);
        assert!(connected
            .recent_sessions_rows_text
            .contains("alice@example.com:2200"));
    }

    /// W5-A3：菜单/按钮状态感知依赖的布尔量必须跟随运行时状态变化。
    #[test]
    fn projection_state_flags_track_session_and_selection() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        // 冷启动：无会话、无选中项，SFTP 与终端选区也不可用。
        let welcome = runtime.projection();
        assert!(!welcome.has_active_session);
        assert!(!welcome.active_session_connected);
        assert!(!welcome.has_saved_selection);
        assert!(!welcome.sftp_available);
        assert!(!welcome.terminal_has_selection);

        // 快速连接后：有活动会话且已连接，但还没有选区/保存选中项。
        let connected = runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");
        assert!(connected.has_active_session);
        assert!(connected.active_session_connected);
        assert!(!connected.has_saved_selection);
        assert!(!connected.terminal_has_selection);
        assert!(!connected.sftp_available);

        // 终端全选 → Copy 菜单项的选区开关跟随。
        let selected = runtime
            .select_all_active_terminal()
            .expect("select all terminal");
        assert!(selected.terminal_has_selection);
        assert!(selected.terminal_selection_active);

        // 保存活动会话会选中新 profile → Edit/Update/Delete 菜单项可用。
        let saved = runtime.save_active_session().expect("save active session");
        assert!(saved.has_saved_selection);

        // 断开后 connected 开关跟随运行时状态。
        let disconnected = runtime
            .disconnect_active_session()
            .expect("disconnect active session");
        assert!(disconnected.has_active_session);
        assert!(!disconnected.active_session_connected);

        // SFTP 生命周期进入 ready → SFTP 操作项可用。
        runtime.sftp_session = SftpSessionLifecycle::Ready {
            session_key: "runtime-1".to_owned(),
        };
        assert!(runtime.projection().sftp_available);
    }

    fn start_tcp_probe_target() -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind tcp probe target");
        let port = listener.local_addr().expect("listener addr").port();
        let handle = thread::spawn(move || {
            let _ = listener.accept();
        });
        (port, handle)
    }

    fn collect_log_files(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if !root.exists() {
            return files;
        }
        let mut stack = vec![root.to_path_buf()];
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                let Ok(entries) = fs::read_dir(&path) else {
                    continue;
                };
                for entry in entries.flatten() {
                    stack.push(entry.path());
                }
            } else if path.is_file() {
                files.push(path);
            }
        }
        files
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
