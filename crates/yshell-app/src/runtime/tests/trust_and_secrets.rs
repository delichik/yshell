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
    assert!(projection.sftp_visible);
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

#[test]
fn replace_host_key_requires_confirmation_text() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let session_key = runtime.active_session_key().expect("active session");
    runtime.pending_host_key_prompt = Some(PendingHostKeyPrompt {
        session_key,
        host: "example.com".to_owned(),
        port: 2200,
        username: "alice".to_owned(),
        presented: HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "sha256:new".to_owned(),
        },
        expected: Some(HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "sha256:old".to_owned(),
        }),
        known_hosts_path: runtime.config_store.known_hosts_file(),
    });

    let error = runtime
        .replace_host_key_and_connect()
        .expect_err("replace should require confirmation");

    assert!(error.message.contains("type REPLACE"));
    assert!(runtime.pending_host_key_prompt.is_some());
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
    store.save(&document).expect("save config");

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
