//! Host-key trust prompts, known-hosts persistence and secret-store reset flows.

use std::fs;
use tempfile::tempdir;
use yshell_config::{
    AuthMethod as ConfigAuthMethod, AuthProfile, ConfigDocument, ConfigStore, FolderProfile,
    QuickConnectTarget,
};
use yshell_secret::{FileKeychain, SecretRef, SecretString};
use yshell_ssh::{
    AuthMethod as SshAuthMethod, HostKeyFingerprint, HostKeyProblem, KnownHosts, SshError,
    SshErrorKind,
};

use super::*;

#[test]
fn runtime_boots_with_startup_projection() {
    let temp = tempdir().expect("tempdir");
    let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let projection = runtime.projection();

    assert!(projection.status_text.contains("Saved sessions discovered"));
    assert_eq!(projection.active_session_kind_text, "welcome");
    assert_eq!(projection.saved_session_count, 0);
    assert_eq!(projection.tab_count, 0);
}

#[test]
fn runtime_new_defaults_to_operating_system_keychain_without_master_password_env() {
    let _guard = lock_env();
    std::env::remove_var("YSHELL_MASTER_PASSWORD");
    let temp = tempdir().expect("tempdir");
    let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    assert_eq!(runtime.secret_store_kind.id(), "os");
    assert!(runtime.secret_store_path.is_none());
}

#[test]
fn unknown_host_key_error_surfaces_first_trust_prompt() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let session_key = runtime.active_session_key().expect("active session");
    let error = SshError::new(
        SshErrorKind::HostKeyRejected,
        "no known host key".to_owned(),
    )
    .with_host_key_problem(HostKeyProblem::Unknown {
        host: "example.com".to_owned(),
        port: 2200,
        presented: HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "sha256:test".to_owned(),
        },
    });

    assert!(runtime.apply_host_key_prompt_from_error(&session_key, "alice".to_owned(), &error));
    let projection = runtime.projection();

    assert!(projection.host_key_prompt_visible);
    assert_eq!(projection.host_key_prompt_mode_text, "first-trust");
    assert!(projection.host_key_prompt_text.contains("Trust Once"));
}

#[test]
fn trust_host_key_and_save_persists_known_hosts_and_reconnects() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let session_key = runtime.active_session_key().expect("active session");
    runtime.pending_host_key_prompt = Some(PendingHostKeyPrompt {
        session_key: session_key.clone(),
        host: "example.com".to_owned(),
        port: 2200,
        username: "alice".to_owned(),
        presented: HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "sha256:test".to_owned(),
        },
        expected: None,
        known_hosts_path: runtime.config_store.known_hosts_file(),
        resume_password: None,
    });

    let projection = runtime
        .trust_host_key_and_save()
        .expect("trust host key and save");

    assert!(!projection.host_key_prompt_visible);
    assert_eq!(projection.tab_state_text, "connected");
    assert!(projection.tab_has_session);
    let persisted = runtime
        .config_store
        .load_known_hosts()
        .expect("load persisted known_hosts");
    assert!(persisted.get("example.com", 2200).is_some());
}

fn runtime_with_stored_host_key(config_dir: &std::path::Path) -> AppRuntime {
    let store = ConfigStore::new(config_dir);
    let mut known_hosts = KnownHosts::new();
    known_hosts.pin(
        "example.com",
        2200,
        HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "sha256:old".to_owned(),
        },
    );
    store
        .save_known_hosts(&known_hosts)
        .expect("seed known_hosts with the old key");
    AppRuntime::new_with_keychain(config_dir.to_path_buf(), None).expect("runtime without keychain")
}

/// 挂起一次主机密钥确认（`expected = Some(..)` �?changed 模式）�?
/// 返回活动会话 key，供测试检查重连时使用�?effective known_hosts�?
fn queue_host_key_prompt(
    runtime: &mut AppRuntime,
    presented_fingerprint: &str,
    expected_fingerprint: Option<&str>,
) -> String {
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let session_key = runtime.active_session_key().expect("active session");
    runtime.pending_host_key_prompt = Some(PendingHostKeyPrompt {
        session_key: session_key.clone(),
        host: "example.com".to_owned(),
        port: 2200,
        username: "alice".to_owned(),
        presented: HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: presented_fingerprint.to_owned(),
        },
        expected: expected_fingerprint.map(|fingerprint| HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: fingerprint.to_owned(),
        }),
        known_hosts_path: runtime.config_store.known_hosts_file(),
        resume_password: None,
    });
    session_key
}

/// SSH-3：变更模式点"Replace and Trust"= 直接替换旧条目，不再要求输入 REPLACE�?
#[test]
fn changed_host_key_trust_and_save_replaces_stored_entry() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = runtime_with_stored_host_key(temp.path());
    queue_host_key_prompt(&mut runtime, "sha256:new", Some("sha256:old"));

    let projection = runtime
        .trust_host_key_and_save()
        .expect("changed host key should be replaceable without a REPLACE token");

    assert!(!projection.host_key_prompt_visible);
    assert_eq!(projection.tab_state_text, "connected");
    let persisted = runtime
        .config_store
        .load_known_hosts()
        .expect("load persisted known_hosts");
    let stored = persisted.get("example.com", 2200).expect("stored entry");
    assert_eq!(stored.fingerprint, "sha256:new");
    assert!(runtime.pending_host_key_prompt.is_none());
}

/// SSH-3：变更模式点"Trust Once"只写本次运行时的临时表，绝不落盘�?
#[test]
fn changed_host_key_trust_once_does_not_persist() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = runtime_with_stored_host_key(temp.path());
    let session_key = queue_host_key_prompt(&mut runtime, "sha256:new", Some("sha256:old"));

    let projection = runtime
        .trust_host_key_once()
        .expect("trust once must be available for a changed host key");

    assert!(!projection.host_key_prompt_visible);
    assert_eq!(projection.tab_state_text, "connected");
    // 重连使用�?effective known_hosts 里，临时表覆盖了持久表的旧条目�?
    let session_ssh_config = runtime
        .sessions
        .get(&session_key)
        .expect("runtime session")
        .ssh_config
        .clone();
    let effective = runtime.effective_ssh_config(&session_ssh_config);
    assert_eq!(
        effective
            .known_hosts
            .get("example.com", 2200)
            .expect("effective entry")
            .fingerprint,
        "sha256:new"
    );
    let persisted = runtime
        .config_store
        .load_known_hosts()
        .expect("load persisted known_hosts");
    assert_eq!(
        persisted
            .get("example.com", 2200)
            .expect("old entry remains")
            .fingerprint,
        "sha256:old"
    );
    let temporary = runtime
        .temporary_known_hosts
        .get("example.com", 2200)
        .expect("temporary pin exists for this session");
    assert_eq!(temporary.fingerprint, "sha256:new");
    assert!(runtime.pending_host_key_prompt.is_none());
}

/// SSH-3：取消不写任�?known_hosts（持�?临时都不写），也不继续连接�?
#[test]
fn canceling_changed_host_key_writes_nothing() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = runtime_with_stored_host_key(temp.path());
    queue_host_key_prompt(&mut runtime, "sha256:new", Some("sha256:old"));

    let projection = runtime.cancel_host_key_prompt();

    assert!(!projection.host_key_prompt_visible);
    assert!(projection
        .status_text
        .contains("Canceled host key confirmation"));
    assert!(runtime.pending_host_key_prompt.is_none());
    let persisted = runtime
        .config_store
        .load_known_hosts()
        .expect("load persisted known_hosts");
    assert_eq!(
        persisted
            .get("example.com", 2200)
            .expect("old entry remains")
            .fingerprint,
        "sha256:old"
    );
    assert!(runtime
        .temporary_known_hosts
        .get("example.com", 2200)
        .is_none());
}

/// SSH-3：首次信任的 "Trust Once" 同样只写临时表，不落盘�?
#[test]
fn first_trust_once_does_not_persist() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    queue_host_key_prompt(&mut runtime, "sha256:first", None);

    let projection = runtime.trust_host_key_once().expect("trust first key once");

    assert!(!projection.host_key_prompt_visible);
    let persisted = runtime
        .config_store
        .load_known_hosts()
        .expect("load persisted known_hosts");
    assert!(persisted.get("example.com", 2200).is_none());
    let temporary = runtime
        .temporary_known_hosts
        .get("example.com", 2200)
        .expect("temporary pin exists for this session");
    assert_eq!(temporary.fingerprint, "sha256:first");
}

#[test]
fn known_hosts_manager_lists_and_removes_persisted_entries() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut known_hosts = KnownHosts::new();
    known_hosts.pin(
        "alpha.example.test",
        22,
        HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "sha256:alpha".to_owned(),
        },
    );
    known_hosts.pin(
        "beta.example.test",
        2200,
        HostKeyFingerprint {
            algorithm: "ssh-rsa".to_owned(),
            fingerprint: "sha256:beta".to_owned(),
        },
    );
    store
        .save_known_hosts(&known_hosts)
        .expect("save known_hosts");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let opened = runtime.open_known_hosts_manager();
    assert!(opened.known_hosts_modal_visible);
    assert!(opened
        .known_hosts_inventory_rows_text
        .contains("alpha.example.test:22"));
    assert!(opened
        .known_hosts_inventory_rows_text
        .contains("beta.example.test:2200"));
    assert!(!opened.known_hosts_inventory_empty);
    assert!(opened.known_hosts_details_text.contains("Fingerprint:"));
    // D17：详情拆分字段（无英文标签，指纹分组/原样）�?
    assert_eq!(opened.known_hosts_detail_host_text, "alpha.example.test");
    assert_eq!(opened.known_hosts_detail_port_text, "22");
    assert_eq!(opened.known_hosts_detail_algorithm_text, "ssh-ed25519");
    assert_eq!(opened.known_hosts_detail_fingerprint_text, "sha256:alpha");
    assert_eq!(
        opened.known_hosts_detail_path_text,
        runtime
            .config_store
            .known_hosts_file()
            .display()
            .to_string()
    );

    let _ = runtime.select_next_known_host();
    let removed = runtime
        .remove_selected_known_host()
        .expect("remove selected known host");
    assert!(removed.status_text.contains("Removed known host entry"));
    let persisted = runtime
        .config_store
        .load_known_hosts()
        .expect("reload known hosts");
    assert!(persisted.snapshot().len() == 1);
}

#[test]
fn host_keys_manager_detail_fields_match_selected_entry() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut known_hosts = KnownHosts::new();
    known_hosts.pin(
        "gamma.example.test",
        22,
        HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "aabbccddeeff00112233445566778899".to_owned(),
        },
    );
    store
        .save_known_hosts(&known_hosts)
        .expect("save known_hosts");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let opened = runtime.open_host_keys_manager();
    assert!(opened.host_keys_modal_visible);
    // D20：详情拆分字段（无英文标签；�?hex 指纹�?D9 分组）�?
    assert_eq!(opened.host_keys_detail_host_text, "gamma.example.test");
    assert_eq!(opened.host_keys_detail_algorithm_text, "ssh-ed25519");
    assert_eq!(
        opened.host_keys_detail_fingerprint_text,
        "aabb ccdd eeff 0011 2233 4455 6677 8899"
    );
    assert_eq!(
        opened.host_keys_detail_path_text,
        runtime
            .config_store
            .known_hosts_file()
            .display()
            .to_string()
    );
}

#[test]
fn known_hosts_manager_clear_requires_confirmation_and_persists_empty_store() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut known_hosts = KnownHosts::new();
    known_hosts.pin(
        "gamma.example.test",
        2022,
        HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "sha256:gamma".to_owned(),
        },
    );
    store
        .save_known_hosts(&known_hosts)
        .expect("save known_hosts");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let error = runtime
        .clear_all_known_hosts()
        .expect_err("clear should require confirmation");
    assert!(error.message.contains("type CLEAR"));

    let _ = runtime.update_known_hosts_clear_confirmation("CLEAR");
    let cleared = runtime
        .clear_all_known_hosts()
        .expect("clear persisted known hosts");
    assert!(cleared.known_hosts_inventory_empty);
    assert_eq!(cleared.known_hosts_inventory_rows_text, "");
    assert_eq!(cleared.known_hosts_clear_confirmation_text, "");
    assert!(runtime
        .config_store
        .load_known_hosts()
        .expect("reload known hosts")
        .snapshot()
        .is_empty());
}

/// D7：TOFU 顺序——密码型会话的主机密钥确认完成后回到密码弹窗（而不是直接重连：
/// 探针路径下运行时实例尚未建立）�?
#[test]
fn host_key_trust_resumes_password_prompt_for_password_sessions() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    let resume = PendingPasswordPrompt {
        profile_id: "saved-password".to_owned(),
        host: "example.com".to_owned(),
        port: 2200,
        username: "alice".to_owned(),
        auth_method: PendingPasswordAuthMethod::Password,
        reuse_tab_id: None,
        view_mode: crate::session_runtime::SessionViewMode::Terminal,
    };
    runtime.pending_host_key_prompt = Some(PendingHostKeyPrompt {
        session_key: "saved-password-1".to_owned(),
        host: "example.com".to_owned(),
        port: 2200,
        username: "alice".to_owned(),
        presented: HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "sha256:resume".to_owned(),
        },
        expected: None,
        known_hosts_path: runtime.config_store.known_hosts_file(),
        resume_password: Some(resume.clone()),
    });

    let projection = runtime
        .trust_host_key_and_save()
        .expect("trust host key with a password session waiting");

    assert!(!projection.host_key_prompt_visible);
    assert!(projection.password_prompt_visible);
    assert_eq!(
        projection.password_prompt_host_text,
        "alice@example.com:2200"
    );
    // D37：状态栏�?kind 模板（`TextFormats.status-message`），host_text 作为参数�?
    assert_eq!(projection.status_kind, "session-hostkey-confirmed-password");
    assert_eq!(projection.status_param_1, "alice@example.com:2200");
    assert_eq!(projection.status_param_2, "");
    assert!(runtime.pending_host_key_prompt.is_none());
    assert_eq!(runtime.pending_password_prompt.as_ref(), Some(&resume));
    // 主机密钥已经持久化：确认后的密码重试不会再弹 host key 窗口�?
    assert!(runtime
        .config_store
        .load_known_hosts()
        .expect("load known hosts")
        .get("example.com", 2200)
        .is_some());
}

#[test]
fn runtime_new_uses_default_file_keychain_when_master_password_env_is_set() {
    let _guard = lock_env();
    let temp = tempdir().expect("tempdir");
    std::env::set_var("YSHELL_MASTER_PASSWORD", "env-master");

    let secret_store_path = temp.path().join("secret-store.toml");
    let keychain =
        FileKeychain::open_or_create(&secret_store_path, "env-master").expect("file keychain");
    let secret_ref = SecretRef::new("file://saved/password");
    keychain
        .put(secret_ref.clone(), SecretString::from("env-secret"))
        .expect("put secret");

    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    document.auth_profiles.insert(
        "auth-password".to_owned(),
        AuthProfile {
            id: "auth-password".to_owned(),
            name: "Password".to_owned(),
            method: ConfigAuthMethod::Password {
                secret_key: secret_ref.as_str().to_owned(),
            },
        },
    );
    let mut profile = QuickConnectTarget {
        username: Some("root".to_owned()),
        host: "password.example.test".to_owned(),
        port: 22,
    }
    .into_session_profile("saved-password");
    profile.auth_profile_id = Some("auth-password".to_owned());
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save_all(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime
        .open_saved_session("saved-password")
        .expect("open saved session");

    let session_key = runtime.active_session_key().expect("active session");
    let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
    assert!(matches!(
        runtime_session.ssh_config.auth,
        SshAuthMethod::Password {
            ref username,
            ref password
        } if username == "root" && password == "env-secret"
    ));

    std::env::remove_var("YSHELL_MASTER_PASSWORD");
}

#[test]
fn reset_secret_store_requires_exact_confirmation_phrase() {
    let _guard = lock_env();
    let temp = tempdir().expect("tempdir");
    std::env::set_var("YSHELL_MASTER_PASSWORD", "env-master");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.update_secret_reset_confirmation("almost");
    let error = runtime
        .reset_secret_store()
        .expect_err("confirmation should be required");
    assert!(error.to_string().contains("type `RESET SECRETS` exactly"));

    std::env::remove_var("YSHELL_MASTER_PASSWORD");
}

#[test]
fn reset_secret_store_deletes_local_file_and_clears_confirmation() {
    let _guard = lock_env();
    let temp = tempdir().expect("tempdir");
    std::env::set_var("YSHELL_MASTER_PASSWORD", "env-master");

    let secret_store_path = temp.path().join("secret-store.toml");
    let keychain =
        FileKeychain::open_or_create(&secret_store_path, "env-master").expect("file keychain");
    keychain
        .put(
            SecretRef::new("file://saved/password"),
            SecretString::from("env-secret"),
        )
        .expect("put secret");
    assert!(secret_store_path.exists());

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.update_secret_reset_confirmation("RESET SECRETS");
    let projection = runtime.reset_secret_store().expect("reset secret store");

    if secret_store_path.exists() {
        let on_disk = fs::read_to_string(&secret_store_path).expect("read store");
        assert!(!on_disk.contains("env-secret"));
        assert!(!on_disk.contains("file://saved/password"));
    }
    assert_eq!(projection.secret_reset_confirmation_text, "");
    assert!(projection
        .status_text
        .contains("Reset the local secret store"));
    assert_eq!(projection.status_kind, "secrets-reset");
    assert!(projection.status_param_1.ends_with("secret-store.toml"));
    assert_eq!(projection.status_param_2, "");

    std::env::remove_var("YSHELL_MASTER_PASSWORD");
}
