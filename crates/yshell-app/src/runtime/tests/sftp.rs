//! SFTP listing, selection, sorting, breadcrumbs and transfer-queue views.

use std::fs;
use tempfile::tempdir;
use yshell_sftp::{FsEntry, RemoteEditSession, TransferDirection};

use super::*;

#[test]
fn fake_backend_sftp_refresh_explains_the_missing_native_ssh_path() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");

    let projection = runtime.refresh_active_sftp_listing().expect("refresh sftp");

    assert!(projection.status_text.contains("SFTP unavailable"));
    assert_eq!(projection.sftp_listing_kind_text, "sync-fake-backend");
    assert_eq!(projection.sftp_session_status_kind_text, "unavailable");
    assert_eq!(
        projection.sftp_session_status_reason_text,
        "select-native-ssh"
    );
}

#[test]
fn sftp_lifecycle_tracks_backend_and_runtime_session_state() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    // S2/D26：桌面启动默认原生 SSH（不再有 desktop-startup 的 fake 占位）。
    let startup = runtime.prepare_desktop_startup_projection();
    assert_eq!(startup.transport_backend_text, "native-ssh");
    assert_eq!(startup.sftp_session_status_kind_text, "disconnected");
    assert_eq!(startup.sftp_listing_kind_text, "native-ssh-selected");

    // 后续用 fake 适配器验证 SFTP 生命周期（不触网；fake 仍是测试可选项）。
    let _ = runtime.select_fake_transport_backend();
    let connected = runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    assert_eq!(connected.sftp_session_status_kind_text, "unavailable");

    let native = runtime.select_native_ssh_transport_backend();
    assert_eq!(native.sftp_session_status_kind_text, "disconnected");
    assert_eq!(native.sftp_listing_kind_text, "sync-reconnect-native");

    let draft = runtime.handle_new_session();
    assert_eq!(draft.sftp_session_status_kind_text, "disconnected");
    assert_eq!(draft.sftp_listing_kind_text, "draft-disconnected");
}

#[test]
fn sftp_path_helpers_normalize_and_join() {
    assert_eq!(normalize_remote_path("/srv//logs").unwrap(), "/srv//logs");
    assert_eq!(parent_remote_path("/srv/logs"), "/srv");
    assert_eq!(parent_remote_path("/srv"), "/");
    assert_eq!(join_remote_path("/srv", "file.txt"), "/srv/file.txt");
    assert_eq!(join_remote_path("/", "file.txt"), "/file.txt");
}

#[test]
fn sftp_product_operation_inputs_are_projected() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let projection = runtime.set_sftp_operation_inputs(
        "C:/tmp/upload.txt",
        "/srv/source.txt",
        "renamed.txt",
        "755",
    );

    assert_eq!(projection.sftp_local_path_text, "C:/tmp/upload.txt");
    assert_eq!(projection.sftp_remote_target_text, "/srv/source.txt");
    assert_eq!(projection.sftp_secondary_target_text, "renamed.txt");
    assert_eq!(projection.sftp_permissions_text, "755");
}

#[test]
fn sftp_summary_counts_and_sort_kind_are_projected() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.sftp_entries = vec![
        FsEntry::directory("/srv/folder"),
        FsEntry::file("/srv/report.txt", 42),
    ];

    let projection = runtime.projection();
    assert_eq!(projection.sftp_item_count, 2);
    assert_eq!(projection.sftp_dir_count, 1);
    assert_eq!(projection.sftp_item_summary_text, "2 items · 1 dir");

    let sorted = runtime.sort_sftp_by("size");
    assert_eq!(sorted.status_kind, "sftp-sorted");
    assert_eq!(sorted.status_param_1, "size");
    assert_eq!(sorted.status_param_2, "ascending");
    assert!(sorted.status_text.contains("Sorted SFTP entries by Size ↑"));
}

#[test]
fn sftp_transfer_queue_is_visible_in_projection() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let initial = runtime.projection();
    assert!(initial.transfer_queue_empty);
    assert_eq!(initial.transfer_queue_rows_text, "");

    let upload_id = runtime.enqueue_sftp_transfer(
        TransferDirection::Upload,
        "C:/tmp/upload.txt".to_owned(),
        "/srv/upload.txt".to_owned(),
        Some(10),
    );
    let running = runtime.projection();
    assert!(!running.transfer_queue_empty);
    assert!(running.transfer_queue_rows_text.contains(&upload_id));
    assert!(running.transfer_queue_rows_text.contains("upload running"));
    assert!(running.transfer_queue_rows_text.contains("0%"));

    runtime.complete_sftp_transfer(&upload_id, 10);
    let completed = runtime.projection();
    assert!(completed
        .transfer_queue_rows_text
        .contains("upload completed 100%"));

    let download_id = runtime.enqueue_sftp_transfer(
        TransferDirection::Download,
        "/srv/download.txt".to_owned(),
        "C:/tmp/download.txt".to_owned(),
        None,
    );
    runtime.fail_sftp_transfer(&download_id, "network".to_owned());
    let failed = runtime.projection();
    assert!(failed.transfer_queue_rows_text.contains("download failed"));
    assert!(failed.transfer_queue_rows_text.contains("error=network"));
}

#[test]
fn fake_backend_gates_sftp_mutation_entry_points() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    runtime.set_sftp_operation_inputs("", "/srv/source.txt", "renamed.txt", "755");

    let mkdir = runtime.create_sftp_directory().expect("mkdir gate");
    assert!(mkdir.status_text.contains("native-ssh backend"));

    let rename = runtime.rename_sftp_path().expect("rename gate");
    assert!(rename.status_text.contains("native-ssh backend"));

    let chmod = runtime.chmod_sftp_path().expect("chmod gate");
    assert!(chmod.status_text.contains("native-ssh backend"));

    let remote_edit = runtime.start_sftp_remote_edit().expect("remote edit gate");
    assert!(remote_edit.status_text.contains("native-ssh backend"));
}

#[test]
fn sftp_remote_edit_session_is_visible_and_cancelable() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let local_temp_path = temp.path().join("remote-edit.txt");
    fs::write(&local_temp_path, b"draft").expect("write temp edit");

    runtime.remote_edit_session = Some(RemoteEditSession::new(
        "/srv/remote-edit.txt",
        local_temp_path.display().to_string(),
    ));
    let active = runtime.projection();
    assert!(active.sftp_remote_edit_active);
    assert!(active
        .sftp_remote_edit_remote_path_text
        .contains("/srv/remote-edit.txt"));
    assert!(active
        .sftp_remote_edit_local_path_text
        .contains(&local_temp_path.display().to_string()));

    let canceled = runtime
        .cancel_sftp_remote_edit()
        .expect("cancel remote edit");
    assert!(!canceled.sftp_remote_edit_active);
    assert_eq!(canceled.sftp_remote_edit_remote_path_text, "");
    assert!(!local_temp_path.exists());
}

#[test]
fn sftp_secondary_target_and_permissions_helpers_validate_inputs() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.sftp_path = "/srv".to_owned();

    runtime.set_sftp_operation_inputs("", "", "logs", "644");
    assert_eq!(
        runtime
            .sftp_secondary_target_path("missing target")
            .expect("relative target"),
        "/srv/logs"
    );

    runtime.set_sftp_operation_inputs("", "", "/var/logs", "0o755");
    assert_eq!(
        runtime
            .sftp_secondary_target_path("missing target")
            .expect("absolute target"),
        "/var/logs"
    );
    assert_eq!(parse_sftp_permissions("644").expect("644"), 0o644);
    assert_eq!(parse_sftp_permissions("0o755").expect("755"), 0o755);
    assert!(parse_sftp_permissions("888").is_err());
    assert!(parse_sftp_permissions("64").is_err());
}

#[test]
fn sftp_selection_sorting_and_breadcrumbs_follow_visible_rows() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.sftp_path = "/demo".to_owned();
    runtime.sftp_entries = vec![
        FsEntry::directory("/demo/sub"),
        FsEntry::directory("/demo/zeta"),
        demo_file("/demo/a.txt", 1_024),
        demo_file("/demo/b.txt", 4_096),
    ];

    let initial = runtime.projection();
    assert_eq!(initial.sftp_rows.len(), 4);
    assert!(initial.sftp_rows[0].is_dir);
    assert!(initial.sftp_rows[1].is_dir);
    assert_eq!(initial.sftp_selected_index, -1);
    assert_eq!(initial.sftp_item_summary_text, "4 items · 2 dirs");
    assert_eq!(initial.sftp_sort_column_text, "name");
    assert!(initial.sftp_sort_ascending);
    assert_eq!(initial.sftp_empty_text, "");
    assert_eq!(initial.sftp_rows[0].size_text, "");
    assert_eq!(initial.sftp_rows[0].permissions_text, "drwxr-xr-x");
    assert_eq!(initial.sftp_rows[2].kind_text, "File");
    assert_eq!(initial.sftp_rows[2].size_text, "1.0 KB");
    assert_eq!(initial.sftp_rows[2].modified_text, "2023-11-14 22:13");

    let selected = runtime.select_sftp_entry(1);
    assert_eq!(selected.sftp_selected_index, 1);
    assert_eq!(selected.sftp_selected_name_text, "zeta");
    assert_eq!(selected.sftp_selected_path_text, "/demo/zeta");
    assert_eq!(selected.sftp_selected_permissions_text, "755");

    // Flipping the direction keeps the same entry selected by path.
    let sorted = runtime.sort_sftp_by("name");
    assert_eq!(sorted.sftp_sort_column_text, "name");
    assert!(!sorted.sftp_sort_ascending);
    assert_eq!(sorted.sftp_selected_index, 0);
    assert_eq!(sorted.sftp_selected_name_text, "zeta");

    // Size sorting keeps directories first and preserves the selection.
    let by_size = runtime.sort_sftp_by("size");
    assert_eq!(by_size.sftp_sort_column_text, "size");
    assert!(by_size.sftp_sort_ascending);
    assert_eq!(
        by_size
            .sftp_rows
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        vec!["sub", "zeta", "a.txt", "b.txt"]
    );
    assert_eq!(by_size.sftp_selected_index, 1);
    assert_eq!(by_size.sftp_selected_name_text, "zeta");
    let bogus = runtime.sort_sftp_by("bogus").sftp_sort_column_text;
    assert_eq!(bogus, "size");
    assert!(runtime.status_text.contains("Unknown SFTP sort column"));

    // Breadcrumbs expose the clickable parents of the current path.
    let crumbs = runtime.projection().sftp_crumbs;
    assert_eq!(crumbs.len(), 2);
    assert_eq!(crumbs[0].label, "/");
    assert_eq!(crumbs[0].path, "/");
    assert_eq!(
        (crumbs[1].label.as_str(), crumbs[1].path.as_str()),
        ("demo", "/demo")
    );

    // Activating a directory navigates into it even without a live session.
    runtime.select_sftp_entry(0);
    let activated = runtime.activate_sftp_entry().expect("activate directory");
    assert_eq!(activated.sftp_path_text, "/demo/sub");
    assert_eq!(activated.sftp_crumbs.len(), 3);

    let navigated = runtime.open_sftp_crumb(2).expect("open crumb");
    assert_eq!(navigated.sftp_path_text, "/demo/sub");
    let root = runtime.open_sftp_crumb(0).expect("open root crumb");
    assert_eq!(root.sftp_path_text, "/");

    // Clearing the selection resets the selection-derived projection.
    let cleared = runtime.select_sftp_entry(-1);
    assert_eq!(cleared.sftp_selected_index, -1);
    assert_eq!(cleared.sftp_selected_name_text, "");
    assert_eq!(cleared.sftp_selected_path_text, "");
    assert_eq!(cleared.sftp_selected_permissions_text, "");
}

#[test]
fn sftp_hidden_toggle_filters_dotfiles_and_reports_empty_state() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.sftp_path = "/demo".to_owned();
    runtime.sftp_entries = vec![
        demo_file("/demo/.secret", 12),
        demo_file("/demo/readme.txt", 2_048),
    ];

    let hidden_off = runtime.projection();
    assert!(!hidden_off.sftp_show_hidden);
    assert_eq!(hidden_off.sftp_rows.len(), 1);
    assert_eq!(hidden_off.sftp_rows[0].name, "readme.txt");
    assert_eq!(hidden_off.sftp_item_summary_text, "1 item · 0 dirs");

    let shown = runtime.toggle_sftp_hidden_files();
    assert!(shown.sftp_show_hidden);
    assert!(shown.status_text.contains("now shown"));
    assert_eq!(shown.sftp_rows.len(), 2);
    assert_eq!(shown.sftp_rows[0].name, ".secret");

    // Selecting the visible dotfile and hiding hidden files clears it again.
    runtime.select_sftp_entry(0);
    assert_eq!(runtime.projection().sftp_selected_name_text, ".secret");
    let rehidden = runtime.toggle_sftp_hidden_files();
    assert!(!rehidden.sftp_show_hidden);
    assert!(rehidden.status_text.contains("now hidden"));
    assert_eq!(rehidden.sftp_selected_index, -1);
    assert_eq!(rehidden.sftp_rows.len(), 1);

    // A directory with only dotfiles explains why the list is empty.
    runtime.sftp_entries = vec![demo_file("/demo/.only", 1)];
    let empty = runtime.projection();
    assert!(empty.sftp_rows.is_empty());
    assert!(empty.sftp_empty_text.contains("Enable Hidden"));
    runtime.toggle_sftp_hidden_files();
    let revealed = runtime.projection();
    assert_eq!(revealed.sftp_rows.len(), 1);
    assert!(revealed.sftp_empty_text.is_empty());
}

#[test]
fn sftp_selection_operations_assemble_existing_operation_inputs() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    runtime.sftp_path = "/demo".to_owned();
    runtime.sftp_entries = vec![
        FsEntry::directory("/demo/sub"),
        demo_file("/demo/readme.txt", 2_048),
    ];
    runtime.select_sftp_entry(1);

    let rename = runtime
        .rename_sftp_selected("renamed.txt")
        .expect("rename gate");
    assert_eq!(runtime.sftp_remote_target, "/demo/readme.txt");
    assert_eq!(runtime.sftp_secondary_target, "renamed.txt");
    assert!(rename.status_text.contains("native-ssh backend"));

    let chmod = runtime.chmod_sftp_selected("0o600").expect("chmod gate");
    assert_eq!(runtime.sftp_remote_target, "/demo/readme.txt");
    assert_eq!(runtime.sftp_permissions, "0o600");
    assert!(chmod.status_text.contains("native-ssh backend"));

    let delete = runtime.delete_sftp_selected().expect("delete gate");
    assert!(delete.status_text.contains("native-ssh backend"));

    let created = runtime
        .create_sftp_folder_named("logs")
        .expect("mkdir gate");
    assert_eq!(runtime.sftp_secondary_target, "logs");
    assert!(created.status_text.contains("native-ssh backend"));

    let upload = runtime
        .upload_sftp_into_current("C:/tmp/upload.txt")
        .expect("upload gate");
    assert_eq!(runtime.sftp_local_path, "C:/tmp/upload.txt");
    assert!(runtime.sftp_remote_target.is_empty());
    assert!(upload.status_text.contains("native-ssh backend"));

    let download = runtime
        .download_sftp_selected("C:/tmp/download.txt")
        .expect("download gate");
    assert_eq!(runtime.sftp_local_path, "C:/tmp/download.txt");
    assert!(download.status_text.contains("native-ssh backend"));

    let edit = runtime.edit_sftp_selected().expect("edit gate");
    assert!(edit.status_text.contains("native-ssh backend"));

    // Empty inputs and a cleared selection are rejected before any backend work.
    let empty_name = runtime.rename_sftp_selected("   ").expect("empty rename");
    assert!(empty_name.status_text.contains("must not be empty"));
    let empty_folder = runtime
        .create_sftp_folder_named("")
        .expect("empty folder name");
    assert!(empty_folder.status_text.contains("must not be empty"));
    let empty_upload = runtime
        .upload_sftp_into_current("")
        .expect("empty upload path");
    assert!(empty_upload.status_text.contains("must not be empty"));

    runtime.select_sftp_entry(-1);
    let no_selection = runtime.delete_sftp_selected().expect("no selection");
    assert!(no_selection.status_text.contains("Select an SFTP entry"));
    let no_crumb = runtime.open_sftp_crumb(9).expect("missing crumb");
    assert!(no_crumb.status_text.contains("no longer available"));
}

#[test]
fn sftp_session_lifecycle_projects_status_parts() {
    let unavailable = SftpSessionLifecycle::Unavailable {
        reason: SftpUnavailableReason::SelectNativeSsh,
    };
    assert_eq!(unavailable.status_kind(), "unavailable");
    assert_eq!(unavailable.status_reason(), "select-native-ssh");
    assert_eq!(
        unavailable.legacy_status_text(),
        "SFTP unavailable: select the native-ssh backend before using SFTP"
    );

    let disconnected = SftpSessionLifecycle::Disconnected {
        session_key: Some("runtime-1".to_owned()),
    };
    assert_eq!(disconnected.status_kind(), "disconnected");
    assert_eq!(disconnected.status_session_key(), "runtime-1");
    assert_eq!(disconnected.status_detail(), "");

    let ready = SftpSessionLifecycle::Ready {
        session_key: "runtime-2".to_owned(),
    };
    assert_eq!(ready.status_kind(), "ready");
    assert_eq!(ready.status_session_key(), "runtime-2");

    let failed = SftpSessionLifecycle::Failed {
        session_key: None,
        reason: "socket closed".to_owned(),
    };
    assert_eq!(failed.status_kind(), "failed");
    assert_eq!(failed.status_detail(), "socket closed");
    assert_eq!(
        failed.legacy_status_text(),
        "SFTP session failed: socket closed"
    );
}

#[test]
fn sftp_listing_state_projects_kind_and_detail() {
    assert_eq!(SftpListingState::ConnectReal.kind_id(), "connect-real");
    assert_eq!(
        SftpListingState::EmptyDirectory.kind_id(),
        "empty-directory"
    );

    let rows = SftpListingState::Rows("[file] a.txt".to_owned());
    assert_eq!(rows.kind_id(), "rows");
    assert_eq!(rows.rows(), "[file] a.txt");

    let failed = SftpListingState::SyncFailed {
        session_key: "runtime-1".to_owned(),
        path: "/srv".to_owned(),
        error: "permission denied".to_owned(),
    };
    assert_eq!(failed.kind_id(), "sync-failed");
    assert_eq!(failed.session_key(), "runtime-1");
    assert_eq!(failed.path(), "/srv");
    assert_eq!(failed.error(), "permission denied");
}
