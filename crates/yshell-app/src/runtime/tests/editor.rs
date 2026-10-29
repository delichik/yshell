//! Session editor flows: modal state, folder choice, tunnel/proxy persistence.

use std::{fs, sync::Arc};
use tempfile::tempdir;
use yshell_config::{
    AuthMethod as ConfigAuthMethod, AuthProfile, ConfigDocument, ConfigStore, FolderProfile,
    HostKeyPolicy as ConfigHostKeyPolicy, ProxyProfile, ProxyProtocol, QuickConnectTarget,
    SessionProfile, TunnelForward, TunnelForwardKind, TunnelProfile,
};
use yshell_secret::{FakeKeychain, Keychain, SecretRef, SecretString};
use yshell_ssh::{AuthMethod as SshAuthMethod, ForwardingKind, HostKeyPolicy, ProxyConfig};

use super::*;

/// D0c spike fixture: Ed25519 private key (same as runtime::auth unit tests).
const TEST_ED25519_PRIVATE_KEY: &str = concat!(
    "-----BEGIN OPENSSH PRIVATE KEY-----\n",
    "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n",
    "QyNTUxOQAAACBSYrC396PBUCm8lAJ7daz70WfJiRWrxGkVfMeR9mqo7QAAAJgld3oAJXd6\n",
    "AAAAAAtzc2gtZWQyNTUxOQAAACBSYrC396PBUCm8lAJ7daz70WfJiRWrxGkVfMeR9mqo7Q\n",
    "AAAEBjQwofDuXa019zF7tFRxguRzCAMmLfd6RLo4WsAftZh1JisLf3o8FQKbyUAnt1rPvR\n",
    "Z8mJFavEaRV8x5H2aqjtAAAAE3NwaWtlLWVkMjU1MTktcGxhaW4BAg==\n",
    "-----END OPENSSH PRIVATE KEY-----\n",
);
const TEST_ED25519_FINGERPRINT: &str = "SHA256:gufPep5T6QepKH9Ggl3KT9znGwIxL0pWn3fxvDUxNPA";
const LEGACY_KEY_PATH: &str = "/legacy/id_ed25519";

/// 旧配置形态：`AuthMethod::PrivateKey { key_id: None, path, .. }`（SSH-1 legacy）。
fn legacy_private_key_store(
    config_dir: &std::path::Path,
    passphrase_secret_key: Option<&str>,
) -> ConfigStore {
    let store = ConfigStore::new(config_dir);
    let mut document = ConfigDocument::default();
    document.auth_profiles.insert(
        "saved-legacy-auth".to_owned(),
        AuthProfile {
            id: "saved-legacy-auth".to_owned(),
            name: "Legacy Key".to_owned(),
            method: ConfigAuthMethod::PrivateKey {
                key_id: None,
                path: LEGACY_KEY_PATH.to_owned(),
                passphrase_secret_key: passphrase_secret_key.map(str::to_owned),
            },
        },
    );
    let mut profile = QuickConnectTarget {
        username: Some("deploy".to_owned()),
        host: "legacy-key.example.test".to_owned(),
        port: 22,
    }
    .into_session_profile("saved-legacy-key");
    profile.name = "Legacy Key Session".to_owned();
    profile.auth_profile_id = Some("saved-legacy-auth".to_owned());
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save_all(&document).expect("save legacy config");
    store
}

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
    // 兼容性：旧配置导入的会话仍可携带 `host_key_policy`；编辑器不再读取/暴露它，
    // 已消失的选中会话回退为重新载入后的第一个会话（此处没有会话 → 清空）。
    profile.host_key_policy = Some(ConfigHostKeyPolicy::AcceptAnyForTesting);
    let mut folder = FolderProfile::new("folder-ops", "Ops");
    let mut nested = FolderProfile::new("folder-prod", "Prod");
    nested.sessions.push(profile);
    folder.folders.push(nested);
    document.folders.push(folder);
    store.save_all(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime
        .load_selected_saved_session_into_editor()
        .expect("load editor");

    assert_eq!(runtime.editor.name, "Editor Session");
    assert_eq!(runtime.editor.host, "editor.example.test");
    assert_eq!(runtime.editor.port_text, "2202");
    assert_eq!(runtime.editor.username, "ops");
    assert_eq!(runtime.editor.auth_method, EditorAuthMethod::Agent);
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
    assert!(!opened.editor_is_dirty);
    assert_eq!(opened.editor_section_text, "general");

    // Clean close (no edits) skips Discard Changes.
    let closed = runtime.request_close_session_editor_modal();
    assert!(!closed.editor_modal_visible);
    assert!(!closed.editor_discard_dialog_visible);

    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Modal Session");
    let dirty_close = runtime.request_close_session_editor_modal();
    assert!(dirty_close.editor_modal_visible);
    assert!(dirty_close.editor_discard_dialog_visible);
    assert!(dirty_close.editor_is_dirty);

    let canceled = runtime.cancel_discard_session_editor();
    assert!(canceled.editor_modal_visible);
    assert!(!canceled.editor_discard_dialog_visible);

    let _ = runtime.update_editor_host("modal.example.test");
    let saved = runtime
        .save_editor_to_saved_session()
        .expect("save editor session");
    assert!(!saved.editor_modal_visible);
    assert!(!saved.editor_discard_dialog_visible);
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
    // Terminal / Logging sections were removed from the session editor UI
    // (scrollback → Settings; logging → context menu); callbacks fall back to general.
    assert_eq!(
        runtime.select_session_editor_terminal().editor_section_text,
        "general"
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
        "general"
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
    store.save_all(&document).expect("save config");

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
    store.save_all(&document).expect("save config");

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
    store.save_all(&document).expect("save config");

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
    store.save_all(&document).expect("save config");

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
    // 已消失的选中会话回退为重新载入后的第一个会话（此处没有会话 ）。清空）。
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
    // 编辑器不再写主机密钥策略：保存后保持为空，运行时解析回退回 Strict。
    assert!(
        profile.host_key_policy.is_none(),
        "editor must not persist a host key policy"
    );
    assert_eq!(tunnel.forwards[0].kind, TunnelForwardKind::Local);
    assert_eq!(tunnel.forwards[0].bind_port, 15432);
    assert_eq!(tunnel.forwards[0].target_host, "db.internal");
    assert_eq!(tunnel.forwards[0].target_port, 5432);
}

#[test]
fn editor_built_ssh_config_uses_strict_policy_unless_testing_override() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_host("strict-default.example.test");

    // 编辑器不再携带策略：默认解析为严格 known_hosts 校验。
    let config = runtime
        .build_ssh_config_from_editor()
        .expect("build editor ssh config");
    assert_eq!(config.host_key_policy, HostKeyPolicy::Strict);

    // 受控测试路径：accept-any 只能通过显式 override 注入，不是用户选项。
    runtime.set_host_key_policy_override_for_testing(HostKeyPolicy::AcceptAnyForTesting);
    let overridden = runtime
        .build_ssh_config_from_editor()
        .expect("build editor ssh config with override");
    assert_eq!(
        overridden.host_key_policy,
        HostKeyPolicy::AcceptAnyForTesting
    );
}

#[test]
fn editor_selects_managed_proxy_without_creating_profiles() {
    let _guard = lock_env();
    std::env::set_var("YSHELL_MASTER_PASSWORD", "proxy-master");
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    // NET-1：先通过代理管理器创建一条托管条目。
    let _ = runtime.open_proxies_manager();
    let _ = runtime.start_new_proxy();
    let _ = runtime.update_proxy_editor_name("Corp SOCKS");
    let _ = runtime.update_proxy_editor_host("127.0.0.1");
    let _ = runtime.update_proxy_editor_port("1080");
    let _ = runtime.update_proxy_editor_username("proxy-user");
    let _ = runtime.update_proxy_editor_password("proxy-secret");
    let projection = runtime.save_proxy_editor();
    assert_eq!(runtime.config_document.proxy_profiles.len(), 1);
    let proxy_id = runtime
        .config_document
        .proxy_profiles
        .keys()
        .next()
        .cloned()
        .expect("managed proxy id");
    assert_eq!(projection.proxies_editor_id_text, proxy_id);
    let _ = runtime.close_proxies_manager();

    // 新建会话并从下拉里选择托管代理。
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Managed Proxy Session");
    let _ = runtime.update_editor_host("managed-proxy.example.test");
    let projection = runtime.select_editor_proxy(0);
    assert_eq!(projection.editor_proxy_mode_text, "managed");
    assert_eq!(projection.editor_selected_proxy_index, 0);
    assert!(projection
        .editor_proxy_selected_detail_text
        .contains("127.0.0.1:1080"));
    runtime
        .save_editor_to_saved_session()
        .expect("save managed proxy session");

    let saved = runtime.saved_session_profiles();
    let saved_session = saved.first().expect("saved session");
    assert_eq!(
        saved_session.proxy_profile_id.as_deref(),
        Some(proxy_id.as_str())
    );
    // 保存会话不再生成 `<session_id>-proxy`，也不复制管理器条目。
    assert_eq!(runtime.config_document.proxy_profiles.len(), 1);
    assert!(!runtime
        .config_document
        .proxy_profiles
        .contains_key(&format!("{}-proxy", saved_session.id)));

    // 密码落在 secret store，profile 只保存引用。
    let profile = runtime
        .config_document
        .proxy_profiles
        .get(&proxy_id)
        .expect("managed profile");
    let secret_key = profile
        .password_secret_key
        .as_deref()
        .expect("proxy password secret key");
    assert_eq!(
        runtime.resolve_secret_value(secret_key).expect("stored password"),
        "proxy-secret"
    );
    let store_contents =
        fs::read_to_string(temp.path().join("secret-store.toml")).expect("read store");
    assert!(!store_contents.contains("proxy-secret"));

    // 重新加载会话：编辑器应投影为 managed，并且连接配置能解析出同一代理。
    let _ = runtime
        .load_selected_saved_session_into_editor()
        .expect("reload editor");
    let reloaded = runtime.projection();
    assert_eq!(reloaded.editor_proxy_mode_text, "managed");
    assert_eq!(reloaded.editor_selected_proxy_index, 0);
    match &runtime
        .build_ssh_config_from_editor()
        .expect("editor proxy config")
        .proxy
    {
        ProxyConfig::Socks5 {
            address,
            username,
            password,
            ..
        } => {
            assert_eq!(address, "127.0.0.1:1080");
            assert_eq!(username.as_deref(), Some("proxy-user"));
            assert_eq!(password.as_deref(), Some("proxy-secret"));
        }
        other => panic!("expected socks5 proxy, got {other:?}"),
    }

    // 切回 No Proxy：只清会话引用，托管条目本身保留。
    let _ = runtime.set_editor_proxy_mode_none();
    runtime
        .save_editor_to_saved_session()
        .expect("save without proxy");
    let saved = runtime.saved_session_profiles();
    assert!(saved[0].proxy_profile_id.is_none());
    assert_eq!(runtime.config_document.proxy_profiles.len(), 1);

    std::env::remove_var("YSHELL_MASTER_PASSWORD");
}

#[test]
fn legacy_per_session_proxy_loads_and_migrates_to_managed_profile() {
    let temp = tempdir().expect("tempdir");
    let keychain = Arc::new(FakeKeychain::new("proxy-master"));
    keychain
        .put(
            SecretRef::new("local://yshell/legacy/saved-legacy-proxy/password"),
            SecretString::from("legacy-secret"),
        )
        .expect("seed legacy proxy password");
    let keychain: Arc<dyn Keychain> = keychain;
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    // 旧构建写入的 per-session 条目：id = `<session_id>-proxy`。
    document.proxy_profiles.insert(
        "saved-legacy-proxy".to_owned(),
        ProxyProfile {
            id: "saved-legacy-proxy".to_owned(),
            name: "Proxy for Legacy Session".to_owned(),
            protocol: ProxyProtocol::Socks5,
            host: "127.0.0.1".to_owned(),
            port: 1081,
            username: Some("legacy-user".to_owned()),
            resolve_dns_by_proxy: false,
            password_secret_key: Some(
                "local://yshell/legacy/saved-legacy-proxy/password".to_owned(),
            ),
        },
    );
    let mut session = SessionProfile::new(
        "saved-legacy",
        "Legacy Session",
        "legacy-proxy.example.test",
    );
    session.proxy_profile_id = Some("saved-legacy-proxy".to_owned());
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(session);
    document.folders.push(folder);
    // 管理器里另有一条可迁移的托管条目。
    document.proxy_profiles.insert(
        "proxy-1".to_owned(),
        ProxyProfile {
            id: "proxy-1".to_owned(),
            name: "Managed SOCKS".to_owned(),
            protocol: ProxyProtocol::Socks5,
            host: "10.0.0.10".to_owned(),
            port: 1080,
            username: None,
            resolve_dns_by_proxy: true,
            password_secret_key: None,
        },
    );
    store.save_all(&document).expect("save config");

    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain)).expect("runtime");
    let projection = runtime
        .load_selected_saved_session_into_editor()
        .expect("load legacy editor");
    assert_eq!(projection.editor_proxy_mode_text, "legacy");
    assert!(projection
        .editor_proxy_legacy_text
        .contains("127.0.0.1:1081"));
    assert_eq!(runtime.editor.proxy_profile_id.as_deref(), Some("saved-legacy-proxy"));

    // 旧配置仍可连接：legacy 引用解析到保存的口令。
    match &runtime
        .build_ssh_config_from_editor()
        .expect("legacy proxy config")
        .proxy
    {
        ProxyConfig::Socks5 {
            address,
            username,
            password,
            resolve_dns_by_proxy,
        } => {
            assert_eq!(address, "127.0.0.1:1081");
            assert_eq!(username.as_deref(), Some("legacy-user"));
            assert_eq!(password.as_deref(), Some("legacy-secret"));
            assert!(!resolve_dns_by_proxy);
        }
        other => panic!("expected legacy socks5 proxy, got {other:?}"),
    }

    // 选择托管条目并保存 = 迁移；旧 `<session>-proxy` 不再被引用后清理。
    // 下拉按名称排序（Managed SOCKS < Proxy for Legacy Session），托管条目）。0。
    let projection = runtime.select_editor_proxy(0);
    assert_eq!(projection.editor_proxy_mode_text, "managed");
    runtime
        .save_editor_to_saved_session()
        .expect("save migrated session");
    let saved = runtime.saved_session_profiles();
    assert_eq!(saved[0].proxy_profile_id.as_deref(), Some("proxy-1"));
    assert!(!runtime
        .config_document
        .proxy_profiles
        .contains_key("saved-legacy-proxy"));
    assert!(runtime.config_document.proxy_profiles.contains_key("proxy-1"));
}

#[test]
fn dangling_legacy_proxy_reference_is_kept_readable_and_fails_readably() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    let mut session = SessionProfile::new(
        "saved-dangling-proxy",
        "Dangling Proxy",
        "dangling-proxy.example.test",
    );
    session.proxy_profile_id = Some("saved-dangling-proxy-proxy".to_owned());
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(session);
    document.folders.push(folder);
    store.save_all(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let projection = runtime
        .load_selected_saved_session_into_editor()
        .expect("load dangling editor");
    assert_eq!(projection.editor_proxy_mode_text, "legacy");
    assert!(projection
        .editor_proxy_legacy_text
        .contains("saved-dangling-proxy-proxy"));
    let error = runtime
        .build_ssh_config_from_editor()
        .expect_err("dangling proxy must fail with a readable error");
    assert!(error.to_string().contains("no longer exists"));
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
    store.save_all(&document).expect("save config");

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
    store.save_all(&document).expect("save config");

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
    assert_eq!(projection.tunnel_rows.len(), 2);
    assert_eq!(projection.tunnel_rows[0].kind_id, "local");
    assert!(projection.tunnel_rows[0]
        .summary_text
        .contains("127.0.0.1:18080"));
    assert_eq!(projection.tunnel_rows[1].kind_id, "dynamic");
    assert_eq!(projection.tunnel_rows[0].status_id, "idle");
    let session_key = runtime.active_session_id.clone().expect("active session");
    let session = runtime.sessions.get(&session_key).expect("runtime session");
    assert_eq!(
        projection.tunnel_rows[0].can_start,
        session.state == yshell_core::SessionState::Connected
    );
    assert!(!projection.tunnel_rows[0].can_stop);
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
fn editing_password_session_keeps_secret_when_password_left_blank() {
    let _guard = lock_env();
    let temp = tempdir().expect("tempdir");
    std::env::set_var("YSHELL_MASTER_PASSWORD", "editor-master");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Keep Secret Session");
    let _ = runtime.update_editor_host("keep-secret.example.test");
    let _ = runtime.update_editor_username("root");
    let _ = runtime.set_editor_auth_method_password();
    let _ = runtime.update_editor_password("original-secret");
    let _ = runtime
        .save_editor_to_saved_session()
        .expect("create password session");
    // After save the modal closes; look up the new profile by host.
    let profile_id = runtime
        .saved_session_profiles()
        .into_iter()
        .find(|session| session.host == "keep-secret.example.test")
        .expect("saved session")
        .id;

    let _ = runtime.select_saved_session_by_id(&profile_id);
    let loaded = runtime
        .load_selected_saved_session_into_editor()
        .expect("load into editor");
    assert!(loaded.editor_has_stored_password);
    assert!(runtime.editor.password.is_empty());

    // Rename only — leave password blank and save again.
    let _ = runtime.update_editor_name("Keep Secret Session Renamed");
    let resaved = runtime
        .save_editor_to_saved_session()
        .expect("save without retyping password");
    assert!(resaved.status_text.contains("Saved session editor changes"));

    let auth = runtime
        .config_document
        .auth_profiles
        .get(&format!("{profile_id}-auth"))
        .expect("auth profile");
    let secret_key = match &auth.method {
        ConfigAuthMethod::Password { secret_key } => secret_key.clone(),
        other => panic!("expected password auth, got {other:?}"),
    };
    let resolved = runtime
        .resolve_secret_value(&secret_key)
        .expect("resolve kept secret");
    assert_eq!(resolved, "original-secret");

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
fn editor_auth_test_succeeds_without_saving() {
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
    assert_eq!(projection.editor_auth_test_backend_text, "");
    assert_eq!(runtime.saved_session_profiles().len(), 0);
}

#[test]
fn password_auth_test_runs_against_real_ssh_path() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.force_real_ssh_in_tests = true;
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
        .contains("Test shell established"));
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
    store.save_all(&document).expect("save config");

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

#[test]
fn editor_key_auth_lists_managed_keys_and_saves_key_id() {
    let temp = tempdir().expect("tempdir");
    let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("editor-master"));
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain)).expect("runtime");
    let entry = runtime
        .import_private_key("Work key", TEST_ED25519_PRIVATE_KEY, None, false)
        .expect("import managed key");

    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("Managed Key Session");
    let _ = runtime.update_editor_host("managed-key.example.test");
    let _ = runtime.update_editor_username("deploy");
    let _ = runtime.set_editor_auth_method_private_key();

    let listed = runtime.projection();
    assert_eq!(listed.editor_auth_method_text, "private_key");
    assert_eq!(listed.editor_key_options.len(), 1);
    assert_eq!(listed.editor_key_options[0].id, entry.id);
    assert_eq!(listed.editor_key_options[0].label, "Work key");
    assert!(listed.editor_key_options[0]
        .detail
        .contains(TEST_ED25519_FINGERPRINT));
    assert_eq!(listed.editor_selected_key_index, -1);
    assert!(!listed.editor_selected_key_missing);
    assert!(listed.editor_legacy_key_path_text.is_empty());

    let selected = runtime.select_editor_key(0);
    assert_eq!(selected.editor_selected_key_index, 0);
    assert!(selected.editor_legacy_key_path_text.is_empty());

    // Test Connection / draft connection path materializes the managed key material.
    let ssh_config = runtime
        .build_ssh_config_from_editor()
        .expect("build managed-key ssh config");
    match ssh_config.auth {
        SshAuthMethod::PrivateKey {
            ref key_path,
            ref passphrase,
            ..
        } => {
            assert!(
                key_path.ends_with("key-1.key"),
                "unexpected materialized key path {key_path}"
            );
            assert!(std::path::Path::new(key_path).is_file());
            assert!(passphrase.is_none());
        }
        other => panic!("expected private key auth, got {other:?}"),
    }

    let _ = runtime
        .save_editor_to_saved_session()
        .expect("save managed-key session");
    let session = runtime
        .saved_session_profiles()
        .into_iter()
        .find(|session| session.host == "managed-key.example.test")
        .expect("saved session");
    let auth = runtime
        .config_document
        .auth_profiles
        .get(session.auth_profile_id.as_deref().expect("auth id"))
        .expect("auth profile");
    match &auth.method {
        ConfigAuthMethod::PrivateKey {
            key_id,
            path,
            passphrase_secret_key,
        } => {
            assert_eq!(key_id.as_deref(), Some(entry.id.as_str()));
            assert!(path.is_empty(), "managed save must not write a key path");
            assert!(
                passphrase_secret_key.is_none(),
                "managed save must not write a per-session passphrase secret"
            );
        }
        other => panic!("expected private key auth, got {other:?}"),
    }

    // 引用保护：已保存会话使用该 key 时，删除必须走确认/阻断路径。
    let error = runtime
        .remove_private_key(&entry.id, false)
        .expect_err("referenced key must not be removed without confirmation");
    assert!(error.message.contains("still used"));
    assert_eq!(
        runtime
            .private_key_entry(&entry.id)
            .expect("key entry")
            .used_by,
        vec!["Managed Key Session".to_owned()]
    );
}

#[test]
fn editor_key_auth_without_managed_keys_shows_empty_list() {
    let temp = tempdir().expect("tempdir");
    let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("editor-master"));
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain)).expect("runtime");

    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.update_editor_name("No Key Session");
    let _ = runtime.update_editor_host("no-key.example.test");
    let projection = runtime.set_editor_auth_method_private_key();

    assert_eq!(projection.editor_auth_method_text, "private_key");
    assert!(projection.editor_key_options.is_empty());
    assert_eq!(projection.editor_selected_key_index, -1);
    assert!(!projection.editor_selected_key_missing);
    assert!(projection.editor_legacy_key_path_text.is_empty());

    // 空清除 + 无 legacy path：连接与保存都必须给出可读错误，而不是写出空密钥。
    let error = runtime
        .build_ssh_config_from_editor()
        .expect_err("key auth without a key must fail");
    assert!(error.message.contains("managed key"));
    let error = runtime
        .save_editor_to_saved_session()
        .expect_err("key auth without a key must not save");
    assert!(error.message.contains("managed key"));
}

#[test]
fn legacy_path_key_session_is_marked_and_stays_connectable() {
    let temp = tempdir().expect("tempdir");
    let passphrase_ref = SecretRef::new("fake://yshell/legacy-key-passphrase");
    let keychain_inner = FakeKeychain::new("editor-master");
    keychain_inner
        .put(passphrase_ref.clone(), SecretString::from("open-sesame"))
        .expect("seed legacy passphrase");
    let keychain: Arc<dyn Keychain> = Arc::new(keychain_inner);
    legacy_private_key_store(temp.path(), Some(passphrase_ref.as_str()));

    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain)).expect("runtime");
    let _ = runtime.select_saved_session_by_id("saved-legacy-key");
    let loaded = runtime
        .load_selected_saved_session_into_editor()
        .expect("load legacy session into editor");

    assert_eq!(loaded.editor_auth_method_text, "private_key");
    assert_eq!(loaded.editor_legacy_key_path_text, LEGACY_KEY_PATH);
    assert!(loaded.editor_legacy_key_has_passphrase);
    assert!(loaded.editor_key_options.is_empty());
    assert_eq!(loaded.editor_selected_key_index, -1);

    // 旧 path-only 草稿仍可连接：path 原样使用，口令从 secret store 解析。
    let ssh_config = runtime
        .build_ssh_config_from_editor()
        .expect("legacy editor ssh config");
    match ssh_config.auth {
        SshAuthMethod::PrivateKey {
            ref key_path,
            ref passphrase,
            ..
        } => {
            assert_eq!(key_path, LEGACY_KEY_PATH);
            assert_eq!(passphrase.as_deref(), Some("open-sesame"));
        }
        other => panic!("expected private key auth, got {other:?}"),
    }

    // 仅改名保存时保留 legacy path 与已保存口令的 secret 引用（不会破坏旧配置）。
    let _ = runtime.update_editor_name("Legacy Key Session Renamed");
    let _ = runtime
        .save_editor_to_saved_session()
        .expect("save renamed legacy session");
    let auth = runtime
        .config_document
        .auth_profiles
        .get("saved-legacy-auth")
        .expect("legacy auth profile");
    match &auth.method {
        ConfigAuthMethod::PrivateKey {
            key_id,
            path,
            passphrase_secret_key,
        } => {
            assert!(key_id.is_none());
            assert_eq!(path, LEGACY_KEY_PATH);
            assert_eq!(
                passphrase_secret_key.as_deref(),
                Some(passphrase_ref.as_str())
            );
        }
        other => panic!("expected retained legacy private key auth, got {other:?}"),
    }
}

#[test]
fn legacy_path_key_session_migrates_to_managed_key_when_picked() {
    let temp = tempdir().expect("tempdir");
    legacy_private_key_store(temp.path(), None);
    let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("editor-master"));
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain)).expect("runtime");
    let entry = runtime
        .import_private_key("Migrated key", TEST_ED25519_PRIVATE_KEY, None, false)
        .expect("import managed key");
    let _ = runtime.select_saved_session_by_id("saved-legacy-key");
    let loaded = runtime
        .load_selected_saved_session_into_editor()
        .expect("load legacy session into editor");
    assert_eq!(loaded.editor_legacy_key_path_text, LEGACY_KEY_PATH);
    assert_eq!(loaded.editor_key_options.len(), 1);

    let selected = runtime.select_editor_key(0);
    assert_eq!(selected.editor_selected_key_index, 0);
    assert!(selected.editor_legacy_key_path_text.is_empty());
    assert!(!selected.editor_legacy_key_has_passphrase);

    let _ = runtime
        .save_editor_to_saved_session()
        .expect("save migrated session");
    let auth = runtime
        .config_document
        .auth_profiles
        .get("saved-legacy-auth")
        .expect("migrated auth profile");
    match &auth.method {
        ConfigAuthMethod::PrivateKey {
            key_id,
            path,
            passphrase_secret_key,
        } => {
            assert_eq!(key_id.as_deref(), Some(entry.id.as_str()));
            assert!(path.is_empty());
            assert!(passphrase_secret_key.is_none());
        }
        other => panic!("expected migrated managed key auth, got {other:?}"),
    }
}

#[test]
fn deleting_selected_managed_key_clears_editor_selection() {
    let temp = tempdir().expect("tempdir");
    let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("editor-master"));
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain)).expect("runtime");
    let entry = runtime
        .import_private_key("Draft key", TEST_ED25519_PRIVATE_KEY, None, false)
        .expect("import managed key");
    let _ = runtime.start_new_saved_session_editor();
    let _ = runtime.set_editor_auth_method_private_key();
    let _ = runtime.select_editor_key(0);
    assert_eq!(runtime.editor.key_id.as_deref(), Some(entry.id.as_str()));

    // 未保存草稿不计 used_by：强制删除后必须清掉选择，避免保存出悬空 key_id。
    let _ = runtime
        .remove_private_key(&entry.id, true)
        .expect("remove draft key");
    let projection = runtime.projection();
    assert!(projection.editor_key_options.is_empty());
    assert_eq!(projection.editor_selected_key_index, -1);
    assert!(!projection.editor_selected_key_missing);
    assert!(runtime.editor.key_id.is_none());
}
