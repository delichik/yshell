//! Session editor flows: modal state, folder choice, tunnel/proxy persistence.

use std::{fs, sync::Arc};
use tempfile::tempdir;
use yshell_config::{
    AuthMethod as ConfigAuthMethod, AuthProfile, ConfigDocument, ConfigStore, FolderProfile,
    HostKeyPolicy as ConfigHostKeyPolicy, ProxyProfile, ProxyProtocol, QuickConnectTarget,
    SessionProfile, TunnelForward, TunnelForwardKind, TunnelProfile,
};
use yshell_secret::{FakeKeychain, Keychain, SecretRef, SecretString};
use yshell_ssh::{ForwardingKind, ProxyConfig};

use super::*;

#[test]
fn selected_saved_session_can_be_loaded_into_editor() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    document.auth_profiles.insert(
        "auth-agent".to_owned(),
        AuthProfile {
            id: "auth-agent".to_owned(),
            name: "Agent".to_owned(),
            method: ConfigAuthMethod::Agent,
        },
    );
    let mut profile = QuickConnectTarget {
        username: Some("ops".to_owned()),
        host: "editor.example.test".to_owned(),
        port: 2202,
    }
    .into_session_profile("saved-editor");
    profile.name = "Editor Session".to_owned();
    profile.auth_profile_id = Some("auth-agent".to_owned());
    profile.host_key_policy = Some(ConfigHostKeyPolicy::AcceptAnyForTesting);
    let mut folder = FolderProfile::new("folder-ops", "Ops");
    let mut nested = FolderProfile::new("folder-prod", "Prod");
    nested.sessions.push(profile);
    folder.folders.push(nested);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime
        .load_selected_saved_session_into_editor()
        .expect("load editor");

    assert_eq!(runtime.editor.name, "Editor Session");
    assert_eq!(runtime.editor.host, "editor.example.test");
    assert_eq!(runtime.editor.port_text, "2202");
    assert_eq!(runtime.editor.username, "ops");
    assert_eq!(runtime.editor.auth_method, EditorAuthMethod::Agent);
    assert_eq!(
        runtime.editor.host_key_policy,
        ConfigHostKeyPolicy::AcceptAnyForTesting
    );
    assert_eq!(runtime.editor.target_folder_id, "folder-prod");
    let (folder_label, folder_id, folder_known) = runtime.editor_folder_parts();
    assert!(folder_label.contains("Ops / Prod"));
    assert_eq!(folder_id, "folder-prod");
    assert!(folder_known);
}

#[test]
fn session_editor_modal_visibility_tracks_open_close_save_flows() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let opened = runtime.start_new_saved_session_editor();
    assert!(opened.editor_modal_visible);
    assert_eq!(opened.editor_section_text, "general");

    let closed = runtime.close_session_editor_modal();
    assert!(!closed.editor_modal_visible);

    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Modal Session");
    let _ = runtime.update_editor_host("modal.example.test");
    let saved = runtime
        .save_editor_to_saved_session()
        .expect("save editor session");
    assert!(!saved.editor_modal_visible);
}

#[test]
fn session_editor_section_selection_updates_projection() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    let _ = runtime.start_new_saved_session_editor();

    assert_eq!(
        runtime
            .select_session_editor_authentication()
            .editor_section_text,
        "authentication"
    );
    assert_eq!(
        runtime.select_session_editor_terminal().editor_section_text,
        "terminal"
    );
    assert_eq!(
        runtime.select_session_editor_sftp().editor_section_text,
        "sftp"
    );
    assert_eq!(
        runtime.select_session_editor_tunnels().editor_section_text,
        "tunnels"
    );
    assert_eq!(
        runtime.select_session_editor_proxy().editor_section_text,
        "proxy"
    );
    assert_eq!(
        runtime.select_session_editor_logging().editor_section_text,
        "logging"
    );
    assert_eq!(
        runtime.select_session_editor_advanced().editor_section_text,
        "advanced"
    );
    assert_eq!(
        runtime
            .select_session_editor_appearance()
            .editor_section_text,
        "appearance"
    );
    assert_eq!(
        runtime.select_session_editor_general().editor_section_text,
        "general"
    );
}

#[test]
fn editor_folder_selection_controls_persistence_target() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    let mut parent = FolderProfile::new("folder-servers", "Servers");
    parent
        .folders
        .push(FolderProfile::new("folder-prod", "Prod"));
    document.folders.push(parent);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.start_new_saved_session_editor();
    while runtime.editor.target_folder_id != "folder-prod" {
        let _ = runtime.select_next_editor_folder();
    }
    let _ = runtime.update_editor_name("Prod Session");
    let _ = runtime.update_editor_host("prod.example.test");
    let _ = runtime.update_editor_username("ops");

    let projection = runtime
        .save_editor_to_saved_session()
        .expect("save editor into nested folder");

    assert!(projection.editor_folder_known);
    assert!(projection
        .editor_folder_label_text
        .contains("Servers / Prod"));
    assert!(runtime
        .config_document
        .find_folder("folder-prod")
        .expect("prod folder")
        .sessions
        .iter()
        .any(|session| session.name == "Prod Session"));
    assert!(runtime
        .saved_session_inventory_parts()
        .0
        .contains("[folder] Servers"));
    assert!(runtime
        .saved_session_inventory_parts()
        .0
        .contains("[folder] Prod"));
}

#[test]
fn editor_can_create_new_folder_under_current_target() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    let mut parent = FolderProfile::new("folder-servers", "Servers");
    parent
        .folders
        .push(FolderProfile::new("folder-prod", "Prod"));
    document.folders.push(parent);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    while runtime.editor.target_folder_id != "folder-prod" {
        let _ = runtime.select_next_editor_folder();
    }
    let _ = runtime.update_new_folder_name("Blue");

    let projection = runtime
        .create_folder_under_editor_target()
        .expect("create nested folder");

    assert!(projection.status_text.contains("Created folder `Blue`"));
    assert!(projection
        .editor_folder_label_text
        .contains("Servers / Prod / Blue"));
    assert_eq!(runtime.new_folder_name, "");
    let prod = runtime
        .config_document
        .find_folder("folder-prod")
        .expect("prod folder");
    assert!(prod.folders.iter().any(|folder| folder.name == "Blue"));
    assert!(runtime
        .saved_session_inventory_parts()
        .0
        .contains("[folder] Blue"));
}

#[test]
fn editor_rejects_duplicate_folder_names_under_same_parent() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    let mut parent = FolderProfile::new("folder-servers", "Servers");
    let mut prod = FolderProfile::new("folder-prod", "Prod");
    prod.folders.push(FolderProfile::new("folder-blue", "Blue"));
    parent.folders.push(prod);
    document.folders.push(parent);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    while runtime.editor.target_folder_id != "folder-prod" {
        let _ = runtime.select_next_editor_folder();
    }
    let _ = runtime.update_new_folder_name("blue");

    let error = runtime
        .create_folder_under_editor_target()
        .expect_err("duplicate folder name should fail");

    assert!(error.to_string().contains("already exists under `Prod`"));
}

#[test]
fn root_folder_creation_from_sidebar_blank_menu_persists_and_projects() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let projection = runtime
        .create_root_saved_folder("Lab")
        .expect("create root folder");

    assert!(projection.status_text.contains("Created folder `Lab`"));
    assert!(projection
        .session_tree_rows
        .iter()
        .any(|row| row.kind == "folder" && row.label == "Lab"));
    // 落盘：重新载入配置后文件夹仍在根级。
    let reloaded = ConfigStore::new(temp.path())
        .load_or_recover()
        .expect("reload");
    assert!(reloaded
        .document
        .folders
        .iter()
        .any(|folder| folder.name == "Lab"));
    // 重名（大小写不敏感）与空名都会被拒绝。
    assert!(runtime.create_root_saved_folder("lab").is_err());
    assert!(runtime.create_root_saved_folder("  ").is_err());
}

#[test]
fn refresh_saved_sessions_reloads_config_and_prunes_stale_selection() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    document
        .folders
        .push(FolderProfile::new("folder-lab", "Lab"));
    let mut session = SessionProfile::new("session-one", "One", "one.example.test");
    session.port = 2200;
    document.folders[0].sessions.push(session);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    assert!(runtime
        .session_tree_rows()
        .iter()
        .any(|row| row.id == "session-one"));

    // 外部编辑（模拟另一个实例写盘）：删掉 Lab，改加 Ops。
    let mut edited = ConfigDocument::default();
    edited.folders.push(FolderProfile::new("folder-ops", "Ops"));
    store.save(&edited).expect("save edited config");

    let projection = runtime.refresh_saved_sessions().expect("refresh");

    assert!(projection
        .status_text
        .contains("Reloaded saved sessions from config.toml"));
    assert!(projection
        .session_tree_rows
        .iter()
        .any(|row| row.kind == "folder" && row.label == "Ops"));
    assert!(!projection
        .session_tree_rows
        .iter()
        .any(|row| row.id == "session-one"));
    // 已消失的选中会话回退为重新载入后的第一个会话（此处没有会话 → 清空）。
    assert_eq!(runtime.selected_saved_session_id, None);
    assert!(!projection.has_saved_selection);
}

#[test]
fn editor_can_add_tunnel_forward_and_persist_it() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Tunnel Session");
    let _ = runtime.update_editor_host("tunnel.example.test");
    let _ = runtime.update_editor_username("ops");
    let _ = runtime.set_editor_host_key_policy_trust_on_first_use();
    let _ = runtime.set_editor_tunnel_kind_local();
    let _ = runtime.update_editor_tunnel_bind_host("127.0.0.1");
    let _ = runtime.update_editor_tunnel_bind_port("15432");
    let _ = runtime.update_editor_tunnel_target_host("db.internal");
    let _ = runtime.update_editor_tunnel_target_port("5432");
    let added = runtime
        .add_editor_tunnel_forward()
        .expect("add tunnel forward");
    assert_eq!(added.editor_tunnel_summary_kind_text, "rows");
    assert_eq!(added.editor_tunnel_summary_count, 1);
    assert!(added
        .editor_tunnel_summary_rows_text
        .contains("local 127.0.0.1:15432 -> db.internal:5432"));

    let saved = runtime
        .save_editor_to_saved_session()
        .expect("save tunnel session");
    assert!(saved.status_text.contains("Saved session editor changes"));
    let profile = runtime
        .saved_session_profiles()
        .into_iter()
        .next()
        .expect("saved profile");
    let tunnel = profile.tunnel.expect("saved tunnel profile");
    assert_eq!(tunnel.forwards.len(), 1);
    assert_eq!(
        profile.host_key_policy,
        Some(ConfigHostKeyPolicy::TrustOnFirstUse)
    );
    assert_eq!(tunnel.forwards[0].kind, TunnelForwardKind::Local);
    assert_eq!(tunnel.forwards[0].bind_port, 15432);
    assert_eq!(tunnel.forwards[0].target_host, "db.internal");
    assert_eq!(tunnel.forwards[0].target_port, 5432);
}

#[test]
fn editor_can_create_proxy_profile_and_persist_secret() {
    let _guard = lock_env();
    std::env::set_var("YSHELL_MASTER_PASSWORD", "proxy-master");
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Proxy Session");
    let _ = runtime.update_editor_host("proxy-session.example.test");
    let _ = runtime.set_editor_proxy_mode_custom();
    let _ = runtime.set_editor_proxy_protocol_socks5();
    let _ = runtime.update_editor_proxy_host("127.0.0.1");
    let _ = runtime.update_editor_proxy_port("1080");
    let _ = runtime.update_editor_proxy_username("proxy-user");
    let _ = runtime.update_editor_proxy_password("proxy-secret");

    let projection = runtime
        .save_editor_to_saved_session()
        .expect("save proxy editor");

    assert_eq!(projection.editor_proxy_summary_kind_text, "custom");
    assert_eq!(projection.editor_proxy_protocol_text, "socks5");
    assert_eq!(
        projection.editor_proxy_summary_address_text,
        "127.0.0.1:1080"
    );
    assert_eq!(projection.editor_proxy_summary_user_text, "proxy-user");
    let saved = runtime.saved_session_profiles();
    let saved_session = &saved[0];
    let proxy = runtime
        .config_document
        .proxy_profiles
        .get(saved_session.proxy_profile_id.as_deref().expect("proxy id"))
        .expect("proxy profile");
    assert_eq!(proxy.protocol, ProxyProtocol::Socks5);
    assert_eq!(proxy.host, "127.0.0.1");
    assert_eq!(proxy.port, 1080);
    assert_eq!(proxy.username.as_deref(), Some("proxy-user"));
    let secret_key = proxy
        .password_secret_key
        .as_deref()
        .expect("proxy password secret key");
    let store_contents =
        fs::read_to_string(temp.path().join("secret-store.toml")).expect("read store");
    assert!(store_contents.contains(secret_key));
    assert!(!store_contents.contains("proxy-secret"));

    std::env::remove_var("YSHELL_MASTER_PASSWORD");
}

#[test]
fn saved_session_proxy_profile_is_applied_to_runtime_shell_config() {
    let temp = tempdir().expect("tempdir");
    let keychain = Arc::new(FakeKeychain::new("proxy-master"));
    keychain
        .put(
            SecretRef::new("local://yshell/proxy-password"),
            SecretString::from("proxy-secret"),
        )
        .expect("seed proxy password");
    let keychain: Arc<dyn Keychain> = keychain;
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    document.proxy_profiles.insert(
        "proxy-1".to_owned(),
        ProxyProfile {
            id: "proxy-1".to_owned(),
            name: "SOCKS".to_owned(),
            protocol: ProxyProtocol::Socks5,
            host: "127.0.0.1".to_owned(),
            port: 1080,
            username: Some("proxy-user".to_owned()),
            resolve_dns_by_proxy: true,
            password_secret_key: Some("local://yshell/proxy-password".to_owned()),
        },
    );
    let mut profile = QuickConnectTarget {
        username: Some("ops".to_owned()),
        host: "proxy-runtime.example.test".to_owned(),
        port: 2200,
    }
    .into_session_profile("saved-proxy");
    profile.name = "Proxy Runtime".to_owned();
    profile.proxy_profile_id = Some("proxy-1".to_owned());
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain)).expect("runtime");
    runtime
        .open_saved_session("saved-proxy")
        .expect("open proxy session");

    let session_key = runtime.active_session_id.clone().expect("active session");
    let session = runtime.sessions.get(&session_key).expect("runtime session");
    match &session.ssh_config.proxy {
        ProxyConfig::Socks5 {
            address,
            username,
            password,
            resolve_dns_by_proxy,
        } => {
            assert_eq!(address, "127.0.0.1:1080");
            assert_eq!(username.as_deref(), Some("proxy-user"));
            assert_eq!(password.as_deref(), Some("proxy-secret"));
            assert!(*resolve_dns_by_proxy);
        }
        other => panic!("expected socks5 proxy, got {other:?}"),
    }
}

#[test]
fn saved_session_tunnel_profile_is_applied_to_runtime_shell_config() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    let mut profile = QuickConnectTarget {
        username: Some("ops".to_owned()),
        host: "tunnel-runtime.example.test".to_owned(),
        port: 2222,
    }
    .into_session_profile("saved-tunnel");
    profile.name = "Tunnel Runtime".to_owned();
    profile.tunnel = Some(TunnelProfile {
        forwards: vec![
            TunnelForward {
                kind: TunnelForwardKind::Local,
                bind_host: "127.0.0.1".to_owned(),
                bind_port: 18080,
                target_host: "service.internal".to_owned(),
                target_port: 8080,
            },
            TunnelForward {
                kind: TunnelForwardKind::Dynamic,
                bind_host: "127.0.0.1".to_owned(),
                bind_port: 19050,
                target_host: String::new(),
                target_port: 0,
            },
        ],
    });
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let projection = runtime
        .open_saved_session("saved-tunnel")
        .expect("open tunnel session");

    assert_eq!(projection.tunnels_summary_kind_text, "rows");
    assert!(projection
        .tunnels_summary_rows_text
        .contains("local 127.0.0.1:18080 -> service.internal:8080"));
    assert!(projection
        .tunnels_summary_rows_text
        .contains("dynamic 127.0.0.1:19050"));
    let session_key = runtime.active_session_id.clone().expect("active session");
    let session = runtime.sessions.get(&session_key).expect("runtime session");
    assert_eq!(session.ssh_config.tunnels.len(), 2);
    assert_eq!(session.ssh_config.tunnels[0].kind, ForwardingKind::Local);
    assert_eq!(session.ssh_config.tunnels[1].kind, ForwardingKind::Dynamic);
}

#[test]
fn editor_can_create_password_saved_session_and_persist_secret() {
    let _guard = lock_env();
    let temp = tempdir().expect("tempdir");
    std::env::set_var("YSHELL_MASTER_PASSWORD", "editor-master");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Password Session");
    let _ = runtime.update_editor_host("editor-password.example.test");
    let _ = runtime.update_editor_port("2222");
    let _ = runtime.update_editor_username("root");
    let _ = runtime.set_editor_auth_method_password();
    let _ = runtime.update_editor_password("super-secret");
    let projection = runtime
        .save_editor_to_saved_session()
        .expect("save editor session");

    assert!(projection
        .status_text
        .contains("Saved session editor changes"));
    let saved = runtime.saved_session_profiles();
    assert_eq!(saved.len(), 1);
    let saved_session = &saved[0];
    assert_eq!(saved_session.host, "editor-password.example.test");
    let auth = runtime
        .config_document
        .auth_profiles
        .get(saved_session.auth_profile_id.as_deref().expect("auth id"))
        .expect("auth profile");
    let secret_key = match &auth.method {
        ConfigAuthMethod::Password { secret_key } => secret_key.clone(),
        other => panic!("expected password auth, got {other:?}"),
    };
    let store_contents =
        fs::read_to_string(temp.path().join("secret-store.toml")).expect("read store");
    assert!(store_contents.contains(&secret_key));
    assert!(!store_contents.contains("super-secret"));

    std::env::remove_var("YSHELL_MASTER_PASSWORD");
}

#[test]
fn editor_can_create_keyboard_interactive_saved_session_and_persist_secret() {
    let _guard = lock_env();
    let temp = tempdir().expect("tempdir");
    std::env::set_var("YSHELL_MASTER_PASSWORD", "editor-master");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Keyboard Interactive Session");
    let _ = runtime.update_editor_host("editor-kbdint.example.test");
    let _ = runtime.update_editor_port("2222");
    let _ = runtime.update_editor_username("root");
    let _ = runtime.set_editor_auth_method_keyboard_interactive();
    let _ = runtime.update_editor_password("challenge-secret");
    let projection = runtime
        .save_editor_to_saved_session()
        .expect("save editor session");

    assert!(projection
        .status_text
        .contains("Saved session editor changes"));
    let saved = runtime.saved_session_profiles();
    assert_eq!(saved.len(), 1);
    let saved_session = &saved[0];
    let auth = runtime
        .config_document
        .auth_profiles
        .get(saved_session.auth_profile_id.as_deref().expect("auth id"))
        .expect("auth profile");
    let secret_key = match &auth.method {
        ConfigAuthMethod::KeyboardInteractive { secret_key } => secret_key.clone(),
        other => panic!("expected keyboard-interactive auth, got {other:?}"),
    };
    let store_contents =
        fs::read_to_string(temp.path().join("secret-store.toml")).expect("read store");
    assert!(store_contents.contains(&secret_key));
    assert!(!store_contents.contains("challenge-secret"));

    std::env::remove_var("YSHELL_MASTER_PASSWORD");
}

#[test]
fn editor_password_save_requires_enabled_secret_store() {
    let _guard = lock_env();
    std::env::remove_var("YSHELL_MASTER_PASSWORD");
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Password Session");
    let _ = runtime.update_editor_host("editor-password.example.test");
    let _ = runtime.set_editor_auth_method_password();
    let _ = runtime.update_editor_password("super-secret");
    let error = runtime
        .save_editor_to_saved_session()
        .expect_err("password save should require secret store");

    assert!(error
        .to_string()
        .contains("requires an enabled secret store"));
}

#[test]
fn fake_backend_editor_auth_test_succeeds_without_saving() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Auth Test Session");
    let _ = runtime.update_editor_host("auth-test.example.test");
    let _ = runtime.update_editor_port("2222");
    let _ = runtime.update_editor_username("ops");

    let projection = runtime.test_editor_auth();

    assert_eq!(projection.editor_auth_test_kind_text, "success");
    assert!(projection
        .editor_auth_test_host_text
        .contains("auth-test.example.test:2222"));
    assert_eq!(projection.editor_auth_test_backend_text, "fake");
    assert_eq!(runtime.saved_session_profiles().len(), 0);
}

#[test]
fn native_ssh_password_auth_test_runs_without_old_system_bridge_limitation() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.select_native_ssh_transport_backend();
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Password Auth Test");
    let _ = runtime.update_editor_host("password-auth.example.test");
    let _ = runtime.update_editor_username("root");
    let _ = runtime.set_editor_auth_method_password();
    let _ = runtime.update_editor_password("secret");

    let projection = runtime.test_editor_auth();

    assert!(
        matches!(
            projection.editor_auth_test_kind_text.as_str(),
            "success" | "failed"
        ),
        "auth test should still produce a structured status"
    );
}

#[test]
fn editor_save_and_connect_opens_the_saved_session_runtime() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Connect Session");
    let _ = runtime.update_editor_host("connect.example.test");
    let _ = runtime.update_editor_port("2224");
    let _ = runtime.update_editor_username("ops");
    let _ = runtime.set_editor_auth_method_agent();

    let projection = runtime.save_editor_and_connect().expect("save and connect");

    assert!(projection
        .active_session_name_text
        .contains("Connect Session"));
    assert_eq!(projection.tab_state_text, "connected");
    assert!(projection
        .terminal_body_text
        .contains("Fake shell established"));
    assert_eq!(runtime.saved_session_profiles().len(), 1);
}

#[test]
fn session_search_filters_hierarchical_inventory() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    let mut parent = FolderProfile::new("folder-1", "Servers");
    let mut nested = FolderProfile::new("folder-2", "Prod");
    nested.sessions.push(
        QuickConnectTarget {
            username: Some("ops".to_owned()),
            host: "prod.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("session-prod"),
    );
    parent.sessions.push(
        QuickConnectTarget {
            username: Some("dev".to_owned()),
            host: "dev.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("session-dev"),
    );
    parent.folders.push(nested);
    document.folders.push(parent);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let filtered = runtime.update_session_search("prod");

    assert_eq!(filtered.session_search_text, "prod");
    assert!(filtered
        .saved_session_inventory_rows_text
        .contains("[folder] Servers"));
    assert!(filtered
        .saved_session_inventory_rows_text
        .contains("[folder] Prod"));
    assert!(filtered
        .saved_session_inventory_rows_text
        .contains("prod.example.test"));
    assert!(!filtered
        .saved_session_inventory_rows_text
        .contains("dev.example.test"));
}
