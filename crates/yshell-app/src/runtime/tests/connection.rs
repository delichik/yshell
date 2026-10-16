//! Connect/disconnect state transitions and real-SSH failure paths.

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
        .contains("Test shell established"));
}

#[test]
fn real_ssh_marks_failed_when_non_ssh_target_rejects_handshake() {
    let (port, accept_handle) = start_tcp_probe_target();
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.force_real_ssh_in_tests = true;

    let projection = runtime
        .handle_quick_connect(&format!("alice@127.0.0.1:{port}"))
        .expect("quick connect");
    accept_handle.join().expect("accept thread");

    assert_eq!(projection.tab_state_text, "failed");
    assert!(projection.status_text.contains("Could not connect"));
    assert!(!projection
        .terminal_body_text
        .contains("Shell runtime failed"));
}

#[test]
fn desktop_startup_projection_is_ready_without_backend_labels() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let projection = runtime.prepare_desktop_startup_projection();

    assert!(projection.status_text.contains("Saved sessions discovered"));
    assert!(!projection.status_text.contains("fake"));
    assert!(!projection.status_text.contains("native-ssh"));
    assert_eq!(projection.sftp_listing_kind_text, "connect-real");
    assert_eq!(projection.sftp_session_status_kind_text, "disconnected");
}

/// N4: auth window updates in place on retry failure (keeps typed values).
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

    assert!(runtime.apply_auth_prompt_from_error(context("session-2"), &retry));
    let prompt = runtime
        .pending_auth_prompt
        .as_ref()
        .expect("pending prompt");
    assert!(prompt.password_text.is_empty());
    assert_eq!(prompt.attempts, 1);
}
