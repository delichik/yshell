//! Saved-session CRUD and the sidebar session tree.

use tempfile::tempdir;

use super::*;

#[test]
fn save_active_session_persists_minimal_profile() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");

    let projection = runtime.save_active_session().expect("save active session");

    assert!(projection
        .status_text
        .contains("Saved active runtime session"));
    assert_eq!(runtime.saved_session_profiles().len(), 1);
    assert_eq!(runtime.saved_session_profiles()[0].host, "example.com");
    assert_eq!(runtime.saved_session_profiles()[0].port, 2200);
}

#[test]
fn open_saved_session_reuses_saved_profile_path() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("ops@saved.example.test:2222")
        .expect("quick connect");
    runtime.save_active_session().expect("save active session");

    let profile_id = runtime.saved_session_profiles()[0].id.clone();
    let projection = runtime
        .open_saved_session(&profile_id)
        .expect("open saved session");

    assert!(projection
        .active_session_name_text
        .contains("saved.example.test"));
    assert_eq!(projection.active_session_kind_text, "session");
    assert!(projection
        .terminal_body_text
        .contains("Fake shell established"));
}

#[test]
fn saved_session_selection_rotates_and_opens_selected_profile() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("ops@one.example.test:2222")
        .expect("quick connect");
    runtime.save_active_session().expect("save active session");
    runtime
        .handle_quick_connect("ops@two.example.test:2223")
        .expect("quick connect");
    runtime.save_active_session().expect("save active session");

    let initial = runtime.projection();
    assert_eq!(initial.saved_session_selection_kind_text, "profile");
    assert!(initial
        .saved_session_selection_name_text
        .contains("two.example.test"));

    let previous = runtime.select_previous_saved_session();
    assert!(previous
        .saved_session_selection_name_text
        .contains("one.example.test"));
    assert!(previous
        .saved_session_inventory_rows_text
        .contains("> ops@one.example.test:2222"));

    let opened = runtime
        .open_selected_saved_session()
        .expect("open selected saved session");
    assert!(opened.active_session_name_text.contains("one.example.test"));
    assert!(!opened.recent_sessions_empty);
    assert!(opened
        .recent_sessions_rows_text
        .contains("one.example.test"));
}

#[test]
fn session_tree_flattens_folders_and_sessions_in_display_order() {
    let temp = tempdir().expect("tempdir");
    let _store = saved_session_tree_store(temp.path());
    let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let rows = runtime.projection().session_tree_rows;
    let outline = rows
        .iter()
        .map(|row| (row.kind.as_str(), row.depth, row.label.as_str()))
        .collect::<Vec<_>>();

    assert_eq!(
        outline,
        vec![
            ("folder", 0, SAVED_SESSIONS_FOLDER_NAME),
            ("session", 1, "One"),
            ("folder", 1, "Prod"),
            ("session", 2, "Prod DB"),
            ("session", 2, "Cache"),
        ]
    );
    // 文件夹 detail = 子树内可见会话数；会话 detail = user@host。
    assert_eq!(rows[0].detail, "3");
    assert_eq!(rows[2].detail, "2");
    assert_eq!(rows[1].detail, "one.example.test");
    assert_eq!(rows[3].detail, "ops@db.example.test");
    assert!(rows
        .iter()
        .filter(|row| row.kind == "folder")
        .all(|row| row.expanded));
    assert!(rows
        .iter()
        .filter(|row| row.kind == "session")
        .all(|row| !row.expanded));
    // hydrate 默认选中第一个已保存会话。
    assert!(rows.iter().any(|row| row.id == "saved-one" && row.selected));
}

#[test]
fn session_tree_collapse_hides_children_and_expand_restores_them() {
    let temp = tempdir().expect("tempdir");
    let _store = saved_session_tree_store(temp.path());
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let collapsed = runtime
        .toggle_saved_folder("folder-prod")
        .expect("toggle prod folder");
    assert_eq!(collapsed.session_tree_rows.len(), 3);
    let prod = collapsed
        .session_tree_rows
        .iter()
        .find(|row| row.id == "folder-prod")
        .expect("prod row");
    assert!(!prod.expanded);
    assert!(!collapsed
        .session_tree_rows
        .iter()
        .any(|row| row.id == "saved-prod-db" || row.id == "saved-prod-cache"));

    let root_collapsed = runtime
        .toggle_saved_folder(SAVED_SESSIONS_FOLDER_ID)
        .expect("collapse root folder");
    assert_eq!(root_collapsed.session_tree_rows.len(), 1);
    assert_eq!(
        root_collapsed.session_tree_rows[0].id,
        SAVED_SESSIONS_FOLDER_ID
    );
    assert!(!root_collapsed.session_tree_rows[0].expanded);
    assert_eq!(root_collapsed.session_tree_rows[0].detail, "3");

    let root_expanded = runtime
        .toggle_saved_folder(SAVED_SESSIONS_FOLDER_ID)
        .expect("expand root folder");
    assert_eq!(root_expanded.session_tree_rows.len(), 3);
    let all_expanded = runtime
        .toggle_saved_folder("folder-prod")
        .expect("expand prod folder");
    assert_eq!(all_expanded.session_tree_rows.len(), 5);

    // 未知文件夹 id 报错，且不改变已有折叠状态。
    let missing = runtime
        .toggle_saved_folder("folder-missing")
        .expect_err("toggle missing folder");
    assert!(missing.message.contains("was not found"));

    // 折叠是内存态：不写回配置，重新加载后默认全部展开。
    let reloaded = AppRuntime::new(temp.path().to_path_buf()).expect("runtime reload");
    assert_eq!(reloaded.projection().session_tree_rows.len(), 5);
}

#[test]
fn session_tree_search_keeps_hit_chains_and_folder_subtrees() {
    let temp = tempdir().expect("tempdir");
    let _store = saved_session_tree_store(temp.path());
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    // 命中会话：只保留它的祖先文件夹链。
    let session_hit = runtime.update_session_search("db.example");
    let outline = session_hit
        .session_tree_rows
        .iter()
        .map(|row| (row.kind.as_str(), row.id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        outline,
        vec![
            ("folder", SAVED_SESSIONS_FOLDER_ID),
            ("folder", "folder-prod"),
            ("session", "saved-prod-db"),
        ]
    );
    assert_eq!(session_hit.session_tree_rows[0].detail, "1");

    // 命中文件夹：整棵子树（含不匹配查询的子项）都显示。
    let folder_hit = runtime.update_session_search("Prod");
    let labels = folder_hit
        .session_tree_rows
        .iter()
        .map(|row| row.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec![SAVED_SESSIONS_FOLDER_NAME, "Prod", "Prod DB", "Cache"]
    );
    assert_eq!(folder_hit.session_tree_rows[0].detail, "2");

    // 进入搜索会展开全部：命中项不会藏在折叠的文件夹里。
    let collapsed = runtime
        .toggle_saved_folder(SAVED_SESSIONS_FOLDER_ID)
        .expect("collapse root folder");
    assert_eq!(collapsed.session_tree_rows.len(), 1);
    let _ = runtime.update_session_search("");
    let auto_expanded = runtime.update_session_search("Cache");
    assert!(auto_expanded
        .session_tree_rows
        .iter()
        .any(|row| row.id == "saved-prod-cache"));

    let cleared = runtime.update_session_search("");
    assert_eq!(cleared.session_tree_rows.len(), 5);
}

#[test]
fn session_tree_selection_switches_between_folder_and_session() {
    let temp = tempdir().expect("tempdir");
    let _store = saved_session_tree_store(temp.path());
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let session_selected = runtime.select_saved_session_by_id("saved-prod-db");
    assert!(session_selected.has_saved_selection);
    assert!(session_selected
        .session_tree_rows
        .iter()
        .any(|row| row.id == "saved-prod-db" && row.selected));
    assert!(!session_selected
        .session_tree_rows
        .iter()
        .any(|row| row.kind == "folder" && row.selected));

    let folder_selected = runtime.select_saved_session_by_id("folder-prod");
    assert!(!folder_selected.has_saved_selection);
    assert!(folder_selected
        .session_tree_rows
        .iter()
        .any(|row| row.id == "folder-prod" && row.selected));
    assert!(!folder_selected
        .session_tree_rows
        .iter()
        .any(|row| row.kind == "session" && row.selected));

    // 未知 id 不改变选择。
    let unknown = runtime.select_saved_session_by_id("saved-missing");
    assert!(unknown
        .session_tree_rows
        .iter()
        .any(|row| row.id == "folder-prod" && row.selected));

    // 双击文件夹 = 折叠；双击会话 = 选中并打开。
    let toggled = runtime
        .activate_saved_session_tree_node("folder-prod")
        .expect("activate folder");
    assert!(!toggled
        .session_tree_rows
        .iter()
        .any(|row| row.id == "saved-prod-db"));

    let opened = runtime
        .activate_saved_session_tree_node("saved-one")
        .expect("activate session");
    assert_eq!(opened.active_session_name_text, "One");
    assert!(opened
        .session_tree_rows
        .iter()
        .any(|row| row.id == "saved-one" && row.selected));

    let missing = runtime
        .activate_saved_session_tree_node("saved-missing")
        .expect_err("activate missing session");
    assert!(missing.message.contains("was not found"));
}

#[test]
fn selected_saved_session_can_be_updated_and_deleted() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("ops@one.example.test:2222")
        .expect("quick connect");
    let saved = runtime.save_active_session().expect("save active session");
    assert_eq!(saved.status_kind, "session-saved");
    assert!(saved.status_param_1.contains("one-example-test"));
    assert_eq!(saved.status_param_2, "1");
    runtime
        .handle_quick_connect("ops@two.example.test:2223")
        .expect("quick connect");
    runtime.save_active_session().expect("save active session");

    let _ = runtime.select_previous_saved_session();
    runtime
        .handle_quick_connect("ops@updated.example.test:2299")
        .expect("quick connect updated");

    let updated = runtime
        .update_selected_saved_session_from_active()
        .expect("update selected saved");
    assert!(updated.status_text.contains("Updated saved session"));
    assert!(runtime
        .saved_session_profiles()
        .iter()
        .any(|profile| profile.host == "updated.example.test" && profile.port == 2299));

    let deleted = runtime
        .delete_selected_saved_session()
        .expect("delete selected saved");
    assert!(deleted.status_text.contains("Deleted saved session"));
    assert_eq!(deleted.status_kind, "session-deleted");
    assert!(deleted.status_param_1.contains("one-example-test"));
    assert_eq!(runtime.saved_session_profiles().len(), 1);
}

#[test]
fn open_first_saved_session_uses_minimal_ui_path() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("ops@saved.example.test:2222")
        .expect("quick connect");
    runtime.save_active_session().expect("save active session");

    let projection = runtime
        .open_first_saved_session()
        .expect("open first saved session");

    assert!(projection
        .active_session_name_text
        .contains("saved.example.test"));
    assert_eq!(projection.tab_state_text, "connected");
    assert!(!projection.recent_sessions_empty);
    assert!(projection
        .recent_sessions_rows_text
        .contains("saved.example.test"));
}
