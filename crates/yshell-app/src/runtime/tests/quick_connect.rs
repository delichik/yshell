//! N2：Quick Connect 页（标签、历史、快速链接、行投影、`+` 行为）。

use tempfile::tempdir;
use yshell_config::NewTabMode;

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
fn submit_invalid_target_sets_inline_error_without_tab() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);

    let projection = runtime
        .submit_quick_connect("bad host:notaport")
        .expect("submit");

    assert!(!projection.quick_connect_error_text.is_empty());
    assert!(projection.status_text.contains("Quick Connect error"));
    // 解析失败不建标签、不记历史，输入保留（方便直接改）。
    assert!(runtime.tabs.is_empty());
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
    // 连接本身不受影响。
    assert!(runtime.tabs[0].session_id().is_some());
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
fn quick_connect_tab_open_activate_and_empty_state_fallback() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 1);

    // 启动落点：没有标签 → 内容区显示 QC 页，但**不新建标签**（"无标签则不新建"）。
    let startup = runtime.projection();
    assert!(startup.quick_connect_visible);
    assert_eq!(startup.tab_count, 0);

    // `+`（quick-connect 模式）/ Ctrl+O：显式打开 QC 标签。
    let projection = runtime.open_quick_connect_tab();
    assert!(projection.quick_connect_visible);
    assert_eq!(projection.tab_count, 1);
    assert_eq!(projection.tabs[0].kind_text, "quick-connect");
    assert!(projection.tabs[0].title.is_empty(), "标题由 Slint @tr 渲染");
    assert_tab_invariants(&runtime);
    let qc_tab = runtime.quick_connect_tab_id().expect("qc tab");

    // 打开终端标签：QC 页让位。
    let terminal_tab = open_saved_tab(&mut runtime, "saved-0");
    assert!(!runtime.projection().quick_connect_visible);
    assert_eq!(runtime.tabs.len(), 2);

    // 重复打开/激活：不重复新建。
    runtime.open_quick_connect_tab();
    assert!(runtime.projection().quick_connect_visible);
    assert_eq!(runtime.tabs.len(), 2);
    assert_tab_invariants(&runtime);

    // 关掉终端标签 → QC 标签成为活动标签。
    let confirmed = close_tab_confirmed(&mut runtime, &terminal_tab);
    assert!(confirmed.quick_connect_visible);
    assert_eq!(confirmed.tab_count, 1);
    assert_tab_invariants(&runtime);

    // 关掉 QC 标签本身：没有标签，内容区仍显示 QC 页（空态），不新建标签。
    let closed = runtime.request_close_tab(&qc_tab).expect("close qc");
    assert_eq!(closed.tab_count, 0);
    assert!(closed.quick_connect_visible);
    assert!(runtime.tabs.is_empty());
    assert_tab_invariants(&runtime);
}

#[test]
fn new_tab_default_follows_ui_mode() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);

    // 默认 quick-connect：`+` 打开 QC 页。
    let projection = runtime.handle_new_tab_default();
    assert!(projection.quick_connect_visible);
    assert!(!projection.editor_modal_visible);

    // session-editor：`+` 打开 Session Editor（Ctrl+N 同一条路径）。
    runtime.config_document.ui.new_tab_mode = NewTabMode::SessionEditor;
    let projection = runtime.handle_new_tab_default();
    assert!(projection.editor_modal_visible);
    // QC 标签仍是活动标签（编辑器只是模态弹窗），不重复新建标签。
    assert!(projection.quick_connect_visible);
    assert_eq!(projection.tab_count, 1);
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
