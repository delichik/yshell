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
        let tabs = self.tab_data();
        let tab_has_disconnected = self.has_disconnected_tabs();
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
        let (terminal_scrollback_lines, terminal_viewport_rows) =
            self.active_terminal_scroll_geometry();
        // N6：终端日志（菜单/状态栏入口、REC 指示、弹窗字段）。
        let (
            terminal_logging_start_enabled,
            terminal_logging_stop_enabled,
            terminal_logging_open_file_enabled,
            terminal_logging_open_folder_enabled,
        ) = self.terminal_logging_menu_flags();
        let logging_session_key = self.active_session_id.clone();
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
        let (quick_connect_rows, quick_links_rows, quick_connect_summary_text) = (
            self.quick_connect_row_data(),
            self.quick_link_row_data(),
            self.quick_connect_summary_text(),
        );
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
        let host_keys = self.host_keys_projection_parts();
        let auth_prompt = self.auth_prompt_projection_parts();
        // N5：终端外观（Settings 外观弹窗 / Folder Editor / Session Editor）。
        let settings_appearance_preview = self.settings_appearance_preview_frame();
        let settings_appearance_status_text = self.settings_appearance_status.display_text();
        let settings_appearance_schemes = theme::scheme_options();
        let settings_appearance_palette = palette_parts(self.settings_appearance_palette());
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
            settings_appearance_visible: self.settings_appearance_visible,
            settings_appearance_color_scheme_text: self.settings_appearance.color_scheme.clone(),
            settings_appearance_foreground_text: self.settings_appearance.foreground.clone(),
            settings_appearance_background_text: self.settings_appearance.background.clone(),
            settings_appearance_cursor_text: self.settings_appearance.cursor.clone(),
            settings_appearance_selection_text: self.settings_appearance.selection.clone(),
            settings_appearance_font_family_text: self.settings_appearance.font_family.clone(),
            settings_appearance_font_size_text: self.settings_appearance.font_size_text.clone(),
            settings_appearance_fallback_fonts_text: self
                .settings_appearance
                .fallback_fonts_text
                .clone(),
            settings_appearance_status_text,
            settings_appearance_schemes,
            settings_appearance_palette_swatches: settings_appearance_palette,
            settings_appearance_preview,
            folder_editor_visible: self.folder_editor.visible,
            folder_editor_name_text: self.folder_editor.folder_name.clone(),
            folder_editor_path_text: self.folder_editor.folder_path.clone(),
            folder_editor_status_text: self.folder_editor.status_text.clone(),
            folder_editor_appearance_fields,
            folder_editor_terminal_fields,
            folder_editor_logging_fields,
            saved_folder_selected: self.selected_saved_folder_id.is_some(),
            editor_appearance_fields,
            editor_terminal_fields,
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
            // D36：弹窗正文拆分字段（WS-C 的 HostKeyDialog 逐行渲染；空值行不占位）。
            host_key_prompt_target_text: host_key_prompt.target,
            host_key_prompt_known_hosts_path_text: host_key_prompt.known_hosts_path,
            host_key_prompt_algorithm_text: host_key_prompt.algorithm,
            host_key_prompt_fingerprint_text: host_key_prompt.fingerprint,
            host_key_prompt_expected_algorithm_text: host_key_prompt.expected_algorithm,
            host_key_prompt_expected_fingerprint_text: host_key_prompt.expected_fingerprint,
            host_key_prompt_mode_text: self.host_key_prompt_mode_text().to_owned(),
            host_key_prompt_confirmation_text: self.host_key_replace_confirmation.clone(),
            password_prompt_visible: self.pending_password_prompt.is_some(),
            password_prompt_host_text: self.password_prompt_host_text(),
            tab_name_text,
            tab_state_text,
            tab_has_session,
            tabs,
            active_tab_id: self.active_tab_id.clone().unwrap_or_default(),
            tab_count: i32::try_from(self.tabs.len()).unwrap_or(i32::MAX),
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
            // --- N1 Phase 2 -------------------------------------------------
            local_path_text: self.local_pane.dir.display().to_string(),
            local_rows,
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
            status_text: self.status_text.clone(),
            status_kind,
            status_param_1,
            status_param_2,
            transport_backend_text: self.transport_backend.label().to_owned(),
            sftp_visible: self.panel_visible(PanelId::Sftp),
            tunnels_visible: self.panel_visible(PanelId::Tunnels),
            commands_visible: self.panel_visible(PanelId::QuickCommands),
            sessions_visible: self.panel_visible(PanelId::Sessions),
            transfers_visible: self.panel_visible(PanelId::Transfers),
            layout: layout_projection(&panel_view),
            app_version_text: env!("CARGO_PKG_VERSION").to_owned(),
            quick_connect_visible: self.quick_connect_visible(),
            quick_connect_input_text: self.quick_connect_input.clone(),
            quick_connect_error_text: self.quick_connect_error_text.clone(),
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
            host_keys_modal_visible: host_keys.visible,
            host_keys_groups: host_keys.groups,
            host_keys_selected_group: host_keys.selected_group,
            host_keys_selected_entry: host_keys.selected_entry,
            host_keys_details_text: host_keys.details_text,
            host_keys_detail_host_text: host_keys.detail_host_text,
            host_keys_detail_algorithm_text: host_keys.detail_algorithm_text,
            host_keys_detail_fingerprint_text: host_keys.detail_fingerprint_text,
            host_keys_detail_path_text: host_keys.detail_path_text,
            host_keys_path_text: host_keys.path_text,
            host_keys_status_text: host_keys.status_text,
            host_keys_import_visible: host_keys.import_visible,
            host_keys_import_text: host_keys.import_text,
            host_keys_clear_confirmation_text: host_keys.clear_confirmation_text,
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
            .active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
        {
            Some(session) => (true, session.display_name.clone()),
            None => (false, String::new()),
        }
    }

    pub(crate) fn terminal_body_parts(&self) -> (&'static str, String) {
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

    pub(crate) fn terminal_visible_lines(&self) -> Vec<String> {
        self.active_session_id
            .as_ref()
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
                    "Runtime is ready. Open a Quick Connect target or create a draft session."
                        .to_owned(),
                ]
            })
    }

    pub(crate) fn terminal_cursor_column(&self) -> i32 {
        self.active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
            .map(|session| i32::from(session.cursor_position().0))
            .unwrap_or(0)
    }

    pub(crate) fn terminal_cursor_row(&self) -> i32 {
        self.active_session_id
            .as_ref()
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
                "Host key changed for {}@{}:{}.\nKnown hosts: {}\nExpected: {} {}\nPresented: {} {}\nType REPLACE to enable replacement.",
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

/// N4：主机密钥页投影。
struct HostKeysProjectionParts {
    visible: bool,
    groups: Vec<HostKeyGroupData>,
    selected_group: i32,
    selected_entry: i32,
    details_text: String,
    /// D20：详情拆分字段（标签由 UI 侧 `@tr` 渲染；指纹为 D9 分组格式）。
    detail_host_text: String,
    detail_algorithm_text: String,
    detail_fingerprint_text: String,
    detail_path_text: String,
    path_text: String,
    status_text: String,
    import_visible: bool,
    import_text: String,
    clear_confirmation_text: String,
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

    fn active_deploy_target(&self) -> Option<String> {
        let session_key = self.active_session_id.as_ref()?;
        let session = self.sessions.get(session_key)?;
        Some(format!(
            "{}@{}:{}",
            session.username_label(),
            session.host_label(),
            session.ssh_config.port
        ))
    }

    fn host_keys_projection_parts(&self) -> HostKeysProjectionParts {
        let groups = self.host_key_group_data();
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
            details_text: self.host_keys_details_text(),
            detail_host_text: self.host_keys_detail_host_text(),
            detail_algorithm_text: self.host_keys_detail_algorithm_text(),
            detail_fingerprint_text: self.host_keys_detail_fingerprint_text(),
            detail_path_text: self.host_keys_detail_path_text(),
            path_text: self.config_store.known_hosts_file().display().to_string(),
            status_text: self.host_keys_status_text.clone(),
            import_visible: self.host_keys_import_visible,
            import_text: self.host_keys_import_text.clone(),
            clear_confirmation_text: self.known_hosts_clear_confirmation.clone(),
        }
    }

    fn auth_prompt_projection_parts(&self) -> AuthPromptProjectionParts {
        let prompt = self.pending_auth_prompt.as_ref();
        let key_entries = self.private_key_entries();
        let key_options = key_entries
            .iter()
            .map(|entry| AuthKeyOptionData {
                id: entry.id.clone(),
                label: entry.label.clone(),
                detail: format!("{} · {}", entry.algorithm, entry.fingerprint),
            })
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
        // e2e/dev：fake 场景固定两轮（第 1 轮显示 Continue，第 2 轮显示 Connect）。
        let challenge_round_total = if self.fake_keyboard_scenario() {
            rounds_seen.max(2)
        } else {
            rounds_seen
        };
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
    // --- N5：Settings 外观弹窗 ---------------------------------------------------
    pub settings_appearance_visible: bool,
    pub settings_appearance_color_scheme_text: String,
    pub settings_appearance_foreground_text: String,
    pub settings_appearance_background_text: String,
    pub settings_appearance_cursor_text: String,
    pub settings_appearance_selection_text: String,
    pub settings_appearance_font_family_text: String,
    pub settings_appearance_font_size_text: String,
    pub settings_appearance_fallback_fonts_text: String,
    pub settings_appearance_status_text: String,
    pub settings_appearance_schemes: Vec<theme::ThemeSchemeOption>,
    pub settings_appearance_palette_swatches: Vec<[u8; 3]>,
    pub settings_appearance_preview: Option<yshell_terminal::TerminalFrame>,
    // --- N5：Folder Editor ------------------------------------------------------
    pub folder_editor_visible: bool,
    pub folder_editor_name_text: String,
    pub folder_editor_path_text: String,
    pub folder_editor_status_text: String,
    pub folder_editor_appearance_fields: Vec<TriStateFieldData>,
    pub folder_editor_terminal_fields: Vec<TriStateFieldData>,
    pub folder_editor_logging_fields: Vec<TriStateFieldData>,
    /// Selected saved-session tree node is a folder (menu entry enabling).
    pub saved_folder_selected: bool,
    // --- N5：Session Editor 外观/终端覆盖 ----------------------------------------
    pub editor_appearance_fields: Vec<TriStateFieldData>,
    pub editor_terminal_fields: Vec<TriStateFieldData>,
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
    pub host_key_prompt_confirmation_text: String,
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
    // --- N1 Phase 2：本地栏 / 队列抽屉 / 冲突与属性 / 剪贴板 -------------------
    /// Local pane path (lexically normalized absolute path).
    pub local_path_text: String,
    /// Local pane rows, including the leading `..` marker.
    pub local_rows: Vec<crate::local_fs::LocalRowData>,
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
    /// N3：Sessions / Transfers 面板可见性（View/Panels 菜单勾选项）。
    pub sessions_visible: bool,
    pub transfers_visible: bool,
    /// N3：内容区布局投影（面板框 px 几何 + 断点标志）。
    pub layout: LayoutProjection,
    pub app_version_text: String,
    // --- N2：Quick Connect 页（`ui/pages/quick_connect.slint` 的投影）------------
    /// 活动标签是否是 Quick Connect 页（宿主据此切换内容区）。
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
    // --- N4：主机密钥页 ------------------------------------------------------
    pub host_keys_modal_visible: bool,
    pub host_keys_groups: Vec<HostKeyGroupData>,
    pub host_keys_selected_group: i32,
    pub host_keys_selected_entry: i32,
    pub host_keys_details_text: String,
    /// D20：主机密钥页详情拆分字段（标签由 UI 侧 `@tr` 渲染；指纹为 D9 分组格式）。
    pub host_keys_detail_host_text: String,
    pub host_keys_detail_algorithm_text: String,
    pub host_keys_detail_fingerprint_text: String,
    pub host_keys_detail_path_text: String,
    pub host_keys_path_text: String,
    pub host_keys_status_text: String,
    pub host_keys_import_visible: bool,
    pub host_keys_import_text: String,
    pub host_keys_clear_confirmation_text: String,
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

/// N3：内容区布局投影（断点标志 + 五个面板框 + 分栏手柄 + 拖拽指示）。
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
