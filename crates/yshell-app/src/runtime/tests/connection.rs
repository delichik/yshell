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
fn connect_job_messages_mark_the_ui_dirty() {
    use crate::connect_jobs::{ConnectJobMessage, ConnectJobOutcome};
    use yshell_ssh::{SshError, SshErrorKind};

    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let session_key = runtime.active_session_key().expect("active session");
    // Quick connect returns a projection directly; clear whatever dirty state
    // it left so the assertions below only observe connect-job updates.
    let _ = runtime.take_ui_dirty();

    runtime.apply_connect_job_message(ConnectJobMessage::Progress {
        session_key: session_key.clone(),
        stage: "handshake".to_owned(),
    });
    assert!(
        runtime.take_ui_dirty(),
        "a progress update from the Host supervisor must repaint the Shell overlay"
    );

    runtime.apply_connect_job_message(ConnectJobMessage::Finished {
        session_key,
        password_retry: false,
        is_reconnect: false,
        outcome: ConnectJobOutcome::Failed {
            error: SshError::new(SshErrorKind::Handshake, "handshake failed"),
        },
    });
    assert!(
        runtime.take_ui_dirty(),
        "a failed connect must repaint the Shell so the overlay leaves the spinner"
    );
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

#[derive(Debug)]
struct StubShellSession {
    connected: bool,
    disconnect_reason: Option<String>,
    output: std::collections::VecDeque<Vec<u8>>,
}

impl StubShellSession {
    fn new(connected: bool, disconnect_reason: Option<String>) -> Self {
        Self {
            connected,
            disconnect_reason,
            output: std::collections::VecDeque::new(),
        }
    }

    fn with_output(
        connected: bool,
        disconnect_reason: Option<String>,
        chunks: Vec<Vec<u8>>,
    ) -> Self {
        Self {
            connected,
            disconnect_reason,
            output: chunks.into(),
        }
    }
}

impl yshell_ssh::ShellSession for StubShellSession {
    fn is_connected(&self) -> bool {
        self.connected
    }

    fn poll_output(&mut self) -> yshell_ssh::SshResult<Vec<u8>> {
        Ok(self.output.pop_front().unwrap_or_default())
    }

    fn write_input(&mut self, _bytes: &[u8]) -> yshell_ssh::SshResult<()> {
        Ok(())
    }

    fn resize_pty(&mut self, _size: yshell_ssh::PtySize) -> yshell_ssh::SshResult<()> {
        Ok(())
    }

    fn disconnect(&mut self) -> yshell_ssh::SshResult<()> {
        self.connected = false;
        Ok(())
    }

    fn disconnect_reason(&self) -> Option<String> {
        self.disconnect_reason.clone()
    }
}

#[test]
fn mark_session_worker_exited_surfaces_reason_and_marks_ui_dirty() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let session_key = runtime.active_session_key().expect("active session");
    let reason =
        "Io: failed while reading SSH stdout: transport read ([Session(-43)] transport read)";
    runtime
        .sessions
        .get_mut(&session_key)
        .expect("session")
        .attach_shell_session(Box::new(StubShellSession::new(
            false,
            Some(reason.to_owned()),
        )));
    let _ = runtime.take_ui_dirty();

    runtime.mark_session_worker_exited(&session_key);

    assert!(
        runtime.take_ui_dirty(),
        "unexpected worker exit must mark UI dirty so the Host tick repaints"
    );
    let session = runtime.sessions.get(&session_key).expect("session");
    assert_eq!(session.state, yshell_core::SessionState::Failed);
    assert_eq!(session.connection_error, reason);
    let projection = runtime.projection();
    assert!(projection.connection_overlay_failed);
    assert_eq!(projection.connection_overlay_error_text, reason);

    // Clean EOF already marked Disconnected without an error reason must not be
    // overwritten into Failed when the worker process reaps afterwards.
    runtime
        .handle_quick_connect("bob@example.com:2200")
        .expect("quick connect clean");
    let clean_key = runtime.active_session_key().expect("clean session");
    runtime
        .sessions
        .get_mut(&clean_key)
        .expect("clean session")
        .set_state(yshell_core::SessionState::Disconnected);
    runtime
        .sessions
        .get_mut(&clean_key)
        .expect("clean session")
        .attach_shell_session(Box::new(StubShellSession::new(false, None)));
    runtime.mark_session_worker_exited(&clean_key);
    assert_eq!(
        runtime.sessions.get(&clean_key).expect("clean").state,
        yshell_core::SessionState::Disconnected
    );
}

#[test]
fn mark_session_worker_exited_preserves_clean_exit_and_final_output_before_poll() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let session_key = runtime.active_session_key().expect("active session");
    runtime
        .sessions
        .get_mut(&session_key)
        .expect("session")
        .attach_shell_session(Box::new(StubShellSession::with_output(
            false,
            None,
            vec![b"logout\r\n".to_vec()],
        )));
    let _ = runtime.take_ui_dirty();

    runtime.mark_session_worker_exited(&session_key);

    assert!(
        runtime.take_ui_dirty(),
        "clean worker exit before tick poll must mark UI dirty"
    );
    let session = runtime.sessions.get(&session_key).expect("session");
    assert_eq!(session.state, yshell_core::SessionState::Disconnected);
    assert!(session.connection_error.is_empty());
    assert!(session.visible_text().contains("logout"));
}

#[test]
fn poll_session_output_distinguishes_worker_failure_from_clean_eof() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect failed");
    let failed_key = runtime.active_session_key().expect("failed session");
    runtime
        .sessions
        .get_mut(&failed_key)
        .expect("session")
        .attach_shell_session(Box::new(StubShellSession::new(
            false,
            Some("Io: transport read".to_owned()),
        )));
    runtime
        .poll_session_output(&failed_key)
        .expect("poll failed session");
    let failed_session = runtime.sessions.get(&failed_key).expect("failed session");
    assert_eq!(failed_session.state, yshell_core::SessionState::Failed);
    assert_eq!(failed_session.connection_error, "Io: transport read");

    runtime
        .handle_quick_connect("bob@example.com:2200")
        .expect("quick connect clean");
    let clean_key = runtime.active_session_key().expect("clean session");
    runtime
        .sessions
        .get_mut(&clean_key)
        .expect("session")
        .attach_shell_session(Box::new(StubShellSession::new(false, None)));
    runtime
        .poll_session_output(&clean_key)
        .expect("poll clean session");
    let clean_session = runtime.sessions.get(&clean_key).expect("clean session");
    assert_eq!(clean_session.state, yshell_core::SessionState::Disconnected);
    assert!(clean_session.connection_error.is_empty());
}
