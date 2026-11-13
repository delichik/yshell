//! Extra AppRuntime methods used by out-of-process tool windows.

use serde_json::Value;

use crate::runtime::AppRuntime;

use super::runtime_dispatch::{arg_bool, arg_i32, arg_str, arg_u64};
use super::shell_ipc::projection_json;

pub(crate) fn dispatch(
    runtime: &mut AppRuntime,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    let value = dispatch_inner(runtime, method, &args)?;
    Ok(value)
}

fn proj(runtime: &AppRuntime) -> Result<Value, String> {
    Ok(projection_json(&runtime.projection()))
}

fn fail_status(runtime: &mut AppRuntime, error: impl std::fmt::Display) -> Result<Value, String> {
    runtime.set_status_kind(
        "editor-submit-failed",
        format!("{error}"),
        String::new(),
        String::new(),
    );
    proj(runtime)
}

fn dispatch_inner(runtime: &mut AppRuntime, method: &str, args: &Value) -> Result<Value, String> {
    match method {
        "reduce_motion" => Ok(Value::Bool(runtime.config_document.ui.reduce_motion)),
        "ensure_debug_log_dir" => runtime
            .ensure_debug_log_dir()
            .map(|path| Value::String(path.display().to_string()))
            .map_err(|e| e.to_string()),
        "apply_file_dialog_picked" => {
            let kind = arg_str(args, 0)?;
            let path = arg_str(args, 1)?;
            let kind = crate::dialog_events::dialog_kind_from_name(&kind)
                .ok_or_else(|| format!("unknown dialog kind: {kind}"))?;
            Ok(projection_json(
                &crate::dialog_events::apply_dialog_outcome(
                    runtime,
                    crate::dialog_events::FileDialogOutcome::Picked { kind, path },
                ),
            ))
        }
        "set_status_kind" => {
            runtime.set_status_kind(
                &arg_str(args, 0)?,
                arg_str(args, 1)?,
                arg_str(args, 2)?,
                arg_str(args, 3)?,
            );
            Ok(Value::Null)
        }
        "selected_private_key_public_line" => runtime
            .selected_private_key_public_line()
            .map(Value::String)
            .map_err(|e| e.to_string()),
        "generated_private_key_public_line" => Ok(runtime
            .generated_private_key_public_line()
            .map(Value::String)
            .unwrap_or(Value::Null)),
        "active_session_logging_path" => {
            Ok(Value::String(runtime.active_session_logging_path_text()))
        }
        "save_selected_private_key_passphrase" => Ok(projection_json(
            &runtime.save_selected_private_key_passphrase(),
        )),
        "forget_selected_private_key_passphrase" => Ok(projection_json(
            &runtime.forget_selected_private_key_passphrase(),
        )),
        "update_private_keys_passphrase_input" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_private_keys_passphrase_input(&a0),
            ))
        }
        "rename_selected_private_key" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.rename_selected_private_key(&a0)))
        }
        "open_private_keys_generate_dialog" => Ok(projection_json(
            &runtime.open_private_keys_generate_dialog(),
        )),
        "open_private_keys_import_dialog" => {
            Ok(projection_json(&runtime.open_private_keys_import_dialog()))
        }
        "prepare_private_key_context_menu" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(
                &runtime.prepare_private_key_context_menu(a0),
            ))
        }
        "test_private_key_from_menu" => Ok(projection_json(&runtime.test_private_key_from_menu())),
        "export_private_key_from_menu" => {
            Ok(projection_json(&runtime.export_private_key_from_menu()))
        }
        "deploy_private_key_from_menu" => {
            Ok(projection_json(&runtime.deploy_private_key_from_menu()))
        }
        "remove_private_key_from_menu" => {
            Ok(projection_json(&runtime.remove_private_key_from_menu()))
        }
        "confirm_private_key_remove" => Ok(projection_json(&runtime.confirm_private_key_remove())),
        "cancel_private_key_remove" => Ok(projection_json(&runtime.cancel_private_key_remove())),
        "close_private_keys_generate_dialog" => Ok(projection_json(
            &runtime.close_private_keys_generate_dialog(),
        )),
        "finish_private_keys_generate_dialog" => Ok(projection_json(
            &runtime.finish_private_keys_generate_dialog(),
        )),
        "update_private_keys_generate_label" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_private_keys_generate_label(&a0),
            ))
        }
        "update_private_keys_generate_comment" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_private_keys_generate_comment(&a0),
            ))
        }
        "update_private_keys_generate_passphrase" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_private_keys_generate_passphrase(&a0),
            ))
        }
        "update_private_keys_generate_confirm" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_private_keys_generate_confirm(&a0),
            ))
        }
        "select_private_keys_generate_algorithm" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(
                &runtime.select_private_keys_generate_algorithm(a0),
            ))
        }
        "generate_private_key_from_form" => {
            Ok(projection_json(&runtime.generate_private_key_from_form()))
        }
        "close_private_keys_import_dialog" => {
            Ok(projection_json(&runtime.close_private_keys_import_dialog()))
        }
        "select_private_keys_import_tab" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.select_private_keys_import_tab(a0)))
        }
        "update_private_keys_import_label" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_private_keys_import_label(&a0),
            ))
        }
        "update_private_keys_import_path" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_private_keys_import_path(&a0),
            ))
        }
        "update_private_keys_import_passphrase" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_private_keys_import_passphrase(&a0),
            ))
        }
        "update_private_keys_import_paste_text" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_private_keys_import_paste_text(&a0),
            ))
        }
        "import_private_key_from_dialog" => {
            Ok(projection_json(&runtime.import_private_key_from_dialog()))
        }
        "close_private_keys_export_dialog" => {
            Ok(projection_json(&runtime.close_private_keys_export_dialog()))
        }
        "update_private_keys_export_path" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_private_keys_export_path(&a0),
            ))
        }
        "export_private_key_to_path" => Ok(projection_json(&runtime.export_private_key_to_path())),
        "close_private_keys_deploy_dialog" => {
            Ok(projection_json(&runtime.close_private_keys_deploy_dialog()))
        }
        "select_private_key_deploy_target" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(
                &runtime.select_private_key_deploy_target(a0),
            ))
        }
        "confirm_private_key_deploy" => Ok(projection_json(&runtime.confirm_private_key_deploy())),
        "select_proxy_for_edit" => {
            let index = arg_i32(args, 0)?;
            let _ = runtime.select_proxy(index);
            Ok(projection_json(&runtime.begin_edit_proxy()))
        }
        "update_proxy_editor_name" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_proxy_editor_name(&a0)))
        }
        "update_proxy_editor_host" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_proxy_editor_host(&a0)))
        }
        "update_proxy_editor_port" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_proxy_editor_port(&a0)))
        }
        "update_proxy_editor_username" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_proxy_editor_username(&a0)))
        }
        "update_proxy_editor_password" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_proxy_editor_password(&a0)))
        }
        "select_proxy_editor_protocol" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.select_proxy_editor_protocol(a0)))
        }
        "toggle_proxy_editor_dns" => Ok(projection_json(&runtime.toggle_proxy_editor_dns())),
        "test_selected_proxy" => Ok(projection_json(&runtime.test_selected_proxy())),
        "set_referencing_sessions_to_no_proxy" => Ok(projection_json(
            &runtime.set_referencing_sessions_to_no_proxy(),
        )),
        "dismiss_proxy_remove_warning" => {
            Ok(projection_json(&runtime.dismiss_proxy_remove_warning()))
        }
        "prepare_proxy_context_menu" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.prepare_proxy_context_menu(a0)))
        }
        "duplicate_proxy_from_menu" => Ok(projection_json(&runtime.duplicate_proxy_from_menu())),
        "test_proxy_from_menu" => Ok(projection_json(&runtime.test_proxy_from_menu())),
        "remove_proxy_from_menu" => Ok(projection_json(&runtime.remove_proxy_from_menu())),
        "confirm_proxy_remove" => Ok(projection_json(&runtime.confirm_proxy_remove())),
        "cancel_proxy_remove" => Ok(projection_json(&runtime.cancel_proxy_remove())),
        "clone_color_theme" => {
            let id = arg_str(args, 0)?;
            match runtime.clone_color_theme(&id) {
                Ok(p) => Ok(projection_json(&p)),
                Err(error) => fail_status(runtime, error),
            }
        }
        "clone_selected_color_theme" => {
            let source = runtime.theme_manager_selection_id();
            match runtime.clone_color_theme(&source) {
                Ok(p) => Ok(projection_json(&p)),
                Err(error) => fail_status(runtime, error),
            }
        }
        "open_theme_editor" => {
            let id = arg_str(args, 0)?;
            Ok(projection_json(&runtime.open_theme_editor(&id)))
        }
        "request_delete_color_theme" => {
            let id = arg_str(args, 0)?;
            Ok(projection_json(&runtime.request_delete_color_theme(&id)))
        }
        "request_delete_selected_color_theme" => {
            let id = runtime.theme_manager_selection_id();
            Ok(projection_json(&runtime.request_delete_color_theme(&id)))
        }
        "close_theme_editor" => Ok(projection_json(&runtime.close_theme_editor())),
        "save_theme_editor_with_appearance" => {
            match runtime.save_theme_editor() {
                Ok(_) => {
                    if let Err(error) = runtime.save_settings_appearance() {
                        let _ = fail_status(runtime, error)?;
                    }
                }
                Err(error) => {
                    let _ = fail_status(runtime, error)?;
                }
            }
            proj(runtime)
        }
        "update_theme_editor_name" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_theme_editor_name(&a0)))
        }
        "update_theme_editor_role_color" => {
            let role = arg_str(args, 0)?;
            let hex = arg_str(args, 1)?;
            Ok(projection_json(
                &runtime.update_theme_editor_role_color(&role, &hex),
            ))
        }
        "update_theme_editor_ansi_color" => {
            let index = arg_i32(args, 0)?;
            let hex = arg_str(args, 1)?;
            Ok(projection_json(
                &runtime.update_theme_editor_ansi_color(index, &hex),
            ))
        }
        "update_settings_appearance_font_family" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_appearance_font_family(&a0),
            ))
        }
        "update_settings_appearance_font_size" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_appearance_font_size(&a0),
            ))
        }
        "update_settings_appearance_backup_font" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_settings_appearance_backup_font(&a0),
            ))
        }
        "confirm_delete_color_theme" => runtime
            .confirm_delete_color_theme()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "cancel_delete_color_theme" => Ok(projection_json(&runtime.cancel_delete_color_theme())),
        "select_host_key" => {
            let group = arg_i32(args, 0)?;
            let entry = arg_i32(args, 1)?;
            Ok(projection_json(&runtime.select_host_key(group, entry)))
        }
        "open_host_keys_add_dialog" => Ok(projection_json(&runtime.open_host_keys_add_dialog())),
        "open_host_keys_import_dialog" => {
            Ok(projection_json(&runtime.open_host_keys_import_dialog()))
        }
        "open_host_keys_export_dialog" => {
            Ok(projection_json(&runtime.open_host_keys_export_dialog()))
        }
        "open_host_keys_edit_dialog" => {
            let group = arg_i32(args, 0)?;
            let entry = arg_i32(args, 1)?;
            Ok(projection_json(
                &runtime.open_host_keys_edit_dialog(group, entry),
            ))
        }
        "prepare_host_keys_context_menu" => {
            let group = arg_i32(args, 0)?;
            let entry = arg_i32(args, 1)?;
            Ok(projection_json(
                &runtime.prepare_host_keys_context_menu(group, entry),
            ))
        }
        "cancel_host_keys_add_dialog" => {
            Ok(projection_json(&runtime.cancel_host_keys_add_dialog()))
        }
        "update_host_keys_add_text" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_host_keys_add_text(&a0)))
        }
        "submit_host_keys_add_dialog" => {
            Ok(projection_json(&runtime.submit_host_keys_add_dialog()))
        }
        "close_host_keys_import_dialog" => {
            Ok(projection_json(&runtime.close_host_keys_import_dialog()))
        }
        "update_host_keys_import_path" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_host_keys_import_path(&a0)))
        }
        "import_host_keys_from_dialog" => {
            Ok(projection_json(&runtime.import_host_keys_from_dialog()))
        }
        "close_host_keys_export_dialog" => {
            Ok(projection_json(&runtime.close_host_keys_export_dialog()))
        }
        "update_host_keys_export_path" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_host_keys_export_path(&a0)))
        }
        "export_host_keys_to_path" => Ok(projection_json(&runtime.export_host_keys_to_path())),
        "clear_all_host_keys" => runtime
            .clear_all_host_keys()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "remove_selected_host_key" => runtime
            .remove_selected_host_key()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "update_logging_dialog_directory" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_logging_dialog_directory(&a0),
            ))
        }
        "update_logging_dialog_file_name" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_logging_dialog_file_name(&a0),
            ))
        }
        "select_auth_prompt_method" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.select_auth_prompt_method(a0)))
        }
        "update_auth_prompt_password" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_auth_prompt_password(&a0)))
        }
        "select_auth_prompt_key" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.select_auth_prompt_key(a0)))
        }
        "update_auth_prompt_passphrase" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_auth_prompt_passphrase(&a0)))
        }
        "toggle_auth_prompt_use_agent" => {
            let a0 = arg_bool(args, 0)?;
            Ok(projection_json(&runtime.toggle_auth_prompt_use_agent(a0)))
        }
        "update_auth_prompt_keyboard_answer" => {
            let index = arg_i32(args, 0)?;
            let value = arg_str(args, 1)?;
            Ok(projection_json(
                &runtime.update_auth_prompt_keyboard_answer(index, &value),
            ))
        }
        "submit_auth_prompt_keyboard_round" => Ok(projection_json(
            &runtime.submit_auth_prompt_keyboard_round(),
        )),
        "select_session_editor_authentication" => Ok(projection_json(
            &runtime.select_session_editor_authentication(),
        )),
        "select_session_editor_login_script" => Ok(projection_json(
            &runtime.select_session_editor_login_script(),
        )),
        "set_editor_inherit_auth" => {
            let a0 = arg_bool(args, 0)?;
            Ok(projection_json(&runtime.set_editor_inherit_auth(a0)))
        }
        "set_editor_inherit_login_script" => {
            let a0 = arg_bool(args, 0)?;
            Ok(projection_json(
                &runtime.set_editor_inherit_login_script(a0),
            ))
        }
        "set_editor_inherit_proxy" => {
            let a0 = arg_bool(args, 0)?;
            Ok(projection_json(&runtime.set_editor_inherit_proxy(a0)))
        }
        "set_editor_inherit_tunnels" => {
            let a0 = arg_bool(args, 0)?;
            Ok(projection_json(&runtime.set_editor_inherit_tunnels(a0)))
        }
        "set_editor_inherit_terminal_style" => {
            let a0 = arg_bool(args, 0)?;
            Ok(projection_json(
                &runtime.set_editor_inherit_terminal_style(a0),
            ))
        }
        "update_editor_username" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_editor_username(&a0)))
        }
        "update_editor_icon_index" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.update_editor_icon_index(a0)))
        }
        "reset_editor_icon" => Ok(projection_json(&runtime.reset_editor_icon())),
        "update_editor_notes" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_editor_notes(&a0)))
        }
        "set_editor_auth_method_private_key" => Ok(projection_json(
            &runtime.set_editor_auth_method_private_key(),
        )),
        "set_editor_auth_method_keyboard_interactive" => Ok(projection_json(
            &runtime.set_editor_auth_method_keyboard_interactive(),
        )),
        "update_editor_password" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_editor_password(&a0)))
        }
        "update_editor_passphrase" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_editor_passphrase(&a0)))
        }
        "add_editor_login_step" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.add_editor_login_step(&a0)))
        }
        "edit_editor_login_step" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.edit_editor_login_step(a0)))
        }
        "select_editor_login_step" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.select_editor_login_step(a0)))
        }
        "duplicate_editor_login_step" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.duplicate_editor_login_step(a0)))
        }
        "delete_editor_login_step" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.delete_editor_login_step(a0)))
        }
        "move_editor_login_step" => {
            let from = arg_i32(args, 0)?;
            let to = arg_i32(args, 1)?;
            Ok(projection_json(&runtime.move_editor_login_step(from, to)))
        }
        "delete_selected_editor_login_step" => {
            let index = runtime.editor_selected_login_step_index();
            Ok(projection_json(&runtime.delete_editor_login_step(index)))
        }
        "begin_add_editor_tunnel" => Ok(projection_json(&runtime.begin_add_editor_tunnel())),
        "begin_edit_editor_tunnel" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.begin_edit_editor_tunnel(a0)))
        }
        "select_editor_tunnel" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.select_editor_tunnel(a0)))
        }
        "duplicate_editor_tunnel" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.duplicate_editor_tunnel(a0)))
        }
        "delete_editor_tunnel" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.delete_editor_tunnel(a0)))
        }
        "select_editor_scheme_index" => {
            let a0 = arg_i32(args, 0)?;
            Ok(projection_json(&runtime.select_editor_scheme_index(a0)))
        }
        "update_editor_login_step_name" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_editor_login_step_name(&a0)))
        }
        "update_editor_login_step_keys" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_editor_login_step_keys(&a0)))
        }
        "set_editor_login_step_tokens" => {
            let a0 = arg_bool(args, 0)?;
            Ok(projection_json(&runtime.set_editor_login_step_tokens(a0)))
        }
        "update_editor_login_step_millis" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_editor_login_step_millis(&a0),
            ))
        }
        "update_editor_tunnel_name" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_editor_tunnel_name(&a0)))
        }
        "set_editor_tunnel_kind" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.set_editor_tunnel_kind(&a0)))
        }
        "update_editor_tunnel_dialog_bind_host" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_editor_tunnel_dialog_bind_host(&a0),
            ))
        }
        "update_editor_tunnel_dialog_bind_port" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_editor_tunnel_dialog_bind_port(&a0),
            ))
        }
        "update_editor_tunnel_dialog_target_host" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_editor_tunnel_dialog_target_host(&a0),
            ))
        }
        "update_editor_tunnel_dialog_target_port" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(
                &runtime.update_editor_tunnel_dialog_target_port(&a0),
            ))
        }
        "set_editor_tunnel_enabled" => {
            let a0 = arg_bool(args, 0)?;
            Ok(projection_json(&runtime.set_editor_tunnel_enabled(a0)))
        }
        "reset_all_folder_editor" => Ok(projection_json(&runtime.reset_all_folder_editor())),
        "apply_folder_editor_field_action" => {
            let key = arg_str(args, 0)?;
            let action = arg_str(args, 1)?;
            Ok(projection_json(
                &runtime.apply_folder_editor_field_action(&key, &action),
            ))
        }
        "update_folder_editor_field" => {
            let key = arg_str(args, 0)?;
            let value = arg_str(args, 1)?;
            Ok(projection_json(
                &runtime.update_folder_editor_field(&key, &value),
            ))
        }
        "folder_editor_field_toggled" => {
            let key = arg_str(args, 0)?;
            Ok(projection_json(&runtime.folder_editor_field_toggled(&key)))
        }
        "folder_editor_field_selected" => {
            let key = arg_str(args, 0)?;
            let index = arg_i32(args, 1)?;
            Ok(projection_json(
                &runtime.folder_editor_field_selected(&key, index),
            ))
        }
        "update_sftp_chmod_mode" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_sftp_chmod_mode(&a0)))
        }
        "update_selected_saved_session_from_active" => runtime
            .update_selected_saved_session_from_active()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "select_saved_session_by_id" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.select_saved_session_by_id(&a0)))
        }
        "activate_saved_session_tree_node" => {
            let a0 = arg_str(args, 0)?;
            runtime
                .activate_saved_session_tree_node(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "open_selected_saved_session_in_new_tab" => runtime
            .open_selected_saved_session_in_new_tab()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "update_session_search" => {
            let a0 = arg_str(args, 0)?;
            Ok(projection_json(&runtime.update_session_search(&a0)))
        }
        "rename_saved_session" => {
            let a0 = arg_str(args, 0)?;
            let a1 = arg_str(args, 1)?;
            runtime
                .rename_saved_session(&a0, &a1)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "start_new_saved_session_editor_in_folder" => {
            let a0 = arg_str(args, 0)?;
            runtime
                .start_new_saved_session_editor_in_folder(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "load_selected_saved_session_into_editor" => runtime
            .load_selected_saved_session_into_editor()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "resolve_mcp_consent" => {
            let accepted = arg_bool(args, 0)?;
            let dont_ask = arg_bool(args, 1).unwrap_or(false);
            let decision = if accepted {
                crate::runtime::mcp::McpConsentDecision::Accept {
                    dont_ask_again: dont_ask,
                }
            } else {
                crate::runtime::mcp::McpConsentDecision::Reject
            };
            runtime.resolve_mcp_consent(decision);
            Ok(projection_json(&runtime.projection()))
        }
        "create_root_saved_folder" => {
            let a0 = arg_str(args, 0)?;
            runtime
                .create_root_saved_folder(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "clear_active_terminal_scrollback" => runtime
            .clear_active_terminal_scrollback()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "create_sftp_folder_named" => {
            let a0 = arg_str(args, 0)?;
            runtime
                .create_sftp_folder_named(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "rename_sftp_selected" => {
            let a0 = arg_str(args, 0)?;
            runtime
                .rename_sftp_selected(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "upload_sftp_into_current" => {
            let a0 = arg_str(args, 0)?;
            runtime
                .upload_sftp_into_current(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "download_sftp_selected" => {
            let a0 = arg_str(args, 0)?;
            runtime
                .download_sftp_selected(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "copy_sftp_selection_to" => {
            let a0 = arg_str(args, 0)?;
            let a1 = arg_str(args, 1).unwrap_or_else(|_| "ask".to_owned());
            runtime
                .copy_sftp_selection_to(&a0, &a1)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "delete_sftp_selection" => runtime
            .delete_sftp_selection()
            .map(|p| projection_json(&p))
            .map_err(|e| e.to_string()),
        "chmod_sftp_selection" => {
            let a0 = arg_str(args, 0)?;
            runtime
                .chmod_sftp_selection(&a0)
                .map(|p| projection_json(&p))
                .map_err(|e| e.to_string())
        }
        "request_close_window" => {
            let window_id = arg_u64(args, 0)?;
            let active = runtime.active_connections_in_window(window_id);
            let is_last = runtime.is_last_window(window_id);
            let confirm_close = runtime.confirm_close_session_enabled();
            let confirm_quit = runtime.confirm_quit_app_enabled();
            let action = if is_last {
                if active > 0 && confirm_quit {
                    "confirm_quit"
                } else {
                    runtime.close_window_tabs(window_id);
                    "close_now"
                }
            } else if active > 0 && confirm_close {
                runtime.set_window_close_pending(window_id, true);
                runtime.mark_ui_dirty();
                "confirm_close"
            } else {
                runtime.close_window_tabs(window_id);
                "close_now"
            };
            Ok(serde_json::json!({
                "action": action,
                "active_count": active,
                "is_last": is_last,
            }))
        }
        "confirm_close_window" => {
            let window_id = arg_u64(args, 0)?;
            let dont_ask = arg_bool(args, 1).unwrap_or(false);
            runtime.set_window_close_pending(window_id, false);
            if dont_ask {
                let _ = runtime.set_confirm_close_session(false);
            }
            runtime.close_window_tabs(window_id);
            Ok(projection_json(&runtime.projection()))
        }
        "cancel_close_window" => {
            let window_id = arg_u64(args, 0)?;
            runtime.set_window_close_pending(window_id, false);
            runtime.mark_ui_dirty();
            Ok(projection_json(&runtime.projection()))
        }
        other => Err(format!("unknown ui method: {other}")),
    }
}
