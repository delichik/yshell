//! Tab open/close/activate flows and the polling cursor.

use tempfile::tempdir;
use yshell_core::SessionState;

use super::*;

#[test]
fn tabs_open_activate_and_project_in_display_order() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 3);

    let first_tab = open_saved_tab(&mut runtime, "saved-0");
    let second_tab = open_saved_tab(&mut runtime, "saved-1");
    let projection = runtime.projection();

    assert_eq!(projection.tab_count, 2);
    assert_eq!(
        projection
            .tabs
            .iter()
            .map(|tab| tab.id.clone())
            .collect::<Vec<_>>(),
        vec![first_tab.clone(), second_tab.clone()]
    );
    assert_eq!(projection.active_tab_id, second_tab);
    assert_eq!(projection.tabs[0].title, "Session 0");
    assert_eq!(projection.tabs[1].title, "Session 1");
    assert!(projection.tabs.iter().all(|tab| tab.connected));
    assert_eq!(projection.tabs[0].state_text, "connected");
    assert!(!projection.tabs[0].active);
    assert!(projection.tabs[1].active);
    assert!(projection.has_active_session);
    assert_tab_invariants(&runtime);

    let activated = runtime.activate_tab(&first_tab).expect("activate tab");
    assert_eq!(activated.active_tab_id, first_tab);
    assert!(activated.tabs[0].active);
    assert!(!activated.tabs[1].active);
    assert_eq!(
        runtime.active_session_id.as_deref(),
        runtime.tabs[0].session_id()
    );
    assert_tab_invariants(&runtime);

    // §7 启用条件针对被右键的标签计算。
    let first_menu = runtime.prepare_tab_context_menu(&first_tab);
    assert!(first_menu.tab_menu_close_others_enabled);
    assert!(!first_menu.tab_menu_close_left_enabled);
    assert!(first_menu.tab_menu_close_right_enabled);
    assert!(first_menu.tab_menu_close_all_enabled);
    assert!(!first_menu.tab_menu_close_disconnected_enabled);
    let second_menu = runtime.prepare_tab_context_menu(&second_tab);
    assert!(second_menu.tab_menu_close_left_enabled);
    assert!(!second_menu.tab_menu_close_right_enabled);

    assert!(runtime.activate_tab("missing-tab").is_err());
    assert_eq!(runtime.prepare_tab_context_menu("missing-tab").tab_count, 2);
}

#[test]
fn duplicate_saved_session_opens_distinct_tabs_and_runtimes() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 2);

    let first_tab = open_saved_tab(&mut runtime, "saved-0");
    let first_session = runtime.tabs[0]
        .session_id()
        .expect("terminal tab")
        .to_owned();
    let second_tab = open_saved_tab(&mut runtime, "saved-0");
    let second_session = runtime.tabs[1]
        .session_id()
        .expect("terminal tab")
        .to_owned();

    assert_ne!(first_tab, second_tab);
    assert_ne!(first_session, second_session);
    assert_eq!(runtime.tabs.len(), 2);
    assert!(runtime.sessions.contains_key(&first_session));
    assert!(runtime.sessions.contains_key(&second_session));
    assert!(runtime.sessions[&first_session].shell_session.is_some());
    assert!(runtime.sessions[&second_session].shell_session.is_some());
    assert_tab_invariants(&runtime);

    // 关闭其中一个标签只清理它自己的运行时。
    let confirmed = close_tab_confirmed(&mut runtime, &first_tab);
    assert_eq!(confirmed.tab_count, 1);
    assert_eq!(confirmed.active_tab_id, second_tab);
    assert!(!runtime.sessions.contains_key(&first_session));
    assert!(runtime.sessions.contains_key(&second_session));
    assert_tab_invariants(&runtime);
}

#[test]
fn closing_active_tab_selects_right_neighbor_then_left() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 3);
    let first_tab = open_saved_tab(&mut runtime, "saved-0");
    let second_tab = open_saved_tab(&mut runtime, "saved-1");
    let third_tab = open_saved_tab(&mut runtime, "saved-2");

    // 关闭非活动标签：活动标签与显示顺序保持不变。
    let after_inactive = close_tab_confirmed(&mut runtime, &second_tab);
    assert_eq!(
        after_inactive
            .tabs
            .iter()
            .map(|tab| tab.id.clone())
            .collect::<Vec<_>>(),
        vec![first_tab.clone(), third_tab.clone()]
    );
    assert_eq!(after_inactive.active_tab_id, third_tab);

    // 关闭活动标签：优先右侧相邻。
    runtime.activate_tab(&first_tab).expect("activate first");
    let after_right = close_tab_confirmed(&mut runtime, &first_tab);
    assert_eq!(after_right.active_tab_id, third_tab);
    assert_eq!(after_right.tab_count, 1);

    // 没有右侧相邻时回退到左侧。
    let after_last = close_tab_confirmed(&mut runtime, &third_tab);
    assert_eq!(after_last.tab_count, 0);
    assert_eq!(after_last.active_tab_id, "");
    assert!(after_last.tabs.is_empty());
    // N2：没有标签时内容区回到 Quick Connect 页（不新建标签）。
    assert!(after_last.quick_connect_visible);
    assert!(!after_last.has_active_session);
    assert!(runtime.active_session_id.is_none());
    assert_tab_invariants(&runtime);
}

#[test]
fn close_request_requires_confirmation_and_cancel_keeps_tabs() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 1);
    let tab_id = open_saved_tab(&mut runtime, "saved-0");

    let pending = runtime.request_close_tab(&tab_id).expect("request close");
    assert!(pending.close_tabs_confirm_visible);
    assert!(pending.close_tabs_confirm_single);
    assert_eq!(pending.close_tabs_confirm_name_text, "Session 0");
    assert_eq!(pending.close_tabs_confirm_count, 1);
    assert_eq!(pending.close_tabs_confirm_active_count, 1);
    assert_eq!(
        pending.tab_count, 1,
        "nothing is closed before confirmation"
    );

    let canceled = runtime.cancel_close_tabs();
    assert!(!canceled.close_tabs_confirm_visible);
    assert_eq!(canceled.tab_count, 1);
    assert!(runtime.pending_close_tabs.is_none());

    let confirmed = close_tab_confirmed(&mut runtime, &tab_id);
    assert_eq!(confirmed.tab_count, 0);
    // N2：没有标签时内容区回到 Quick Connect 页（不新建标签）。
    assert!(confirmed.quick_connect_visible);
    assert!(confirmed.status_text.contains("Closed tab"));
    assert!(runtime.request_close_tab("missing-tab").is_err());
}

#[test]
fn idle_tab_closes_without_confirmation() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);

    let opened = runtime.handle_new_session();
    assert_eq!(opened.tab_count, 1);
    assert_eq!(opened.tabs[0].state_text, "idle");
    assert!(!opened.tabs[0].connected);
    assert!(opened.tab_has_disconnected);

    let closed = runtime
        .request_close_tab(&opened.tabs[0].id)
        .expect("close idle tab");
    assert!(!closed.close_tabs_confirm_visible);
    assert_eq!(closed.tab_count, 0);
    assert_eq!(closed.active_tab_id, "");
    // N2：没有标签时内容区回到 Quick Connect 页（不新建标签）。
    assert!(closed.quick_connect_visible);
    assert!(!closed.has_active_session);
    assert_tab_invariants(&runtime);
}

#[test]
fn batch_close_scopes_build_target_sets_and_keep_order() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 5);
    let mut tabs = Vec::new();
    for index in 0..5 {
        tabs.push(open_saved_tab(&mut runtime, &format!("saved-{index}")));
    }
    // 断开 B/D（非活动连接），用于验证 disconnected scope 与确认计数。
    for index in [1usize, 3] {
        let key = runtime.tabs[index]
            .session_id()
            .expect("terminal tab")
            .to_owned();
        runtime
            .sessions
            .get_mut(&key)
            .unwrap()
            .set_state(SessionState::Disconnected);
    }
    assert!(runtime.projection().tab_has_disconnected);

    // disconnected：目标集合 = B/D，无需确认直接关闭。
    let closed = runtime
        .request_close_tabs(CLOSE_SCOPE_DISCONNECTED)
        .expect("close disconnected tabs");
    assert!(!closed.close_tabs_confirm_visible);
    assert_eq!(
        closed
            .tabs
            .iter()
            .map(|tab| tab.id.clone())
            .collect::<Vec<_>>(),
        vec![tabs[0].clone(), tabs[2].clone(), tabs[4].clone()]
    );
    assert_eq!(closed.active_tab_id, tabs[4]);
    assert!(!closed.tab_has_disconnected);

    // 未知 scope / 未知引用标签报错。
    assert!(runtime.request_close_tabs("bogus").is_err());
    assert!(runtime.request_close_tabs("others:missing").is_err());

    // others:<C> = A/E 两个活动连接；left:<C> 为空；right:<A> = C/E。
    for (scope, expected_count) in [
        (format!("others:{}", tabs[2]), 2usize),
        (format!("right:{}", tabs[0]), 2usize),
        (format!("left:{}", tabs[2]), 1usize),
    ] {
        let pending = runtime.request_close_tabs(&scope).expect("scope");
        assert!(pending.close_tabs_confirm_visible);
        assert!(!pending.close_tabs_confirm_single);
        assert_eq!(
            pending.close_tabs_confirm_count,
            i32::try_from(expected_count).unwrap()
        );
        assert_eq!(
            pending.close_tabs_confirm_active_count,
            i32::try_from(expected_count).unwrap()
        );
        runtime.cancel_close_tabs();
    }

    // all 的目标集合为全部三个标签（右→左关闭顺序）。
    let pending_all = runtime.request_close_tabs(CLOSE_SCOPE_ALL).expect("all");
    assert_eq!(pending_all.close_tabs_confirm_count, 3);
    assert_eq!(pending_all.close_tabs_confirm_active_count, 3);
    let pending = runtime.pending_close_tabs.clone().expect("pending close");
    assert_eq!(
        pending.tab_ids,
        vec![tabs[4].clone(), tabs[2].clone(), tabs[0].clone()]
    );
    runtime.cancel_close_tabs();

    // 确认 right:<A>：按右→左关闭 C/E；活动 E 被关后回退到存活的左侧 A。
    let _ = runtime.request_close_tabs(&format!("right:{}", tabs[0]));
    let confirmed = runtime.confirm_close_tabs().expect("confirm batch");
    assert_eq!(confirmed.tab_count, 1);
    assert_eq!(confirmed.active_tab_id, tabs[0]);
    assert_tab_invariants(&runtime);
}

#[test]
fn batch_close_skips_already_closed_tabs_in_stable_order() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 4);
    let mut tabs = Vec::new();
    for index in 0..4 {
        tabs.push(open_saved_tab(&mut runtime, &format!("saved-{index}")));
    }

    let _ = runtime.request_close_tabs(CLOSE_SCOPE_ALL).expect("all");
    let pending = runtime.pending_close_tabs.clone().expect("pending close");
    assert_eq!(
        pending.tab_ids,
        vec![
            tabs[3].clone(),
            tabs[2].clone(),
            tabs[1].clone(),
            tabs[0].clone()
        ]
    );
    // 模拟一个目标标签已被其它路径关闭：确认时跳过，不 panic。
    runtime.close_tab_immediate(&tabs[2]);
    let confirmed = runtime.confirm_close_tabs().expect("confirm batch");
    assert_eq!(confirmed.tab_count, 0);
    // N2：没有标签时内容区回到 Quick Connect 页（不新建标签）。
    assert!(confirmed.quick_connect_visible);
    assert_eq!(confirmed.status_text, "Closed 3 tab(s).");
    assert_tab_invariants(&runtime);
}

#[test]
fn poll_all_prioritizes_active_tab_and_rotates_the_cursor() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 10);
    for index in 0..10 {
        open_saved_tab(&mut runtime, &format!("saved-{index}"));
    }
    let session_keys: Vec<String> = runtime
        .tabs
        .iter()
        .filter_map(|tab| tab.session_id().map(str::to_owned))
        .collect();
    for (index, session_key) in session_keys.iter().enumerate() {
        queue_shell_output(&mut runtime, session_key, &format!("marker-{index}"));
    }

    let projection = runtime
        .poll_all_terminal_outputs()
        .expect("poll tick")
        .expect("active output must refresh the projection");
    assert_eq!(projection.tab_count, 10);
    // 活动标签（最后一个）在游标轮转之前就被消费。
    assert!(runtime.sessions[&session_keys[9]]
        .visible_text()
        .contains("marker-9"));
    // 单 tick 上限 8：1 个活动 + 前 7 个后台标签。
    for (index, session_key) in session_keys.iter().enumerate().take(7) {
        assert!(
            runtime.sessions[session_key]
                .visible_text()
                .contains(&format!("marker-{index}")),
            "session {index} should be polled in the first tick"
        );
    }
    for (index, session_key) in session_keys.iter().enumerate().skip(7).take(2) {
        assert!(
            !runtime.sessions[session_key]
                .visible_text()
                .contains(&format!("marker-{index}")),
            "session {index} must wait for the next tick"
        );
    }
    assert_eq!(runtime.terminal_poll_cursor, 7);

    // 第二个 tick 补完剩余会话；只有后台输出时不再重投影。
    let second = runtime.poll_all_terminal_outputs().expect("second tick");
    assert!(second.is_none());
    for (index, session_key) in session_keys.iter().enumerate() {
        assert!(runtime.sessions[session_key]
            .visible_text()
            .contains(&format!("marker-{index}")));
    }
    assert_eq!(runtime.terminal_poll_cursor, 5);
}

#[test]
fn background_output_updates_grid_and_background_close_refreshes_tabs() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 2);
    open_saved_tab(&mut runtime, "saved-0");
    open_saved_tab(&mut runtime, "saved-1");
    let background_session = runtime.tabs[0]
        .session_id()
        .expect("terminal tab")
        .to_owned();

    queue_shell_output(&mut runtime, &background_session, "bg-only");
    let without_projection = runtime
        .poll_all_terminal_outputs()
        .expect("background poll");
    assert!(without_projection.is_none());
    assert!(runtime.sessions[&background_session]
        .visible_text()
        .contains("bg-only"));

    // 后台 shell 关闭：状态落到 disconnected，标签条投影需要刷新。
    runtime
        .sessions
        .get_mut(&background_session)
        .expect("background session")
        .shell_session
        .as_mut()
        .unwrap()
        .disconnect()
        .expect("disconnect background shell");
    let projection = runtime
        .poll_all_terminal_outputs()
        .expect("state poll")
        .expect("background state change must refresh the projection");
    assert_eq!(projection.tabs[0].state_text, "disconnected");
    assert!(!projection.tabs[0].connected);
    assert!(projection.tab_has_disconnected);
    // 活动标签不受影响。
    assert!(projection.tabs[1].connected);
    assert_eq!(projection.active_tab_id, runtime.tabs[1].tab_id);
}
