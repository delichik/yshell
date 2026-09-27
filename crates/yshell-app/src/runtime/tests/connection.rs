//! Transport backend selection and connect/disconnect state transitions.

use tempfile::tempdir;

use super::*;

#[test]
fn disconnect_and_reconnect_update_runtime_state() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");

    let disconnected = runtime
        .disconnect_active_session()
        .expect("disconnect active session");
    assert_eq!(disconnected.tab_state_text, "disconnected");

    let reconnected = runtime
        .reconnect_active_session()
        .expect("reconnect active session");
    assert_eq!(reconnected.tab_state_text, "connected");
    assert!(reconnected
        .terminal_body_text
        .contains("Fake shell established"));
}

#[test]
fn native_ssh_backend_marks_failed_when_non_ssh_target_rejects_native_handshake() {
    let (port, accept_handle) = start_tcp_probe_target();
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let projection = runtime.select_native_ssh_transport_backend();
    assert_eq!(projection.transport_backend_text, "native-ssh");

    let projection = runtime
        .handle_quick_connect(&format!("alice@127.0.0.1:{port}"))
        .expect("quick connect");
    accept_handle.join().expect("accept thread");

    assert_eq!(projection.tab_state_text, "failed");
    assert!(projection
        .status_text
        .contains("live shell was not reached"));
    assert!(projection
        .terminal_body_text
        .contains("Shell runtime failed"));
}

#[test]
fn transport_backend_selection_is_runtime_visible() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let fake_projection = runtime.select_fake_transport_backend();
    assert_eq!(fake_projection.transport_backend_text, "fake");
    assert!(fake_projection
        .status_text
        .contains("deterministic in-process shell adapter"));

    let native_projection = runtime.select_native_ssh_transport_backend();
    assert_eq!(native_projection.transport_backend_text, "native-ssh");
    assert!(native_projection
        .status_text
        .contains("embedded ssh2 shell path"));
}

#[test]
fn desktop_startup_defaults_to_native_ssh_without_losing_startup_context() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let projection = runtime.prepare_desktop_startup_projection();

    assert_eq!(projection.transport_backend_text, "native-ssh");
    assert!(projection.status_text.contains("Saved sessions discovered"));
    // 启动文案保持 `startup_status()`，不再追加/出现任何后端字样（S2/D26）。
    assert!(!projection.status_text.contains("fake"));
    assert!(!projection.status_text.contains("native-ssh"));
    assert_eq!(projection.sftp_listing_kind_text, "native-ssh-selected");
    assert_eq!(projection.sftp_session_status_kind_text, "disconnected");
}

/// N4：认证弹窗在"同一会话重试再次失败"时原地更新（保留已填内容，设计 §2），
/// 其它会话的错误则挂起新的弹窗。
#[test]
fn failed_auth_retry_updates_the_prompt_in_place_and_keeps_typed_values() {
    use yshell_ssh::{AuthMethods, AuthProblemKind, SshError, SshErrorKind};

    use crate::runtime::connection::AuthPromptContext;

    fn context(session_key: &str) -> AuthPromptContext {
        AuthPromptContext {
            session_key: session_key.to_owned(),
            profile_id: None,
            host: "example.test".to_owned(),
            port: 22,
            username: "alice".to_owned(),
        }
    }

    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let error = SshError::new(SshErrorKind::Authentication, "denied").with_auth_context(
        Some(AuthMethods::parse("password,publickey").expect("methods")),
        AuthProblemKind::InvalidCredentials,
    );
    assert!(runtime.apply_auth_prompt_from_error(context("session-1"), &error));
    runtime.update_auth_prompt_password("hunter2");
    runtime.toggle_auth_prompt_remember_password(true);

    // 同一会话重试再次失败：不新建弹窗，只刷新服务端方式与错误条。
    let retry = SshError::new(SshErrorKind::Authentication, "denied again").with_auth_context(
        Some(AuthMethods::parse("password").expect("methods")),
        AuthProblemKind::InvalidCredentials,
    );
    assert!(runtime.apply_auth_prompt_from_error(context("session-1"), &retry));

    let prompt = runtime
        .pending_auth_prompt
        .as_ref()
        .expect("pending prompt");
    assert_eq!(prompt.password_text, "hunter2");
    assert!(prompt.remember_password);
    assert_eq!(prompt.attempts, 2);
    assert!(prompt.method_allowed(AuthPromptMethod::Password));
    assert!(!prompt.method_allowed(AuthPromptMethod::PublicKey));

    // 另一个会话的认证错误：新弹窗（不继承上一个会话的输入）。
    assert!(runtime.apply_auth_prompt_from_error(context("session-2"), &retry));
    let prompt = runtime
        .pending_auth_prompt
        .as_ref()
        .expect("pending prompt");
    assert!(prompt.password_text.is_empty());
    assert_eq!(prompt.attempts, 1);
}
