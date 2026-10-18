//! `AppProjection` value parts and state flags.

use tempfile::tempdir;

use super::*;

#[test]
fn fingerprints_are_grouped_in_user_facing_text() {
    // D9：裸 hex 指纹按 4 佝分组；SHA256:… 形弝原样保留。
    assert_eq!(
        format_fingerprint_groups("00112233445566"),
        "0011 2233 4455 66"
    );
    assert_eq!(format_fingerprint_groups(""), "");
    assert_eq!(
        format_fingerprint_groups("SHA256:AbCd/Ef+Gh"),
        "SHA256:AbCd/Ef+Gh"
    );

    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    runtime.pending_host_key_prompt = Some(PendingHostKeyPrompt {
        session_key: "session-1".to_owned(),
        host: "example.com".to_owned(),
        port: 2222,
        username: "alice".to_owned(),
        presented: yshell_ssh::HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "aabbccddeeff0011".to_owned(),
        },
        expected: None,
        known_hosts_path: runtime.config_store.known_hosts_file(),
        resume_password: None,
    });
    let text = runtime.projection().host_key_prompt_text;
    assert!(
        text.contains("Presented: ssh-ed25519 aabb ccdd eeff 0011"),
        "fingerprint should be grouped: {text}"
    );
}

#[test]
fn host_key_prompt_split_fields_track_prompt_state() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");

    // 无挂起杝示：拆分字段全为空串（弹窗打开剝 UI 保挝空布局）。
    let idle = runtime.projection();
    assert!(!idle.host_key_prompt_visible);
    assert_eq!(idle.host_key_prompt_target_text, "");
    assert_eq!(idle.host_key_prompt_known_hosts_path_text, "");
    assert_eq!(idle.host_key_prompt_algorithm_text, "");
    assert_eq!(idle.host_key_prompt_fingerprint_text, "");
    assert_eq!(idle.host_key_prompt_expected_algorithm_text, "");
    assert_eq!(idle.host_key_prompt_expected_fingerprint_text, "");

    let known_hosts_path = runtime.config_store.known_hosts_file();
    runtime.pending_host_key_prompt = Some(PendingHostKeyPrompt {
        session_key: "session-1".to_owned(),
        host: "example.com".to_owned(),
        port: 2222,
        username: "alice".to_owned(),
        presented: yshell_ssh::HostKeyFingerprint {
            algorithm: "ecdsa-sha2-nistp256".to_owned(),
            fingerprint: "aabbccddeeff0011".to_owned(),
        },
        expected: Some(yshell_ssh::HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "0011223344556677".to_owned(),
        }),
        known_hosts_path: known_hosts_path.clone(),
        resume_password: None,
    });

    // changed 模弝：expected 侧非空，指纹均按 4 佝分组。
    let changed = runtime.projection();
    assert!(changed.host_key_prompt_visible);
    assert_eq!(
        changed.host_key_prompt_target_text,
        "alice@example.com:2222"
    );
    assert_eq!(
        changed.host_key_prompt_known_hosts_path_text,
        known_hosts_path.display().to_string()
    );
    assert_eq!(
        changed.host_key_prompt_algorithm_text,
        "ecdsa-sha2-nistp256"
    );
    assert_eq!(
        changed.host_key_prompt_fingerprint_text,
        "aabb ccdd eeff 0011"
    );
    assert_eq!(
        changed.host_key_prompt_expected_algorithm_text,
        "ssh-ed25519"
    );
    assert_eq!(
        changed.host_key_prompt_expected_fingerprint_text,
        "0011 2233 4455 6677"
    );
    // SSH-3：变更模式回退正文不再提 REPLACE，而是三个动作里的 Replace and Trust。
    assert!(changed.host_key_prompt_text.contains("Replace and Trust"));
    assert!(!changed.host_key_prompt_text.contains("REPLACE"));

    // first-trust：expected 侧为空（空值行在弹窗里丝坠佝）。
    runtime
        .pending_host_key_prompt
        .as_mut()
        .expect("pending prompt")
        .expected = None;
    let first_trust = runtime.projection();
    assert_eq!(first_trust.host_key_prompt_expected_algorithm_text, "");
    assert_eq!(first_trust.host_key_prompt_expected_fingerprint_text, "");
}

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
        startup_snippet: "welcome".to_owned(),
    };
    assert_eq!(success.kind_id(), "success");
    assert_eq!(success.startup_snippet(), "welcome");
    assert_eq!(
        success.legacy_text(),
        "Auth test succeeded for ops@example.test:22. Startup: welcome"
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

    // 冷坯动：无会话〝无选中项，SFTP 与终端选区也丝坯用。
    let welcome = runtime.projection();
    assert!(!welcome.has_active_session);
    assert!(!welcome.active_session_connected);
    assert!(!welcome.has_saved_selection);
    assert!(!welcome.sftp_available);
    assert!(!welcome.terminal_has_selection);

    // 快速连接坎：有活动会话且已连接，但还没有选区/保存选中项。
    let connected = runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    assert!(connected.has_active_session);
    assert!(connected.active_session_connected);
    assert!(!connected.has_saved_selection);
    assert!(!connected.terminal_has_selection);
    assert!(connected.sftp_available);

    // 终端全选 → Copy 蝜坕项的选区开关跟隝。
    let selected = runtime
        .select_all_active_terminal()
        .expect("select all terminal");
    assert!(selected.terminal_has_selection);
    assert!(selected.terminal_selection_active);

    // 保存活动会话会选中新 profile → Edit/Update/Delete 蝜坕项坯用。
    let saved = runtime.save_active_session().expect("save active session");
    assert!(saved.has_saved_selection);

    // 断开坎 connected 开关跟隝违行时状思。
    let disconnected = runtime
        .disconnect_active_session()
        .expect("disconnect active session");
    assert!(disconnected.has_active_session);
    assert!(!disconnected.active_session_connected);

    // SFTP 生命周期进入 ready → SFTP 擝作项坯用。
    runtime.sftp_session = SftpSessionLifecycle::Ready {
        session_key: "runtime-1".to_owned(),
    };
    assert!(runtime.projection().sftp_available);
}

#[test]
fn quick_connect_visible_projection_is_driven_by_the_overlay_state() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    // TAB-6：启动时覆盖层默认打开，但零标签（QC 不再是标签）。
    let startup = runtime.projection();
    assert!(startup.quick_connect_visible);
    assert_eq!(startup.tab_count, 0);
    assert!(startup.tabs.is_empty());

    // 关闭覆盖层：零标签仍合法，不因"无标签"强制重开。
    let closed = runtime.dismiss_quick_connect();
    assert!(!closed.quick_connect_visible);
    assert_eq!(closed.tab_count, 0);
    assert!(closed.tabs.is_empty());
    assert_eq!(closed.active_tab_id, "");

    // 连接终端：追加标签，覆盖层保持关闭（打开会话 = 显示终端）。
    let connected = runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    assert!(!connected.quick_connect_visible);
    assert_eq!(connected.tab_count, 1);
    let tab_id = connected.active_tab_id.clone();

    // 显式打开覆盖层：可见性切换但标签投影原样（数量/活动标签不变）。
    let opened = runtime.open_quick_connect();
    assert!(opened.quick_connect_visible);
    assert_eq!(opened.tab_count, 1);
    assert_eq!(opened.active_tab_id, tab_id);
    assert!(opened.tabs[0].active);
}
