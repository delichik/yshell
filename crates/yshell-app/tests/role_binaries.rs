//! Smoke tests for the thin role binaries (`yshell-host`, `yshell-shell`,
//! `yshell-session-worker`, `yshell-mcp-daemon`).
//!
//! They must all start, expose the shared CLI (`--version` / `--help`), and the
//! Host-spawned roles must fail fast with a clear error when launched bare
//! (no session id / IPC / MCP token). Nothing here starts a real Host or GUI.

use std::process::{Command, Output};

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

#[test]
fn every_role_binary_reports_version_and_help() {
    let binaries = [
        ("yshell", env!("CARGO_BIN_EXE_yshell")),
        ("yshell-host", env!("CARGO_BIN_EXE_yshell-host")),
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
    let output = run(env!("CARGO_BIN_EXE_yshell-session-worker"), &[]);
    assert!(
        !output.status.success(),
        "a bare yshell-session-worker must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("session-worker requires --ipc-port"),
        "expected a clear session-worker argument error, got: {stderr:?}"
    );
}

#[test]
fn mcp_daemon_binary_fails_clearly_without_a_token() {
    let output = run(env!("CARGO_BIN_EXE_yshell-mcp-daemon"), &[]);
    assert!(
        !output.status.success(),
        "a bare yshell-mcp-daemon must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--mcp-daemon requires --mcp-token"),
        "expected a clear mcp-daemon token error, got: {stderr:?}"
    );
}

#[test]
fn shared_flags_still_work_on_role_binaries() {
    // `--check-config` / `--print-config-dir` are pure CLI commands: they prove
    // the role bins keep the shared options without launching a Host or GUI.
    // An explicit `--config-dir` keeps the smoke isolated from user settings.
    let config_dir = std::env::temp_dir().join("yshell-role-binaries-smoke");
    let config_dir = config_dir.to_string_lossy().into_owned();

    for exe in [
        env!("CARGO_BIN_EXE_yshell-host"),
        env!("CARGO_BIN_EXE_yshell-shell"),
        env!("CARGO_BIN_EXE_yshell-session-worker"),
        env!("CARGO_BIN_EXE_yshell-mcp-daemon"),
    ] {
        let output = run(
            exe,
            &["--config-dir", config_dir.as_str(), "--print-config-dir"],
        );
        assert_success(exe, &output, "--print-config-dir");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            config_dir,
            "{exe} must honour --config-dir"
        );
    }
}
