//! Saved-session auth profiles: secrets, password/keyboard-interactive prompts and retries.

use std::{fs, sync::Arc};
use tempfile::tempdir;
use yshell_config::{
    AuthMethod as ConfigAuthMethod, AuthProfile, ConfigDocument, ConfigStore, FolderProfile,
    HostKeyPolicy as ConfigHostKeyPolicy, QuickConnectTarget,
};
use yshell_secret::{FakeKeychain, Keychain, SecretRef, SecretString};
use yshell_ssh::{AuthMethod as SshAuthMethod, HostKeyPolicy};

use super::*;

#[test]
fn quick_connect_enters_runtime_pipeline() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");

    let projection = runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");

    assert_eq!(projection.active_session_kind_text, "session");
    assert!(projection
        .active_session_name_text
        .contains("alice@example.com:2200"));
    assert_eq!(projection.tab_state_text, "connected");
    // D15：连接成功写用户向文案（kind 供 Slint @tr 模板映射）。
    assert_eq!(projection.status_kind, "session-connected");
    assert!(projection
        .status_text
        .contains("Connected to alice@example.com:2200"));
    assert!(projection.tab_name_text.contains("alice@example.com:2200"));
    assert!(projection
        .terminal_body_text
        .contains("Fake shell established"));
    assert_eq!(runtime.emitted_events().len(), 1);
}

#[test]
fn hydrates_saved_sessions_from_config() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(
        QuickConnectTarget {
            username: Some("ops".to_owned()),
            host: "saved.example.test".to_owned(),
            port: 22,
        }
        .into_session_profile("saved-session-1"),
    );
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    assert_eq!(runtime.saved_session_profiles().len(), 1);
    let projection = runtime.projection();
    assert_eq!(projection.active_session_kind_text, "saved-sessions");
    assert_eq!(projection.saved_session_count, 1);
    assert!(projection
        .saved_session_selection_name_text
        .contains("saved.example.test"));
    assert!(projection
        .saved_session_selection_kind_text
        .contains("profile"));
    assert!(projection
        .saved_session_inventory_rows_text
        .contains("[folder] Saved Sessions"));
}

#[test]
fn saved_session_agent_auth_profile_is_applied_to_runtime_shell_config() {
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
        host: "agent.example.test".to_owned(),
        port: 22,
    }
    .into_session_profile("saved-agent");
    profile.auth_profile_id = Some("auth-agent".to_owned());
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime
        .open_saved_session("saved-agent")
        .expect("open saved session");

    let session_key = runtime.active_session_key().expect("active session");
    let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
    assert!(matches!(
        runtime_session.ssh_config.auth,
        SshAuthMethod::Agent { ref username } if username == "ops"
    ));
    assert_eq!(
        runtime_session.ssh_config.host_key_policy,
        HostKeyPolicy::Strict
    );
}

#[test]
fn saved_session_host_key_policy_is_applied_to_runtime_shell_config() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    let mut profile = QuickConnectTarget {
        username: Some("ops".to_owned()),
        host: "host-key.example.test".to_owned(),
        port: 22,
    }
    .into_session_profile("saved-host-key");
    profile.host_key_policy = Some(ConfigHostKeyPolicy::TrustOnFirstUse);
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime
        .open_saved_session("saved-host-key")
        .expect("open saved session");

    let session_key = runtime.active_session_key().expect("active session");
    let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
    assert_eq!(
        runtime_session.ssh_config.host_key_policy,
        HostKeyPolicy::TrustOnFirstUse
    );
}

#[test]
fn runtime_shell_config_carries_host_key_policy_before_connect() {
    // D7：密码挂起前的主机密钥探针读取 `runtime.ssh_config`（activate 之后才被
    // resolved 配置覆盖），所以 `from_profile` 必须先把会话策略装进运行时配置，
    // 否则探针退回 Strict，对 accept_any_for_testing 会话错误地弹确认框。
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    let mut profile = QuickConnectTarget {
        username: Some("ops".to_owned()),
        host: "probe.example.test".to_owned(),
        port: 22,
    }
    .into_session_profile("saved-probe");
    profile.host_key_policy = Some(ConfigHostKeyPolicy::AcceptAnyForTesting);
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let profile = document.find_session("saved-probe").expect("session");
    let runtime_session = SessionRuntime::from_profile(profile, 0);
    assert_eq!(
        runtime_session.ssh_config.host_key_policy,
        HostKeyPolicy::AcceptAnyForTesting
    );
}

#[test]
fn saved_session_password_auth_profile_uses_secret_from_keychain() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let secret_ref = SecretRef::new("fake://yshell/password");
    let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("master"));
    keychain
        .put(secret_ref.clone(), SecretString::from("hunter2"))
        .expect("put secret");

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

    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain)).expect("runtime");
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
        } if username == "root" && password == "hunter2"
    ));
}

#[test]
fn saved_session_keyboard_interactive_auth_profile_uses_secret_from_keychain() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let secret_ref = SecretRef::new("fake://yshell/keyboard-interactive");
    let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("master"));
    keychain
        .put(secret_ref.clone(), SecretString::from("one-time-secret"))
        .expect("put secret");

    let mut document = ConfigDocument::default();
    document.auth_profiles.insert(
        "auth-kbdint".to_owned(),
        AuthProfile {
            id: "auth-kbdint".to_owned(),
            name: "Keyboard Interactive".to_owned(),
            method: ConfigAuthMethod::KeyboardInteractive {
                secret_key: secret_ref.as_str().to_owned(),
            },
        },
    );
    let mut profile = QuickConnectTarget {
        username: Some("root".to_owned()),
        host: "kbdint.example.test".to_owned(),
        port: 22,
    }
    .into_session_profile("saved-kbdint");
    profile.auth_profile_id = Some("auth-kbdint".to_owned());
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain)).expect("runtime");
    let _ = runtime
        .open_saved_session("saved-kbdint")
        .expect("open saved session");

    let session_key = runtime.active_session_key().expect("active session");
    let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
    assert!(matches!(
        runtime_session.ssh_config.auth,
        SshAuthMethod::KeyboardInteractive {
            ref username,
            ref secret
        } if username == "root" && secret == "one-time-secret"
    ));
}

#[test]
fn saved_session_private_key_auth_profile_resolves_passphrase_secret() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let secret_ref = SecretRef::new("fake://yshell/passphrase");
    let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("master"));
    keychain
        .put(secret_ref.clone(), SecretString::from("open-sesame"))
        .expect("put secret");

    let mut document = ConfigDocument::default();
    document.auth_profiles.insert(
        "auth-key".to_owned(),
        AuthProfile {
            id: "auth-key".to_owned(),
            name: "Private Key".to_owned(),
            method: ConfigAuthMethod::PrivateKey {
                key_id: None,
                path: "/tmp/id_ed25519".to_owned(),
                passphrase_secret_key: Some(secret_ref.as_str().to_owned()),
            },
        },
    );
    let mut profile = QuickConnectTarget {
        username: Some("deploy".to_owned()),
        host: "key.example.test".to_owned(),
        port: 22,
    }
    .into_session_profile("saved-key");
    profile.auth_profile_id = Some("auth-key".to_owned());
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain)).expect("runtime");
    let _ = runtime
        .open_saved_session("saved-key")
        .expect("open saved session");

    let session_key = runtime.active_session_key().expect("active session");
    let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
    assert!(matches!(
        runtime_session.ssh_config.auth,
        SshAuthMethod::PrivateKey {
            ref username,
            ref key_path,
            ref passphrase
        } if username == "deploy"
            && key_path == "/tmp/id_ed25519"
            && passphrase.as_deref() == Some("open-sesame")
    ));
}

#[test]
fn saved_session_password_auth_profile_prompts_without_keychain() {
    let _guard = lock_env();
    std::env::remove_var("YSHELL_MASTER_PASSWORD");
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    document.auth_profiles.insert(
        "auth-password".to_owned(),
        AuthProfile {
            id: "auth-password".to_owned(),
            name: "Password".to_owned(),
            method: ConfigAuthMethod::Password {
                secret_key: "fake://yshell/missing".to_owned(),
            },
        },
    );
    let mut profile = QuickConnectTarget {
        username: Some("root".to_owned()),
        host: "password.example.test".to_owned(),
        port: 2200,
    }
    .into_session_profile("saved-password");
    profile.auth_profile_id = Some("auth-password".to_owned());
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    let projection = runtime
        .open_saved_session("saved-password")
        .expect("missing password should suspend the connection, not error");

    // 投影：弹窗可见 + user@host:port 文案；连接尚未创建。
    assert!(projection.password_prompt_visible);
    assert_eq!(
        projection.password_prompt_host_text,
        "root@password.example.test:2200"
    );
    assert!(projection.status_text.contains("Password required"));
    assert!(runtime.active_session_id.is_none());

    // 挂起目标保存了重试所需的最小上下文。
    let pending = runtime
        .pending_password_prompt
        .as_ref()
        .expect("pending password prompt");
    assert_eq!(
        pending,
        &PendingPasswordPrompt {
            profile_id: "saved-password".to_owned(),
            host: "password.example.test".to_owned(),
            port: 2200,
            username: "root".to_owned(),
            auth_method: PendingPasswordAuthMethod::Password,
        }
    );
}

#[test]
fn submit_password_retries_saved_session_and_connects() {
    let _guard = lock_env();
    std::env::remove_var("YSHELL_MASTER_PASSWORD");
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    document.auth_profiles.insert(
        "auth-password".to_owned(),
        AuthProfile {
            id: "auth-password".to_owned(),
            name: "Password".to_owned(),
            method: ConfigAuthMethod::Password {
                secret_key: "fake://yshell/missing".to_owned(),
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
    let config_before = fs::read(temp.path().join("config.toml")).expect("read config");

    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    let prompted = runtime
        .open_saved_session("saved-password")
        .expect("open saved session");
    assert!(prompted.password_prompt_visible);

    // fake 后端：带密码重试后进入 connected。
    // 说明：fake 后端不校验密码正确性，"密码错误 → 认证失败"只能在
    // native-ssh + 真实服务器上验证；这里断言的是"挂起 → 重试 → 连接成功"
    // 这条状态机，以及密码不写密钥库/配置文件。
    let projection = runtime
        .submit_password("hunter2")
        .expect("submit password should retry the connection");
    assert!(!projection.password_prompt_visible);
    assert!(projection.has_active_session);
    assert!(projection.active_session_connected);
    assert_eq!(projection.active_session_state_text, "connected");
    assert!(runtime.pending_password_prompt.is_none());

    // 密码只存在于本次运行时的 ssh 配置里；密钥库与配置文件都不应被写入。
    let session_key = runtime.active_session_key().expect("active session");
    let session = runtime.sessions.get(&session_key).expect("runtime session");
    assert!(matches!(
        session.ssh_config.auth,
        SshAuthMethod::Password {
            ref username,
            ref password
        } if username == "root" && password == "hunter2"
    ));
    assert!(runtime.keychain.is_none());
    let config_after = fs::read(temp.path().join("config.toml")).expect("read config");
    assert_eq!(
        config_before, config_after,
        "password retry must not write config"
    );
    assert!(
        !fs::read_dir(temp.path())
            .expect("temp dir")
            .flatten()
            .any(|entry| entry.file_name() == "secret-store.toml"),
        "password retry must not create a secret store"
    );
}

#[test]
fn submit_password_reports_failure_and_closes_prompt_when_target_is_gone() {
    let _guard = lock_env();
    std::env::remove_var("YSHELL_MASTER_PASSWORD");
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    document.auth_profiles.insert(
        "auth-password".to_owned(),
        AuthProfile {
            id: "auth-password".to_owned(),
            name: "Password".to_owned(),
            method: ConfigAuthMethod::Password {
                secret_key: "fake://yshell/missing".to_owned(),
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

    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    let prompted = runtime
        .open_saved_session("saved-password")
        .expect("open saved session");
    assert!(prompted.password_prompt_visible);

    // 弹窗挂起后目标被删掉：重试失败写进 status_text，弹窗关闭。
    let deleted = runtime
        .delete_selected_saved_session()
        .expect("delete the prompted saved session");
    assert!(!deleted.has_saved_selection);

    let projection = runtime
        .submit_password("hunter2")
        .expect("failure is reported through the projection");
    assert!(!projection.password_prompt_visible);
    assert!(!projection.has_active_session);
    assert!(projection.status_text.contains("Could not connect"));
    assert!(runtime.pending_password_prompt.is_none());
}

#[test]
fn submit_password_reports_auth_failure_from_native_backend() {
    // native-ssh + 本地 TCP 探针：握手必失败，用来验证"密码重试失败"会写进
    // status_text（fake 后端不会失败，覆盖不到这条路径）。
    let (port, accept_handle) = start_tcp_probe_target();
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    document.auth_profiles.insert(
        "auth-password".to_owned(),
        AuthProfile {
            id: "auth-password".to_owned(),
            name: "Password".to_owned(),
            method: ConfigAuthMethod::Password {
                secret_key: "fake://yshell/missing".to_owned(),
            },
        },
    );
    let mut profile = QuickConnectTarget {
        username: Some("alice".to_owned()),
        host: "127.0.0.1".to_owned(),
        port,
    }
    .into_session_profile("saved-password");
    profile.auth_profile_id = Some("auth-password".to_owned());
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    let _ = runtime.select_native_ssh_transport_backend();
    let prompted = runtime
        .open_saved_session("saved-password")
        .expect("open saved session");
    assert!(prompted.password_prompt_visible);

    let projection = runtime
        .submit_password("wrong-password")
        .expect("retry reports failure through the projection");
    accept_handle.join().expect("accept thread");

    assert!(!projection.password_prompt_visible);
    assert_eq!(projection.tab_state_text, "failed");
    assert!(projection.status_text.contains("Could not connect"));
    // D1：内部失败诊断不再写进终端网格。
    assert!(!projection
        .terminal_body_text
        .contains("Shell runtime failed"));
    assert!(runtime.pending_password_prompt.is_none());
}

#[test]
fn cancel_password_prompt_clears_pending_state() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    runtime.pending_password_prompt = Some(PendingPasswordPrompt {
        profile_id: "saved-password".to_owned(),
        host: "password.example.test".to_owned(),
        port: 22,
        username: "root".to_owned(),
        auth_method: PendingPasswordAuthMethod::KeyboardInteractive,
    });
    assert!(runtime.projection().password_prompt_visible);
    assert_eq!(
        runtime.projection().password_prompt_host_text,
        "root@password.example.test:22"
    );

    let projection = runtime.cancel_password_prompt();
    assert!(!projection.password_prompt_visible);
    assert!(projection.password_prompt_host_text.is_empty());
    assert!(runtime.pending_password_prompt.is_none());

    // 没有挂起目标时提交密码是显式错误（UI 层不应触发）。
    let error = runtime
        .submit_password("hunter2")
        .expect_err("submit without a pending prompt must fail");
    assert!(error.to_string().contains("no password prompt is pending"));
}

#[test]
fn saved_session_keyboard_interactive_auth_prompts_without_keychain() {
    let _guard = lock_env();
    std::env::remove_var("YSHELL_MASTER_PASSWORD");
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    document.auth_profiles.insert(
        "auth-kbdint".to_owned(),
        AuthProfile {
            id: "auth-kbdint".to_owned(),
            name: "Keyboard Interactive".to_owned(),
            method: ConfigAuthMethod::KeyboardInteractive {
                secret_key: "fake://yshell/missing-kbdint".to_owned(),
            },
        },
    );
    let mut profile = QuickConnectTarget {
        username: Some("ops".to_owned()),
        host: "kbdint.example.test".to_owned(),
        port: 22,
    }
    .into_session_profile("saved-kbdint");
    profile.auth_profile_id = Some("auth-kbdint".to_owned());
    let mut folder = FolderProfile::new("saved-sessions", "Saved Sessions");
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    let projection = runtime
        .open_saved_session("saved-kbdint")
        .expect("keyboard-interactive should prompt too");
    assert!(projection.password_prompt_visible);
    assert_eq!(
        runtime
            .pending_password_prompt
            .as_ref()
            .map(|prompt| prompt.auth_method),
        Some(PendingPasswordAuthMethod::KeyboardInteractive)
    );
}

/// D6：认证弹窗副标题 `user@host:port` 只允许出现一次端口（回归 B 轮5 高：
/// `tester@127.0.0.1:2222:2222`）。
#[test]
fn auth_prompt_host_shows_port_once() {
    use crate::runtime::connection::AuthPromptContext;
    use yshell_ssh::{AuthMethods, AuthProblemKind, SshError, SshErrorKind};

    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    let _ = runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let session_key = runtime.active_session_key().expect("active session");
    let session = runtime.sessions.get(&session_key).expect("runtime session");

    // D6：生产路径与测试共用 `AuthPromptContext::for_runtime`（host 只放裸主机），
    // 合成认证错误驱动弹窗，副标题只应出现一次端口。
    let context = AuthPromptContext::for_runtime(&session_key, session);
    assert_eq!(context.host, "example.com");
    let error = SshError::new(SshErrorKind::Authentication, "denied").with_auth_context(
        Some(AuthMethods::parse("password").expect("methods")),
        AuthProblemKind::InvalidCredentials,
    );
    assert!(runtime.apply_auth_prompt_from_error(context, &error));

    let projection = runtime.projection();
    assert!(projection.auth_prompt_visible);
    assert_eq!(projection.auth_prompt_host_text, "alice@example.com:2200");

    // 密码挂起路径同样只拼一次端口。
    let prompt = PendingPasswordPrompt {
        profile_id: "saved-password".to_owned(),
        host: "example.com".to_owned(),
        port: 2200,
        username: "alice".to_owned(),
        auth_method: PendingPasswordAuthMethod::Password,
    };
    assert_eq!(prompt.host_text(), "alice@example.com:2200");
}
