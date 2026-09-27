//! Unit tests for the runtime composition layer, split by feature domain.

use std::{
    fs, net::TcpListener, path::Path, path::PathBuf, sync::Mutex, sync::OnceLock, thread,
    time::Duration,
};
use yshell_config::{ConfigDocument, ConfigStore, FolderProfile, SessionProfile};
use yshell_sftp::FsEntry;

use super::*;

mod connection;
mod editor;
mod live;
mod logging_and_settings;
mod projection;
mod quick_connect;
mod session_auth;
mod sessions;
mod sftp;
mod tabs;
mod terminal;
mod trust_and_secrets;

pub(crate) fn demo_file(path: &str, size_bytes: u64) -> FsEntry {
    let mut entry = FsEntry::file(path, size_bytes);
    entry.modified = Some(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    entry
}

pub(crate) fn saved_session_tree_store(config_dir: &Path) -> ConfigStore {
    let store = ConfigStore::new(config_dir);
    let mut document = ConfigDocument::default();
    let mut root = FolderProfile::new(SAVED_SESSIONS_FOLDER_ID, SAVED_SESSIONS_FOLDER_NAME);
    root.sessions
        .push(SessionProfile::new("saved-one", "One", "one.example.test"));
    let mut prod = FolderProfile::new("folder-prod", "Prod");
    let mut database = SessionProfile::new("saved-prod-db", "Prod DB", "db.example.test");
    database.username = Some("ops".to_owned());
    prod.sessions.push(database);
    prod.sessions.push(SessionProfile::new(
        "saved-prod-cache",
        "Cache",
        "cache.example.test",
    ));
    root.folders.push(prod);
    document.folders.push(root);
    store.save(&document).expect("save config");
    store
}

pub(crate) fn start_tcp_probe_target() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind tcp probe target");
    let port = listener.local_addr().expect("listener addr").port();
    let handle = thread::spawn(move || {
        let _ = listener.accept();
    });
    (port, handle)
}

pub(crate) fn collect_log_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !root.exists() {
        return files;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            let Ok(entries) = fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.flatten() {
                stack.push(entry.path());
            }
        } else if path.is_file() {
            files.push(path);
        }
    }
    files
}

pub(crate) fn tab_test_store(config_dir: &Path, count: usize) -> ConfigStore {
    let store = ConfigStore::new(config_dir);
    let mut document = ConfigDocument::default();
    let mut folder = FolderProfile::new(SAVED_SESSIONS_FOLDER_ID, SAVED_SESSIONS_FOLDER_NAME);
    for index in 0..count {
        folder.sessions.push(SessionProfile::new(
            format!("saved-{index}"),
            format!("Session {index}"),
            format!("host{index}.example.test"),
        ));
    }
    document.folders.push(folder);
    store.save(&document).expect("save config");
    store
}

pub(crate) fn tab_runtime(config_dir: &Path, count: usize) -> AppRuntime {
    tab_test_store(config_dir, count);
    AppRuntime::new_with_keychain(config_dir.to_path_buf(), None).expect("runtime")
}

pub(crate) fn open_saved_tab(runtime: &mut AppRuntime, profile_id: &str) -> String {
    runtime
        .open_saved_session(profile_id)
        .expect("open saved session");
    runtime.active_tab_id.clone().expect("active tab")
}

pub(crate) fn queue_shell_output(runtime: &mut AppRuntime, session_key: &str, marker: &str) {
    let session = runtime
        .sessions
        .get_mut(session_key)
        .expect("runtime session");
    let shell = session.shell_session.as_mut().expect("attached shell");
    shell
        .write_input(format!("{marker}\n").as_bytes())
        .expect("queue shell output");
}

pub(crate) fn close_tab_confirmed(runtime: &mut AppRuntime, tab_id: &str) -> AppProjection {
    let pending = runtime.request_close_tab(tab_id).expect("request close");
    assert!(
        pending.close_tabs_confirm_visible,
        "an active connection must ask for confirmation"
    );
    runtime.confirm_close_tabs().expect("confirm close")
}

pub(crate) fn assert_tab_invariants(runtime: &AppRuntime) {
    match (&runtime.active_tab_id, &runtime.active_session_id) {
        (Some(tab_id), Some(session_id)) => {
            let tab = runtime
                .tabs
                .iter()
                .find(|tab| &tab.tab_id == tab_id)
                .expect("active tab must exist in tabs");
            assert_eq!(tab.session_id(), Some(session_id.as_str()));
        }
        (Some(tab_id), None) => {
            // N2：没有运行时会话的活动标签只能是快速连接页。
            assert!(
                runtime.tab_kind_is_quick_connect(tab_id),
                "active tab `{tab_id}` has no session but is not the Quick Connect page"
            );
        }
        (None, None) => {}
        (tab_id, session_id) => {
            panic!("active tab/session invariant broken: {tab_id:?} / {session_id:?}")
        }
    }
}

pub(crate) fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
