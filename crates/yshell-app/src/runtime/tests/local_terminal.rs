//! TAB-4/TAB-5：`+` 本地终端与冷启动工作区。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tempfile::tempdir;
use yshell_config::NewTabMode;
use yshell_core::SessionState;
use yshell_local::LocalPtyConfig;
use yshell_ssh::{PtySize, ShellSession, SshError, SshErrorKind, SshResult};

use super::*;

fn test_size(columns: u16, rows: u16) -> PtySize {
    PtySize {
        columns,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Deterministic shell with a drop signal, so the close path can assert the
/// process/session was really released (no dangling runtime).
#[derive(Debug)]
struct RecordingShell {
    connected: bool,
    pty_size: PtySize,
    pending: VecDeque<Vec<u8>>,
    dropped: Arc<AtomicBool>,
}

impl RecordingShell {
    fn new(dropped: Arc<AtomicBool>) -> Self {
        let mut pending = VecDeque::new();
        pending.push_back(b"local-shell-ready\n$ ".to_vec());
        Self {
            connected: true,
            pty_size: test_size(80, 24),
            pending,
            dropped,
        }
    }
}

impl Drop for RecordingShell {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl ShellSession for RecordingShell {
    fn is_connected(&self) -> bool {
        self.connected
    }

    fn poll_output(&mut self) -> SshResult<Vec<u8>> {
        Ok(self.pending.pop_front().unwrap_or_default())
    }

    fn write_input(&mut self, bytes: &[u8]) -> SshResult<()> {
        if !self.connected {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "recording shell is disconnected",
            ));
        }
        let text = String::from_utf8_lossy(bytes).replace('\r', "\\r");
        self.pending
            .push_back(format!("local-shell received: {text}\n$ ").into_bytes());
        Ok(())
    }

    fn resize_pty(&mut self, size: PtySize) -> SshResult<()> {
        if !self.connected {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "recording shell is disconnected",
            ));
        }
        self.pty_size = size;
        Ok(())
    }

    fn disconnect(&mut self) -> SshResult<()> {
        self.connected = false;
        Ok(())
    }
}

fn injected_shell(dropped: Arc<AtomicBool>) -> Box<dyn ShellSession> {
    Box::new(RecordingShell::new(dropped))
}

#[test]
fn from_local_uses_local_ids_and_shell_name_targets() {
    let dropped = Arc::new(AtomicBool::new(false));
    let runtime = SessionRuntime::from_local(7, "zsh".to_owned(), injected_shell(dropped));

    assert_eq!(runtime.session_id().as_str(), "local-7");
    assert_eq!(runtime.tab_id().as_str(), "tab-local-7");
    assert!(runtime.is_local());
    assert_eq!(runtime.display_name, "zsh");
    assert_eq!(runtime.state, SessionState::Connected);
    assert_eq!(runtime.connection_target_text(), "zsh");
    assert_eq!(runtime.host_label(), "zsh");
    assert_eq!(runtime.username_label(), "");
    assert!(runtime
        .shell_is_connected()
        .is_some_and(|connected| connected));
}

#[test]
fn local_tab_attaches_shell_receives_input_and_drops_on_close() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);
    let dropped = Arc::new(AtomicBool::new(false));

    let projection = runtime.open_local_terminal_with_shell(
        "zsh",
        test_size(100, 30),
        injected_shell(Arc::clone(&dropped)),
    );

    assert_eq!(projection.tab_count, 1);
    assert_eq!(projection.active_tab_id, "tab-local-1");
    assert_eq!(projection.tabs[0].title, "zsh");
    assert_eq!(projection.tabs[0].state_text, "connected");
    assert!(projection.has_active_session);
    assert!(!projection.quick_connect_visible);
    // 状态栏/目标摘要显示 shell 名，而不是占位 `user@example.com:22`。
    assert_eq!(projection.status_connection_target_text, "zsh");
    assert_eq!(
        runtime.active_session_id.as_deref(),
        Some("local-1"),
        "local session key"
    );

    let session = runtime.sessions.get("local-1").expect("local runtime");
    assert_eq!(session.current_pty_size().columns, 100);
    assert_eq!(session.current_pty_size().rows, 30);
    assert!(session.visible_text().contains("local-shell-ready"));

    runtime
        .send_active_terminal_input("echo hi\n")
        .expect("local input");
    assert!(runtime.sessions["local-1"]
        .visible_text()
        .contains("local-shell received: echo hi"));

    runtime.close_tab_immediate("tab-local-1");

    assert!(
        dropped.load(Ordering::SeqCst),
        "closing the tab must drop the local shell session"
    );
    assert!(runtime.tabs.is_empty());
    assert!(runtime.sessions.is_empty());
    assert!(runtime.active_session_id.is_none());
}

#[test]
fn dropping_the_runtime_releases_local_shells() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);
    let dropped = Arc::new(AtomicBool::new(false));
    runtime.open_local_terminal_with_shell(
        "sh",
        test_size(80, 24),
        injected_shell(Arc::clone(&dropped)),
    );
    assert!(!dropped.load(Ordering::SeqCst));

    drop(runtime);
    assert!(
        dropped.load(Ordering::SeqCst),
        "application exit must release the local shell session"
    );
}

#[cfg(unix)]
#[test]
fn local_shell_spawn_failure_is_reported_without_a_broken_tab() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);
    runtime.dismiss_quick_connect();

    let failed = runtime
        .open_local_terminal_with_config(LocalPtyConfig::new("/nonexistent/yshell-local-shell"));
    assert_eq!(failed.tab_count, 0);
    assert!(failed
        .status_text
        .contains("Could not start the local shell"));
    assert!(runtime.tabs.is_empty());
    assert!(runtime.sessions.is_empty());
    assert!(!failed.has_active_session);

    // 失败后 `+` 仍可直接重试；这里用 /bin/sh 注入成功路径。
    runtime.config_document.ui.new_tab_mode = NewTabMode::SessionEditor;
    let retried =
        runtime.open_local_terminal_with_config(LocalPtyConfig::new("/bin/sh").with_size(90, 24));
    assert_eq!(retried.tab_count, 1);
    assert_eq!(retried.tabs[0].title, "sh");
    assert!(!retried.editor_modal_visible);
}

#[cfg(unix)]
#[test]
fn plus_always_creates_a_new_tab_and_ignores_new_tab_mode() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);

    runtime.config_document.ui.new_tab_mode = NewTabMode::QuickConnect;
    let first = runtime.handle_new_tab_default();
    assert_eq!(first.tab_count, 1);
    assert_eq!(first.tabs[0].title, "New Tab");
    assert_eq!(first.tabs[0].mode_text, "new");
    assert!(first.quick_connect_visible);
    assert!(!first.editor_modal_visible);

    runtime.config_document.ui.new_tab_mode = NewTabMode::SessionEditor;
    let second = runtime.handle_new_tab_default();
    assert_eq!(second.tab_count, 2);
    assert_eq!(second.tabs[1].title, "New Tab");
    assert!(second.quick_connect_visible);
    assert!(!second.editor_modal_visible);

    assert_tab_invariants(&runtime);
}

#[cfg(unix)]
#[test]
fn local_shell_accepts_input_and_returns_output_over_the_pty() {
    let _guard = lock_env();
    std::env::set_var("YSHELL_LOCAL_SHELL", "/bin/sh");
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);

    let projection = runtime.open_local_terminal();
    assert_eq!(projection.tab_count, 1);
    assert_eq!(projection.tabs[0].title, "sh");
    assert_eq!(projection.tabs[0].state_text, "connected");

    runtime
        .send_active_terminal_input("echo __yshell_local_io__\n")
        .expect("write to local PTY");
    let mut seen = false;
    for _ in 0..300 {
        let _ = runtime.poll_all_terminal_outputs();
        if runtime.sessions["local-1"]
            .visible_text()
            .contains("__yshell_local_io__")
        {
            seen = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(seen, "local PTY output should reach the terminal grid");

    runtime.close_tab_immediate("tab-local-1");
    std::env::remove_var("YSHELL_LOCAL_SHELL");
    assert!(runtime.sessions.is_empty());
    assert!(runtime.tabs.is_empty());
}

#[test]
fn startup_opens_one_new_tab() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);
    let dropped = Arc::new(AtomicBool::new(false));

    let projection = runtime.prepare_desktop_startup_workspace_with_shell(
        "sh",
        test_size(120, 32),
        injected_shell(Arc::clone(&dropped)),
    );
    assert_eq!(projection.tab_count, 1);
    assert!(projection.quick_connect_visible);
    assert!(!projection.has_active_session);
    assert_eq!(projection.tabs[0].title, "New Tab");
    assert_eq!(projection.tabs[0].mode_text, "new");
    assert!(runtime.sessions.is_empty());
    assert_eq!(runtime.tabs.len(), 1);

    // 第二次启动/恢复调用不重复创建标签。
    let second = runtime.prepare_desktop_startup_workspace_with_shell(
        "bash",
        test_size(120, 32),
        injected_shell(Arc::new(AtomicBool::new(false))),
    );
    assert_eq!(second.tab_count, 1);
    assert!(second.quick_connect_visible);
    assert_eq!(runtime.sessions.len(), 0);
    assert_eq!(runtime.tabs.len(), 1);
}

#[test]
fn real_startup_path_creates_one_new_tab() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);

    let started = runtime.prepare_desktop_startup_workspace();
    assert_eq!(started.tab_count, 1);
    assert!(started.quick_connect_visible);
    assert_eq!(started.tabs[0].title, "New Tab");
    assert_eq!(started.tabs[0].mode_text, "new");

    let second = runtime.prepare_desktop_startup_workspace();
    assert_eq!(second.tab_count, 1);
    assert!(second.quick_connect_visible);
    assert!(runtime.sessions.is_empty());
    assert_eq!(runtime.tabs.len(), 1);
}

#[test]
fn local_sessions_guard_reconnect_menu_auth_and_sftp_paths() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = tab_runtime(temp.path(), 0);
    let dropped = Arc::new(AtomicBool::new(false));
    runtime.open_local_terminal_with_shell(
        "bash",
        test_size(90, 24),
        injected_shell(Arc::clone(&dropped)),
    );
    // 已连接时 Disconnect 可用，Reconnect 不可用。
    let connected_menu = runtime.prepare_tab_context_menu("tab-local-1");
    assert!(!connected_menu.tab_menu_reconnect_enabled);
    assert!(connected_menu.tab_menu_disconnect_enabled);

    // 即使本地 shell 已退出（state=disconnected），Reconnect 也必须保持禁用；
    // 不能因为状态像断线的 SSH 会话就走重连路径。
    runtime
        .sessions
        .get_mut("local-1")
        .expect("local runtime")
        .set_state(SessionState::Disconnected);

    let menu = runtime.prepare_tab_context_menu("tab-local-1");
    assert!(
        !menu.tab_menu_reconnect_enabled,
        "local tabs cannot reconnect"
    );
    assert!(!menu.tab_menu_disconnect_enabled);

    runtime
        .reconnect_tab_session("tab-local-1")
        .expect("guarded reconnect is a no-op");
    assert!(runtime.status_text.contains("cannot be reconnected"));
    assert_eq!(runtime.tabs.len(), 1);
    assert_eq!(
        runtime.sessions["local-1"].state,
        SessionState::Disconnected,
        "reconnect guard must not run an SSH connection or fake a connected state"
    );
    runtime
        .sessions
        .get_mut("local-1")
        .expect("local runtime")
        .set_state(SessionState::Connected);

    let sftp = runtime
        .refresh_active_sftp_listing()
        .expect("SFTP refresh is guarded");
    assert_eq!(sftp.sftp_session_status_kind_text, "disconnected");
    assert_eq!(sftp.sftp_listing_kind_text, "sync-disconnected");
    assert!(runtime.status_text.contains("SFTP is unavailable"));
    assert!(runtime.sftp_live.is_empty());

    assert!(runtime
        .prepare_active_sftp_operation("browsing remote files")
        .expect("SFTP operation is guarded")
        .is_none());
    assert!(runtime.status_text.contains("SFTP is unavailable"));

    // 认证/主机密钥弹窗不会为本地会话出现；密钥部署也不显示 SSH 目标。
    let projection = runtime.projection();
    assert!(!projection.auth_prompt_visible);
    assert!(!projection.host_key_prompt_visible);
    assert!(projection
        .private_keys_deploy_summary_text
        .contains("Open a session to the target host first"));
    assert!(runtime.save_active_session().is_err());

    // 本地 shell 仍可用：守卫路径没有破坏输入/输出。
    runtime
        .send_active_terminal_input("guard-check\n")
        .expect("local shell input");
    assert!(runtime.sessions["local-1"]
        .visible_text()
        .contains("guard-check"));
    assert!(!dropped.load(Ordering::SeqCst));
}
