//! N9: input sync — bidirectional group model (no source/target roles),
//! membership fan-out, pause/stop and the AI/MCP opt-out.

use tempfile::tempdir;
use yshell_core::SessionState;

use super::*;

/// 打开 `count` 个已保存会话标签（测试壳 → 全部 connected），返回
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

fn join_group(runtime: &mut AppRuntime, tab_id: &str) {
    runtime
        .toggle_tab_receives_key_input(tab_id)
        .expect("join sync group");
}

#[test]
fn enabling_sync_adds_the_focused_session_to_the_group() {
    let (_temp, mut runtime, tabs, _sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate terminal");

    // 组模型：第一个加入的会话即创建组（不再有 “no targets” 错误）。
    let projection = runtime
        .start_input_sync_mode("toggle")
        .expect("enable sync");
    assert!(projection.input_sync_active);
    assert_eq!(projection.input_sync_mode_text, "group");
    assert_eq!(projection.input_sync_target_count, 0);
    assert!(projection.terminal_sync_stop_enabled);
    assert!(projection.terminal_sync_all_enabled);
    // `terminal_sync_visible_enabled` = 焦点会话已在组内（菜单勾选/文案）。
    assert!(projection.terminal_sync_visible_enabled);
    assert_eq!(projection.status_kind, "input-sync-member-on");
    assert_eq!(projection.tabs[0].sync_role_text, "sync");
    assert_eq!(projection.tabs[1].sync_role_text, "");
    // 组模型没有“源会话”投影。
    assert_eq!(projection.input_sync_source_name_text, "");
    assert_eq!(runtime.input_sync.members.len(), 1);
    assert!(runtime.input_sync.is_member(&tabs[0]));
}

#[test]
fn group_members_broadcast_bidirectionally_in_member_order() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate first");
    for tab in &tabs {
        join_group(&mut runtime, tab);
    }
    let projection = runtime.projection();
    assert_eq!(projection.input_sync_target_count, 2);

    // 焦点在成员 0：写入 → 其它两个成员各收到一次。
    runtime
        .send_active_terminal_input("first\n")
        .expect("send first");
    runtime
        .send_active_terminal_input("second\n")
        .expect("send second");
    for session in &sessions[1..] {
        let text = target_text(&runtime, session);
        assert_eq!(text.matches("received input: first").count(), 1);
        assert_eq!(text.matches("received input: second").count(), 1);
        assert!(text.find("first") < text.find("second"), "order preserved");
    }

    // 焦点换到另一个成员：同样广播回其余成员（双向，无源/目的角色）。
    runtime.activate_tab(&tabs[1]).expect("activate member 1");
    runtime
        .send_active_terminal_input("from-member-one\n")
        .expect("send from member 1");
    assert!(target_text(&runtime, &sessions[0]).contains("received input: from-member-one"));
    assert!(target_text(&runtime, &sessions[2]).contains("received input: from-member-one"));
}

#[test]
fn input_from_a_non_member_is_not_broadcast() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate first");
    join_group(&mut runtime, &tabs[0]);
    join_group(&mut runtime, &tabs[1]);

    // 焦点落在非成员 tab2：输入只写给 tab2，不进组。
    runtime.activate_tab(&tabs[2]).expect("activate non-member");
    let before_first = target_text(&runtime, &sessions[0]);
    let before_second = target_text(&runtime, &sessions[1]);
    runtime
        .send_active_terminal_input("local-only\n")
        .expect("type on non-member");
    assert_eq!(target_text(&runtime, &sessions[0]), before_first);
    assert_eq!(target_text(&runtime, &sessions[1]), before_second);
    assert!(target_text(&runtime, &sessions[2]).contains("received input: local-only"));
}

#[test]
fn pause_keeps_the_group_but_stops_broadcast_until_resume() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate first");
    for tab in &tabs {
        join_group(&mut runtime, tab);
    }

    let paused = runtime.start_input_sync_mode("pause").expect("pause");
    assert!(paused.input_sync_active, "paused group keeps its members");
    assert!(runtime.input_sync.paused);
    assert_eq!(paused.input_sync_mode_text, "paused");
    assert_eq!(paused.status_kind, "input-sync-paused");
    assert_eq!(runtime.input_sync.members.len(), 3);

    let before = target_text(&runtime, &sessions[1]);
    runtime
        .send_active_terminal_input("while-paused\n")
        .expect("send while paused");
    assert_eq!(
        target_text(&runtime, &sessions[1]),
        before,
        "paused sync must not copy input"
    );

    let resumed = runtime.start_input_sync_mode("resume").expect("resume");
    assert_eq!(resumed.input_sync_mode_text, "group");
    assert_eq!(resumed.status_kind, "input-sync-resumed");
    runtime
        .send_active_terminal_input("after-resume\n")
        .expect("send after resume");
    assert!(target_text(&runtime, &sessions[1]).contains("received input: after-resume"));
}

#[test]
fn stop_clears_the_group_and_stops_broadcast() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate first");
    for tab in &tabs {
        join_group(&mut runtime, tab);
    }

    let stopped = runtime.stop_input_sync_command();
    assert!(!stopped.input_sync_active);
    assert_eq!(stopped.input_sync_mode_text, "");
    assert_eq!(stopped.input_sync_target_count, 0);
    assert!(!stopped.terminal_sync_stop_enabled);
    assert!(stopped.tabs.iter().all(|tab| tab.sync_role_text.is_empty()));
    assert_eq!(stopped.status_kind, "input-sync-stopped");

    let before = target_text(&runtime, &sessions[1]);
    runtime
        .send_active_terminal_input("after-stop\n")
        .expect("send after stop");
    assert_eq!(target_text(&runtime, &sessions[1]), before);
}

#[test]
fn closing_a_member_removes_it_and_keeps_the_rest_of_the_group() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate first");
    for tab in &tabs {
        join_group(&mut runtime, tab);
    }

    runtime.close_tab_immediate(&tabs[2]);
    let projection = runtime.projection();
    assert!(projection.input_sync_active);
    assert_eq!(projection.input_sync_target_count, 1);
    assert_eq!(projection.input_sync_notice_kind_text, "member-removed");
    assert!(projection
        .input_sync_notice_param_text
        .contains("Session 2"));

    runtime
        .send_active_terminal_input("still\n")
        .expect("send to remaining member");
    assert!(target_text(&runtime, &sessions[1]).contains("received input: still"));
}

#[test]
fn disconnecting_a_member_reconciles_the_group() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate first");
    for tab in &tabs {
        join_group(&mut runtime, tab);
    }

    let after_target = runtime
        .disconnect_tab_session(&tabs[1])
        .expect("disconnect member");
    assert!(after_target.input_sync_active);
    assert_eq!(after_target.input_sync_target_count, 1);
    assert_eq!(after_target.input_sync_notice_kind_text, "member-removed");
    assert_eq!(after_target.tabs[1].sync_role_text, "");
    let _ = &sessions;

    // 断开最后一个成员 → 组清空、chip 不再显示。
    runtime
        .disconnect_tab_session(&tabs[0])
        .expect("disconnect focus member");
    runtime
        .disconnect_tab_session(&tabs[2])
        .expect("disconnect last member");
    let projection = runtime.projection();
    assert!(!projection.input_sync_active);
    assert!(!runtime.input_sync.paused);
}

#[test]
fn poll_tick_removes_members_whose_connection_dropped() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate first");
    for tab in &tabs {
        join_group(&mut runtime, tab);
    }

    runtime
        .sessions
        .get_mut(&sessions[2])
        .expect("member session")
        .set_state(SessionState::Disconnected);
    let polled = runtime
        .poll_all_terminal_outputs()
        .expect("poll tick")
        .expect("sync reconciliation must refresh the projection");
    assert!(polled.input_sync_active);
    assert_eq!(polled.input_sync_target_count, 1);
    assert_eq!(polled.input_sync_notice_kind_text, "member-removed");
    assert!(!runtime.input_sync.is_member(&tabs[2]));
}

#[test]
fn ai_input_opts_the_target_session_out_of_sync() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(3);
    runtime.activate_tab(&tabs[0]).expect("activate first");
    for tab in &tabs {
        join_group(&mut runtime, tab);
    }

    let before_other = target_text(&runtime, &sessions[2]);
    // MCP/AI 写入 tab1：tab1 自动退出同步，且这次输入不广播给其它成员。
    runtime
        .mcp_send_terminal_input(&tabs[1], "ai-command", true)
        .expect("ai input");
    let projection = runtime.projection();
    assert!(!runtime.input_sync.is_member(&tabs[1]));
    assert_eq!(projection.tabs[1].sync_role_text, "");
    assert_eq!(projection.input_sync_notice_kind_text, "ai-input-opt-out");
    assert!(projection
        .input_sync_notice_param_text
        .contains("Session 1"));
    assert_eq!(projection.status_kind, "input-sync-ai-opt-out");
    assert_eq!(
        target_text(&runtime, &sessions[2]),
        before_other,
        "AI input must never be broadcast to group members"
    );
    // 其余成员仍在组内（只把该会话移出）。
    assert!(runtime.input_sync.is_member(&tabs[0]));
    assert!(runtime.input_sync.is_member(&tabs[2]));
}

#[test]
fn control_key_broadcast_sets_a_non_blocking_notice() {
    let (_temp, mut runtime, tabs, sessions) = sync_setup(2);
    runtime.activate_tab(&tabs[0]).expect("activate first");
    for tab in &tabs {
        join_group(&mut runtime, tab);
    }

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
    assert_eq!(
        target_text(&runtime, &sessions[1])
            .matches("received input:")
            .count(),
        before_writes + 1
    );

    // Enter / 普通粘贴不触发控制键提示（先清掉上一条 notice）。
    runtime.stop_input_sync_command();
    for tab in &tabs {
        join_group(&mut runtime, tab);
    }
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
    runtime.activate_tab(&tabs[0]).expect("activate first");
    for tab in &tabs {
        join_group(&mut runtime, tab);
    }

    let before_source = target_text(&runtime, &sessions[0]);
    let before_target = target_text(&runtime, &sessions[1]);
    let _ = runtime
        .send_active_terminal_key("", false, false, false, false)
        .expect("empty key");
    assert_eq!(target_text(&runtime, &sessions[0]), before_source);
    assert_eq!(target_text(&runtime, &sessions[1]), before_target);

    runtime
        .send_active_terminal_input("你好\n")
        .expect("commit");
    let text = target_text(&runtime, &sessions[1]);
    assert_eq!(text.matches("received input: 你好").count(), 1);
}

#[test]
fn unknown_action_and_disconnected_tab_are_rejected() {
    let (_temp, mut runtime, tabs, _sessions) = sync_setup(1);
    runtime.activate_tab(&tabs[0]).expect("activate");
    assert!(runtime.start_input_sync_mode("bogus").is_err());

    // 断开后不能加入同步组（标签右键/终端菜单会以 tooltip 说明原因）。
    runtime
        .disconnect_tab_session(&tabs[0])
        .expect("disconnect");
    assert!(runtime.toggle_tab_receives_key_input(&tabs[0]).is_err());
}

/// New Tab 是标签页，不破坏已有的同步组；关闭后组保持。
#[test]
fn new_tab_does_not_change_input_sync_state() {
    let (_temp, mut runtime, tabs, _sessions) = sync_setup(2);
    runtime.activate_tab(&tabs[0]).expect("activate first");
    for tab in &tabs {
        join_group(&mut runtime, tab);
    }

    let opened = runtime.open_quick_connect();
    assert!(opened.quick_connect_visible);
    assert_eq!(opened.tab_count, 3);
    assert!(opened.input_sync_active);
    // 焦点在 New Tab（非成员）时，chip 把全部组成员都算作目标。
    assert_eq!(opened.input_sync_target_count, 2);
    assert_tab_invariants(&runtime);

    let closed = runtime.dismiss_quick_connect();
    assert!(!closed.quick_connect_visible);
    assert_eq!(closed.tab_count, 2);
    assert!(closed.input_sync_active);
    assert_eq!(closed.input_sync_target_count, 1);
    assert_tab_invariants(&runtime);
}
