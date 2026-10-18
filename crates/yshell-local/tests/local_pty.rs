//! Unix integration tests for the local PTY backend.
//!
//! These tests require a POSIX shell at `/bin/sh`, which is available on the
//! Linux CI/dev environment. Windows behavior is covered by the platform code
//! but must be checked manually (see the crate README / task report).

#![cfg(unix)]

use std::thread;
use std::time::{Duration, Instant};

use yshell_local::{LocalPty, LocalPtyConfig, PtySize, ShellSession};

/// Upper bound for any single wait in these tests. Healthy runs finish in tens
/// of milliseconds; the bound only exists so a regression fails instead of
/// hanging the test binary.
const POLL_TIMEOUT: Duration = Duration::from_secs(3);

fn spawn_sh() -> LocalPty {
    LocalPty::spawn(
        LocalPtyConfig::new("/bin/sh")
            .with_env("PS1", "$ ")
            .with_size(80, 24),
    )
    .expect("spawn /bin/sh in a local PTY")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Poll `shell` until `needle` shows up in the cumulative output.
fn poll_until(shell: &mut dyn ShellSession, needle: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + POLL_TIMEOUT;
    let mut collected = Vec::new();
    loop {
        let chunk = shell.poll_output().expect("poll local shell output");
        collected.extend_from_slice(&chunk);
        if contains(&collected, needle) {
            return collected;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {:?}; collected output: {:?}",
            String::from_utf8_lossy(needle),
            String::from_utf8_lossy(&collected)
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn wait_for_disconnect(shell: &mut dyn ShellSession) {
    let deadline = Instant::now() + POLL_TIMEOUT;
    while shell.is_connected() {
        assert!(
            Instant::now() < deadline,
            "shell did not report a disconnect before the timeout"
        );
        let _ = shell.poll_output().expect("poll while waiting for exit");
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn echo_marker_round_trip_then_disconnect_is_prompt() {
    let mut shell = spawn_sh();
    assert!(shell.is_connected());
    assert!(!shell.is_closed());

    // The command line echo contains `yshell-marker-%s`, so matching
    // `yshell-marker-42` proves the command actually produced output.
    shell
        .write_input(b"printf 'yshell-marker-%s\\n' 42\n")
        .expect("write input");
    let output = poll_until(&mut shell, b"yshell-marker-42");
    assert!(contains(&output, b"yshell-marker-42"));

    let started = Instant::now();
    shell.disconnect().expect("disconnect local shell");
    assert!(!shell.is_connected());
    assert!(shell.is_closed());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "disconnect took {:?}",
        started.elapsed()
    );
    assert!(
        shell.exit_status().is_some(),
        "the killed child process should have been reaped"
    );
    assert!(shell.write_input(b"echo nope\n").is_err());
    assert!(shell.resize_pty(PtySize::default()).is_err());
    assert!(shell
        .poll_output()
        .expect("poll after disconnect")
        .is_empty());
    shell.disconnect().expect("disconnect must be idempotent");
}

#[test]
fn initial_size_is_applied() {
    let mut shell = LocalPty::spawn(
        LocalPtyConfig::new("/bin/sh")
            .with_env("PS1", "$ ")
            .with_size(90, 25),
    )
    .expect("spawn /bin/sh with an explicit initial size");

    shell.write_input(b"stty size\n").expect("write stty size");
    let output = poll_until(&mut shell, b"25 90");
    assert!(contains(&output, b"25 90"));
    shell.disconnect().expect("disconnect local shell");
}

#[test]
fn resize_is_applied_to_the_pty() {
    let mut shell = spawn_sh();
    shell
        .resize_pty(PtySize {
            columns: 100,
            rows: 30,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("resize local PTY");
    assert_eq!(shell.size().columns, 100);
    assert_eq!(shell.size().rows, 30);

    shell.write_input(b"stty size\n").expect("write stty size");
    let output = poll_until(&mut shell, b"30 100");
    assert!(contains(&output, b"30 100"));
    shell.disconnect().expect("disconnect local shell");
}

#[test]
fn cwd_env_and_exit_status_are_honored() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let expected_cwd = directory
        .path()
        .canonicalize()
        .expect("canonicalize temporary directory");
    let script = "printf 'cwd=%s\\n' \"$(pwd -P)\"; \
                  printf 'env=%s\\n' \"$YSHELL_LOCAL_TEST_ENV\"; \
                  exit 7";
    let config = LocalPtyConfig::new("/bin/sh")
        .with_args(["-c", script])
        .with_cwd(directory.path())
        .with_env("YSHELL_LOCAL_TEST_ENV", "env-value-9317");
    let mut shell = LocalPty::spawn(config).expect("spawn sh -c");

    let output = poll_until(&mut shell, b"env=env-value-9317");
    let text = String::from_utf8_lossy(&output);
    assert!(
        text.contains(&format!("cwd={}", expected_cwd.display())),
        "expected cwd {} in output: {text:?}",
        expected_cwd.display()
    );
    assert!(
        text.contains("env=env-value-9317"),
        "expected environment variable in output: {text:?}"
    );

    wait_for_disconnect(&mut shell);
    // Reap the exit status in case the reader observed EOF before the poll did.
    let _ = shell.poll_output().expect("poll after shell exit");
    let status = shell
        .exit_status()
        .expect("exit status after the shell exited");
    assert_eq!(status.code(), 7);
    assert!(!status.success());
    shell.disconnect().expect("cleanup after shell exit");
}

#[test]
fn missing_shell_reports_a_readable_error() {
    let error = LocalPty::spawn(LocalPtyConfig::new("/nonexistent/yshell-shell-under-test"))
        .expect_err("spawning a missing shell must fail");

    assert!(
        error
            .message
            .contains("/nonexistent/yshell-shell-under-test"),
        "error should mention the shell path, got: {}",
        error.message
    );
}

#[test]
fn missing_cwd_is_rejected_before_spawning() {
    let error = LocalPty::spawn(
        LocalPtyConfig::new("/bin/sh").with_cwd("/nonexistent/yshell-cwd-under-test"),
    )
    .expect_err("spawning with a missing cwd must fail");

    assert!(
        error.message.contains("/nonexistent/yshell-cwd-under-test"),
        "error should mention the working directory, got: {}",
        error.message
    );
}

#[test]
fn boxed_shell_can_be_driven_through_the_shell_session_trait() {
    let mut shell: Box<dyn ShellSession> =
        LocalPty::spawn_boxed(LocalPtyConfig::new("/bin/sh").with_env("PS1", "$ "))
            .expect("spawn boxed local shell");

    shell
        .write_input(b"printf 'boxed-%s\\n' ok\n")
        .expect("write input");
    let output = poll_until(shell.as_mut(), b"boxed-ok");
    assert!(contains(&output, b"boxed-ok"));

    shell.disconnect().expect("disconnect boxed shell");
    assert!(!shell.is_connected());
}

#[test]
fn dropping_a_running_shell_does_not_hang_or_leak_the_child() {
    let started = Instant::now();
    let process_id = {
        let shell = spawn_sh();
        let process_id = shell.process_id().expect("child process id");
        assert!(shell.is_connected());
        drop(shell);
        process_id
    };
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "drop took {:?}",
        started.elapsed()
    );

    #[cfg(target_os = "linux")]
    assert!(
        !std::path::Path::new(&format!("/proc/{process_id}")).exists(),
        "child process {process_id} still exists after drop"
    );
}

#[test]
fn utf8_output_passes_through_unmodified() {
    let mut shell =
        LocalPty::spawn(LocalPtyConfig::new("/bin/sh").with_args(["-c", "printf 'héllo-世界\\n'"]))
            .expect("spawn sh -c");

    let output = poll_until(&mut shell, "héllo-世界".as_bytes());
    assert!(contains(&output, "héllo-世界".as_bytes()));
    shell.disconnect().expect("disconnect local shell");
}

#[test]
fn polling_an_idle_shell_never_blocks() {
    let mut shell = spawn_sh();

    // Drain the prompt first so the timed loop measures only the empty path.
    let deadline = Instant::now() + POLL_TIMEOUT;
    while Instant::now() < deadline {
        if shell.poll_output().expect("drain prompt").is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }

    let started = Instant::now();
    for _ in 0..32 {
        let _ = shell.poll_output().expect("poll idle shell");
    }
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "32 idle polls took {:?}",
        started.elapsed()
    );
    shell.disconnect().expect("disconnect local shell");
}
