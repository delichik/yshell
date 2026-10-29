//! NET-1：代理管理器生命周期、引用删除（D8）与后台测试动作。

use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    thread,
    time::Duration,
};

use tempfile::tempdir;
use yshell_config::{
    ConfigDocument, ConfigStore, FolderProfile, ProxyProfile, ProxyProtocol, SessionProfile,
};
use yshell_secret::{FakeKeychain, Keychain};

use super::*;

fn proxy_profile(id: &str, name: &str, host: &str, port: u16) -> ProxyProfile {
    ProxyProfile {
        id: id.to_owned(),
        name: name.to_owned(),
        protocol: ProxyProtocol::Socks5,
        host: host.to_owned(),
        port,
        username: Some("proxy-user".to_owned()),
        resolve_dns_by_proxy: true,
        password_secret_key: None,
    }
}

fn store_with_proxies(
    config_dir: &std::path::Path,
    proxies: Vec<ProxyProfile>,
    sessions: Vec<SessionProfile>,
) -> ConfigStore {
    let store = ConfigStore::new(config_dir);
    let mut document = ConfigDocument::default();
    for proxy in proxies {
        document.proxy_profiles.insert(proxy.id.clone(), proxy);
    }
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions = sessions;
    document.folders.push(folder);
    store.save_all(&document).expect("save config");
    store
}

#[test]
fn proxy_manager_create_edit_duplicate_delete() {
    let _guard = lock_env();
    std::env::set_var("YSHELL_MASTER_PASSWORD", "proxy-manager-master");
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let opened = runtime.open_proxies_manager();
    assert!(opened.proxies_modal_visible);
    assert!(opened.proxies_editor_is_new);

    let _ = runtime.start_new_proxy();
    let _ = runtime.update_proxy_editor_name("Corp SOCKS");
    let _ = runtime.update_proxy_editor_host("127.0.0.1");
    let _ = runtime.update_proxy_editor_port("1080");
    let _ = runtime.update_proxy_editor_username("proxy-user");
    let _ = runtime.update_proxy_editor_password("proxy-secret");
    let projection = runtime.save_proxy_editor();
    assert!(!projection.proxies_modal_visible, "Save closes the proxies dialog");
    assert_eq!(runtime.config_document.proxy_profiles.len(), 1);
    assert_eq!(projection.proxies_selected_index, 0);
    assert_eq!(projection.proxies_rows.len(), 1);
    assert_eq!(projection.proxies_rows[0].name, "Corp SOCKS");
    assert_eq!(projection.proxies_rows[0].address, "127.0.0.1:1080");
    assert!(projection.proxies_rows[0].password_stored);
    assert_eq!(projection.proxies_editor_id_text, "proxy-1");
    let secret_key = runtime
        .config_document
        .proxy_profiles
        .get("proxy-1")
        .and_then(|profile| profile.password_secret_key.clone())
        .expect("password secret ref");
    assert_eq!(
        runtime
            .resolve_secret_value(&secret_key)
            .expect("stored proxy password"),
        "proxy-secret"
    );
    let store_contents =
        std::fs::read_to_string(temp.path().join("secret-store.toml")).expect("read store");
    assert!(!store_contents.contains("proxy-secret"));

    // 编辑同一条：重新打开管理器，id 不变，引用会话立刻看到新值。
    let _ = runtime.open_proxies_manager();
    let _ = runtime.begin_edit_proxy();
    let _ = runtime.update_proxy_editor_name("Corp SOCKS (edited)");
    let _ = runtime.update_proxy_editor_host("10.0.0.5");
    let _ = runtime.update_proxy_editor_port("1081");
    let projection = runtime.save_proxy_editor();
    assert!(!projection.proxies_modal_visible);
    assert_eq!(runtime.config_document.proxy_profiles.len(), 1);
    assert_eq!(projection.proxies_rows[0].name, "Corp SOCKS (edited)");
    assert_eq!(projection.proxies_rows[0].address, "10.0.0.5:1081");
    assert_eq!(projection.proxies_editor_id_text, "proxy-1");

    // 复制：新 id + 复制口令。
    let _ = runtime.open_proxies_manager();
    let projection = runtime.duplicate_selected_proxy();
    assert_eq!(runtime.config_document.proxy_profiles.len(), 2);
    assert_eq!(projection.proxies_rows.len(), 2);
    let copy = runtime
        .config_document
        .proxy_profiles
        .values()
        .find(|profile| profile.id != "proxy-1")
        .expect("duplicated profile")
        .clone();
    assert_eq!(copy.name, "Corp SOCKS (edited) copy");
    let copy_secret = copy
        .password_secret_key
        .as_deref()
        .expect("copied password secret ref");
    assert_eq!(
        runtime
            .resolve_secret_value(copy_secret)
            .expect("copied password"),
        "proxy-secret"
    );

    // 删除未被引用的副本：先确认再删除。
    let copy_index = runtime
        .proxy_row_data()
        .iter()
        .position(|row| row.id == copy.id)
        .expect("copy row")
        as i32;
    let _ = runtime.select_proxy(copy_index);
    let projection = runtime.request_proxy_remove();
    assert!(!projection.proxies_remove_blocked);
    assert!(projection.proxies_remove_confirm_visible);
    let projection = runtime.confirm_proxy_remove();
    assert!(!projection.proxies_remove_confirm_visible);
    assert_eq!(runtime.config_document.proxy_profiles.len(), 1);
    assert!(!runtime
        .config_document
        .proxy_profiles
        .contains_key(&copy.id));

    std::env::remove_var("YSHELL_MASTER_PASSWORD");
}

#[test]
fn deleting_a_referenced_proxy_is_blocked_and_can_be_cleared_explicitly() {
    let temp = tempdir().expect("tempdir");
    let mut prod = SessionProfile::new("session-prod", "Prod", "prod.example.test");
    prod.proxy_profile_id = Some("proxy-1".to_owned());
    let mut staging = SessionProfile::new("session-staging", "Staging", "staging.example.test");
    staging.proxy_profile_id = Some("proxy-1".to_owned());
    let other = SessionProfile::new("session-direct", "Direct", "direct.example.test");
    store_with_proxies(
        temp.path(),
        vec![proxy_profile("proxy-1", "Shared SOCKS", "127.0.0.1", 1080)],
        vec![prod, staging, other],
    );

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.open_proxies_manager();
    let _ = runtime.select_proxy(0);
    let projection = runtime.request_proxy_remove();

    // D8：阻止删除，并列出引用会话；配置文件保持完整。
    assert!(projection.proxies_remove_blocked);
    assert!(!projection.proxies_remove_confirm_visible);
    assert!(projection.proxies_referencing_text.contains("Prod"));
    assert!(projection.proxies_referencing_text.contains("Staging"));
    assert_eq!(projection.proxies_status_kind_text, "proxies-remove-blocked");
    assert_eq!(projection.proxies_status_param_1_text, "Shared SOCKS");
    assert!(projection.proxies_status_param_2_text.contains("Prod"));
    assert!(projection.proxies_status_text.contains("Prod"));
    assert!(runtime
        .config_document
        .proxy_profiles
        .contains_key("proxy-1"));

    // 显式"设为无代理"：只清引用，条目保留。
    let projection = runtime.set_referencing_sessions_to_no_proxy();
    assert!(!projection.proxies_remove_blocked);
    assert_eq!(projection.proxies_rows[0].used_by, "");
    let sessions = runtime.saved_session_profiles();
    assert!(sessions
        .iter()
        .all(|session| session.proxy_profile_id.is_none()));

    // 现在可以正常确认删除。
    let projection = runtime.request_proxy_remove();
    assert!(!projection.proxies_remove_blocked);
    assert!(projection.proxies_remove_confirm_visible);
    let projection = runtime.confirm_proxy_remove();
    assert!(projection.proxies_rows.is_empty());
    assert!(runtime.config_document.proxy_profiles.is_empty());
}

#[test]
fn proxy_test_action_reports_success_and_failure() {
    // 最小 SOCKS5 stub：读取 greeting，回"无需认证"。
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind socks5 stub");
    let port = listener.local_addr().expect("local addr").port();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept probe");
        let mut header = [0u8; 2];
        socket.read_exact(&mut header).expect("greeting header");
        let mut methods = vec![0u8; usize::from(header[1])];
        socket.read_exact(&mut methods).expect("greeting methods");
        socket.write_all(&[0x05, 0x00]).expect("method reply");
    });

    // 关闭一个临时监听器，得到必然拒绝连接的端口。
    let closed = TcpListener::bind(("127.0.0.1", 0)).expect("bind closed port");
    let closed_port = closed.local_addr().expect("local addr").port();
    drop(closed);

    let temp = tempdir().expect("tempdir");
    store_with_proxies(
        temp.path(),
        vec![
            proxy_profile("proxy-broken", "A Broken", "127.0.0.1", closed_port),
            proxy_profile("proxy-ok", "B Working", "127.0.0.1", port),
        ],
        Vec::new(),
    );
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.open_proxies_manager();
    assert_eq!(runtime.proxy_row_data().len(), 2);

    // 成功路径（SOCKS5 协商，不阻塞 UI：worker + poll）。
    let ok_index = runtime
        .proxy_row_data()
        .iter()
        .position(|row| row.id == "proxy-ok")
        .expect("ok row") as i32;
    let _ = runtime.select_proxy(ok_index);
    let projection = runtime.test_selected_proxy();
    assert!(projection.proxies_test_in_flight);
    runtime.wait_for_proxy_test_result(Duration::from_secs(3));
    let projection = runtime.projection();
    assert!(!projection.proxies_test_in_flight);
    assert!(projection.proxies_test_result_text.contains("Test passed"));
    assert!(projection.proxies_test_result_text.contains("SOCKS5"));
    server.join().expect("socks5 stub");

    // 失败路径：端口拒绝连接，错误可读并回填到页面。
    let broken_index = runtime
        .proxy_row_data()
        .iter()
        .position(|row| row.id == "proxy-broken")
        .expect("broken row") as i32;
    let _ = runtime.select_proxy(broken_index);
    let _ = runtime.test_selected_proxy();
    runtime.wait_for_proxy_test_result(Duration::from_secs(3));
    let projection = runtime.projection();
    assert!(!projection.proxies_test_in_flight);
    assert!(projection.proxies_test_result_text.contains("Test failed"));
    assert!(projection
        .proxies_test_result_text
        .contains("cannot connect to proxy"));
}

#[test]
fn proxy_manager_open_close_and_password_input_hygiene() {
    let temp = tempdir().expect("tempdir");
    store_with_proxies(
        temp.path(),
        vec![proxy_profile("proxy-1", "SOCKS", "127.0.0.1", 1080)],
        Vec::new(),
    );
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("proxy-manager-master"));
    runtime.keychain = Some(RuntimeKeychain(keychain));
    let opened = runtime.open_proxies_manager();
    assert!(opened.proxies_modal_visible);
    assert_eq!(opened.proxies_selected_index, 0);
    assert!(!opened.proxies_editor_is_new);
    assert_eq!(opened.proxies_editor_name_text, "SOCKS");

    let _ = runtime.update_proxy_editor_password("typed-but-not-saved");
    let closed = runtime.close_proxies_manager();
    assert!(!closed.proxies_modal_visible);
    assert!(runtime.proxies_editor_password.is_empty());
}
