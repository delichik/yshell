//! Process-level smoke tests for the shipped binary layout:
//!
//! - `yshell` is the headless **Host entry** (it becomes the singleton Host and
//!   spawns one GUI Shell child). Tests only ever run it with a metadata command
//!   (`--version` / `--help` / `--print-config-dir`) — never bare, because that
//!   would start a real Host + GUI.
//! - `yshell-shell` is the GUI Shell child (normally spawned with `--attach`).
//! - `yshell-session-worker` / `yshell-mcp-daemon` are Host-spawned children.
//!
//! The bare worker/daemon role binaries must fail fast with a clear error; a
//! directly launched GUI Shell must fail cleanly when no Host is running
//! (never open a window). Nothing here starts a Host, a GUI or a session.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Generous upper bound for a role binary that is expected to exit immediately.
const EXIT_TIMEOUT: Duration = Duration::from_secs(20);

/// A Host-spawned Shell launched directly retries the Host bridge before it
/// gives up (see `connect_shell_client_attached`), so allow a little longer.
const SHELL_NO_HOST_TIMEOUT: Duration = Duration::from_secs(45);

fn run(exe: &str, args: &[&str]) -> Output {
    Command::new(exe)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run {exe} with {args:?}: {error}"))
}

fn assert_success(name: &str, output: &Output, args: &str) {
    assert!(
        output.status.success(),
        "{name} {args} failed with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Result of a launch that must terminate on its own; kills the child if it
/// does not (which would mean it opened a GUI or started a real Host).
struct Finished {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Finished {
    fn combined_output(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

fn run_expecting_exit(exe: &str, args: &[&OsStr], context: &str, timeout: Duration) -> Finished {
    // Redirect to files instead of pipes: a hung child cannot block the test on
    // a full pipe while we poll for its exit.
    let logs = tempfile::tempdir().expect("tempdir for role binary logs");
    let stdout_path = logs.path().join("stdout.log");
    let stderr_path = logs.path().join("stderr.log");
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            File::create(&stdout_path).expect("create stdout log"),
        ))
        .stderr(Stdio::from(
            File::create(&stderr_path).expect("create stderr log"),
        ))
        .spawn()
        .unwrap_or_else(|error| panic!("failed to run {exe} {context}: {error}"));

    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait on role binary") {
            return Finished {
                status,
                stdout: fs::read_to_string(&stdout_path).unwrap_or_default(),
                stderr: fs::read_to_string(&stderr_path).unwrap_or_default(),
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "{exe} {context} did not exit within {timeout:?}; \
                 it may have opened a GUI or started a real Host"
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn host_entry_reports_version_help_and_config_dir() {
    // `yshell` is the Host entry: only metadata commands are safe to run here.
    let version = run(env!("CARGO_BIN_EXE_yshell"), &["--version"]);
    assert_success("yshell", &version, "--version");
    let stdout = String::from_utf8_lossy(&version.stdout);
    assert!(
        stdout.contains("yshell"),
        "yshell --version should print the product/version label, got: {stdout:?}"
    );

    let help = run(env!("CARGO_BIN_EXE_yshell"), &["--help"]);
    assert_success("yshell", &help, "--help");
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(
        stdout.contains("YShell") && stdout.contains("USAGE"),
        "yshell --help should print the shared CLI help, got: {stdout:?}"
    );

    let config_dir = tempfile::tempdir().expect("tempdir");
    let output = run_expecting_exit(
        env!("CARGO_BIN_EXE_yshell"),
        &[
            OsStr::new("--config-dir"),
            config_dir.path().as_os_str(),
            OsStr::new("--print-config-dir"),
        ],
        "--print-config-dir",
        EXIT_TIMEOUT,
    );
    assert!(
        output.status.success(),
        "yshell --print-config-dir failed with {}: {}",
        output.status,
        output.stderr
    );
    assert_eq!(
        output.stdout.trim(),
        config_dir.path().to_string_lossy().as_ref(),
        "yshell must honour --config-dir"
    );
}

#[test]
fn role_binaries_report_version_and_help() {
    let binaries = [
        ("yshell-shell", env!("CARGO_BIN_EXE_yshell-shell")),
        (
            "yshell-session-worker",
            env!("CARGO_BIN_EXE_yshell-session-worker"),
        ),
        ("yshell-mcp-daemon", env!("CARGO_BIN_EXE_yshell-mcp-daemon")),
    ];

    for (name, exe) in binaries {
        let version = run(exe, &["--version"]);
        assert_success(name, &version, "--version");
        let stdout = String::from_utf8_lossy(&version.stdout);
        assert!(
            stdout.contains("yshell"),
            "{name} --version should print the product/version label, got: {stdout:?}"
        );

        let help = run(exe, &["--help"]);
        assert_success(name, &help, "--help");
        let stdout = String::from_utf8_lossy(&help.stdout);
        assert!(
            stdout.contains("YShell") && stdout.contains("USAGE"),
            "{name} --help should print the shared CLI help, got: {stdout:?}"
        );
    }
}

#[test]
fn session_worker_binary_fails_clearly_without_spawn_arguments() {
    let output = run_expecting_exit(
        env!("CARGO_BIN_EXE_yshell-session-worker"),
        &[],
        "with no arguments",
        EXIT_TIMEOUT,
    );
    assert!(
        !output.status.success(),
        "a bare yshell-session-worker must exit non-zero"
    );
    let stderr = output.stderr;
    assert!(
        stderr.contains("session-worker requires --ipc-port"),
        "expected a clear session-worker argument error, got: {stderr:?}"
    );
}

#[test]
fn mcp_daemon_binary_fails_clearly_without_a_token() {
    let output = run_expecting_exit(
        env!("CARGO_BIN_EXE_yshell-mcp-daemon"),
        &[],
        "with no arguments",
        EXIT_TIMEOUT,
    );
    assert!(
        !output.status.success(),
        "a bare yshell-mcp-daemon must exit non-zero"
    );
    let stderr = output.stderr;
    assert!(
        stderr.contains("--mcp-daemon requires --mcp-token"),
        "expected a clear mcp-daemon token error, got: {stderr:?}"
    );
}

#[test]
fn shell_binary_without_a_host_fails_cleanly_without_opening_a_gui() {
    // The GUI Shell child is normally spawned by the Host with `--attach`.
    // Launched directly against an empty config dir there is no Host to attach
    // to, so it must report that and exit instead of opening a window.
    let config_dir = tempfile::tempdir().expect("tempdir");
    let output = run_expecting_exit(
        env!("CARGO_BIN_EXE_yshell-shell"),
        &[OsStr::new("--config-dir"), config_dir.path().as_os_str()],
        "with an empty config dir (no Host)",
        SHELL_NO_HOST_TIMEOUT,
    );
    assert!(
        !output.status.success(),
        "yshell-shell without a Host must exit non-zero, got {}; output: {}",
        output.status,
        output.combined_output()
    );
    let combined = output.combined_output().to_ascii_lowercase();
    assert!(
        !combined.trim().is_empty(),
        "yshell-shell must explain why it cannot start, not fail silently"
    );
    assert!(
        combined.contains("host") || combined.contains("attach"),
        "expected an error mentioning the missing Host / attach, got: {combined:?}"
    );
    assert!(
        !combined.contains("panicked"),
        "yshell-shell must fail cleanly, got: {combined:?}"
    );
}
