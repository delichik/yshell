//! `AppProjection` value parts and state flags.

use tempfile::tempdir;

use super::*;

#[test]
fn secret_store_status_projects_kind_and_path_values() {
    let temp = tempdir().expect("tempdir");
    let runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

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
