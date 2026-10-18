//! N2：Quick Connect 页（标签、历史、快速链接、行投影、`+` 行为）。

use tempfile::tempdir;

use super::*;

/// 读取磁盘上的配置（验证即时写回）。
fn reload_config(config_dir: &std::path::Path) -> ConfigDocument {
    ConfigStore::new(config_dir.to_path_buf())
        .load_or_recover()
        .expect("reload config")
        .document
}

#[test]
fn submit_records_history_on_success_and_clears_input() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);

    let projection = runtime
        .submit_quick_connect("ssh://root@example.com:2200")
        .expect("submit");

    assert_eq!(projection.quick_connect_error_text, "");
    assert_eq!(
        projection.quick_connect_last_target_text,
        "root@example.com:2200"
    );
    // 成功后清空输入框。
    assert_eq!(projection.quick_connect_input_text, "");
    // 直连开的是终端标签，QC 页不可见。
    assert!(!projection.quick_connect_visible);
    assert_eq!(runtime.tabs.len(), 1);
    assert!(runtime.tabs[0].session_id().is_some());

    // 历史：canonical 形式、use_count=1、有时间戳。
    let history = &runtime.config_document.quick_connect.history;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].target, "root@example.com:2200");
    assert_eq!(history[0].use_count, 1);
    assert!(history[0].last_used_at > 0);
    // 不写入 sessions 树（配置里没有新的已保存会话）。
    assert!(runtime.saved_session_profiles().is_empty());
    // 行投影：历史条以 recent 出现，带相对时间文案。
    assert!(projection
        .quick_connect_rows
        .iter()
        .any(|row| row.kind == "recent"
            && row.target == "root@example.com:2200"
            && !row.last_used_text.is_empty()));
    // 即时写回：磁盘上的配置已经有这条历史。
    assert_eq!(reload_config(temp.path()).quick_connect.history.len(), 1);
}

#[test]
fn failed_submit_clears_stale_last_target() {
    // 与设置 YSHELL_FAKE_AUTH_FAILURE 的测试串行，避免读到别的场景。
    let _guard = lock_env();
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);
    let _ = runtime
        .submit_quick_connect("ssh://root@example.com:2200")
        .expect("successful submit");
    assert_eq!(
        runtime.projection().quick_connect_last_target_text,
        "root@example.com:2200"
    );

    // D10：新的失败尝试不得让页面继续显示"Connected to …"。
    let (port, accept_handle) = start_tcp_probe_target();
    runtime.force_real_ssh_in_tests = true;
    let projection = runtime
        .submit_quick_connect(&format!("root@127.0.0.1:{port}"))
        .expect("failing submit is reported through the projection");
    accept_handle.join().expect("accept thread");

    assert_eq!(projection.tab_state_text, "failed");
    assert_eq!(projection.quick_connect_last_target_text, "");
    assert!(runtime.config_document.quick_connect.history.len() == 1);
}

#[test]
fn submit_invalid_target_sets_inline_error_without_tab() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);

    let projection = runtime
        .submit_quick_connect("bad host:notaport")
        .expect("submit");

    assert!(!projection.quick_connect_error_text.is_empty());
    assert!(projection.status_text.contains("Quick Connect error"));
    // 解析失败保持 New Tab 打开（方便直接改），不建会话、不记历史。
    assert!(projection.quick_connect_visible);
    assert_eq!(runtime.tabs.len(), 1);
    assert!(runtime.tabs[0].is_new_tab());
    assert!(runtime.config_document.quick_connect.history.is_empty());
    assert_eq!(projection.quick_connect_input_text, "bad host:notaport");

    // 重新输入会清掉错误态。
    let projection = runtime.update_quick_connect_input("root@example.com");
    assert_eq!(projection.quick_connect_input_text, "root@example.com");
    assert_eq!(projection.quick_connect_error_text, "");
}

#[test]
fn history_dedupes_truncates_and_clears() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);
    runtime.config_document.quick_connect.limit = 3;

    for host in [
        "a.example.test",
        "b.example.test",
        "c.example.test",
        "d.example.test",
    ] {
        runtime
            .submit_quick_connect(&format!("ops@{host}:22"))
            .expect("connect");
    }
    let history = &runtime.config_document.quick_connect.history;
    assert_eq!(history.len(), 3, "limit 生效");
    assert_eq!(history[0].target, "ops@d.example.test:22", "最新在前");

    // 去重 + 计数：重复提交已存在的目标不会新增条目。
    runtime
        .submit_quick_connect("ops@b.example.test:22")
        .expect("connect again");
    let history = &runtime.config_document.quick_connect.history;
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].target, "ops@b.example.test:22");
    assert_eq!(history[0].use_count, 2);

    // 清空 + 持久化。
    let projection = runtime.quick_connect_history_clear().expect("clear");
    assert!(projection.quick_connect_rows.is_empty());
    assert!(runtime.config_document.quick_connect.history.is_empty());
    assert!(reload_config(temp.path()).quick_connect.history.is_empty());
}

#[test]
fn disabled_history_is_not_recorded() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);
    runtime.config_document.quick_connect.enabled = false;

    let projection = runtime
        .submit_quick_connect("ops@example.com:22")
        .expect("connect");

    assert!(runtime.config_document.quick_connect.history.is_empty());
    assert!(!projection.quick_connect_history_enabled);
    assert!(projection.quick_connect_summary_text.contains("disabled"));
    // D37：历史关闭态没有模板（kind 为空 → UI 回退 `quick_connect_summary_text`）。
    assert_eq!(projection.quick_connect_summary_kind_text, "");
    // 连接本身不受影响。
    assert!(runtime.tabs[0].session_id().is_some());
}

#[test]
fn history_summary_projects_kind_and_params() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);
    runtime.config_document.quick_connect.limit = 3;

    for host in ["a.example.test", "b.example.test"] {
        runtime
            .submit_quick_connect(&format!("ops@{host}:22"))
            .expect("connect");
    }

    // D37：启用态由 `TextFormats.status-message` 按 kind + 条数/上限渲染。
    let projection = runtime.projection();
    assert_eq!(
        projection.quick_connect_summary_kind_text,
        "quick-connect-history"
    );
    assert_eq!(projection.quick_connect_summary_param_1_text, "2");
    assert_eq!(projection.quick_connect_summary_param_2_text, "3");
    assert_eq!(
        projection.quick_connect_summary_text,
        "2 recorded target(s); keep up to 3."
    );
}

#[test]
fn pin_and_remove_quick_links_persist() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);

    // 先造一个同目标的已保存会话：pin 的标签应使用会话名。
    runtime
        .handle_quick_connect("ops@example.com:2200")
        .expect("connect");
    runtime.save_active_session().expect("save");
    let saved_name = runtime.saved_session_profiles()[0].name.clone();

    let projection = runtime
        .quick_connect_pin("ssh://ops@example.com:2200")
        .expect("pin");
    assert_eq!(projection.quick_links_rows.len(), 1);
    assert_eq!(
        projection.quick_links_rows[0].target,
        "ops@example.com:2200"
    );
    assert_eq!(projection.quick_links_rows[0].label, saved_name);
    assert_eq!(projection.quick_links_rows[0].id, "quick-link-1");

    // 重复 pin 不新增。
    runtime
        .quick_connect_pin("ops@example.com:2200")
        .expect("pin again");
    assert_eq!(runtime.config_document.quick_links.len(), 1);

    // 即时写回。
    let reloaded = reload_config(temp.path());
    assert_eq!(reloaded.quick_links.len(), 1);
    assert_eq!(reloaded.quick_links[0].id, "quick-link-1");

    // 移除（按 id）。
    runtime.quick_link_remove("quick-link-1").expect("remove");
    assert!(runtime.config_document.quick_links.is_empty());
    assert!(reload_config(temp.path()).quick_links.is_empty());

    // 移除不存在的 id 只提示状态栏，不报错。
    let projection = runtime
        .quick_link_remove("missing")
        .expect("remove missing");
    assert!(projection.status_text.contains("was not found"));
}

#[test]
fn submit_invalid_input_keeps_the_new_tab_and_success_converts_it() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);
    // 启动新标签页。
    let opened = runtime.open_new_tab();
    assert!(opened.quick_connect_visible);
    assert_eq!(opened.tab_count, 1);

    // 解析失败：内联错误 + 标签保持 New Tab，不建会话。
    let invalid = runtime
        .submit_quick_connect("bad host:notaport")
        .expect("invalid submit");
    assert!(!invalid.quick_connect_error_text.is_empty());
    assert!(invalid.quick_connect_visible);
    assert_eq!(invalid.tab_count, 1);
    assert!(runtime.tabs[0].is_new_tab());

    // 成功：同一个 New Tab 原地变成 Terminal，标签数量不变。
    let connected = runtime
        .submit_quick_connect("ssh://root@example.com:2200")
        .expect("submit");
    assert!(!connected.quick_connect_visible);
    assert_eq!(connected.tab_count, 1);
    assert!(connected.has_active_session);
    assert_eq!(runtime.tabs[0].kind.mode_text(), "terminal");
    assert_tab_invariants(&runtime);
}

#[test]
fn open_and_close_new_tab_changes_tab_count_by_one() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 1);

    // 打开 New Tab。
    let opened = runtime.open_quick_connect();
    assert!(opened.quick_connect_visible);
    assert_eq!(opened.tab_count, 1);
    assert_eq!(runtime.tabs[0].kind.mode_text(), "new");

    // 重复打开只聚焦已有 New Tab，不新增。
    let reopened = runtime.open_quick_connect();
    assert!(reopened.quick_connect_visible);
    assert_eq!(reopened.tab_count, 1);

    // 打开终端标签后 New Tab 不再是活动标签。
    let tab = open_saved_tab(&mut runtime, "saved-0");
    assert!(!runtime.projection().quick_connect_visible);
    let before = runtime.tabs.len();
    let opened = runtime.open_quick_connect();
    assert!(opened.quick_connect_visible);
    assert_eq!(opened.tab_count, i32::try_from(before + 1).unwrap());
    assert_ne!(opened.active_tab_id, tab);
    let dismissed = runtime.dismiss_quick_connect();
    assert!(!dismissed.quick_connect_visible);
    assert_eq!(dismissed.tab_count, i32::try_from(before).unwrap());
    assert_tab_invariants(&runtime);

    // toggle（Ctrl+O）同样打开/关闭 New Tab。
    assert!(runtime.toggle_quick_connect().quick_connect_visible);
    let toggled_off = runtime.toggle_quick_connect();
    assert!(!toggled_off.quick_connect_visible);
    assert_eq!(runtime.tabs.len(), before);
}

#[test]
fn closing_the_last_new_tab_leaves_an_empty_workspace_until_reopened() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 1);

    let opened = runtime.open_quick_connect();
    assert!(opened.quick_connect_visible);
    let closed = runtime.dismiss_quick_connect();
    assert!(!closed.quick_connect_visible);
    assert_eq!(closed.tab_count, 0);
    assert!(closed.tabs.is_empty());
    assert_eq!(closed.active_tab_id, "");
    assert!(!closed.has_active_session);

    // Ctrl+O 仍可重新打开新标签页。
    let reopened = runtime.open_quick_connect();
    assert!(reopened.quick_connect_visible);
    assert_eq!(reopened.tab_count, 1);
}

#[test]
fn new_tab_lives_in_the_focused_split_pane() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 2);
    let _first = open_saved_tab(&mut runtime, "saved-0");
    let second = open_saved_tab(&mut runtime, "saved-1");

    // 左右分屏（被拖侧 = 右叶并获得焦点）。
    let split = runtime
        .place_tab_split(&second, 0, "right")
        .expect("split right");
    assert!(split.workspace_split.active);
    assert_eq!(split.workspace_split.leaves.len(), 2);
    let left = split.workspace_split.leaves[0].pane_id;
    let right = split.workspace_split.leaves[1].pane_id;

    let opened = runtime.open_quick_connect();
    assert!(opened.quick_connect_visible);
    let focused = opened
        .workspace_split
        .leaves
        .iter()
        .find(|leaf| leaf.focused)
        .expect("focused leaf");
    assert_eq!(focused.pane_id, right);
    assert_eq!(focused.mode_text, "new");
    assert_eq!(opened.tab_count, 3);

    // 聚焦左叶：右叶仍是 New Tab 模式（按叶子独立渲染）。
    let moved = runtime.focus_split_pane(left).expect("focus left pane");
    let right_leaf = moved
        .workspace_split
        .leaves
        .iter()
        .find(|leaf| leaf.pane_id == right)
        .expect("right leaf");
    assert_eq!(right_leaf.mode_text, "new");
    assert_eq!(moved.tab_count, 3);

    // 回到右叶并关闭 New Tab：分屏和两个终端标签保留。
    let moved = runtime.focus_split_pane(right).expect("focus right pane");
    assert!(moved.quick_connect_visible);
    let dismissed = runtime.dismiss_quick_connect();
    assert!(!dismissed.quick_connect_visible);
    assert!(dismissed.workspace_split.active);
    assert_eq!(dismissed.tab_count, 2);
    assert!(dismissed
        .workspace_split
        .leaves
        .iter()
        .all(|leaf| leaf.mode_text != "new"));
    assert_eq!(runtime.sessions.len(), 2);
}

#[test]
fn underlying_terminal_stays_alive_while_the_overlay_is_open() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 1);
    let tab = open_saved_tab(&mut runtime, "saved-0");
    let session = runtime.tabs[0].session_id().expect("session").to_owned();
    runtime
        .send_active_terminal_input("keep-alive\n")
        .expect("send input");
    assert!(runtime.sessions[&session]
        .visible_text()
        .contains("keep-alive"));

    runtime.open_quick_connect();
    let overlayed = runtime.projection();
    assert!(overlayed.quick_connect_visible);
    runtime.dismiss_quick_connect();

    // 会话没被重建/清空：输出仍在，活动标签/会话不变。
    assert!(runtime.sessions.contains_key(&session));
    assert!(runtime.sessions[&session]
        .visible_text()
        .contains("keep-alive"));
    assert_eq!(runtime.active_tab_id.as_deref(), Some(tab.as_str()));
    assert_eq!(runtime.active_session_id.as_deref(), Some(session.as_str()));
    assert_tab_invariants(&runtime);
}

#[test]
fn saved_session_connect_adds_saved_row_and_history() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 1);

    runtime.open_saved_session("saved-0").expect("open saved");

    let projection = runtime.projection();
    let saved_row = projection
        .quick_connect_rows
        .iter()
        .find(|row| row.kind == "saved")
        .expect("saved row");
    assert_eq!(saved_row.title, "Session 0");
    assert_eq!(saved_row.target, "host0.example.test:22");
    assert!(!saved_row.last_used_text.is_empty());
    // 同一目标在历史里也有一条（行投影按目标去重，saved 优先显示）。
    assert!(runtime
        .config_document
        .quick_connect
        .history
        .iter()
        .any(|entry| entry.target == "host0.example.test:22"));
    assert_eq!(
        projection
            .quick_connect_rows
            .iter()
            .filter(|row| row.target == "host0.example.test:22")
            .count(),
        1
    );
}

#[test]
fn save_as_session_prefills_editor() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);

    let projection = runtime
        .start_quick_connect_session_editor("ssh://bob@example.net:2222")
        .expect("prefill editor");

    assert!(projection.editor_modal_visible);
    assert_eq!(runtime.editor.host, "example.net");
    assert_eq!(runtime.editor.port_text, "2222");
    assert_eq!(runtime.editor.username, "bob");
    assert_eq!(projection.editor_host_text, "example.net");
    assert_eq!(projection.editor_port_text, "2222");
    assert_eq!(projection.editor_username_text, "bob");
}
