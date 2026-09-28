//! N9: key input sync — state machine, fan-out and invariants.

use tempfile::tempdir;
use yshell_core::SessionState;

use super::*;

/// 打开 `count` 个已保存会话标签（fake 后端 → 全部 connected），返回
/// `(tempdir, runtime, tab ids, session keys)`。
fn sync_setup(count: usize) -> (tempfile::TempDir, AppRuntime, Vec<String>, Vec<String>) {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), count);
    let mut tabs = Vec::new();
    let mut sessions = Vec::new();
    for index in 0..count {
        let tab = open_saved_tab(&mut runtime, &format!("saved-{index}"));
        let session = runtime.tabs[index]
            .session_id()
            .expect("terminal tab")
            .to_owned();
        tabs.push(tab);
        sessions.push(session);
    }
    (temp, runtime, tabs, sessions)
}

fn target_text(runtime: &AppRuntime, session: &str) -> String {
    runtime.sessions[session].visible_text()
}

#[test]
fn start_input_sync_all_collects_connected_targets_and_marks_roles() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate source");

    let projection = runtime.start_input_sync_mode("all").expect("start sync");

    assert!(projection.input_sync_active);
    assert_eq!(projection.input_sync_mode_text, "all");
    assert_eq!(projection.input_sync_target_count, 2);
    assert!(projection.terminal_sync_stop_enabled);
    assert!(projection.terminal_sync_all_enabled);
    assert!(projection.terminal_sync_visible_enabled);
    assert_eq!(projection.tabs[0].sync_role_text, "source");
    assert_eq!(projection.tabs[1].sync_role_text, "target");
    assert_eq!(projection.tabs[2].sync_role_text, "target");
    assert_eq!(projection.status_kind, "input-sync-started");
    assert!(projection.status_text.contains("Sending key input"));
    assert_eq!(projection.input_sync_source_name_text, "Session 0");

    // 不变量：源不在 targets；目标 = 其它全部已连接终端。
    assert_eq!(runtime.input_sync.source.as_deref(), Some(tabs[0].as_str()));
    assert!(!runtime.input_sync.targets.contains(&tabs[0]));
    for tab in &tabs[1..] {
        assert!(runtime.input_sync.targets.contains(tab));
    }
    let _ = sessions;
}

#[test]
fn visible_mode_matches_all_while_split_view_is_absent() {
    let (_temp, mut runtime, tabs, _sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate source");

    let projection = runtime
        .start_input_sync_mode("visible")
        .expect("start visible sync");

    // 当前无分屏：Visible 与 All 的目标集合一致（Slint 侧用 note 说明）。
    assert_eq!(projection.input_sync_mode_text, "visible");
    assert_eq!(projection.input_sync_target_count, 2);
    assert_eq!(projection.status_kind, "input-sync-started");
    assert_eq!(projection.status_param_2, "visible");
    assert_eq!(runtime.input_sync.targets.len(), 2);
}

#[test]
fn fanout_writes_every_target_once_in_source_order_without_feedback() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate source");
    runtime.start_input_sync_mode("all").expect("start sync");

    let source_projection = runtime
        .send_active_terminal_input("first\n")
        .expect("send first");
    assert!(source_projection.input_sync_active);
    runtime
        .send_active_terminal_input("second\n")
        .expect("send second");

    for session in &sessions[1..] {
        let text = target_text(&runtime, session);
        assert_eq!(
            text.matches("received input: first").count(),
            1,
            "target must receive each source write exactly once"
        );
        let first = text.find("received input: first").expect("first write");
        let second = text.find("received input: second").expect("second write");
        assert!(first < second, "fan-out order must follow source input order");
    }
    // 目标回显不再回灌源：源只看到自己的一次写入。
    let source_text = target_text(&runtime, &sessions[0]);
    assert_eq!(source_text.matches("received input: first").count(), 1);
}

#[test]
fn stop_input_sync_clears_state_and_stops_fanout() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate source");
    runtime.start_input_sync_mode("all").expect("start sync");

    let stopped = runtime.stop_input_sync_command();
    assert!(!stopped.input_sync_active);
    assert_eq!(stopped.input_sync_target_count, 0);
    assert!(!stopped.terminal_sync_stop_enabled);
    assert!(stopped
        .tabs
        .iter()
        .all(|tab| tab.sync_role_text.is_empty()));
    assert_eq!(stopped.status_kind, "input-sync-stopped");

    let before = target_text(&runtime, &sessions[1]);
    runtime
        .send_active_terminal_input("after-stop\n")
        .expect("send after stop");
    assert_eq!(
        target_text(&runtime, &sessions[1]),
        before,
        "stopped sync must not copy input"
    );
}

#[test]
fn closing_target_tab_removes_it_but_keeps_sync() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate source");
    runtime.start_input_sync_mode("all").expect("start sync");

    runtime.close_tab_immediate(&tabs[2]);
    let projection = runtime.projection();
    assert!(projection.input_sync_active);
    assert_eq!(projection.input_sync_target_count, 1);
    assert_eq!(projection.input_sync_notice_kind_text, "target-removed");
    assert!(projection.input_sync_notice_param_text.contains("Session 2"));
    assert!(projection
        .tabs
        .iter()
        .all(|tab| tab.title != "Session 2"));

    runtime
        .send_active_terminal_input("still\n")
        .expect("send to remaining target");
    assert!(target_text(&runtime, &sessions[1]).contains("received input: still"));
}

#[test]
fn closing_source_tab_stops_sync_with_notice() {
    let (_temp, mut runtime, tabs, _sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate source");
    runtime.start_input_sync_mode("all").expect("start sync");

    runtime.close_tab_immediate(&tabs[0]);
    let projection = runtime.projection();
    assert!(!projection.input_sync_active);
    assert_eq!(projection.input_sync_target_count, 0);
    assert_eq!(projection.input_sync_notice_kind_text, "source-closed");
    assert!(projection
        .tabs
        .iter()
        .all(|tab| tab.sync_role_text.is_empty()));
}

#[test]
fn disconnecting_target_or_source_reconciles_the_sync_state() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate source");
    runtime.start_input_sync_mode("all").expect("start sync");

    // 目标断开：立即移出，源与其它目标继续。
    let after_target = runtime
        .disconnect_tab_session(&tabs[1])
        .expect("disconnect target");
    assert!(after_target.input_sync_active);
    assert_eq!(after_target.input_sync_target_count, 1);
    assert_eq!(after_target.input_sync_notice_kind_text, "target-removed");
    assert_eq!(after_target.tabs[1].sync_role_text, "");
    let _ = &sessions;

    // 源断开：同步停止并留下可见提示。
    let after_source = runtime
        .disconnect_tab_session(&tabs[0])
        .expect("disconnect source");
    assert!(!after_source.input_sync_active);
    assert_eq!(after_source.input_sync_notice_kind_text, "source-disconnected");
}

#[test]
fn poll_tick_removes_targets_whose_connection_dropped() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate source");
    runtime.start_input_sync_mode("all").expect("start sync");

    runtime
        .sessions
        .get_mut(&sessions[2])
        .expect("target session")
        .set_state(SessionState::Disconnected);
    let polled = runtime
        .poll_all_terminal_outputs()
        .expect("poll tick")
        .expect("sync reconciliation must refresh the projection");
    assert!(polled.input_sync_active);
    assert_eq!(polled.input_sync_target_count, 1);
    assert_eq!(polled.input_sync_notice_kind_text, "target-removed");
    assert!(!runtime.input_sync.targets.contains(&tabs[2]));
}

#[test]
fn toggle_receive_key_input_switches_to_selected_mode() {
    let (_temp, mut runtime, tabs, _sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate source");
    runtime.start_input_sync_mode("all").expect("start sync");

    let off = runtime
        .toggle_tab_receives_key_input(&tabs[2])
        .expect("toggle off");
    assert_eq!(off.input_sync_mode_text, "selected");
    assert_eq!(off.input_sync_target_count, 1);
    assert_eq!(off.tabs[2].sync_role_text, "");
    assert_eq!(off.tabs[1].sync_role_text, "target");
    assert_eq!(off.status_kind, "input-sync-receive-toggled");
    assert_eq!(off.status_param_2, "off");

    let on = runtime
        .toggle_tab_receives_key_input(&tabs[2])
        .expect("toggle on");
    assert_eq!(on.input_sync_target_count, 2);
    assert_eq!(on.tabs[2].sync_role_text, "target");
    assert_eq!(on.status_param_2, "on");

    // 源不能接收自己的输入；未同步时勾选报错（UI 侧禁用 + tooltip 说明）。
    assert!(runtime.toggle_tab_receives_key_input(&tabs[0]).is_err());
    runtime.stop_input_sync_command();
    assert!(runtime.toggle_tab_receives_key_input(&tabs[2]).is_err());
}

#[test]
fn control_key_broadcast_sets_non_blocking_notice() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(2);
    runtime.activate_tab(&tabs[0]).expect("activate source");
    runtime.start_input_sync_mode("all").expect("start sync");

    let before_writes = target_text(&runtime, &sessions[1])
        .matches("received input:")
        .count();
    let ctrl_c = runtime
        .send_active_terminal_key("c", true, false, false, false)
        .expect("ctrl+c");
    assert_eq!(ctrl_c.input_sync_notice_kind_text, "control-broadcast");
    assert_eq!(ctrl_c.input_sync_notice_param_text, "Ctrl+C");
    // D4：按键本身不写状态文案（提示只在 chip 上，不弹窗）。
    assert!(!ctrl_c.status_text.contains("Sent"));
    // 控制字节（0x03）会被终端解析器当控制字符丢弃，但 shell 侧确实收到了一次写入。
    assert_eq!(
        target_text(&runtime, &sessions[1])
            .matches("received input:")
            .count(),
        before_writes + 1
    );

    // bootstrap 的 Ctrl+C 无选区路径：控制字符（ctrl 标志为 false）也要识别。
    let control_byte = runtime
        .send_active_terminal_key("\u{3}", false, false, false, false)
        .expect("control byte");
    assert_eq!(control_byte.input_sync_notice_param_text, "Ctrl+C");

    let normal = runtime
        .send_active_terminal_key("x", false, false, false, false)
        .expect("normal key");
    assert_eq!(normal.input_sync_notice_kind_text, "control-broadcast");

    // Enter / 普通粘贴不触发控制键提示。
    runtime.stop_input_sync_command();
    runtime.start_input_sync_mode("all").expect("restart sync");
    let enter = runtime
        .send_active_terminal_key("\u{000a}", false, false, false, false)
        .expect("enter");
    assert!(enter.input_sync_notice_kind_text.is_empty());
    let pasted = runtime
        .send_active_terminal_input("plain paste\n")
        .expect("paste");
    assert!(pasted.input_sync_notice_kind_text.is_empty());
}

#[test]
fn ime_commit_text_is_sent_once_and_empty_keys_are_noops() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(2);
    runtime.activate_tab(&tabs[0]).expect("activate source");
    runtime.start_input_sync_mode("all").expect("start sync");

    // 组合期间的按键不会到达汇聚点（Slint 只在提交后给文本）；空文本 = 无操作。
    let before_source = target_text(&runtime, &sessions[0]);
    let before_target = target_text(&runtime, &sessions[1]);
    let _ = runtime
        .send_active_terminal_key("", false, false, false, false)
        .expect("empty key");
    assert_eq!(target_text(&runtime, &sessions[0]), before_source);
    assert_eq!(target_text(&runtime, &sessions[1]), before_target);

    // 提交后的整段文本按一次写入同步。
    runtime
        .send_active_terminal_input("你好\n")
        .expect("commit");
    let text = target_text(&runtime, &sessions[1]);
    assert_eq!(text.matches("received input: 你好").count(), 1);
}

#[test]
fn input_to_non_source_tab_is_not_broadcast() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate source");
    runtime.start_input_sync_mode("all").expect("start sync");

    // 切到目标标签（T2）打字：只写给 T2，不广播给 T3。
    runtime.activate_tab(&tabs[1]).expect("activate target");
    let before_third = target_text(&runtime, &sessions[2]);
    runtime
        .send_active_terminal_input("direct-only\n")
        .expect("type on target");
    assert!(target_text(&runtime, &sessions[1]).contains("received input: direct-only"));
    assert_eq!(
        target_text(&runtime, &sessions[2]),
        before_third,
        "input typed on a target tab must not be re-broadcast"
    );

    // 切回源标签：广播恢复。
    runtime.activate_tab(&tabs[0]).expect("activate source again");
    runtime
        .send_active_terminal_input("broadcast-again\n")
        .expect("type on source");
    assert!(target_text(&runtime, &sessions[2]).contains("received input: broadcast-again"));
}

#[test]
fn no_targets_start_is_reported_without_ghost_sync() {
    let (_temp, mut runtime, tabs, _sessions) = sync_setup(1);
    runtime.activate_tab(&tabs[0]).expect("activate source");

    let projection = runtime.start_input_sync_mode("all").expect("start sync");
    assert!(!projection.input_sync_active);
    assert_eq!(projection.input_sync_target_count, 0);
    assert_eq!(projection.status_kind, "input-sync-no-targets");
    assert!(runtime.input_sync.source.is_none());
    assert!(runtime.start_input_sync_mode("bogus").is_err());
}
