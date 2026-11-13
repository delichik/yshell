//! Host-side AppRuntime method dispatch for Shell `UiCall` IPC.
//!
//! Generated from bootstrap callback surface; Host owns AppRuntime.

use serde_json::{json, Value};

use crate::runtime::AppRuntime;

use super::shell_ipc::{projection_json, WireTerminalAppearance};

/// Dispatch a named UI method against the Host-owned runtime.
pub fn dispatch(runtime: &mut AppRuntime, method: &str, args: Value) -> Result<Value, String> {
    match method {
        "projection" => Ok(projection_json(&runtime.projection())),
        "projection_for_window" => {
            let window_id = arg_u64(&args, 0)?;
            Ok(projection_json(&runtime.projection_for_window(window_id)))
        }
        "prepare_desktop_startup_projection" => {
            let _ = runtime.prepare_desktop_startup_projection();
            Ok(projection_json(&runtime.projection()))
        }
        "prepare_desktop_startup_workspace" => {
            let _ = runtime.prepare_desktop_startup_workspace();
            Ok(projection_json(&runtime.projection()))
        }
        "request_master_password_unlock" => {
            let _ = runtime.request_master_password_unlock();
            Ok(projection_json(&runtime.projection()))
        }
        "update_master_password_unlock_password" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(
                &runtime.update_master_password_unlock_password(&a0),
            ))
        }
        "submit_master_password_unlock" => {
            Ok(projection_json(&runtime.submit_master_password_unlock()))
        }
        "forgot_master_password" => Ok(projection_json(&runtime.forgot_master_password())),
        "dismiss_master_password_unlock" => {
            Ok(projection_json(&runtime.dismiss_master_password_unlock()))
        }
        "cancel_auth_prompt" => Ok(projection_json(&runtime.cancel_auth_prompt())),
        "submit_auth_prompt" => Ok(projection_json(&runtime.submit_auth_prompt())),
        "register_window" => {
            let window_id = arg_u64(&args, 0)?;
            runtime.register_window(window_id);
            Ok(Value::Null)
        }
        "allocate_window" => {
            // Host-authoritative window id allocation for second-launch windows:
            // the new window starts with a fresh New Tab page (otherwise the
            // Shell would immediately close the empty secondary window).
            let window_id = runtime.allocate_window_id();
            runtime.register_window(window_id);
            runtime.focus_window(window_id);
            let _ = runtime.open_new_tab();
            Ok(json!(window_id))
        }
        "focus_window" => {
            let window_id = arg_u64(&args, 0)?;
            runtime.focus_window(window_id);
            Ok(Value::Null)
        }
        "close_window_tabs" => {
            let window_id = arg_u64(&args, 0)?;
            runtime.close_window_tabs(window_id);
            Ok(Value::Null)
        }
        "note_ui_scale_factor" => {
            let scale = arg_f32(&args, 0)?;
            runtime.note_ui_scale_factor(scale);
            Ok(Value::Null)
        }
        "take_ui_dirty" => Ok(Value::Bool(runtime.take_ui_dirty())),
        "mark_ui_dirty" => {
            runtime.mark_ui_dirty();
            Ok(Value::Null)
        }
        "mcp_enabled" => Ok(Value::Bool(runtime.mcp_enabled())),
        "ui_language" => Ok(Value::String(runtime.ui_language().to_owned())),
        "main_window_id" => Ok(match runtime.main_window_id() {
            Some(id) => json!(id),
            None => Value::Null,
        }),
        "window_of_tab" => {
            let tab_id = arg_str(&args, 0)?;
            Ok(match runtime.window_of_tab(&tab_id) {
                Some(id) => json!(id),
                None => Value::Null,
            })
        }
        "move_tab_to_window" => {
            let tab_id = arg_str(&args, 0)?;
            let window_id = arg_u64(&args, 1)?;
            runtime
                .move_tab_to_window(&tab_id, window_id)
                .map(|_| Value::Null)
                .map_err(|e| e.to_string())
        }
        // D38：Host 是窗口 id 的唯一分配者。Shell 请求 "移动到新窗口" 时由 Host
        // 分配稳定 id、登记窗口并把标签迁过去；Shell 再为返回的 id 创建 Slint 窗口。
        "move_tab_to_new_window" => {
            let tab_id = arg_str(&args, 0)?;
            let window_id = runtime.allocate_window_id();
            runtime.register_window(window_id);
            match runtime.move_tab_to_window(&tab_id, window_id) {
                Ok(()) => Ok(json!({ "window_id": window_id })),
                Err(error) => {
                    // 回滚：不留一个没有任何标签的幽灵窗口。
                    runtime.close_window_tabs(window_id);
                    Err(error.to_string())
                }
            }
        }
        "window_tab_ids" => {
            let window_id = arg_u64(&args, 0)?;
            Ok(json!(runtime.window_tab_ids(window_id)))
        }
        "stop_all_session_logging" => {
            runtime.stop_all_session_logging();
            Ok(Value::Null)
        }
        "poll_all_terminal_outputs" => match runtime.poll_all_terminal_outputs() {
            Ok(Some(p)) => Ok(json!({ "dirty": true, "projection": projection_json(&p) })),
            Ok(None) => Ok(json!({ "dirty": false })),
            Err(e) => Err(e.to_string()),
        },
        "auto_copy_terminal_selection" => Ok(match runtime.auto_copy_terminal_selection() {
            Some(text) => Value::String(text),
            None => Value::Null,
        }),
        "confirm_close_tabs_with" => {
            let dont_ask = arg_bool(&args, 0)?;
            runtime
                .confirm_close_tabs_with(dont_ask)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "reconnect_tab_session" => {
            let tab_id = arg_str(&args, 0)?;
            runtime
                .reconnect_tab_session(&tab_id)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "disconnect_tab_session" => {
            let tab_id = arg_str(&args, 0)?;
            runtime
                .disconnect_tab_session(&tab_id)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "activate_sftp_entry" => runtime
            .activate_sftp_entry()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "activate_sftp_row" => {
            let a0 = arg_i32(&args, 0)?;
            runtime
                .activate_sftp_row(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "activate_tab" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .activate_tab(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "add_editor_tunnel_forward" => runtime
            .add_editor_tunnel_forward()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "cancel_close_tabs" => Ok(projection_json(&runtime.cancel_close_tabs())),
        "cancel_connection_overlay" => runtime
            .cancel_connection_overlay()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "cancel_discard_session_editor" => {
            Ok(projection_json(&runtime.cancel_discard_session_editor()))
        }
        "cancel_host_key_prompt" => Ok(projection_json(&runtime.cancel_host_key_prompt())),
        "cancel_password_prompt" => Ok(projection_json(&runtime.cancel_password_prompt())),
        "cancel_sftp_remote_edit" => runtime
            .cancel_sftp_remote_edit()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "clear_active_terminal" => runtime
            .clear_active_terminal()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "clear_active_terminal_scrollback" => runtime
            .clear_active_terminal_scrollback()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "clear_editor_tunnels" => Ok(projection_json(&runtime.clear_editor_tunnels())),
        "clear_saved_selection" => Ok(projection_json(&runtime.clear_saved_selection())),
        "close_logging_dialog" => Ok(projection_json(&runtime.close_logging_dialog())),
        "close_split_pane" => {
            let pane = arg_list(&args)?
                .first()
                .and_then(|v| v.as_i64())
                .and_then(|i| i32::try_from(i).ok());
            runtime
                .close_split_pane(pane)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "confirm_discard_session_editor" => {
            Ok(projection_json(&runtime.confirm_discard_session_editor()))
        }
        "copy_active_terminal_current_line" => runtime
            .copy_active_terminal_current_line()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "copy_active_terminal_visible_text" => runtime
            .copy_active_terminal_visible_text()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "create_folder_under_editor_target" => runtime
            .create_folder_under_editor_target()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "delete_selected_saved_session" => runtime
            .delete_selected_saved_session()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "delete_sftp_selected" => runtime
            .delete_sftp_selected()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "disconnect_active_session" => runtime
            .disconnect_active_session()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "dismiss_quick_connect" => Ok(projection_json(&runtime.dismiss_quick_connect())),
        "duplicate_selected_saved_session" => runtime
            .duplicate_selected_saved_session()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "duplicate_tab" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .duplicate_tab(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "edit_sftp_selected" => runtime
            .edit_sftp_selected()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "exit_mcp_control" => Ok(projection_json(&runtime.exit_mcp_control())),
        "finish_active_terminal_selection" => runtime
            .finish_active_terminal_selection()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "focus_split_pane" => {
            let a0 = arg_i32(&args, 0)?;
            runtime
                .focus_split_pane(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "handle_new_session" => Ok(projection_json(&runtime.handle_new_session())),
        "handle_new_tab_default" => Ok(projection_json(&runtime.handle_new_tab_default())),
        "handle_quick_connect" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .handle_quick_connect(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "open_first_saved_session" => runtime
            .open_first_saved_session()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "open_proxies_manager" => Ok(projection_json(&runtime.open_proxies_manager())),
        "open_quick_connect" => Ok(projection_json(&runtime.open_quick_connect())),
        "open_selected_saved_session" => runtime
            .open_selected_saved_session()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "open_selected_saved_session_sftp" => runtime
            .open_selected_saved_session_sftp()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "open_sftp_crumb" => {
            let a0 = arg_i32(&args, 0)?;
            runtime
                .open_sftp_crumb(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "open_sftp_parent" => runtime
            .open_sftp_parent()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "open_sftp_path" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .open_sftp_path(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "place_tab_split" => {
            let tab_id = arg_str(&args, 0)?;
            let pane = arg_u64(&args, 1)?;
            let direction = arg_str(&args, 2)?;
            runtime
                .place_tab_split(&tab_id, pane, &direction)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "quick_connect_history_clear" => runtime
            .quick_connect_history_clear()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "reconnect_active_session" => runtime
            .reconnect_active_session()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "refresh_active_sftp_listing" => runtime
            .refresh_active_sftp_listing()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "refresh_saved_sessions" => runtime
            .refresh_saved_sessions()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "request_close_tab" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .request_close_tab(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "request_close_tabs" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .request_close_tabs(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "reset_settings_terminal_defaults" => {
            Ok(projection_json(&runtime.reset_settings_terminal_defaults()))
        }
        "retry_connection_overlay" => runtime
            .retry_connection_overlay()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "save_active_session" => runtime
            .save_active_session()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "save_editor_and_connect" => runtime
            .save_editor_and_connect()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "save_editor_to_saved_session" => runtime
            .save_editor_to_saved_session()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "save_global_logging_settings" => runtime
            .save_global_logging_settings()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "save_settings_sftp" => runtime
            .save_settings_sftp()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "save_settings_terminal" => runtime
            .save_settings_terminal()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "save_sftp_remote_edit" => runtime
            .save_sftp_remote_edit()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "scroll_active_terminal" => {
            let a0 = arg_i32(&args, 0)?;
            runtime
                .scroll_active_terminal(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "scroll_active_terminal_to_bottom" => runtime
            .scroll_active_terminal_to_bottom()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "select_all_active_terminal" => runtime
            .select_all_active_terminal()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "select_editor_key" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(&runtime.select_editor_key(a0)))
        }
        "select_editor_proxy" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(&runtime.select_editor_proxy(a0)))
        }
        "select_next_editor_folder" => Ok(projection_json(&runtime.select_next_editor_folder())),
        "select_next_saved_session" => Ok(projection_json(&runtime.select_next_saved_session())),
        "select_next_terminal_match" => runtime
            .select_next_terminal_match()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "select_previous_editor_folder" => {
            Ok(projection_json(&runtime.select_previous_editor_folder()))
        }
        "select_previous_saved_session" => {
            Ok(projection_json(&runtime.select_previous_saved_session()))
        }
        "select_previous_terminal_match" => runtime
            .select_previous_terminal_match()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "select_screen_active_terminal" => runtime
            .select_screen_active_terminal()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "select_session_editor_advanced" => {
            Ok(projection_json(&runtime.select_session_editor_advanced()))
        }
        "select_session_editor_appearance" => {
            Ok(projection_json(&runtime.select_session_editor_appearance()))
        }
        "select_session_editor_general" => {
            Ok(projection_json(&runtime.select_session_editor_general()))
        }
        "select_session_editor_logging" => {
            Ok(projection_json(&runtime.select_session_editor_logging()))
        }
        "select_session_editor_proxy" => {
            Ok(projection_json(&runtime.select_session_editor_proxy()))
        }
        "select_session_editor_sftp" => Ok(projection_json(&runtime.select_session_editor_sftp())),
        "select_session_editor_terminal" => {
            Ok(projection_json(&runtime.select_session_editor_terminal()))
        }
        "select_session_editor_tunnels" => {
            Ok(projection_json(&runtime.select_session_editor_tunnels()))
        }
        "select_sftp_entry" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(&runtime.select_sftp_entry(a0)))
        }
        "send_active_terminal_input" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .send_active_terminal_input(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_confirm_close_session" => {
            let a0 = arg_bool(&args, 0)?;
            runtime
                .set_confirm_close_session(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_confirm_quit_app" => {
            let a0 = arg_bool(&args, 0)?;
            runtime
                .set_confirm_quit_app(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_debug_logging" => {
            let a0 = arg_bool(&args, 0)?;
            runtime
                .set_debug_logging(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_editor_auth_method_agent" => {
            Ok(projection_json(&runtime.set_editor_auth_method_agent()))
        }
        "set_editor_auth_method_password" => {
            Ok(projection_json(&runtime.set_editor_auth_method_password()))
        }
        "set_editor_proxy_mode_none" => Ok(projection_json(&runtime.set_editor_proxy_mode_none())),
        "set_editor_tunnel_kind_dynamic" => {
            Ok(projection_json(&runtime.set_editor_tunnel_kind_dynamic()))
        }
        "set_editor_tunnel_kind_local" => {
            Ok(projection_json(&runtime.set_editor_tunnel_kind_local()))
        }
        "set_editor_tunnel_kind_remote" => {
            Ok(projection_json(&runtime.set_editor_tunnel_kind_remote()))
        }
        "set_global_logging_format_raw" => {
            Ok(projection_json(&runtime.set_global_logging_format_raw()))
        }
        "set_logging_dialog_options" => Ok(projection_json(&runtime.set_logging_dialog_options(
            arg_bool(&args, 0)?,
            arg_bool(&args, 1)?,
            arg_bool(&args, 2)?,
            arg_bool(&args, 3)?,
            arg_bool(&args, 4)?,
        ))),
        "set_logging_dialog_path_error" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.set_logging_dialog_path_error(&a0)))
        }
        "set_mcp_enabled" => {
            let a0 = arg_bool(&args, 0)?;
            runtime
                .set_mcp_enabled(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "update_mcp_port" => {
            let a0 = arg_str(&args, 0)?;
            let port = a0
                .parse::<u16>()
                .map_err(|_| format!("invalid MCP port `{a0}`"))?;
            runtime
                .set_mcp_port(port)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_mcp_listen_lan" => {
            let lan = arg_bool(&args, 0)?;
            let mode = if lan {
                yshell_config::McpListenMode::Lan
            } else {
                yshell_config::McpListenMode::Local
            };
            runtime
                .set_mcp_listen_mode(mode)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "update_mcp_api_key" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .set_mcp_api_key(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "generate_mcp_api_key" => runtime
            .generate_mcp_api_key()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "set_mcp_view_user_connections" => {
            let a0 = arg_bool(&args, 0)?;
            runtime
                .set_mcp_view_user_connections(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_mcp_control_user_connections" => {
            let a0 = arg_bool(&args, 0)?;
            runtime
                .set_mcp_control_user_connections(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_mcp_auto_approve_control" => {
            let a0 = arg_bool(&args, 0)?;
            runtime
                .set_mcp_auto_approve_control(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_private_keys_status_feedback" => {
            let kind = arg_str(&args, 0)?;
            let text = arg_str(&args, 1)?;
            let p1 = arg_str(&args, 2)?;
            let p2 = arg_str(&args, 3)?;
            Ok(projection_json(
                &runtime.set_private_keys_status_feedback(&kind, text, p1, p2),
            ))
        }
        "set_split_ratio" => {
            let split_id = arg_i32(&args, 0)?;
            let ratio = arg_f32(&args, 1)?;
            runtime
                .set_split_ratio(split_id, ratio)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_terminal_search_across" => {
            let a0 = arg_bool(&args, 0)?;
            runtime
                .set_terminal_search_across(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_terminal_search_regex" => {
            let a0 = arg_bool(&args, 0)?;
            runtime
                .set_terminal_search_regex(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_ui_language" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .set_ui_language(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "sort_sftp_by" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.sort_sftp_by(&a0)))
        }
        "split_workspace" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .split_workspace(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "start_logging_from_dialog" => runtime
            .start_logging_from_dialog()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "start_new_saved_session_editor" => {
            Ok(projection_json(&runtime.start_new_saved_session_editor()))
        }
        "stop_input_sync_command" => Ok(projection_json(&runtime.stop_input_sync_command())),
        "stop_logging_from_dialog" => runtime
            .stop_logging_from_dialog()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "submit_password" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .submit_password(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "terminal_logging_start" => Ok(projection_json(&runtime.terminal_logging_start())),
        "terminal_logging_stop" => runtime
            .terminal_logging_stop()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "test_editor_auth" => Ok(projection_json(&runtime.test_editor_auth())),
        "toggle_commands" => Ok(projection_json(&runtime.toggle_commands())),
        "toggle_global_logging_enabled" => {
            Ok(projection_json(&runtime.toggle_global_logging_enabled()))
        }
        "toggle_quick_connect" => Ok(projection_json(&runtime.toggle_quick_connect())),
        "toggle_saved_folder" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .toggle_saved_folder(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "toggle_sftp" => Ok(projection_json(&runtime.toggle_sftp())),
        "toggle_sftp_hidden_files" => Ok(projection_json(&runtime.toggle_sftp_hidden_files())),
        "toggle_tunnels" => Ok(projection_json(&runtime.toggle_tunnels())),
        "trust_host_key_and_save" => runtime
            .trust_host_key_and_save()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "trust_host_key_once" => runtime
            .trust_host_key_once()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "update_editor_host" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.update_editor_host(&a0)))
        }
        "update_editor_name" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.update_editor_name(&a0)))
        }
        "update_editor_port" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.update_editor_port(&a0)))
        }
        "update_quick_connect_input" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.update_quick_connect_input(&a0)))
        }
        "open_settings_appearance" => Ok(projection_json(&runtime.open_settings_appearance())),
        "close_settings_appearance" => Ok(projection_json(&runtime.close_settings_appearance())),
        "open_private_keys_manager" => Ok(projection_json(&runtime.open_private_keys_manager())),
        "close_private_keys_manager" => Ok(projection_json(&runtime.close_private_keys_manager())),
        "close_proxies_manager" => Ok(projection_json(&runtime.close_proxies_manager())),
        "open_theme_manager" => Ok(projection_json(&runtime.open_theme_manager())),
        "close_theme_manager" => Ok(projection_json(&runtime.close_theme_manager())),
        "open_host_keys_manager" => Ok(projection_json(&runtime.open_host_keys_manager())),
        "close_host_keys_manager" => Ok(projection_json(&runtime.close_host_keys_manager())),
        "open_logging_dialog" => Ok(projection_json(&runtime.open_logging_dialog())),
        "open_settings_terminal_styles_manager" => Ok(projection_json(
            &runtime.open_settings_terminal_styles_manager(),
        )),
        "save_settings_appearance" => runtime
            .save_settings_appearance()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "save_settings_all" => runtime
            .save_settings_all()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "update_settings_appearance_mode" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_appearance_mode(a0),
            ))
        }
        "update_settings_appearance_accent" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_appearance_accent(a0),
            ))
        }
        "select_settings_appearance_style" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(
                &runtime.select_settings_appearance_style(a0),
            ))
        }
        "update_settings_terminal_middle_click" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_terminal_middle_click(a0),
            ))
        }
        "update_settings_terminal_right_click" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_terminal_right_click(a0),
            ))
        }
        "update_settings_terminal_multi_line_paste" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_terminal_multi_line_paste(a0),
            ))
        }
        "update_settings_terminal_auto_copy" => {
            let a0 = arg_bool(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_terminal_auto_copy(a0),
            ))
        }
        "update_settings_scrollback_lines" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_scrollback_lines(&a0),
            ))
        }
        "update_settings_scrollback_max_cells" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_scrollback_max_cells(&a0),
            ))
        }
        "update_settings_sftp_show_hidden" => {
            let a0 = arg_bool(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_sftp_show_hidden(a0),
            ))
        }
        "update_settings_sftp_conflict_action" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_sftp_conflict_action(a0),
            ))
        }
        "update_settings_sftp_external_editor" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_sftp_external_editor(a0),
            ))
        }
        "update_settings_sftp_external_editor_path" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_sftp_external_editor_path(&a0),
            ))
        }
        "set_reduce_motion" => {
            let a0 = arg_bool(&args, 0)?;
            runtime
                .set_reduce_motion(a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "open_master_password_dialog" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.open_master_password_dialog(&a0)))
        }
        "close_master_password_dialog" => {
            Ok(projection_json(&runtime.close_master_password_dialog()))
        }
        "update_master_password_field" => {
            let a0 = arg_str(&args, 0)?;
            let a1 = arg_str(&args, 1)?;
            Ok(projection_json(
                &runtime.update_master_password_field(&a0, &a1),
            ))
        }
        "submit_master_password_dialog" => runtime
            .submit_master_password_dialog()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "select_private_key" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(&runtime.select_private_key(a0)))
        }
        "select_proxy" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(&runtime.select_proxy(a0)))
        }
        "begin_edit_proxy" => Ok(projection_json(&runtime.begin_edit_proxy())),
        "start_new_proxy" => Ok(projection_json(&runtime.start_new_proxy())),
        "save_proxy_editor" => Ok(projection_json(&runtime.save_proxy_editor())),
        "cancel_proxy_editor" => Ok(projection_json(&runtime.cancel_proxy_editor())),
        "select_theme_manager_row" => {
            let a0 = arg_i32(&args, 0)?;
            Ok(projection_json(&runtime.select_theme_manager_row(a0)))
        }
        "apply_theme_manager_selection" => runtime
            .apply_theme_manager_selection()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "close_session_editor_modal" => Ok(projection_json(&runtime.close_session_editor_modal())),
        "request_close_session_editor_modal" => Ok(projection_json(
            &runtime.request_close_session_editor_modal(),
        )),
        "close_folder_editor" => Ok(projection_json(&runtime.close_folder_editor())),
        "save_folder_editor" => runtime
            .save_folder_editor()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "cancel_editor_login_step" => Ok(projection_json(&runtime.cancel_editor_login_step())),
        "save_editor_login_step" => runtime
            .save_editor_login_step()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "cancel_editor_tunnel_dialog" => {
            Ok(projection_json(&runtime.cancel_editor_tunnel_dialog()))
        }
        "save_editor_tunnel_dialog" => runtime
            .save_editor_tunnel_dialog()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "resolve_sftp_conflict" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .resolve_sftp_conflict(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "cancel_sftp_conflict" => Ok(projection_json(&runtime.cancel_sftp_conflict())),
        "close_sftp_properties" => Ok(projection_json(&runtime.close_sftp_properties())),
        "apply_sftp_chmod_dialog" => Ok(projection_json(&runtime.apply_sftp_chmod_dialog())),
        "cancel_sftp_chmod_dialog" => Ok(projection_json(&runtime.cancel_sftp_chmod_dialog())),
        "open_sftp_properties" => Ok(projection_json(&runtime.open_sftp_properties())),
        "open_sftp_chmod_dialog" => Ok(projection_json(&runtime.open_sftp_chmod_dialog())),
        "set_mcp_port" => {
            let raw = arg_list(&args)?
                .first()
                .ok_or_else(|| "args[0] required".to_owned())?;
            let port = if let Some(text) = raw.as_str() {
                text.parse::<u16>()
                    .map_err(|_| format!("invalid MCP port `{text}`"))?
            } else {
                let n = raw
                    .as_u64()
                    .ok_or_else(|| "args[0] port required".to_owned())?;
                u16::try_from(n).map_err(|_| "args[0] u16 required".to_owned())?
            };
            runtime
                .set_mcp_port(port)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_mcp_api_key" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .set_mcp_api_key(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "open_folder_editor" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .open_folder_editor(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "open_private_keys_export_dialog" => {
            Ok(projection_json(&runtime.open_private_keys_export_dialog()))
        }
        "save_theme_editor" => runtime
            .save_theme_editor()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "activate_saved_session_tree_node" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .activate_saved_session_tree_node(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "edit_folder_defaults" => {
            let folder_id = runtime.selected_saved_folder_id.clone().unwrap_or_default();
            runtime
                .open_folder_editor(&folder_id)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "expand_panel_side" => {
            let a0 = arg_str(&args, 0)?;
            let Some(side) = panel_side_from_str(&a0) else {
                return Ok(projection_json(&runtime.projection()));
            };
            runtime
                .expand_panel_side(side)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "load_selected_saved_session_into_editor" => runtime
            .load_selected_saved_session_into_editor()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "open_selected_saved_session_in_new_tab" => runtime
            .open_selected_saved_session_in_new_tab()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "rename_saved_session" => {
            let a0 = arg_str(&args, 0)?;
            let a1 = arg_str(&args, 1)?;
            runtime
                .rename_saved_session(&a0, &a1)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "select_saved_session_by_id" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.select_saved_session_by_id(&a0)))
        }
        "start_new_saved_session_editor_in_folder" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .start_new_saved_session_editor_in_folder(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "toggle_panel_side_expanded" => {
            let a0 = arg_str(&args, 0)?;
            let Some(side) = panel_side_from_str(&a0) else {
                return Ok(projection_json(&runtime.projection()));
            };
            runtime
                .toggle_panel_side_expanded(side)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "toggle_panel_visible" => {
            let a0 = arg_str(&args, 0)?;
            let Some(panel) = panel_id_from_str(&a0) else {
                return Ok(projection_json(&runtime.projection()));
            };
            runtime
                .toggle_panel_visible(panel)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "update_selected_saved_session_from_active" => runtime
            .update_selected_saved_session_from_active()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "update_session_search" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.update_session_search(&a0)))
        }

        // --- D38 Remote Shell wiring: additive dispatch surface ---------------
        //
        // The out-of-process Shell (`remote_shell.rs`) needs Host methods for
        // the MainWindow callbacks that were previously only wired in-process
        // (`bootstrap.rs`). Each arm below forwards to an existing `AppRuntime`
        // method; only the argument decoding / viewport-row translation is new.
        //
        // Terminal clipboard / selection / search.
        "copy_active_terminal_selection" => runtime
            .copy_active_terminal_selection()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "terminal_clipboard_text" => {
            Ok(Value::String(runtime.terminal_clipboard_text().to_owned()))
        }
        "paste_text_into_terminal" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .paste_text_into_terminal(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "paste_terminal_clipboard" => runtime
            .paste_terminal_clipboard()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "paste_text_into_terminal_bracketed" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .paste_text_into_terminal_bracketed(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "paste_cross_pane" => Ok(projection_json(&runtime.paste_cross_pane())),
        "find_in_active_terminal" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .find_in_active_terminal(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "activate_terminal_find_across_match" => {
            let index = usize::try_from(arg_i32(&args, 0)?).unwrap_or(0);
            runtime
                .activate_terminal_find_across_match(index)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        // The Shell reports logical-pixel pointer positions turned into
        // (column, viewport-row); the Host owns the scrollback mapping.
        "begin_active_terminal_selection_at" => {
            let column = arg_u16(&args, 0)?;
            let row = terminal_absolute_row(runtime, arg_i32(&args, 1)?)?;
            runtime
                .begin_active_terminal_selection(column, row)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "update_active_terminal_selection_at" => {
            let column = arg_u16(&args, 0)?;
            let row = terminal_absolute_row(runtime, arg_i32(&args, 1)?)?;
            runtime
                .update_active_terminal_selection(column, row)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "select_word_in_active_terminal_at" => {
            let column = arg_u16(&args, 0)?;
            let row = terminal_absolute_row(runtime, arg_i32(&args, 1)?)?;
            runtime
                .select_word_in_active_terminal(column, row)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "scroll_active_terminal_to_line" => {
            let line = arg_i32(&args, 0)?;
            runtime
                .scroll_active_terminal_to_line(line)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "send_active_terminal_key" => {
            let text = arg_str(&args, 0)?;
            let ctrl = arg_bool(&args, 1)?;
            let alt = arg_bool(&args, 2)?;
            let shift = arg_bool(&args, 3)?;
            let meta = arg_bool(&args, 4)?;
            runtime
                .send_active_terminal_key(&text, ctrl, alt, shift, meta)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "start_input_sync_mode" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .start_input_sync_mode(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "toggle_tab_receives_key_input" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .toggle_tab_receives_key_input(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "prepare_tab_context_menu" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.prepare_tab_context_menu(&a0)))
        }
        "rename_tab" => {
            let tab_id = arg_str(&args, 0)?;
            let title = arg_str(&args, 1)?;
            runtime
                .rename_tab(&tab_id, &title)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "move_tab_to_pane" => {
            let tab_id = arg_str(&args, 0)?;
            let pane_id = arg_u64(&args, 1)?;
            runtime
                .move_tab_to_pane(&tab_id, pane_id)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        // SFTP pane surface.
        "select_sftp_row" => {
            let index = arg_i32(&args, 0)?;
            let ctrl = arg_bool(&args, 1)?;
            let shift = arg_bool(&args, 2)?;
            Ok(projection_json(
                &runtime.select_sftp_row(index, ctrl, shift),
            ))
        }
        "chmod_sftp_selected" => {
            let permissions = arg_str(&args, 0)?;
            runtime
                .chmod_sftp_selected(&permissions)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "set_sftp_column_ratio" => {
            let ratio = arg_f32(&args, 0)?;
            Ok(projection_json(&runtime.set_sftp_column_ratio(ratio)))
        }
        "switch_active_tab_to_sftp" => runtime
            .switch_active_tab_to_sftp()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "switch_tab_to_sftp" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .switch_tab_to_sftp(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "switch_tab_to_terminal" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .switch_tab_to_terminal(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        // SFTP operation dialogs pass four form fields; set them first.
        "upload_sftp_file"
        | "download_sftp_file"
        | "create_sftp_directory"
        | "rename_sftp_path"
        | "chmod_sftp_path"
        | "start_sftp_remote_edit" => {
            let local_path = arg_str(&args, 0)?;
            let remote_target = arg_str(&args, 1)?;
            let secondary_target = arg_str(&args, 2)?;
            let permissions = arg_str(&args, 3)?;
            let _ = runtime.set_sftp_operation_inputs(
                &local_path,
                &remote_target,
                &secondary_target,
                &permissions,
            );
            let result = match method {
                "upload_sftp_file" => runtime.upload_sftp_file(),
                "download_sftp_file" => runtime.download_sftp_file(),
                "create_sftp_directory" => runtime.create_sftp_directory(),
                "rename_sftp_path" => runtime.rename_sftp_path(),
                "chmod_sftp_path" => runtime.chmod_sftp_path(),
                _ => runtime.start_sftp_remote_edit(),
            };
            result
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "upload_local_selection_to_remote" => {
            let policy = arg_str(&args, 0)?;
            runtime
                .upload_local_selection_to_remote(&policy)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "upload_local_path_to_remote" => {
            let path = arg_str(&args, 0)?;
            let policy = arg_str(&args, 1)?;
            runtime
                .upload_local_path_to_remote(&path, &policy)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "download_sftp_selection_to" => {
            let local_dir = arg_str(&args, 0)?;
            let policy = arg_str(&args, 1)?;
            runtime
                .download_sftp_selection_to(&local_dir, &policy)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "sftp_upload_dir_override" => {
            runtime.sftp_upload_dir_override = runtime.sftp_selected_directory();
            Ok(projection_json(&runtime.projection()))
        }
        // Clipboard text for the "Copy Path" menu entries (the Shell owns the
        // OS clipboard).
        "sftp_copy_paths_text" => Ok(Value::String(runtime.sftp_selected_paths().join("\n"))),
        "local_copy_paths_text" => Ok(Value::String(
            runtime
                .local_selected_paths()
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )),
        "sftp_current_path_text" => Ok(Value::String(runtime.sftp_path.clone())),
        "local_current_path_text" => {
            Ok(Value::String(runtime.local_pane.dir.display().to_string()))
        }
        // Context-menu actions that only touch runtime state. Dialog-opening
        // entries are handled by the Shell (in-window path dialogs).
        "sftp_menu_action" => {
            let action = arg_str(&args, 0)?;
            let local_scope = arg_bool(&args, 1).unwrap_or(false);
            match action.as_str() {
                "switch-to-terminal" => runtime
                    .switch_active_tab_to_terminal()
                    .map(|p| projection_json(&p))
                    .map_err(|e| e.to_string()),
                "open" => runtime
                    .activate_sftp_entry()
                    .map(|p| projection_json(&p))
                    .map_err(|e| e.to_string()),
                "edit" => runtime
                    .edit_sftp_selected()
                    .map(|p| projection_json(&p))
                    .map_err(|e| e.to_string()),
                "properties" => Ok(projection_json(&runtime.open_sftp_properties())),
                "chmod" | "chmod-selection" => {
                    Ok(projection_json(&runtime.open_sftp_chmod_dialog()))
                }
                "move-selection" => runtime
                    .move_local_selection_to_remote()
                    .map(|p| projection_json(&p))
                    .map_err(|e| e.to_string()),
                "copy" => Ok(projection_json(&if local_scope {
                    runtime.copy_local_selection_to_clipboard()
                } else {
                    runtime.copy_remote_selection_to_clipboard()
                })),
                "copy-local" => Ok(projection_json(
                    &runtime.copy_local_selection_to_clipboard(),
                )),
                "delete-local-selection" => runtime
                    .delete_local_selection()
                    .map(|p| projection_json(&p))
                    .map_err(|e| e.to_string()),
                "paste-into-remote" => {
                    Ok(projection_json(&runtime.paste_file_clipboard_into_remote()))
                }
                "paste-into-local" => {
                    Ok(projection_json(&runtime.paste_file_clipboard_into_local()))
                }
                "refresh" => {
                    if local_scope {
                        Ok(projection_json(&runtime.refresh_local_pane()))
                    } else {
                        runtime
                            .refresh_active_sftp_listing()
                            .map(|p| projection_json(&p))
                            .map_err(|e| e.to_string())
                    }
                }
                "local-refresh" => Ok(projection_json(&runtime.refresh_local_pane())),
                "local-home" => Ok(projection_json(&runtime.open_local_home())),
                "toggle-hidden" => Ok(projection_json(&if local_scope {
                    runtime.toggle_local_hidden()
                } else {
                    runtime.toggle_sftp_hidden_files()
                })),
                "local-toggle-hidden" => Ok(projection_json(&runtime.toggle_local_hidden())),
                "up" => runtime
                    .open_sftp_parent()
                    .map(|p| projection_json(&p))
                    .map_err(|e| e.to_string()),
                "home" => runtime
                    .open_sftp_path("/")
                    .map(|p| projection_json(&p))
                    .map_err(|e| e.to_string()),
                other => Err(format!("unsupported SFTP menu action `{other}`")),
            }
        }
        // Drag payloads: the Shell builds an in-process `data-transfer`; the
        // Host resolves the selected paths and the drag modifiers.
        "sftp_drag_payload" => {
            let side = arg_str(&args, 0)?;
            let paths: Vec<String> = if side == "local" {
                runtime
                    .local_selected_paths()
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect()
            } else {
                runtime.sftp_selected_paths()
            };
            Ok(json!({
                "paths": paths,
                "ctrl": runtime.drag_ctrl,
                "shift": runtime.drag_shift,
            }))
        }
        "drop_on_local" => {
            let side = arg_optional_str(&args, 0)?;
            let paths = arg_string_list(&args, 1)?;
            let ctrl = arg_bool(&args, 2)?;
            let shift = arg_bool(&args, 3)?;
            let projection = match side.as_deref() {
                Some("local") => {
                    let paths: Vec<std::path::PathBuf> =
                        paths.iter().map(std::path::PathBuf::from).collect();
                    let same_volume = paths.iter().all(|path| {
                        crate::runtime::same_local_volume(path, &runtime.local_pane.dir)
                    });
                    runtime.drop_local_paths_on_local(
                        &paths,
                        crate::runtime::resolve_drop_move(same_volume, ctrl, shift),
                    )
                }
                Some("remote") => {
                    let paths: Vec<std::path::PathBuf> =
                        paths.iter().map(std::path::PathBuf::from).collect();
                    let entries = runtime.clipboard_remote_entries(&paths);
                    runtime
                        .drop_remote_entries_on_local(
                            &entries,
                            crate::runtime::resolve_drop_move(false, ctrl, shift),
                        )
                        .map_err(|e| e.to_string())?
                }
                _ => {
                    let paths: Vec<std::path::PathBuf> =
                        paths.iter().map(std::path::PathBuf::from).collect();
                    runtime.drop_local_paths_on_local(&paths, false)
                }
            };
            Ok(projection_json(&projection))
        }
        "drop_on_remote" => {
            let side = arg_optional_str(&args, 0)?;
            let index = arg_i32(&args, 1)?;
            let paths = arg_string_list(&args, 2)?;
            let ctrl = arg_bool(&args, 3)?;
            let shift = arg_bool(&args, 4)?;
            let target = runtime
                .sftp_visible_entry_at(index)
                .filter(|entry| matches!(entry.kind, yshell_sftp::FsEntryKind::Directory))
                .map(|entry| entry.path)
                .unwrap_or_else(|| runtime.sftp_path.clone());
            let projection = match side.as_deref() {
                Some("local") => {
                    let paths: Vec<std::path::PathBuf> =
                        paths.iter().map(std::path::PathBuf::from).collect();
                    runtime
                        .drop_local_paths_on_remote(
                            &paths,
                            &target,
                            crate::runtime::resolve_drop_move(false, ctrl, shift),
                        )
                        .map_err(|e| e.to_string())?
                }
                Some("remote") => {
                    let paths: Vec<std::path::PathBuf> =
                        paths.iter().map(std::path::PathBuf::from).collect();
                    let entries = runtime.clipboard_remote_entries(&paths);
                    runtime
                        .drop_remote_entries_on_remote(
                            &entries,
                            &target,
                            crate::runtime::resolve_drop_move(true, ctrl, shift),
                        )
                        .map_err(|e| e.to_string())?
                }
                _ => {
                    let paths: Vec<std::path::PathBuf> =
                        paths.iter().map(std::path::PathBuf::from).collect();
                    runtime
                        .drop_external_paths_on_remote(&paths, &target)
                        .map_err(|e| e.to_string())?
                }
            };
            Ok(projection_json(&projection))
        }
        // Local pane.
        "refresh_local_pane" => Ok(projection_json(&runtime.refresh_local_pane())),
        "retry_local_pane" => Ok(projection_json(&runtime.retry_local_pane())),
        "open_local_path" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.open_local_path(&a0)))
        }
        "open_local_parent" => Ok(projection_json(&runtime.open_local_parent())),
        "open_local_home" => Ok(projection_json(&runtime.open_local_home())),
        "toggle_local_hidden" => Ok(projection_json(&runtime.toggle_local_hidden())),
        "toggle_local_collapsed" => Ok(projection_json(&runtime.toggle_local_collapsed())),
        "sort_local_by" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.sort_local_by(&a0)))
        }
        "select_local_row" => {
            let index = arg_i32(&args, 0)?;
            let ctrl = arg_bool(&args, 1)?;
            let shift = arg_bool(&args, 2)?;
            Ok(projection_json(
                &runtime.select_local_row(index, ctrl, shift),
            ))
        }
        "activate_local_row" => {
            let index = arg_i32(&args, 0)?;
            Ok(projection_json(&runtime.activate_local_row(index)))
        }
        "delete_local_selection" => runtime
            .delete_local_selection()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "copy_selection_to_clipboard" => {
            Ok(projection_json(&runtime.copy_selection_to_clipboard()))
        }
        "cut_selection_to_clipboard" => Ok(projection_json(&runtime.cut_selection_to_clipboard())),
        // Transfers drawer.
        "toggle_transfer_drawer" => Ok(projection_json(&runtime.toggle_transfer_drawer())),
        "open_transfer_drawer" => Ok(projection_json(&runtime.open_transfer_drawer())),
        "pause_sftp_transfer" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .pause_sftp_transfer(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "resume_sftp_transfer" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .resume_sftp_transfer(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "retry_sftp_transfer" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .retry_sftp_transfer(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "cancel_sftp_transfer" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .cancel_sftp_transfer(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "remove_sftp_transfer" => {
            let a0 = arg_str(&args, 0)?;
            runtime
                .remove_sftp_transfer(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "clear_completed_sftp_transfers" => runtime
            .clear_completed_sftp_transfers()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "pause_all_sftp_transfers" => Ok(projection_json(&runtime.pause_all_sftp_transfers())),
        "resume_all_sftp_transfers" => Ok(projection_json(&runtime.resume_all_sftp_transfers())),
        "start_tunnel" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.start_tunnel(&a0)))
        }
        "stop_tunnel" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.stop_tunnel(&a0)))
        }
        "add_tunnel_from_dock_panel" => Ok(projection_json(&runtime.add_tunnel_from_dock_panel())),
        // Panels (dock layout).
        "panel_area_resized" => {
            let width = arg_f32(&args, 0)?;
            let height = arg_f32(&args, 1)?;
            Ok(projection_json(&runtime.set_panel_area_size(width, height)))
        }
        "panel_action" => {
            let id = arg_str(&args, 0)?;
            let action = arg_str(&args, 1)?;
            let Some(panel) = panel_id_from_str(&id) else {
                return Ok(projection_json(&runtime.projection()));
            };
            let result = match action.as_str() {
                "move-left" => runtime.move_panel(panel, yshell_config::PanelSide::Left, None),
                "move-right" => runtime.move_panel(panel, yshell_config::PanelSide::Right, None),
                "collapse" => runtime.set_panel_collapsed(panel, true),
                "expand" => runtime.set_panel_collapsed(panel, false),
                "collapse-toggle" => runtime.toggle_panel_collapsed(panel),
                "hide" => runtime.hide_panel(panel),
                _ => Ok(runtime.projection()),
            };
            result
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "panel_width_preview" => {
            let side = arg_str(&args, 0)?;
            let width = arg_f32(&args, 1)?;
            let Some(side) = panel_side_from_str(&side) else {
                return Ok(projection_json(&runtime.projection()));
            };
            Ok(projection_json(
                &runtime.preview_panel_side_width(side, width),
            ))
        }
        "panel_width_commit" | "panel_split_commit" => runtime
            .commit_panel_layout()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "panel_split_preview" => {
            let side = arg_str(&args, 0)?;
            let boundary = usize::try_from(arg_i32(&args, 1)?).unwrap_or(usize::MAX);
            let y = arg_f32(&args, 2)?;
            let Some(side) = panel_side_from_str(&side) else {
                return Ok(projection_json(&runtime.projection()));
            };
            Ok(projection_json(
                &runtime.preview_panel_split_pixels(side, boundary, y),
            ))
        }
        "panel_drag_start" | "panel_drag_move" | "panel_drag_drop" => {
            let id = arg_str(&args, 0)?;
            let x = arg_f32(&args, 1)?;
            let y = arg_f32(&args, 2)?;
            let Some(panel) = panel_id_from_str(&id) else {
                return Ok(projection_json(&runtime.projection()));
            };
            let result = match method {
                "panel_drag_start" => Ok(runtime.panel_drag_start(panel, x, y)),
                "panel_drag_move" => Ok(runtime.panel_drag_move(panel, x, y)),
                _ => runtime.panel_drag_drop(panel, x, y),
            };
            result
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "panel_drag_cancel" => Ok(projection_json(&runtime.panel_drag_cancel())),
        // Window / viewport reporting (PTY sizing + DPI).
        "note_terminal_viewport_size" => {
            let columns = arg_u16(&args, 0)?;
            let rows = arg_u16(&args, 1)?;
            runtime.note_terminal_viewport_size(columns, rows);
            Ok(Value::Null)
        }
        "sync_active_terminal_size" => {
            let columns = arg_u16(&args, 0)?;
            let rows = arg_u16(&args, 1)?;
            match runtime.sync_active_terminal_size_passive(columns, rows) {
                Ok(Some(projection)) => Ok(projection_json(&projection)),
                Ok(None) => Ok(projection_json(&runtime.projection())),
                Err(error) => Err(error.to_string()),
            }
        }
        // D38 多窗口：Shell 每个窗口各自上报视口，Host 按窗口解析该窗口的活动会话。
        "sync_window_terminal_size" => {
            let window_id = arg_u64(&args, 0)?;
            let columns = arg_u16(&args, 1)?;
            let rows = arg_u16(&args, 2)?;
            match runtime.sync_window_terminal_size(window_id, columns, rows, false) {
                Ok(changed) => Ok(Value::Bool(changed)),
                Err(error) => Err(error.to_string()),
            }
        }
        // D38 分屏：Shell 上报次 pane 视口，Host 同步该窗口次会话的 PTY 尺寸。
        "sync_window_secondary_terminal_size" => {
            let window_id = arg_u64(&args, 0)?;
            let columns = arg_u16(&args, 1)?;
            let rows = arg_u16(&args, 2)?;
            match runtime.sync_window_secondary_terminal_size(window_id, columns, rows) {
                Ok(changed) => Ok(Value::Bool(changed)),
                Err(error) => Err(error.to_string()),
            }
        }
        // D38 渲染对齐：该窗口活动会话冻结的终端外观（调色板 + 字体 + 字号）。
        // 与 in-process 的 `window_terminal_appearance` 同一数据源；Shell 用它
        // 同步自己的渲染器（字号变化后由 Shell 重新上报视口/PTY 尺寸）。
        "window_terminal_appearance" => {
            let window_id = arg_u64(&args, 0)?;
            match runtime.window_terminal_appearance(window_id) {
                Some(appearance) => {
                    serde_json::to_value(WireTerminalAppearance::from_runtime(&appearance))
                        .map_err(|error| error.to_string())
                }
                None => Ok(Value::Null),
            }
        }
        // Misc Shell surface.
        "quick_connect_copied_status" => {
            let a0 = arg_str(&args, 0)?;
            Ok(projection_json(&runtime.quick_connect_copied_status(&a0)))
        }
        "connection_overlay_error_text" => {
            Ok(Value::String(runtime.connection_overlay_error_text()))
        }

        other => super::dialog_dispatch::dispatch(runtime, other, args),
    }
}

pub(crate) fn arg_list(args: &Value) -> Result<&Vec<Value>, String> {
    args.as_array()
        .ok_or_else(|| "args must be a JSON array".into())
}

pub(crate) fn arg_str(args: &Value, index: usize) -> Result<String, String> {
    arg_list(args)?
        .get(index)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("args[{index}] string required"))
}

pub(crate) fn arg_u64(args: &Value, index: usize) -> Result<u64, String> {
    let v = arg_list(args)?
        .get(index)
        .ok_or_else(|| format!("args[{index}] required"))?;
    v.as_u64()
        .or_else(|| v.as_i64().map(|i| i as u64))
        .ok_or_else(|| format!("args[{index}] integer required"))
}

pub(crate) fn arg_i32(args: &Value, index: usize) -> Result<i32, String> {
    let v = arg_list(args)?
        .get(index)
        .ok_or_else(|| format!("args[{index}] required"))?;
    v.as_i64()
        .and_then(|i| i32::try_from(i).ok())
        .ok_or_else(|| format!("args[{index}] i32 required"))
}

pub(crate) fn arg_bool(args: &Value, index: usize) -> Result<bool, String> {
    arg_list(args)?
        .get(index)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| format!("args[{index}] bool required"))
}

pub(crate) fn arg_u16(args: &Value, index: usize) -> Result<u16, String> {
    let value = arg_u64(args, index)?;
    u16::try_from(value).map_err(|_| format!("args[{index}] u16 required"))
}

/// Optional string argument (`null` / missing means "not provided").
fn arg_optional_str(args: &Value, index: usize) -> Result<Option<String>, String> {
    match arg_list(args)?.get(index) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|text| Some(text.to_owned()))
            .ok_or_else(|| format!("args[{index}] string required")),
    }
}

/// JSON array of strings (drag payload paths).
fn arg_string_list(args: &Value, index: usize) -> Result<Vec<String>, String> {
    arg_list(args)?
        .get(index)
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .ok_or_else(|| format!("args[{index}] string array required"))
}

/// Translate a viewport row reported by the Shell into the scrollback-absolute
/// terminal row used by the selection API.
fn terminal_absolute_row(runtime: &AppRuntime, viewport_row: i32) -> Result<u16, String> {
    let metrics = runtime
        .active_terminal_viewport_metrics()
        .ok_or_else(|| "no active terminal session".to_owned())?;
    let row = metrics
        .top_absolute_row
        .saturating_add(usize::try_from(viewport_row.max(0)).unwrap_or(usize::MAX));
    u16::try_from(row).map_err(|_| "terminal row out of range".to_owned())
}

fn arg_f32(args: &Value, index: usize) -> Result<f32, String> {
    let v = arg_list(args)?
        .get(index)
        .ok_or_else(|| format!("args[{index}] required"))?;
    v.as_f64()
        .map(|f| f as f32)
        .ok_or_else(|| format!("args[{index}] number required"))
}

fn panel_id_from_str(id: &str) -> Option<yshell_config::PanelId> {
    match id {
        "sessions" => Some(yshell_config::PanelId::Sessions),
        "local" => Some(yshell_config::PanelId::Local),
        "sftp" => Some(yshell_config::PanelId::Sftp),
        "tunnels" => Some(yshell_config::PanelId::Tunnels),
        "quick_commands" => Some(yshell_config::PanelId::QuickCommands),
        "transfers" => Some(yshell_config::PanelId::Transfers),
        _ => None,
    }
}

fn panel_side_from_str(side: &str) -> Option<yshell_config::PanelSide> {
    match side {
        "left" => Some(yshell_config::PanelSide::Left),
        "right" => Some(yshell_config::PanelSide::Right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    use crate::runtime::AppRuntime;

    /// The D38 render-parity arms are additive: without a live session they
    /// report "nothing to sync" instead of failing the Shell.
    #[test]
    fn render_parity_arms_are_safe_without_sessions() {
        // `AppRuntime` + this (very large) dispatch function both need generous
        // debug-build stack frames, so the body runs on an explicit thread.
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let temp = TempDir::new().expect("tempdir");
                let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
                    .expect("runtime");
                runtime.register_window(0);
                let appearance = dispatch(&mut runtime, "window_terminal_appearance", json!([0]))
                    .expect("appearance dispatch");
                assert!(
                    appearance.is_null(),
                    "a window without an active session has no appearance"
                );
                let secondary = dispatch(
                    &mut runtime,
                    "sync_window_secondary_terminal_size",
                    json!([0, 80, 24]),
                )
                .expect("secondary size dispatch");
                assert_eq!(secondary, Value::Bool(false));
            })
            .expect("spawn test thread")
            .join()
            .expect("join test thread");
    }
}
