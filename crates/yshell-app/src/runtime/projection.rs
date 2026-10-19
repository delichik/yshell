//! `AppProjection` assembly and its sub-projections.

use crate::session_runtime::SessionRuntime;
use crate::sftp_view::{format_sftp_size, sftp_kind_text};
use std::env;
use yshell_config::{PanelId, PanelSide};
use yshell_core::SessionState;
use yshell_sftp::FsEntryKind;

use super::panels::{PanelFrameView, PanelLayoutView};
use super::theme::palette_parts;
use super::*;

impl AppRuntime {
    /// N7：为指定窗口组装投影（终端/标签字段解析到该窗口的活动标签）。
    ///
    /// 只临时覆盖投影解析的窗口指针，不改聚焦窗口——多窗口轮询会对每个窗口各调用
    /// 一次。`AppRuntime` 单实例共享，调用方负责把结果应用到对应的 `MainWindow`。
    pub fn projection_for_window(&self, window_id: u64) -> AppProjection {
        let previous = self.projection_window.replace(Some(window_id));
        let projection = self.projection();
        self.projection_window.set(previous);
        projection
    }

    /// Whether the projected window's active tab is a New Tab page.
    fn projection_quick_connect_visible(&self) -> bool {
        let tab_id = self.projection_active_tab_id();
        if tab_id.is_empty() {
            return false;
        }
        self.tab_index(&tab_id)
            .is_some_and(|index| self.tabs[index].is_new_tab())
    }

    pub fn projection(&self) -> AppProjection {
        let (editor_folder_label_text, editor_folder_id_text, editor_folder_known) =
            self.editor_folder_parts();
        let (editor_target_editing, editor_target_session_id_text) = self.editor_target_parts();
        // SSH-1：会话编辑器 Key 认证的托管密钥清单与选中项。
        let editor_key_entries = self.private_key_entries();
        let editor_key_options = editor_key_entries
            .iter()
            .map(auth_key_option_data)
            .collect::<Vec<_>>();
        let editor_selected_key_index = self
            .editor
            .key_id
            .as_deref()
            .and_then(|key_id| {
                editor_key_entries
                    .iter()
                    .position(|entry| entry.id == key_id)
            })
            .map(|index| i32::try_from(index).unwrap_or(i32::MAX))
            .unwrap_or(-1);
        let editor_selected_key_missing = self
            .editor
            .key_id
            .as_deref()
            .is_some_and(|key_id| self.config_document.find_key(key_id).is_none());
        // NET-1：会话编辑器代理下拉（托管条目）+ legacy 只读摘要。
        let editor_proxy_options = self.managed_proxy_options();
        let editor_selected_proxy_index = self.editor_proxy_selected_index(&editor_proxy_options);
        let editor_proxy_selected_detail_text = self.editor_proxy_selected_detail();
        let editor_proxy_legacy_text = self.editor_proxy_legacy_text();
        let editor_proxy_selected_missing = self.editor_proxy_selected_missing();
        let (tunnels_summary_kind_text, tunnels_summary_rows_text) = self.tunnels_summary_parts();
        let tunnel_rows = self.tunnel_rows();
        let (session_summary_rows_text, session_summary_hidden_count) =
            self.session_summary_parts();
        let (selection_kind_text, selection_name_text, selection_host_text, selection_id_text) =
            self.saved_session_selection_parts();
        let (inventory_rows_text, inventory_empty) = self.saved_session_inventory_parts();
        let session_tree_rows = self.session_tree_rows();
        let (recent_rows_text, recent_empty) = self.recent_sessions_parts();
        let (active_kind_text, active_name_text, active_state_text) = self.active_session_parts();
        let connection_overlay = self.connection_overlay_projection();
        let status_connection = self.status_connection_projection();
        let (tab_has_session, tab_name_text, tab_state_text) = self.tab_parts();
        let (terminal_title_has_session, terminal_title_name_text) = self.terminal_title_parts();
        let (terminal_body_kind_text, terminal_body_text) = self.terminal_body_parts();
        // N7：标签条只投影"当前投影窗口"的标签（`projection_window` 未设置时 =
        // 聚焦窗口；无窗口登记时 = 全部标签）。
        let tabs = self.tab_data_for_projection();
        let active_tab_mode_text = tabs
            .iter()
            .find(|tab| tab.active)
            .map(|tab| tab.mode_text.clone())
            .unwrap_or_default();
        let tab_has_disconnected = self.has_disconnected_tabs();
        let projection_window_active_tab = self.projection_active_tab_id();
        let projection_window = self.projection_window_id();
        let workspace_split = self.workspace_split_projection();
        let tab_menu_split_enabled = !projection_window_active_tab.is_empty()
            && (!workspace_split.active || workspace_split.can_split);
        let tab_menu_close_pane_enabled = workspace_split.can_close_pane;
        let (
            tab_menu_close_others_enabled,
            tab_menu_close_left_enabled,
            tab_menu_close_right_enabled,
            tab_menu_close_all_enabled,
            tab_menu_close_disconnected_enabled,
        ) = self.tab_menu_flags();
        // D18：标签右键菜单的 Reconnect/Disconnect 启用条件（针对被右键标签）。
        let (tab_menu_reconnect_enabled, tab_menu_disconnect_enabled) =
            self.tab_menu_connection_flags();
        // N7：标签右键"移动到新窗口 / 移动到主窗口"的启用条件。
        let (tab_menu_move_to_new_window_enabled, tab_menu_move_to_main_window_enabled) =
            self.tab_menu_move_flags();
        let (tab_menu_mode_text, tab_menu_switch_mode_enabled) = self.tab_menu_switch_mode_flags();
        let active_tab_can_switch_to_sftp = !projection_window_active_tab.is_empty()
            && self.tab_can_switch_to_sftp(&projection_window_active_tab);
        let pending_close = self.pending_close_tabs.as_ref();
        let (terminal_search_kind_text, terminal_search_match_count, terminal_search_current_index) =
            self.terminal_search_summary_parts();
        let (sftp_remote_edit_remote_path_text, sftp_remote_edit_local_path_text) =
            self.sftp_remote_edit_parts();
        let (transfer_queue_rows_text, transfer_queue_empty) = self.transfer_queue_parts();
        let (sftp_item_count, sftp_dir_count) = self.sftp_visible_counts();
        // N1 Phase 2: local pane, queue drawer, conflict dialog, properties and
        // the in-app file clipboard.
        let local_rows = self.local_rows();
        let (local_item_count, local_dir_count) = self.local_summary_counts();
        let transfer_rows = self.sftp_transfer_rows();
        let transfer_counts: TransferQueueCounts = self.sftp_transfer_counts();
        let (sftp_properties_name_text, sftp_properties_kind_text, sftp_properties_size_text) =
            self.sftp_properties_parts();
        let (clipboard_side_text, clipboard_count, clipboard_cut) = self.clipboard_parts();
        let (status_kind, status_param_1, status_param_2) = self.status_i18n_parts();
        // N7：终端滚动/帧数据解析"投影窗口"自己的活动会话（不再是全局活动会话）。
        // 无窗口覆盖（`projection_window` 为空 = 聚焦窗口投影）时沿用 keys.rs 的
        // 活动终端 API，保持单窗口路径的语义与测试不变。
        let focus_terminal = self.focus_terminal_runtime();
        let (terminal_scrollback_lines, terminal_viewport_rows) = match self.projection_window.get()
        {
            Some(_) => focus_terminal
                .map(|runtime| {
                    let metrics = runtime.terminal_viewport_metrics();
                    (
                        metrics
                            .top_absolute_row
                            .saturating_add(metrics.viewport_offset)
                            .saturating_add(usize::from(metrics.rows)),
                        usize::from(metrics.rows),
                    )
                })
                .unwrap_or((0, 0)),
            None => self.active_terminal_scroll_geometry(),
        };
        let terminal_viewport_columns = focus_terminal
            .map(|runtime| i32::from(runtime.terminal_viewport_metrics().columns))
            .unwrap_or(0);
        // N6：终端日志（菜单/状态栏入口、REC 指示、弹窗字段）。
        let (
            terminal_logging_start_enabled,
            terminal_logging_stop_enabled,
            terminal_logging_open_file_enabled,
            terminal_logging_open_folder_enabled,
        ) = self.terminal_logging_menu_flags();
        let logging_session_key = self.focus_session_key().map(str::to_owned);
        let logging_active = logging_session_key
            .as_deref()
            .is_some_and(|session_key| self.session_logging_active(session_key));
        let logging_mode_text = logging_session_key
            .as_deref()
            .map(|session_key| self.session_logging_mode(session_key).id())
            .unwrap_or("off")
            .to_owned();
        let logging_path_text = logging_session_key
            .as_deref()
            .map(|session_key| self.session_logging_path_text(session_key))
            .unwrap_or_default();
        let logging_file_name_text = std::path::Path::new(&logging_path_text)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        // N9：同步发送按键（终端/标签菜单旗标 + 状态栏 chip / 标签角标数据）。
        let (terminal_sync_all_enabled, terminal_sync_visible_enabled, terminal_sync_stop_enabled) =
            self.input_sync_menu_flags();
        let (
            tab_menu_receive_key_input_enabled,
            tab_menu_receives_key_input_checked,
            tab_menu_receive_key_input_reason_text,
        ) = self.tab_menu_receive_key_input_flags();
        let input_sync_active = self.input_sync.is_active();
        let input_sync_mode_text = self.input_sync.mode.id().to_owned();
        let input_sync_target_count =
            i32::try_from(self.input_sync.targets.len()).unwrap_or(i32::MAX);
        let input_sync_source_name_text = self.input_sync_source_name();
        let input_sync_notice_kind_text = self.input_sync.notice_kind.clone();
        let input_sync_notice_param_text = self.input_sync.notice_param.clone();
        let (quick_connect_rows, quick_links_rows, quick_connect_summary_text) = (
            self.quick_connect_row_data(),
            self.quick_link_row_data(),
            self.quick_connect_summary_text(),
        );
        let (quick_connect_input_text, quick_connect_error_text) =
            self.new_tab_parts_for(&self.projection_active_tab_id());
        // D37：QC 摘要的 kind/参数（页面走 `TextFormats.status-message`；文本留作回退）。
        let (
            quick_connect_summary_kind_text,
            quick_connect_summary_param_1_text,
            quick_connect_summary_param_2_text,
        ) = self.quick_connect_summary_parts();
        // D36：主机密钥弹窗正文拆分字段（与 `host_key_prompt_text()` 同源）。
        let host_key_prompt = self.host_key_prompt_parts();
        // N3：内容区 px 布局（Rust 计算 → 投影下发；Slint 侧不再从 window.width 反推）。
        let panel_view = self.panel_layout_view();
        let private_keys = self.private_keys_projection_parts();
        let proxies = self.proxies_projection_parts();
        let host_keys = self.host_keys_projection_parts();
        let auth_prompt = self.auth_prompt_projection_parts();
        // N5：终端外观（Settings Appearance 内联 / Folder Editor / Session Editor）。
        let settings_appearance_preview = self.settings_appearance_preview_frame();
        // TERM-4：Session Editor 外观分区的 live preview（仅打开且停在外观分区）。
        let session_editor_preview = self.session_editor_preview_frame();
        let settings_appearance_status_text = self.settings_appearance_status.display_text();
        let settings_appearance_color_scheme_id =
            theme::resolved_global_scheme_id(&self.config_document.terminal, &self.theme_catalog);
        let settings_appearance_color_scheme_name_text =
            theme::scheme_display_name(&settings_appearance_color_scheme_id, &self.theme_catalog);
        let settings_appearance_color_scheme_builtin =
            yshell_config::ThemeCatalog::is_builtin(&settings_appearance_color_scheme_id);
        let settings_appearance_palette = palette_parts(self.settings_appearance_palette());
        // TERM-1: the independent Theme Manager list + preview.
        let theme_manager_default_id =
            theme::resolved_global_scheme_id(&self.config_document.terminal, &self.theme_catalog);
        let theme_manager_selected_id = self.theme_manager_selection_id();
        let theme_manager_rows = theme::theme_manager_rows(
            &self.theme_catalog,
            &theme_manager_selected_id,
            &theme_manager_default_id,
        );
        let theme_manager_selected_index = theme_manager_rows
            .iter()
            .position(|row| row.selected)
            .map_or(-1, |index| i32::try_from(index).unwrap_or(i32::MAX));
        let theme_manager_builtin_count =
            i32::try_from(theme_manager_rows.iter().filter(|row| row.builtin).count())
                .unwrap_or(i32::MAX);
        let theme_manager_user_count = i32::try_from(theme_manager_rows.len()).unwrap_or(i32::MAX)
            - theme_manager_builtin_count;
        let theme_manager_default_name_text =
            theme::scheme_display_name(&theme_manager_default_id, &self.theme_catalog);
        let theme_manager_selected_is_default =
            theme_manager_selected_id == theme_manager_default_id;
        let theme_manager_preview = self.theme_manager_preview_frame();
        let theme_manager_pending_delete = self.theme_manager_pending_delete.as_deref();
        let theme_manager_remove_name_text = theme_manager_pending_delete
            .map(|id| theme::scheme_display_name(id, &self.theme_catalog))
            .unwrap_or_default();
        let theme_manager_remove_reference_count = theme_manager_pending_delete
            .map(|id| {
                i32::try_from(theme::theme_reference_count(&self.config_document, id))
                    .unwrap_or(i32::MAX)
            })
            .unwrap_or(0);
        let theme_editor_open = self.theme_editor.is_some();
        let theme_editor_builtin = self
            .theme_editor
            .as_ref()
            .is_some_and(|editor| editor.builtin);
        let theme_editor_id = self
            .theme_editor
            .as_ref()
            .map(|editor| editor.id.clone())
            .unwrap_or_default();
        let theme_editor_name = self
            .theme_editor
            .as_ref()
            .map(|editor| editor.name.clone())
            .unwrap_or_default();
        let theme_editor_foreground = self
            .theme_editor
            .as_ref()
            .map(|editor| editor.foreground.clone())
            .unwrap_or_default();
        let theme_editor_background = self
            .theme_editor
            .as_ref()
            .map(|editor| editor.background.clone())
            .unwrap_or_default();
        let theme_editor_cursor = self
            .theme_editor
            .as_ref()
            .map(|editor| editor.cursor.clone())
            .unwrap_or_default();
        let theme_editor_selection = self
            .theme_editor
            .as_ref()
            .map(|editor| editor.selection.clone())
            .unwrap_or_default();
        let theme_editor_ansi = self
            .theme_editor
            .as_ref()
            .map(|editor| editor.ansi.to_vec())
            .unwrap_or_default();
        let folder_editor_appearance_fields = self.folder_editor_appearance_fields();
        let folder_editor_terminal_fields = self.folder_editor_terminal_fields();
        let folder_editor_logging_fields = self.folder_editor_logging_fields();
        let editor_appearance_fields = self.editor_appearance_fields();
        let editor_terminal_fields = self.editor_terminal_fields();

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
            editor_password_text: self.editor.password.clone(),
            editor_key_options,
            editor_selected_key_index,
            editor_selected_key_missing,
            editor_legacy_key_path_text: self.editor.legacy_key_path.clone(),
            editor_legacy_key_has_passphrase: self.editor.legacy_key_has_passphrase,
            editor_has_stored_password: self.editor.has_stored_password,
            editor_target_session_id_text,
            editor_target_editing,
            editor_auth_test_kind_text: self.editor_auth_test_status.kind_id().to_owned(),
            editor_auth_test_host_text: self.editor_auth_test_status.host_label().to_owned(),
            editor_auth_test_backend_text: String::new(),
            editor_auth_test_startup_text: self
                .editor_auth_test_status
                .startup_snippet()
                .to_owned(),
            editor_auth_test_error_text: self.editor_auth_test_status.error().to_owned(),
            editor_proxy_mode_text: self.editor.proxy_mode.label().to_owned(),
            editor_proxy_options,
            editor_selected_proxy_index,
            editor_proxy_selected_detail_text,
            editor_proxy_legacy_text,
            editor_proxy_selected_missing,
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
            settings_sftp_max_parallel_transfers_text: self
                .settings_sftp_max_parallel_transfers_text
                .clone(),
            settings_language_text: self.config_document.ui.language.clone(),
            settings_debug_logging_enabled: self.config_document.ui.debug_logging,
            settings_confirm_close_session: self.config_document.ui.confirm_close_session,
            settings_confirm_quit_app: self.config_document.ui.confirm_quit_app,
            settings_debug_log_dir_text: self.debug_log_dir().display().to_string(),
            settings_terminal_status_kind_text: self.settings_terminal_status.kind_id().to_owned(),
            settings_terminal_status_field_text: self
                .settings_terminal_status
                .field_id()
                .to_owned(),
            settings_terminal_status_value_text: self.settings_terminal_status.value_text(),
            settings_terminal_status_limit_text: self.settings_terminal_status.limit_text(),
            settings_appearance_visible: self.settings_appearance_visible,
            settings_appearance_color_scheme_name_text,
            settings_appearance_color_scheme_builtin,
            settings_appearance_font_family_text: self.settings_appearance.font_family.clone(),
            settings_appearance_font_options: self.settings_appearance.font_options(),
            settings_appearance_font_index: self.settings_appearance.font_index(),
            settings_appearance_font_size_text: self.settings_appearance.font_size_text.clone(),
            settings_appearance_backup_font_text: self.settings_appearance.backup_font_text.clone(),
            settings_appearance_status_text,
            settings_appearance_palette_swatches: settings_appearance_palette,
            settings_appearance_preview,
            theme_manager_visible: self.theme_manager_visible,
            theme_manager_rows,
            theme_manager_selected_index,
            theme_manager_default_name_text,
            theme_manager_selected_is_default,
            theme_manager_builtin_count,
            theme_manager_user_count,
            theme_manager_preview,
            theme_manager_remove_confirm_visible: self.theme_manager_pending_delete.is_some(),
            theme_manager_remove_name_text,
            theme_manager_remove_reference_count,
            theme_editor_open,
            theme_editor_builtin,
            theme_editor_id,
            theme_editor_name,
            theme_editor_foreground,
            theme_editor_background,
            theme_editor_cursor,
            theme_editor_selection,
            theme_editor_ansi,
            folder_editor_visible: self.folder_editor.visible,
            folder_editor_name_text: self.folder_editor.folder_name.clone(),
            folder_editor_path_text: self.folder_editor.folder_path.clone(),
            folder_editor_status_text: self.folder_editor.status_text.clone(),
            folder_editor_status_kind_text: self.folder_editor.status_kind.clone(),
            folder_editor_status_param_1_text: self.folder_editor.status_param_1.clone(),
            folder_editor_status_param_2_text: self.folder_editor.status_param_2.clone(),
            folder_editor_appearance_fields,
            folder_editor_terminal_fields,
            folder_editor_logging_fields,
            saved_folder_selected: self.selected_saved_folder_id.is_some(),
            editor_appearance_fields,
            editor_terminal_fields,
            session_editor_preview,
            editor_modal_visible: self.editor_modal_visible,
            editor_discard_dialog_visible: self.editor_discard_dialog_visible,
            editor_is_dirty: self.editor_is_dirty(),
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
            known_hosts_detail_host_text: self.known_hosts_detail_host_text(),
            known_hosts_detail_port_text: self.known_hosts_detail_port_text(),
            known_hosts_detail_algorithm_text: self.known_hosts_detail_algorithm_text(),
            known_hosts_detail_fingerprint_text: self.known_hosts_detail_fingerprint_text(),
            known_hosts_detail_path_text: self.known_hosts_detail_path_text(),
            known_hosts_path_text: self.config_store.known_hosts_file().display().to_string(),
            known_hosts_clear_confirmation_text: self.known_hosts_clear_confirmation.clone(),
            active_session_kind_text: active_kind_text.to_owned(),
            active_session_name_text: active_name_text,
            active_session_state_text: active_state_text,
            has_active_session: self.focus_terminal_runtime().is_some(),
            active_session_connected: self
                .focus_terminal_runtime()
                .is_some_and(|session| session.state == SessionState::Connected),
            connection_overlay_visible: connection_overlay.visible,
            connection_overlay_target_text: connection_overlay.target,
            connection_overlay_stage_text: connection_overlay.stage,
            connection_overlay_error_text: connection_overlay.error,
            connection_overlay_failed: connection_overlay.failed,
            connection_overlay_pending: connection_overlay.pending,
            status_connection_state_text: status_connection.state,
            status_connection_target_text: status_connection.target,
            status_connection_busy: status_connection.busy,
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
            // D36：弹窗正文拆分字段（WS-C 的 HostKeyDialog 逐行渲染；空值行不占位）。
            host_key_prompt_target_text: host_key_prompt.target,
            host_key_prompt_known_hosts_path_text: host_key_prompt.known_hosts_path,
            host_key_prompt_algorithm_text: host_key_prompt.algorithm,
            host_key_prompt_fingerprint_text: host_key_prompt.fingerprint,
            host_key_prompt_expected_algorithm_text: host_key_prompt.expected_algorithm,
            host_key_prompt_expected_fingerprint_text: host_key_prompt.expected_fingerprint,
            host_key_prompt_mode_text: self.host_key_prompt_mode_text().to_owned(),
            password_prompt_visible: self.pending_password_prompt.is_some(),
            password_prompt_host_text: self.password_prompt_host_text(),
            tab_name_text,
            tab_state_text,
            tab_has_session,
            tab_count: i32::try_from(tabs.len()).unwrap_or(i32::MAX),
            tabs,
            active_tab_id: projection_window_active_tab,
            active_tab_mode_text,
            tab_has_disconnected,
            close_tabs_confirm_visible: pending_close.is_some(),
            close_tabs_confirm_single: pending_close.is_some_and(|pending| pending.single),
            close_tabs_confirm_name_text: pending_close
                .filter(|pending| pending.single)
                .and_then(|pending| pending.tab_ids.first())
                .map(|tab_id| self.tab_display_title(tab_id))
                .unwrap_or_default(),
            close_tabs_confirm_count: pending_close
                .map(|pending| i32::try_from(pending.tab_ids.len()).unwrap_or(i32::MAX))
                .unwrap_or(0),
            close_tabs_confirm_active_count: pending_close
                .map(|pending| i32::try_from(pending.active_connections).unwrap_or(i32::MAX))
                .unwrap_or(0),
            tab_menu_close_others_enabled,
            tab_menu_close_left_enabled,
            tab_menu_close_right_enabled,
            tab_menu_close_all_enabled,
            tab_menu_close_disconnected_enabled,
            tab_menu_reconnect_enabled,
            tab_menu_disconnect_enabled,
            tab_menu_move_to_new_window_enabled,
            tab_menu_move_to_main_window_enabled,
            tab_menu_mode_text,
            tab_menu_switch_mode_enabled,
            active_tab_can_switch_to_sftp,
            // N7：窗口关闭确认（投影窗口的挂起状态；无窗口登记时不可见）。
            close_window_confirm_visible: projection_window
                .is_some_and(|window_id| self.window_close_pending(window_id)),
            close_window_confirm_active_count: projection_window
                .map(|window_id| {
                    i32::try_from(self.active_connections_in_window(window_id)).unwrap_or(i32::MAX)
                })
                .unwrap_or(0),
            // WP4：分屏投影 + 菜单启用态。
            workspace_split,
            tab_menu_split_enabled,
            tab_menu_close_pane_enabled,
            // N9：同步发送按键。
            terminal_sync_all_enabled,
            terminal_sync_visible_enabled,
            terminal_sync_stop_enabled,
            tab_menu_receive_key_input_enabled,
            tab_menu_receives_key_input_checked,
            tab_menu_receive_key_input_reason_text,
            input_sync_active,
            input_sync_mode_text,
            input_sync_target_count,
            input_sync_source_name_text,
            input_sync_notice_kind_text,
            input_sync_notice_param_text,
            terminal_title_name_text,
            terminal_title_has_session,
            terminal_body_text,
            terminal_body_kind_text: terminal_body_kind_text.to_owned(),
            terminal_visible_lines: self.terminal_visible_lines(),
            terminal_cursor_column: self.terminal_cursor_column(),
            terminal_cursor_row: self.terminal_cursor_row(),
            terminal_frame_id: if self.projection_window.get().is_some() {
                focus_terminal.map(SessionRuntime::frame_id).unwrap_or(0)
            } else {
                self.active_terminal_frame_id()
            },
            terminal_scroll_offset: if self.projection_window.get().is_some() {
                focus_terminal
                    .map(|runtime| runtime.viewport_offset() as u64)
                    .unwrap_or(0)
            } else {
                self.active_terminal_scroll_offset()
            },
            terminal_scrollback_lines: i32::try_from(terminal_scrollback_lines).unwrap_or(i32::MAX),
            terminal_viewport_columns,
            terminal_viewport_rows: i32::try_from(terminal_viewport_rows).unwrap_or(i32::MAX),
            terminal_selection_active: focus_terminal
                .is_some_and(SessionRuntime::terminal_selection_active),
            terminal_has_selection: focus_terminal
                .is_some_and(SessionRuntime::terminal_selection_active),
            terminal_has_scrollback: focus_terminal.is_some_and(SessionRuntime::has_scrollback),
            terminal_has_screen_content: focus_terminal
                .is_some_and(SessionRuntime::has_screen_content),
            terminal_search_query_text: self.terminal_search_query.clone(),
            terminal_search_regex: self.terminal_search_regex,
            terminal_search_error_text: self.terminal_search_error.clone(),
            terminal_search_kind_text: terminal_search_kind_text.to_owned(),
            terminal_search_match_count,
            terminal_search_current_index,
            sftp_path_text: self.sftp_path.clone(),
            sftp_column_ratio: self.sftp_column_ratio,
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
            // --- N1 Phase 2 -------------------------------------------------
            local_path_text: self.local_pane.dir.display().to_string(),
            local_rows,
            local_crumbs: self.local_crumbs(),
            local_sort_column_text: self.local_pane.sort_column.id().to_owned(),
            local_sort_ascending: self.local_pane.sort_ascending,
            local_show_hidden: self.local_pane.show_hidden,
            local_item_count: i32::try_from(local_item_count).unwrap_or(i32::MAX),
            local_dir_count: i32::try_from(local_dir_count).unwrap_or(i32::MAX),
            local_empty_text: self.local_empty_text(),
            local_error_text: self.local_error_text(),
            local_error_kind_text: self
                .local_pane
                .error
                .as_ref()
                .map(|error| error.kind_id().to_owned())
                .unwrap_or_default(),
            local_error_retryable: self.local_error_retryable(),
            local_selected_count: i32::try_from(self.local_selection_count()).unwrap_or(i32::MAX),
            local_collapsed: self.local_pane.collapsed,
            local_selected_name_text: self.local_selected_name_text(),
            local_selection_key_text: self
                .local_selected_paths()
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            sftp_selection_key_text: self.sftp_selected_paths().join("\n"),
            sftp_selected_count: i32::try_from(self.sftp_selection_count()).unwrap_or(i32::MAX),
            sftp_menu_target_kind_text: self.sftp_menu_target_kind().to_owned(),
            sftp_selection_single_file: self.sftp_selection_is_single_file(),
            transfer_rows,
            transfer_drawer_expanded: self.transfer_drawer_expanded,
            transfer_total_count: i32::try_from(transfer_counts.total).unwrap_or(i32::MAX),
            transfer_active_count: i32::try_from(transfer_counts.active).unwrap_or(i32::MAX),
            transfer_paused_count: i32::try_from(transfer_counts.paused).unwrap_or(i32::MAX),
            transfer_failed_count: i32::try_from(transfer_counts.failed).unwrap_or(i32::MAX),
            transfer_completed_count: i32::try_from(transfer_counts.completed).unwrap_or(i32::MAX),
            transfer_retryable_count: i32::try_from(transfer_counts.retryable).unwrap_or(i32::MAX),
            sftp_conflict_visible: self.pending_sftp_conflict.is_some(),
            sftp_conflict_source_text: self
                .pending_sftp_conflict
                .as_ref()
                .map(|prompt| prompt.source_text.clone())
                .unwrap_or_default(),
            sftp_conflict_destination_text: self
                .pending_sftp_conflict
                .as_ref()
                .map(|prompt| prompt.destination_text.clone())
                .unwrap_or_default(),
            sftp_conflict_count: self
                .pending_sftp_conflict
                .as_ref()
                .map(|prompt| i32::try_from(prompt.conflicts.len()).unwrap_or(i32::MAX))
                .unwrap_or(0),
            sftp_conflict_paths_text: self
                .pending_sftp_conflict
                .as_ref()
                .map(|prompt| {
                    prompt
                        .conflicts
                        .iter()
                        .take(8)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
            sftp_properties_visible: self.sftp_properties_open,
            sftp_properties_name_text,
            sftp_properties_path_text: self.sftp_selected_path_text(),
            sftp_properties_kind_text,
            sftp_properties_size_text,
            sftp_properties_modified_text: crate::sftp_view::format_sftp_modified(
                self.selected_sftp_entry().and_then(|entry| entry.modified),
            ),
            sftp_properties_permissions_text: self.sftp_selected_permissions_text(),
            clipboard_side_text,
            clipboard_count,
            clipboard_cut,
            tunnels_summary_kind_text: tunnels_summary_kind_text.to_owned(),
            tunnels_summary_rows_text,
            tunnel_rows,
            status_text: self.status_text.clone(),
            status_kind,
            status_param_1,
            status_param_2,
            sftp_visible: self.panel_visible(PanelId::Sftp),
            tunnels_visible: self.panel_visible(PanelId::Tunnels),
            commands_visible: self.panel_visible(PanelId::QuickCommands),
            sessions_visible: self.panel_visible(PanelId::Sessions),
            local_visible: self.panel_visible(PanelId::Local),
            transfers_visible: self.panel_visible(PanelId::Transfers),
            layout: layout_projection(&panel_view),
            app_version_text: env!("CARGO_PKG_VERSION").to_owned(),
            quick_connect_visible: self.projection_quick_connect_visible(),
            quick_connect_input_text,
            quick_connect_error_text,
            quick_connect_last_target_text: self.quick_connect_last_target.clone(),
            quick_connect_history_enabled: self.config_document.quick_connect.enabled,
            quick_connect_summary_text,
            quick_connect_summary_kind_text,
            quick_connect_summary_param_1_text,
            quick_connect_summary_param_2_text,
            quick_connect_rows,
            quick_links_rows,
            private_keys_modal_visible: private_keys.visible,
            private_keys_rows: private_keys.rows,
            private_keys_selected_index: private_keys.selected_index,
            private_keys_summary_text: private_keys.summary_text,
            private_keys_details_text: private_keys.details_text,
            private_keys_public_key_text: private_keys.public_key_text,
            private_keys_test_result_text: private_keys.test_result_text,
            private_keys_status_text: private_keys.status_text,
            private_keys_status_kind_text: private_keys.status_kind_text,
            private_keys_status_param_1_text: private_keys.status_param_1_text,
            private_keys_status_param_2_text: private_keys.status_param_2_text,
            private_keys_remove_confirm_visible: private_keys.remove_confirm_visible,
            private_keys_deploy_confirm_visible: private_keys.deploy_confirm_visible,
            private_keys_deploy_command_text: private_keys.deploy_command_text,
            private_keys_deploy_summary_text: private_keys.deploy_summary_text,
            private_keys_remove_warning_text: private_keys.remove_warning_text,
            private_keys_import_label_text: private_keys.import_label_text,
            private_keys_import_path_text: private_keys.import_path_text,
            private_keys_import_passphrase_text: private_keys.import_passphrase_text,
            private_keys_remember_import_passphrase: private_keys.remember_import_passphrase,
            private_keys_import_enabled: private_keys.import_enabled,
            private_keys_passphrase_text: private_keys.passphrase_text,
            private_keys_passphrase_stored: private_keys.passphrase_stored,
            private_keys_passphrase_manage_enabled: private_keys.passphrase_manage_enabled,
            proxies_modal_visible: proxies.visible,
            proxies_rows: proxies.rows,
            proxies_selected_index: proxies.selected_index,
            proxies_summary_text: proxies.summary_text,
            proxies_status_text: proxies.status_text,
            proxies_status_kind_text: proxies.status_kind_text,
            proxies_status_param_1_text: proxies.status_param_1_text,
            proxies_status_param_2_text: proxies.status_param_2_text,
            proxies_test_result_text: proxies.test_result_text,
            proxies_remove_confirm_visible: proxies.remove_confirm_visible,
            proxies_remove_blocked: proxies.remove_blocked,
            proxies_referencing_text: proxies.referencing_text,
            proxies_editor_is_new: proxies.editor_is_new,
            proxies_editor_id_text: proxies.editor_id_text,
            proxies_editor_name_text: proxies.editor_name_text,
            proxies_editor_protocol_index: proxies.editor_protocol_index,
            proxies_editor_host_text: proxies.editor_host_text,
            proxies_editor_port_text: proxies.editor_port_text,
            proxies_editor_username_text: proxies.editor_username_text,
            proxies_editor_password_text: proxies.editor_password_text,
            proxies_editor_dns_by_proxy: proxies.editor_dns_by_proxy,
            proxies_editor_password_stored: proxies.editor_password_stored,
            proxies_editor_save_enabled: proxies.editor_save_enabled,
            proxies_test_in_flight: proxies.test_in_flight,
            host_keys_modal_visible: host_keys.visible,
            host_keys_groups: host_keys.groups,
            host_keys_selected_group: host_keys.selected_group,
            host_keys_selected_entry: host_keys.selected_entry,
            host_keys_key_count: host_keys.key_count,
            host_keys_details_text: host_keys.details_text,
            host_keys_detail_host_text: host_keys.detail_host_text,
            host_keys_detail_algorithm_text: host_keys.detail_algorithm_text,
            host_keys_detail_fingerprint_text: host_keys.detail_fingerprint_text,
            host_keys_detail_path_text: host_keys.detail_path_text,
            host_keys_path_text: host_keys.path_text,
            host_keys_status_text: host_keys.status_text,
            host_keys_status_kind_text: host_keys.status_kind_text,
            host_keys_status_param_1_text: host_keys.status_param_1_text,
            host_keys_status_param_2_text: host_keys.status_param_2_text,
            host_keys_import_visible: host_keys.import_visible,
            host_keys_import_text: host_keys.import_text,
            auth_prompt_visible: auth_prompt.visible,
            auth_prompt_host_text: auth_prompt.host_text,
            auth_prompt_problem_text: auth_prompt.problem_text,
            auth_prompt_has_usable_method: auth_prompt.has_usable_method,
            auth_prompt_selected_method: auth_prompt.selected_method,
            auth_prompt_password_visible: auth_prompt.password_visible,
            auth_prompt_publickey_visible: auth_prompt.publickey_visible,
            auth_prompt_keyboard_visible: auth_prompt.keyboard_visible,
            auth_prompt_submit_enabled: auth_prompt.submit_enabled,
            auth_prompt_password_text: auth_prompt.password_text,
            auth_prompt_remember_password: auth_prompt.remember_password,
            auth_prompt_key_options: auth_prompt.key_options,
            auth_prompt_selected_key_index: auth_prompt.selected_key_index,
            auth_prompt_selected_key_label: auth_prompt.selected_key_label,
            auth_prompt_passphrase_text: auth_prompt.passphrase_text,
            auth_prompt_use_agent: auth_prompt.use_agent,
            auth_prompt_publickey_submit_enabled: auth_prompt.publickey_submit_enabled,
            auth_prompt_challenge_name: auth_prompt.challenge_name,
            auth_prompt_challenge_instruction: auth_prompt.challenge_instruction,
            auth_prompt_challenge_prompts: auth_prompt.challenge_prompts,
            auth_prompt_challenge_round: auth_prompt.challenge_round,
            auth_prompt_challenge_round_total: auth_prompt.challenge_round_total,
            auth_prompt_keyboard_final_round: auth_prompt.keyboard_final_round,
            auth_prompt_keyboard_submit_enabled: auth_prompt.keyboard_submit_enabled,
            logging_active,
            logging_mode_text,
            logging_path_text,
            logging_file_name_text,
            terminal_logging_start_enabled,
            terminal_logging_stop_enabled,
            terminal_logging_open_file_enabled,
            terminal_logging_open_folder_enabled,
            logging_dialog_visible: self.logging_dialog.visible,
            logging_dialog_session_text: self.logging_dialog.session_name_text.clone(),
            logging_dialog_directory_text: self.logging_dialog.directory_text.clone(),
            logging_dialog_file_name_text: self.logging_dialog.file_name_text.clone(),
            logging_dialog_file_name_error_text: self.logging_dialog.file_name_error_text.clone(),
            logging_dialog_path_error_text: self.logging_dialog.path_error_text.clone(),
            logging_dialog_file_exists: self.logging_dialog.file_exists,
            logging_dialog_raw_format: self.logging_dialog.raw_format,
            logging_dialog_timestamps: self.logging_dialog.timestamps,
            logging_dialog_include_input: self.logging_dialog.include_input,
            logging_dialog_input_confirmed: self.logging_dialog.input_confirmed,
            logging_dialog_overwrite_confirmed: self.logging_dialog.overwrite_confirmed,
        }
    }

    pub(crate) fn startup_status(&self) -> String {
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

    pub(crate) fn active_session_parts(&self) -> (&'static str, String, String) {
        match self
            .focus_session_key()
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

    fn connection_overlay_projection(&self) -> ConnectionOverlayProjection {
        let Some(session) = self.focus_terminal_runtime() else {
            return ConnectionOverlayProjection::hidden();
        };
        let session_key = session.session_id().as_str();
        let pending_host_key = self
            .pending_host_key_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.session_key == session_key);
        let pending_auth = self
            .pending_auth_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.session_key == session_key);
        let pending_password = self.pending_password_prompt.is_some();
        let pending = pending_host_key || pending_auth || pending_password;
        // P2：host-key / auth / password 由模态对话框承载，不与终端 overlay 叠层。
        if pending {
            return ConnectionOverlayProjection::hidden();
        }
        let failed = session.state == SessionState::Failed && !session.connection_error.is_empty();
        let connecting = session.state == SessionState::Connecting || session.connection_in_flight;
        let visible = connecting || failed || session.state == SessionState::Failed;
        if !visible {
            return ConnectionOverlayProjection::hidden();
        }
        let stage = if !session.connection_stage.is_empty() {
            session.connection_stage.clone()
        } else if connecting {
            "tcp".to_owned()
        } else {
            "failed".to_owned()
        };
        ConnectionOverlayProjection {
            visible: true,
            target: session.connection_target_text(),
            stage,
            error: session.connection_error.clone(),
            failed: failed || session.state == SessionState::Failed,
            pending: false,
        }
    }

    fn status_connection_projection(&self) -> StatusConnectionProjection {
        let Some(session) = self.focus_terminal_runtime() else {
            return StatusConnectionProjection {
                state: "idle".to_owned(),
                target: String::new(),
                busy: false,
            };
        };
        let session_key = session.session_id().as_str();
        let pending = self
            .pending_host_key_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.session_key == session_key)
            || self
                .pending_auth_prompt
                .as_ref()
                .is_some_and(|prompt| prompt.session_key == session_key)
            || self.pending_password_prompt.is_some();
        let state = if session.connection_in_flight || session.state == SessionState::Connecting {
            "connecting"
        } else if pending {
            "pending"
        } else {
            match session.state {
                SessionState::Connected => "connected",
                SessionState::Failed => "failed",
                SessionState::Disconnected => "disconnected",
                SessionState::Idle => "idle",
                SessionState::Connecting => "connecting",
            }
        };
        // P2：连接中 / 等待确认已有呼吸圆点与状态文案，不再叠字面 "Busy"。
        let connecting_or_pending = state == "connecting" || state == "pending";
        StatusConnectionProjection {
            state: state.to_owned(),
            target: session.connection_target_text(),
            busy: !connecting_or_pending
                && (session.connection_in_flight || session.state == SessionState::Connecting),
        }
    }

    pub(crate) fn session_summary_parts(&self) -> (String, usize) {
        let lines = self
            .sessions
            .values()
            .take(6)
            .map(SessionRuntime::sidebar_summary)
            .collect::<Vec<_>>();
        let hidden = self.sessions.len().saturating_sub(lines.len());
        (lines.join("\n"), hidden)
    }

    pub(crate) fn saved_session_selection_parts(&self) -> (String, String, String, String) {
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

    pub(crate) fn saved_session_selection_legacy_text(&self) -> String {
        let (kind, name, host, id) = self.saved_session_selection_parts();
        match kind.as_str() {
            "profile" => format!("{name} ({host})"),
            "missing" => format!("{id} (missing)"),
            _ => "No saved session selected".to_owned(),
        }
    }

    pub(crate) fn saved_session_inventory_parts(&self) -> (String, bool) {
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

    pub(crate) fn recent_sessions_parts(&self) -> (String, bool) {
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

    pub(crate) fn terminal_title_parts(&self) -> (bool, String) {
        match self
            .focus_session_key()
            .and_then(|id| self.sessions.get(id))
        {
            Some(session) => (true, session.display_name.clone()),
            None => (false, String::new()),
        }
    }

    pub(crate) fn terminal_body_parts(&self) -> (&'static str, String) {
        match self
            .focus_session_key()
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

    pub(crate) fn terminal_visible_lines(&self) -> Vec<String> {
        self.focus_session_key()
            .and_then(|id| self.sessions.get(id))
            .map(|session| {
                let lines = session.visible_lines();
                if lines.iter().all(|line| line.is_empty()) {
                    vec![
                        "Terminal runtime is initialized but no output is available yet."
                            .to_owned(),
                    ]
                } else {
                    lines
                }
            })
            .unwrap_or_else(|| {
                vec![
                    "Runtime is ready. Open a Quick Connect target or create a local terminal tab with +."
                        .to_owned(),
                ]
            })
    }

    pub(crate) fn terminal_cursor_column(&self) -> i32 {
        self.focus_session_key()
            .and_then(|id| self.sessions.get(id))
            .map(|session| i32::from(session.cursor_position().0))
            .unwrap_or(0)
    }

    pub(crate) fn terminal_cursor_row(&self) -> i32 {
        self.focus_session_key()
            .and_then(|id| self.sessions.get(id))
            .map(|session| i32::from(session.cursor_position().1))
            .unwrap_or(0)
    }

    pub(crate) fn host_key_prompt_text(&self) -> String {
        let Some(prompt) = &self.pending_host_key_prompt else {
            return String::new();
        };
        match &prompt.expected {
            Some(expected) => format!(
                "Host key changed for {}@{}:{}.\nKnown hosts: {}\nExpected: {} {}\nPresented: {} {}\nChoose Trust Once or Replace and Trust.",
                prompt.username,
                prompt.host,
                prompt.port,
                prompt.known_hosts_path.display(),
                expected.algorithm,
                format_fingerprint_groups(&expected.fingerprint),
                prompt.presented.algorithm,
                format_fingerprint_groups(&prompt.presented.fingerprint)
            ),
            None => format!(
                "First-time host key for {}@{}:{}.\nKnown hosts: {}\nPresented: {} {}\nChoose Trust Once or Trust and Save.",
                prompt.username,
                prompt.host,
                prompt.port,
                prompt.known_hosts_path.display(),
                prompt.presented.algorithm,
                format_fingerprint_groups(&prompt.presented.fingerprint)
            ),
        }
    }

    /// D36：主机密钥弹窗的结构化正文字段（与 `host_key_prompt_text()` 同源：都读取
    /// `pending_host_key_prompt`；指纹同样走 D9 的 4 位分组）。无挂起提示时全为空串。
    fn host_key_prompt_parts(&self) -> HostKeyPromptParts {
        let Some(prompt) = &self.pending_host_key_prompt else {
            return HostKeyPromptParts::default();
        };
        HostKeyPromptParts {
            target: format!("{}@{}:{}", prompt.username, prompt.host, prompt.port),
            known_hosts_path: prompt.known_hosts_path.display().to_string(),
            algorithm: prompt.presented.algorithm.clone(),
            fingerprint: format_fingerprint_groups(&prompt.presented.fingerprint),
            expected_algorithm: prompt
                .expected
                .as_ref()
                .map(|expected| expected.algorithm.clone())
                .unwrap_or_default(),
            expected_fingerprint: prompt
                .expected
                .as_ref()
                .map(|expected| format_fingerprint_groups(&expected.fingerprint))
                .unwrap_or_default(),
        }
    }

    pub(crate) fn host_key_prompt_mode_text(&self) -> &'static str {
        match self
            .pending_host_key_prompt
            .as_ref()
            .map(PendingHostKeyPrompt::mode)
        {
            Some(HostKeyPromptMode::FirstTrust) => "first-trust",
            Some(HostKeyPromptMode::Changed) => "changed",
            None => "",
        }
    }

    pub(crate) fn password_prompt_host_text(&self) -> String {
        self.pending_password_prompt
            .as_ref()
            .map(PendingPasswordPrompt::host_text)
            .unwrap_or_default()
    }
}

/// D36：主机密钥弹窗结构化正文字段（`projection()` 用；与 `host_key_prompt_text()` 同源）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct HostKeyPromptParts {
    /// `user@host:port`
    target: String,
    known_hosts_path: String,
    /// 本次提供的算法 / 指纹（已分组）。
    algorithm: String,
    fingerprint: String,
    /// changed 模式：已保存的算法 / 指纹（已分组）；first-trust 为空串。
    expected_algorithm: String,
    expected_fingerprint: String,
}

/// N4：私钥页投影（`projection()` 的分段组装）。
struct PrivateKeysProjectionParts {
    visible: bool,
    rows: Vec<PrivateKeyRowData>,
    selected_index: i32,
    summary_text: String,
    details_text: String,
    public_key_text: String,
    test_result_text: String,
    status_text: String,
    /// I18N-1：页面内联状态的 kind/参数（Slint 通过 `TextFormats.status-message` 渲染）。
    status_kind_text: String,
    status_param_1_text: String,
    status_param_2_text: String,
    remove_confirm_visible: bool,
    deploy_confirm_visible: bool,
    deploy_command_text: String,
    deploy_summary_text: String,
    remove_warning_text: String,
    import_label_text: String,
    import_path_text: String,
    import_passphrase_text: String,
    remember_import_passphrase: bool,
    import_enabled: bool,
    passphrase_text: String,
    passphrase_stored: bool,
    passphrase_manage_enabled: bool,
}

/// NET-1：代理管理器页投影。
struct ProxiesProjectionParts {
    visible: bool,
    rows: Vec<ProxyRowData>,
    selected_index: i32,
    summary_text: String,
    status_text: String,
    status_kind_text: String,
    status_param_1_text: String,
    status_param_2_text: String,
    test_result_text: String,
    remove_confirm_visible: bool,
    remove_blocked: bool,
    referencing_text: String,
    editor_is_new: bool,
    editor_id_text: String,
    editor_name_text: String,
    editor_protocol_index: i32,
    editor_host_text: String,
    editor_port_text: String,
    editor_username_text: String,
    editor_password_text: String,
    editor_dns_by_proxy: bool,
    editor_password_stored: bool,
    editor_save_enabled: bool,
    test_in_flight: bool,
}

/// N4：主机密钥页投影。
struct HostKeysProjectionParts {
    visible: bool,
    groups: Vec<HostKeyGroupData>,
    selected_group: i32,
    selected_entry: i32,
    /// 全部密钥条数（跨 host:port 合计）。
    key_count: i32,
    details_text: String,
    /// D20：详情拆分字段（标签由 UI 侧 `@tr` 渲染；指纹为 D9 分组格式）。
    detail_host_text: String,
    detail_algorithm_text: String,
    detail_fingerprint_text: String,
    detail_path_text: String,
    path_text: String,
    status_text: String,
    status_kind_text: String,
    status_param_1_text: String,
    status_param_2_text: String,
    import_visible: bool,
    import_text: String,
}

/// WP1：终端连接 overlay 投影。
struct ConnectionOverlayProjection {
    visible: bool,
    target: String,
    stage: String,
    error: String,
    failed: bool,
    pending: bool,
}

impl ConnectionOverlayProjection {
    fn hidden() -> Self {
        Self {
            visible: false,
            target: String::new(),
            stage: String::new(),
            error: String::new(),
            failed: false,
            pending: false,
        }
    }
}

/// WP1：StatusBar 连接态。
struct StatusConnectionProjection {
    state: String,
    target: String,
    busy: bool,
}

/// N4：认证弹窗投影（服务端能力 + 已填内容 + 多轮挑战）。
struct AuthPromptProjectionParts {
    visible: bool,
    host_text: String,
    problem_text: String,
    has_usable_method: bool,
    selected_method: i32,
    password_visible: bool,
    publickey_visible: bool,
    keyboard_visible: bool,
    submit_enabled: bool,
    password_text: String,
    remember_password: bool,
    key_options: Vec<AuthKeyOptionData>,
    selected_key_index: i32,
    selected_key_label: String,
    passphrase_text: String,
    use_agent: bool,
    publickey_submit_enabled: bool,
    challenge_name: String,
    challenge_instruction: String,
    challenge_prompts: Vec<AuthPromptQuestionData>,
    challenge_round: i32,
    challenge_round_total: i32,
    keyboard_final_round: bool,
    keyboard_submit_enabled: bool,
}

/// 托管密钥 → 认证弹窗/会话编辑器共用的下拉选项（`detail` = 算法 · 指纹）。
fn auth_key_option_data(entry: &PrivateKeyEntryInfo) -> AuthKeyOptionData {
    AuthKeyOptionData {
        id: entry.id.clone(),
        label: entry.label.clone(),
        detail: format!("{} · {}", entry.algorithm, entry.fingerprint),
    }
}

impl AppRuntime {
    fn private_keys_projection_parts(&self) -> PrivateKeysProjectionParts {
        let rows = self.private_key_row_data();
        let selected_index = rows
            .iter()
            .position(|row| row.selected)
            .map(|index| i32::try_from(index).unwrap_or(i32::MAX))
            .unwrap_or(-1);
        let selected = self.selected_private_key_entry();
        let passphrase_saved = self
            .private_key_entries()
            .iter()
            .filter(|entry| entry.passphrase_stored)
            .count();
        let referenced = self
            .private_key_entries()
            .iter()
            .filter(|entry| !entry.used_by.is_empty())
            .count();
        let summary_text = format!(
            "{} key(s) · {} passphrase saved · {} referenced by saved sessions",
            rows.len(),
            passphrase_saved,
            referenced
        );
        let details_text = selected
            .as_ref()
            .map(PrivateKeyEntryInfo::details_text)
            .unwrap_or_default();
        let public_key_text = selected
            .as_ref()
            .map(|entry| entry.public_key.clone())
            .unwrap_or_default();
        let remove_warning_text = selected
            .as_ref()
            .map(|entry| {
                format!(
                    "Removing `{}` deletes its private key material and passphrase from the secret store. Used by: {}.",
                    entry.label,
                    entry.used_by_label()
                )
            })
            .unwrap_or_else(|| "Select a managed key to remove.".to_owned());
        let deploy_command_text = self
            .private_key_deployment(
                selected
                    .as_ref()
                    .map(|entry| entry.id.as_str())
                    .unwrap_or_default(),
            )
            .map(|deployment| deployment.command())
            .unwrap_or_default();
        let deploy_summary_text = match self.active_deploy_target() {
            Some(target) => format!(
                "Append the public key to ~/.ssh/authorized_keys on {target} (idempotent; directory/file permissions are fixed)."
            ),
            None => "Open a session to the target host first; the key is appended idempotently with permission fixes.".to_owned(),
        };
        PrivateKeysProjectionParts {
            visible: self.private_keys_modal_visible,
            rows,
            selected_index,
            summary_text,
            details_text,
            public_key_text,
            test_result_text: self.private_keys_test_result_text.clone(),
            status_text: self.private_keys_status_text.clone(),
            status_kind_text: self.private_keys_status_kind.clone(),
            status_param_1_text: self.private_keys_status_param_1.clone(),
            status_param_2_text: self.private_keys_status_param_2.clone(),
            remove_confirm_visible: self.private_keys_remove_confirm_visible,
            deploy_confirm_visible: self.private_keys_deploy_confirm_visible,
            deploy_command_text,
            deploy_summary_text,
            remove_warning_text,
            import_label_text: self.private_keys_import_label.clone(),
            import_path_text: self.private_keys_import_path.clone(),
            import_passphrase_text: self.private_keys_import_passphrase.clone(),
            remember_import_passphrase: self.private_keys_remember_import_passphrase,
            import_enabled: !self.private_keys_import_path.trim().is_empty(),
            passphrase_text: self.private_keys_passphrase_input.clone(),
            passphrase_stored: selected
                .as_ref()
                .is_some_and(|entry| entry.passphrase_stored),
            passphrase_manage_enabled: selected.is_some(),
        }
    }

    /// NET-1：代理管理器页投影。
    fn proxies_projection_parts(&self) -> ProxiesProjectionParts {
        let rows = self.proxy_row_data();
        let selected_index = rows
            .iter()
            .position(|row| row.selected)
            .map(|index| i32::try_from(index).unwrap_or(i32::MAX))
            .unwrap_or(-1);
        let referenced = self
            .proxy_used_by()
            .values()
            .filter(|names| !names.is_empty())
            .count();
        let summary_text = format!(
            "{} profile(s) · {} referenced by saved sessions",
            rows.len(),
            referenced
        );
        let editing = self
            .proxies_editor_id
            .as_deref()
            .and_then(|id| self.config_document.proxy_profiles.get(id));
        ProxiesProjectionParts {
            visible: self.proxies_modal_visible,
            rows,
            selected_index,
            summary_text,
            status_text: self.proxies_status_text.clone(),
            status_kind_text: self.proxies_status_kind.clone(),
            status_param_1_text: self.proxies_status_param_1.clone(),
            status_param_2_text: self.proxies_status_param_2.clone(),
            test_result_text: self.proxies_test_result_text.clone(),
            remove_confirm_visible: self.proxies_remove_confirm_visible,
            remove_blocked: self.proxies_remove_blocked,
            referencing_text: self.proxies_referencing_text.clone(),
            editor_is_new: self.proxies_editor_id.is_none(),
            editor_id_text: self.proxies_editor_id.clone().unwrap_or_default(),
            editor_name_text: self.proxies_editor_name.clone(),
            editor_protocol_index: proxy_protocol_index(self.proxies_editor_protocol),
            editor_host_text: self.proxies_editor_host.clone(),
            editor_port_text: self.proxies_editor_port_text.clone(),
            editor_username_text: self.proxies_editor_username.clone(),
            editor_password_text: self.proxies_editor_password.clone(),
            editor_dns_by_proxy: self.proxies_editor_dns_by_proxy,
            editor_password_stored: editing
                .is_some_and(|profile| profile.password_secret_key.is_some()),
            editor_save_enabled: !self.proxies_editor_name.trim().is_empty()
                && !self.proxies_editor_host.trim().is_empty()
                && !self.proxies_editor_port_text.trim().is_empty(),
            test_in_flight: self.proxy_test_in_flight.is_some(),
        }
    }

    fn active_deploy_target(&self) -> Option<String> {
        let session_key = self.focus_session_key()?;
        let session = self.sessions.get(session_key)?;
        // TAB-4：本地终端没有 SSH 目标，密钥部署入口不应显示可用目标。
        if session.is_local() {
            return None;
        }
        Some(format!(
            "{}@{}:{}",
            session.username_label(),
            session.host_label(),
            session.ssh_config.port
        ))
    }

    fn host_keys_projection_parts(&self) -> HostKeysProjectionParts {
        let groups = self.host_key_group_data();
        let key_count = i32::try_from(
            groups
                .iter()
                .map(|group| group.entries.len())
                .sum::<usize>(),
        )
        .unwrap_or(i32::MAX);
        HostKeysProjectionParts {
            visible: self.host_keys_modal_visible,
            groups,
            selected_group: self
                .host_keys_selected_group
                .map(|index| i32::try_from(index).unwrap_or(i32::MAX))
                .unwrap_or(-1),
            selected_entry: self
                .host_keys_selected_entry
                .map(|index| i32::try_from(index).unwrap_or(i32::MAX))
                .unwrap_or(-1),
            key_count,
            details_text: self.host_keys_details_text(),
            detail_host_text: self.host_keys_detail_host_text(),
            detail_algorithm_text: self.host_keys_detail_algorithm_text(),
            detail_fingerprint_text: self.host_keys_detail_fingerprint_text(),
            detail_path_text: self.host_keys_detail_path_text(),
            path_text: self.config_store.known_hosts_file().display().to_string(),
            status_text: self.host_keys_status_text.clone(),
            status_kind_text: self.host_keys_status_kind.clone(),
            status_param_1_text: self.host_keys_status_param_1.clone(),
            status_param_2_text: self.host_keys_status_param_2.clone(),
            import_visible: self.host_keys_import_visible,
            import_text: self.host_keys_import_text.clone(),
        }
    }

    fn auth_prompt_projection_parts(&self) -> AuthPromptProjectionParts {
        let prompt = self.pending_auth_prompt.as_ref();
        let key_entries = self.private_key_entries();
        let key_options = key_entries
            .iter()
            .map(auth_key_option_data)
            .collect::<Vec<_>>();
        let selected_key_index = prompt
            .map(|prompt| prompt.selected_key_index(&key_entries))
            .unwrap_or(-1);
        let selected_key_label = usize::try_from(selected_key_index)
            .ok()
            .and_then(|index| key_entries.get(index))
            .map(|entry| format!("{} — {}", entry.label, entry.algorithm))
            .or_else(|| {
                prompt
                    .map(|prompt| prompt.browsed_key_path.clone())
                    .filter(|path| !path.is_empty())
            })
            .unwrap_or_default();
        let visible_methods = prompt
            .map(PendingAuthPrompt::visible_methods)
            .unwrap_or_default();
        let active_round = prompt.and_then(|prompt| prompt.active_keyboard_round.as_ref());
        let rounds_seen = prompt
            .map(|prompt| {
                prompt.keyboard_rounds.len() + usize::from(prompt.active_keyboard_round.is_some())
            })
            .unwrap_or(0);
        let challenge_round_total = rounds_seen;
        let keyboard_submit_enabled =
            prompt.is_some_and(PendingAuthPrompt::keyboard_submit_enabled);
        let keyboard_final_round = prompt
            .map(|prompt| prompt.keyboard_final_round(challenge_round_total.max(1)))
            .unwrap_or(false);
        AuthPromptProjectionParts {
            visible: prompt.is_some(),
            host_text: prompt.map(PendingAuthPrompt::host_text).unwrap_or_default(),
            problem_text: prompt
                .and_then(PendingAuthPrompt::problem_text)
                .unwrap_or_default(),
            has_usable_method: prompt.is_none_or(PendingAuthPrompt::has_usable_method),
            selected_method: prompt.map(|prompt| prompt.selected().index()).unwrap_or(0),
            password_visible: prompt.is_none()
                || visible_methods.contains(&AuthPromptMethod::Password),
            publickey_visible: prompt.is_none()
                || visible_methods.contains(&AuthPromptMethod::PublicKey),
            keyboard_visible: prompt.is_none()
                || visible_methods.contains(&AuthPromptMethod::KeyboardInteractive),
            submit_enabled: prompt.is_some_and(|prompt| {
                prompt.selected() == AuthPromptMethod::Password
                    && !prompt.password_text.trim().is_empty()
            }),
            password_text: prompt
                .map(|prompt| prompt.password_text.clone())
                .unwrap_or_default(),
            remember_password: prompt.is_some_and(|prompt| prompt.remember_password),
            key_options,
            selected_key_index,
            selected_key_label,
            passphrase_text: prompt
                .map(|prompt| prompt.passphrase_text.clone())
                .unwrap_or_default(),
            use_agent: prompt.is_some_and(|prompt| prompt.use_agent),
            publickey_submit_enabled: prompt
                .is_some_and(PendingAuthPrompt::publickey_submit_enabled),
            challenge_name: active_round
                .map(|round| round.name.clone())
                .unwrap_or_default(),
            challenge_instruction: active_round
                .map(|round| round.instruction.clone())
                .unwrap_or_default(),
            challenge_prompts: prompt
                .map(PendingAuthPrompt::active_keyboard_prompts)
                .unwrap_or_default(),
            challenge_round: active_round
                .map(|_| {
                    i32::try_from(
                        prompt
                            .map(|prompt| prompt.keyboard_rounds.len() + 1)
                            .unwrap_or(1),
                    )
                    .unwrap_or(i32::MAX)
                })
                .unwrap_or(0),
            challenge_round_total: i32::try_from(challenge_round_total).unwrap_or(i32::MAX),
            keyboard_final_round,
            keyboard_submit_enabled,
        }
    }
}

/// Value-only projection for the window. Natural-language templates live in
/// `ui/main_window.slint` (`@tr`), so every field here is either a raw value
/// (host/path/count/enum id) or machine data assembled from values.
// WP4：含 `workspace_split.ratio: f32`，故去掉 `Eq`（仍保留 `PartialEq`）。
#[derive(Debug, Clone, PartialEq)]
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
    pub editor_password_text: String,
    /// SSH-1：Key 认证可选的托管密钥（`AuthKeyOptionData`: id/label/detail）。
    pub editor_key_options: Vec<AuthKeyOptionData>,
    pub editor_selected_key_index: i32,
    /// 选中的 `key_id` 已从配置中消失（例如在密钥管理器里被删除）。
    pub editor_selected_key_missing: bool,
    /// 旧 path-only 私钥路径（只读 legacy 提示；空 = 非 legacy）。
    pub editor_legacy_key_path_text: String,
    pub editor_legacy_key_has_passphrase: bool,
    pub editor_has_stored_password: bool,
    pub editor_target_session_id_text: String,
    pub editor_target_editing: bool,
    pub editor_auth_test_kind_text: String,
    pub editor_auth_test_host_text: String,
    pub editor_auth_test_backend_text: String,
    pub editor_auth_test_startup_text: String,
    pub editor_auth_test_error_text: String,
    pub editor_proxy_mode_text: String,
    /// NET-1：可选的托管代理（id/label/detail）。
    pub editor_proxy_options: Vec<ProxyProfileOptionData>,
    pub editor_selected_proxy_index: i32,
    /// 选中托管代理的只读摘要（协议 · 地址 · 用户名 · 口令状态）。
    pub editor_proxy_selected_detail_text: String,
    /// 旧 per-session / 悬空代理引用的只读摘要（非 legacy 时为空）。
    pub editor_proxy_legacy_text: String,
    /// 选中的托管代理已从管理器删除（保存会被拒绝）。
    pub editor_proxy_selected_missing: bool,
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
    /// Settings page field: max parallel SFTP transfers.
    pub settings_sftp_max_parallel_transfers_text: String,
    /// UI language preference: `system` / `zh-CN` / `en-US`.
    pub settings_language_text: String,
    /// Whether process debug logs are written under `{config_dir}/logs`.
    pub settings_debug_logging_enabled: bool,
    /// Whether closing a session with an active connection asks for confirmation.
    pub settings_confirm_close_session: bool,
    /// Whether quitting the application asks for confirmation.
    pub settings_confirm_quit_app: bool,
    /// Absolute path of the debug log directory.
    pub settings_debug_log_dir_text: String,
    pub settings_terminal_status_kind_text: String,
    pub settings_terminal_status_field_text: String,
    pub settings_terminal_status_value_text: String,
    pub settings_terminal_status_limit_text: String,
    // --- N5：Settings → Appearance（字体 + 当前主题摘要） -----------------------
    pub settings_appearance_visible: bool,
    pub settings_appearance_color_scheme_name_text: String,
    pub settings_appearance_color_scheme_builtin: bool,
    pub settings_appearance_font_family_text: String,
    /// D15 ComboBox model: bundled DejaVu + optional imported path.
    pub settings_appearance_font_options: Vec<String>,
    pub settings_appearance_font_index: i32,
    pub settings_appearance_font_size_text: String,
    /// Single backup font path shown in Settings (TERM-5).
    pub settings_appearance_backup_font_text: String,
    pub settings_appearance_status_text: String,
    pub settings_appearance_palette_swatches: Vec<[u8; 3]>,
    pub settings_appearance_preview: Option<yshell_terminal::TerminalFrame>,
    // --- TERM-1：独立主题管理器 ------------------------------------------------
    pub theme_manager_visible: bool,
    pub theme_manager_rows: Vec<ThemeManagerRowData>,
    pub theme_manager_selected_index: i32,
    pub theme_manager_default_name_text: String,
    pub theme_manager_selected_is_default: bool,
    pub theme_manager_builtin_count: i32,
    pub theme_manager_user_count: i32,
    pub theme_manager_preview: Option<yshell_terminal::TerminalFrame>,
    pub theme_manager_remove_confirm_visible: bool,
    pub theme_manager_remove_name_text: String,
    pub theme_manager_remove_reference_count: i32,
    pub theme_editor_open: bool,
    pub theme_editor_builtin: bool,
    pub theme_editor_id: String,
    pub theme_editor_name: String,
    pub theme_editor_foreground: String,
    pub theme_editor_background: String,
    pub theme_editor_cursor: String,
    pub theme_editor_selection: String,
    pub theme_editor_ansi: Vec<String>,
    // --- N5：Folder Editor ------------------------------------------------------
    pub folder_editor_visible: bool,
    pub folder_editor_name_text: String,
    pub folder_editor_path_text: String,
    pub folder_editor_status_text: String,
    /// I18N-1：文件夹默认值页内联状态的 kind/参数（英文 `_status_text` 为回退）。
    pub folder_editor_status_kind_text: String,
    pub folder_editor_status_param_1_text: String,
    pub folder_editor_status_param_2_text: String,
    pub folder_editor_appearance_fields: Vec<TriStateFieldData>,
    pub folder_editor_terminal_fields: Vec<TriStateFieldData>,
    pub folder_editor_logging_fields: Vec<TriStateFieldData>,
    /// Selected saved-session tree node is a folder (menu entry enabling).
    pub saved_folder_selected: bool,
    // --- N5：Session Editor 外观/终端覆盖 ----------------------------------------
    pub editor_appearance_fields: Vec<TriStateFieldData>,
    pub editor_terminal_fields: Vec<TriStateFieldData>,
    /// TERM-4：外观分区 live preview（编辑器关闭或停在其它分区时为 `None`）。
    /// 与 `settings_appearance_preview` 共用同一渲染路径与像素契约。
    pub session_editor_preview: Option<yshell_terminal::TerminalFrame>,
    pub editor_modal_visible: bool,
    pub editor_discard_dialog_visible: bool,
    pub editor_is_dirty: bool,
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
    /// D17：已知主机详情拆分字段（标签由 UI 侧 `@tr` 渲染；指纹为 D9 分组格式）。
    pub known_hosts_detail_host_text: String,
    pub known_hosts_detail_port_text: String,
    pub known_hosts_detail_algorithm_text: String,
    pub known_hosts_detail_fingerprint_text: String,
    pub known_hosts_detail_path_text: String,
    pub known_hosts_path_text: String,
    pub known_hosts_clear_confirmation_text: String,
    pub active_session_kind_text: String,
    pub active_session_name_text: String,
    pub active_session_state_text: String,
    /// W5-A3：当前是否存在活动运行时会话（菜单/按钮状态用，纯布尔量）。
    pub has_active_session: bool,
    /// W5-A3：活动运行时会话是否处于 `connected` 状态。
    pub active_session_connected: bool,
    /// WP1：终端连接 overlay（connecting / failed / pending-confirmation）。
    pub connection_overlay_visible: bool,
    pub connection_overlay_target_text: String,
    pub connection_overlay_stage_text: String,
    pub connection_overlay_error_text: String,
    pub connection_overlay_failed: bool,
    pub connection_overlay_pending: bool,
    /// WP1：StatusBar 连接态圆点 / 目标摘要 / busy。
    pub status_connection_state_text: String,
    pub status_connection_target_text: String,
    pub status_connection_busy: bool,
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
    /// D36：弹窗正文拆分字段（标签由 UI 侧 `@tr` 渲染；指纹已按 4 位分组）。
    /// 无挂起提示时为空串；changed 模式的 expected 侧仅在该模式下非空。
    pub host_key_prompt_target_text: String,
    pub host_key_prompt_known_hosts_path_text: String,
    pub host_key_prompt_algorithm_text: String,
    pub host_key_prompt_fingerprint_text: String,
    pub host_key_prompt_expected_algorithm_text: String,
    pub host_key_prompt_expected_fingerprint_text: String,
    pub host_key_prompt_mode_text: String,
    /// W5：连接需要密码而密钥库无法提供时挂起输入（仅本次连接使用，不落盘）。
    pub password_prompt_visible: bool,
    /// 密码弹窗文案里的 `user@host:port`（由挂起目标组装）。
    pub password_prompt_host_text: String,
    pub tab_name_text: String,
    pub tab_state_text: String,
    pub tab_has_session: bool,
    /// N0：标签条投影（顺序 = 显示顺序）。
    pub tabs: Vec<TabData>,
    /// N0：活动标签 id（无标签时为空串）。
    pub active_tab_id: String,
    /// `new` / `terminal` / `sftp` — active tab mode for the projected window.
    pub active_tab_mode_text: String,
    pub tab_count: i32,
    /// N0：是否存在非 connected/connecting 的标签（`Close Disconnected Tabs` 启用条件）。
    pub tab_has_disconnected: bool,
    /// N0：关闭确认弹窗状态（单个或批量）。
    pub close_tabs_confirm_visible: bool,
    pub close_tabs_confirm_single: bool,
    pub close_tabs_confirm_name_text: String,
    pub close_tabs_confirm_count: i32,
    pub close_tabs_confirm_active_count: i32,
    /// N0：标签右键菜单的启用条件（针对右键时选中的标签计算；见设计 §7）。
    pub tab_menu_close_others_enabled: bool,
    pub tab_menu_close_left_enabled: bool,
    pub tab_menu_close_right_enabled: bool,
    pub tab_menu_close_all_enabled: bool,
    pub tab_menu_close_disconnected_enabled: bool,
    /// D18：被右键标签的 Reconnect/Disconnect 启用条件。
    pub tab_menu_reconnect_enabled: bool,
    pub tab_menu_disconnect_enabled: bool,
    /// N7：标签右键"移动到新窗口 / 移动到主窗口"的启用条件。
    pub tab_menu_move_to_new_window_enabled: bool,
    pub tab_menu_move_to_main_window_enabled: bool,
    /// Mode of the right-clicked tab (`new`/`terminal`/`sftp`).
    pub tab_menu_mode_text: String,
    /// Right-clicked tab can switch between Terminal and SFTP.
    pub tab_menu_switch_mode_enabled: bool,
    /// Active tab can switch Terminal → SFTP (content context menu).
    pub active_tab_can_switch_to_sftp: bool,
    // --- N7：窗口关闭确认（关闭带活动连接的窗口时）--------------------------------
    /// 该窗口正在等待"关闭并断开"确认。
    pub close_window_confirm_visible: bool,
    /// 窗口内处于 connected/connecting 的标签数。
    pub close_window_confirm_active_count: i32,
    // --- WP4：窗口内分屏 --------------------------------------------------------
    pub workspace_split: WorkspaceSplitProjection,
    /// 标签/View 菜单 Split Right/Down 是否可用。
    pub tab_menu_split_enabled: bool,
    /// Close Current Pane 是否可用。
    pub tab_menu_close_pane_enabled: bool,

    // --- N9：同步发送按键（终端/标签菜单 + 状态栏 chip + 标签角标）-------------
    /// 终端右键菜单：`Send Key Input to All Tabs` / `… to Visible Tabs` 可用性。
    pub terminal_sync_all_enabled: bool,
    pub terminal_sync_visible_enabled: bool,
    /// 终端右键菜单：`Stop Sending Key Input` 可用性（同步进行中）。
    pub terminal_sync_stop_enabled: bool,
    /// 标签右键菜单：`Receive Key Input` 的启用/勾选/禁用原因。
    pub tab_menu_receive_key_input_enabled: bool,
    pub tab_menu_receives_key_input_checked: bool,
    pub tab_menu_receive_key_input_reason_text: String,
    /// 是否正在同步发送按键（状态栏 chip / 标签角标）。
    pub input_sync_active: bool,
    /// 目标选择模式 id（`all`/`visible`/`selected`）。
    pub input_sync_mode_text: String,
    pub input_sync_target_count: i32,
    /// 同步源标签的显示名（chip 的 accessible label 用）。
    pub input_sync_source_name_text: String,
    /// chip 的一次性提示（kind + 参数；kind 空 = 无提示）。
    pub input_sync_notice_kind_text: String,
    pub input_sync_notice_param_text: String,
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
    /// 当前视口列数（状态栏尺寸显示）。
    pub terminal_viewport_columns: i32,
    /// 当前视口行数（取自 `active_terminal_viewport_metrics`）：滚动条滑块比例与可见性用。
    pub terminal_viewport_rows: i32,
    pub terminal_selection_active: bool,
    /// W5-A3：活动终端是否存在可复制选区（Edit/Copy 菜单项状态，与
    /// `terminal_selection_active` 同源；后者继续供终端视图渲染选区浮层）。
    pub terminal_has_selection: bool,
    /// WP2：scrollback 非空（Clear Scrollback 启用条件）。
    pub terminal_has_scrollback: bool,
    /// WP2：当前屏幕有可见内容（Select/Copy Screen 启用条件）。
    pub terminal_has_screen_content: bool,
    pub terminal_search_query_text: String,
    pub terminal_search_regex: bool,
    pub terminal_search_error_text: String,
    pub terminal_search_kind_text: String,
    pub terminal_search_match_count: i32,
    pub terminal_search_current_index: i32,
    pub sftp_path_text: String,
    /// Per-SFTP-tab local/remote split ratio (0..1).
    pub sftp_column_ratio: f32,
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
    // --- N1 Phase 2：本地栏 / 队列抽屉 / 冲突与属性 / 剪贴板 -------------------
    /// Local pane path (lexically normalized absolute path).
    pub local_path_text: String,
    /// Local pane rows, including the leading `..` marker.
    pub local_rows: Vec<crate::local_fs::LocalRowData>,
    /// Local pane breadcrumb segments (same shape as the remote ones).
    pub local_crumbs: Vec<crate::runtime::FileCrumbData>,
    pub local_sort_column_text: String,
    pub local_sort_ascending: bool,
    pub local_show_hidden: bool,
    pub local_item_count: i32,
    pub local_dir_count: i32,
    pub local_empty_text: String,
    pub local_error_text: String,
    pub local_error_kind_text: String,
    pub local_error_retryable: bool,
    pub local_selected_count: i32,
    pub local_collapsed: bool,
    /// Names of up to three selected local entries (summary row).
    pub local_selected_name_text: String,
    /// Joined selected local paths; only used as the Slint `data` binding
    /// dependency (the drag payload itself is built by the host).
    pub local_selection_key_text: String,
    /// Joined selected remote paths (same purpose as the local key).
    pub sftp_selection_key_text: String,
    /// Remote multi-selection size (0 = blank-area menu).
    pub sftp_selected_count: i32,
    /// Remote context-menu target kind: `file`/`dir`/`blank`/`multi`.
    pub sftp_menu_target_kind_text: String,
    /// Single regular file selected (enables the single-file actions).
    pub sftp_selection_single_file: bool,
    /// Transfer drawer rows (newest first) and its disclosure state.
    pub transfer_rows: Vec<TransferRowData>,
    pub transfer_drawer_expanded: bool,
    pub transfer_total_count: i32,
    pub transfer_active_count: i32,
    pub transfer_paused_count: i32,
    pub transfer_failed_count: i32,
    pub transfer_completed_count: i32,
    pub transfer_retryable_count: i32,
    /// Ask-policy conflict prompt (re-runs the stored spec with a policy).
    pub sftp_conflict_visible: bool,
    pub sftp_conflict_source_text: String,
    pub sftp_conflict_destination_text: String,
    pub sftp_conflict_count: i32,
    pub sftp_conflict_paths_text: String,
    /// Remote Properties dialog fields (first selected entry).
    pub sftp_properties_visible: bool,
    pub sftp_properties_name_text: String,
    pub sftp_properties_path_text: String,
    pub sftp_properties_kind_text: String,
    pub sftp_properties_size_text: String,
    pub sftp_properties_modified_text: String,
    pub sftp_properties_permissions_text: String,
    /// In-app file clipboard (`local`/`remote`/empty).
    pub clipboard_side_text: String,
    pub clipboard_count: i32,
    pub clipboard_cut: bool,
    pub tunnels_summary_kind_text: String,
    pub tunnels_summary_rows_text: String,
    /// Live dock Tunnels panel rows for the active session.
    pub tunnel_rows: Vec<crate::runtime::TunnelRowData>,
    pub status_text: String,
    /// 状态栏 i18n 模板：覆盖的生产者填 kind + 参数，其余路径保持空 kind，
    /// Slint 端回退英文 `status_text` 原文。
    pub status_kind: String,
    pub status_param_1: String,
    pub status_param_2: String,
    pub sftp_visible: bool,
    pub tunnels_visible: bool,
    pub commands_visible: bool,
    /// N3：Sessions / Local / Transfers 面板可见性（View/Panels 菜单勾选项）。
    pub sessions_visible: bool,
    pub local_visible: bool,
    pub transfers_visible: bool,
    /// N3：内容区布局投影（面板框 px 几何 + 断点标志）。
    pub layout: LayoutProjection,
    pub app_version_text: String,
    // --- N2：Quick Connect 覆盖层（`ui/pages/quick_connect.slint` 的投影）---------
    /// TAB-6：工作区是否显示 Quick Connect 覆盖层（不占标签；只由 overlay 状态驱动）。
    pub quick_connect_visible: bool,
    /// 输入框内容（in-out；Rust 侧在提交成功/清空时写回）。
    pub quick_connect_input_text: String,
    /// 非法输入的内联错误（空 = 无错误）。
    pub quick_connect_error_text: String,
    /// 最近一次连接成功的目标（"保存为会话…"入口；空 = 不显示）。
    pub quick_connect_last_target_text: String,
    /// D12：连接历史是否开启。
    pub quick_connect_history_enabled: bool,
    /// 输入框下方的摘要（历史条数/上限；历史关闭时提示去设置里开启）。
    pub quick_connect_summary_text: String,
    /// D37：QC 摘要的 `TextFormats.status-message` kind/参数（页面按此渲染；
    /// `quick_connect_summary_text` 保留为回退文案）。
    pub quick_connect_summary_kind_text: String,
    pub quick_connect_summary_param_1_text: String,
    pub quick_connect_summary_param_2_text: String,
    /// "最近连接"行（历史 + 已保存会话最近使用，已去重排序）。
    pub quick_connect_rows: Vec<QuickConnectRowData>,
    /// "快速链接"行（已按 sort_order 排序）。
    pub quick_links_rows: Vec<QuickLinkRowData>,
    // --- N4：私钥页 ----------------------------------------------------------
    pub private_keys_modal_visible: bool,
    pub private_keys_rows: Vec<PrivateKeyRowData>,
    pub private_keys_selected_index: i32,
    pub private_keys_summary_text: String,
    pub private_keys_details_text: String,
    pub private_keys_public_key_text: String,
    pub private_keys_test_result_text: String,
    pub private_keys_status_text: String,
    /// I18N-1：私钥页内联状态的 kind/参数（`private_keys_status_text` 为英文回退）。
    pub private_keys_status_kind_text: String,
    pub private_keys_status_param_1_text: String,
    pub private_keys_status_param_2_text: String,
    pub private_keys_remove_confirm_visible: bool,
    pub private_keys_deploy_confirm_visible: bool,
    pub private_keys_deploy_command_text: String,
    pub private_keys_deploy_summary_text: String,
    pub private_keys_remove_warning_text: String,
    pub private_keys_import_label_text: String,
    pub private_keys_import_path_text: String,
    pub private_keys_import_passphrase_text: String,
    pub private_keys_remember_import_passphrase: bool,
    pub private_keys_import_enabled: bool,
    pub private_keys_passphrase_text: String,
    pub private_keys_passphrase_stored: bool,
    pub private_keys_passphrase_manage_enabled: bool,
    // --- NET-1：代理管理器页 ---------------------------------------------------
    pub proxies_modal_visible: bool,
    pub proxies_rows: Vec<ProxyRowData>,
    pub proxies_selected_index: i32,
    pub proxies_summary_text: String,
    pub proxies_status_text: String,
    /// I18N-1：代理页内联状态的 kind/参数（`proxies_status_text` 为英文回退）。
    pub proxies_status_kind_text: String,
    pub proxies_status_param_1_text: String,
    pub proxies_status_param_2_text: String,
    pub proxies_test_result_text: String,
    pub proxies_remove_confirm_visible: bool,
    /// D8：保存会话仍引用该代理，删除被阻止。
    pub proxies_remove_blocked: bool,
    /// 被阻止时的引用会话名（逗号分隔）。
    pub proxies_referencing_text: String,
    pub proxies_editor_is_new: bool,
    pub proxies_editor_id_text: String,
    pub proxies_editor_name_text: String,
    pub proxies_editor_protocol_index: i32,
    pub proxies_editor_host_text: String,
    pub proxies_editor_port_text: String,
    pub proxies_editor_username_text: String,
    pub proxies_editor_password_text: String,
    pub proxies_editor_dns_by_proxy: bool,
    pub proxies_editor_password_stored: bool,
    pub proxies_editor_save_enabled: bool,
    pub proxies_test_in_flight: bool,
    // --- N4：主机密钥页 ------------------------------------------------------
    pub host_keys_modal_visible: bool,
    pub host_keys_groups: Vec<HostKeyGroupData>,
    pub host_keys_selected_group: i32,
    pub host_keys_selected_entry: i32,
    pub host_keys_key_count: i32,
    pub host_keys_details_text: String,
    /// D20：主机密钥页详情拆分字段（标签由 UI 侧 `@tr` 渲染；指纹为 D9 分组格式）。
    pub host_keys_detail_host_text: String,
    pub host_keys_detail_algorithm_text: String,
    pub host_keys_detail_fingerprint_text: String,
    pub host_keys_detail_path_text: String,
    pub host_keys_path_text: String,
    pub host_keys_status_text: String,
    /// I18N-1：主机密钥页内联状态的 kind/参数（`host_keys_status_text` 为英文回退）。
    pub host_keys_status_kind_text: String,
    pub host_keys_status_param_1_text: String,
    pub host_keys_status_param_2_text: String,
    pub host_keys_import_visible: bool,
    pub host_keys_import_text: String,
    // --- N4：认证弹窗 --------------------------------------------------------
    pub auth_prompt_visible: bool,
    pub auth_prompt_host_text: String,
    pub auth_prompt_problem_text: String,
    pub auth_prompt_has_usable_method: bool,
    pub auth_prompt_selected_method: i32,
    pub auth_prompt_password_visible: bool,
    pub auth_prompt_publickey_visible: bool,
    pub auth_prompt_keyboard_visible: bool,
    pub auth_prompt_submit_enabled: bool,
    pub auth_prompt_password_text: String,
    pub auth_prompt_remember_password: bool,
    pub auth_prompt_key_options: Vec<AuthKeyOptionData>,
    pub auth_prompt_selected_key_index: i32,
    pub auth_prompt_selected_key_label: String,
    pub auth_prompt_passphrase_text: String,
    pub auth_prompt_use_agent: bool,
    pub auth_prompt_publickey_submit_enabled: bool,
    pub auth_prompt_challenge_name: String,
    pub auth_prompt_challenge_instruction: String,
    pub auth_prompt_challenge_prompts: Vec<AuthPromptQuestionData>,
    pub auth_prompt_challenge_round: i32,
    pub auth_prompt_challenge_round_total: i32,
    pub auth_prompt_keyboard_final_round: bool,
    pub auth_prompt_keyboard_submit_enabled: bool,
    // --- N6：终端日志（弹窗 / 菜单 / REC / 状态栏）---------------------------
    /// 活动会话是否有日志输出（REC 指示）。
    pub logging_active: bool,
    /// 活动会话的日志类型 id（`off`/`auto`/`manual`）。
    pub logging_mode_text: String,
    /// 活动/最近日志文件路径（空 = 无）。
    pub logging_path_text: String,
    /// 活动/最近日志文件名（状态栏 REC 文案用，去掉目录）。
    pub logging_file_name_text: String,
    /// 终端右键菜单/状态栏入口的启用条件。
    pub terminal_logging_start_enabled: bool,
    pub terminal_logging_stop_enabled: bool,
    pub terminal_logging_open_file_enabled: bool,
    pub terminal_logging_open_folder_enabled: bool,
    /// 日志弹窗：可见性与字段（文本以 Rust 为准，布尔由弹窗回写）。
    pub logging_dialog_visible: bool,
    pub logging_dialog_session_text: String,
    pub logging_dialog_directory_text: String,
    pub logging_dialog_file_name_text: String,
    pub logging_dialog_file_name_error_text: String,
    pub logging_dialog_path_error_text: String,
    pub logging_dialog_file_exists: bool,
    pub logging_dialog_raw_format: bool,
    pub logging_dialog_timestamps: bool,
    pub logging_dialog_include_input: bool,
    pub logging_dialog_input_confirmed: bool,
    pub logging_dialog_overwrite_confirmed: bool,
}

/// 单个面板框的 px 几何（Slint 侧 `PanelFrameData` 的同构体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelFrameData {
    /// "left" / "right" / "hidden"。
    pub placement: String,
    pub collapsed: bool,
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl PanelFrameData {
    fn hidden() -> Self {
        Self {
            placement: "hidden".to_owned(),
            collapsed: false,
            visible: false,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }

    fn from_view(view: &PanelFrameView) -> Self {
        Self {
            placement: view.placement.id().to_owned(),
            collapsed: view.collapsed,
            visible: view.visible,
            x: round_px(view.x),
            y: round_px(view.y),
            width: round_px(view.width),
            height: round_px(view.height),
        }
    }
}

/// N3：分栏手柄投影（Slint 侧 `SplitHandleData` 的同构体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitHandleData {
    /// "left" / "right"。
    pub side: String,
    pub boundary: i32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// N3：面板拖拽插入位置指示线。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelDragIndicator {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// N3：内容区布局投影（断点标志 + 面板框 + 分栏手柄 + 拖拽指示）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutProjection {
    pub left_width_px: i32,
    pub right_width_px: i32,
    pub left_effective_width_px: i32,
    pub right_effective_width_px: i32,
    pub nav_rail_active: bool,
    pub nav_rail_available: bool,
    pub dock_toggle_visible: bool,
    pub dock_auto_collapsed: bool,
    pub layout_drag_scale_permille: i32,
    pub sessions_frame: PanelFrameData,
    pub local_frame: PanelFrameData,
    pub sftp_frame: PanelFrameData,
    pub tunnels_frame: PanelFrameData,
    pub quick_commands_frame: PanelFrameData,
    pub transfers_frame: PanelFrameData,
    pub split_handles: Vec<SplitHandleData>,
    pub drag_indicator: Option<PanelDragIndicator>,
}

fn round_px(value: f32) -> i32 {
    if value.is_finite() {
        value.round() as i32
    } else {
        0
    }
}

fn layout_projection(view: &PanelLayoutView) -> LayoutProjection {
    LayoutProjection {
        left_width_px: round_px(view.left_width),
        right_width_px: round_px(view.right_width),
        left_effective_width_px: round_px(view.left_effective_width),
        right_effective_width_px: round_px(view.right_effective_width),
        nav_rail_active: view.rail_active,
        nav_rail_available: view.rail_available,
        dock_toggle_visible: view.dock_toggle_visible,
        dock_auto_collapsed: view.right_collapsed,
        layout_drag_scale_permille: round_px(view.drag_scale * 1000.0),
        sessions_frame: view
            .frame(PanelId::Sessions)
            .map(PanelFrameData::from_view)
            .unwrap_or_else(PanelFrameData::hidden),
        local_frame: view
            .frame(PanelId::Local)
            .map(PanelFrameData::from_view)
            .unwrap_or_else(PanelFrameData::hidden),
        sftp_frame: view
            .frame(PanelId::Sftp)
            .map(PanelFrameData::from_view)
            .unwrap_or_else(PanelFrameData::hidden),
        tunnels_frame: view
            .frame(PanelId::Tunnels)
            .map(PanelFrameData::from_view)
            .unwrap_or_else(PanelFrameData::hidden),
        quick_commands_frame: view
            .frame(PanelId::QuickCommands)
            .map(PanelFrameData::from_view)
            .unwrap_or_else(PanelFrameData::hidden),
        transfers_frame: view
            .frame(PanelId::Transfers)
            .map(PanelFrameData::from_view)
            .unwrap_or_else(PanelFrameData::hidden),
        split_handles: view
            .split_handles
            .iter()
            .map(|handle| SplitHandleData {
                side: match handle.side {
                    PanelSide::Left => "left".to_owned(),
                    PanelSide::Right => "right".to_owned(),
                },
                boundary: i32::try_from(handle.boundary).unwrap_or(i32::MAX),
                x: round_px(handle.x),
                y: round_px(handle.y),
                width: round_px(handle.width),
                height: round_px(handle.height),
            })
            .collect(),
        drag_indicator: view.drag_indicator.map(|indicator| PanelDragIndicator {
            x: round_px(indicator.x),
            y: round_px(indicator.y),
            width: round_px(indicator.width),
            height: round_px(indicator.height),
        }),
    }
}

/// N1 Phase 2 sub-projections: Properties fields and the file clipboard.
impl AppRuntime {
    /// `(name, kind, size)` for the Properties dialog's first selected entry.
    fn sftp_properties_parts(&self) -> (String, String, String) {
        let Some(entry) = self.selected_sftp_entry() else {
            return (String::new(), String::new(), String::new());
        };
        (
            entry.name.clone(),
            sftp_kind_text(entry.kind).to_owned(),
            if matches!(entry.kind, FsEntryKind::Directory) {
                String::new()
            } else {
                format_sftp_size(entry.size_bytes)
            },
        )
    }

    /// `(side id, count, cut)` for the in-app file clipboard.
    fn clipboard_parts(&self) -> (String, i32, bool) {
        match &self.file_clipboard {
            Some(clipboard) => (
                match clipboard.side {
                    ClipboardSide::Local => "local".to_owned(),
                    ClipboardSide::Remote => "remote".to_owned(),
                },
                i32::try_from(clipboard.paths.len()).unwrap_or(i32::MAX),
                clipboard.cut,
            ),
            None => (String::new(), 0, false),
        }
    }
}
