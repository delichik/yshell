//! Role-process debug logging.
//!
//! When debug logging is enabled the Host publishes
//! `{config_dir}/logs/debug-logging.enabled`; every role child follows that
//! control file and appends to its own `yshell-*-debug.log`. These tests launch
//! the real SessionWorker / MCP daemon / GUI Shell binaries against a
//! `--config-dir` with no Host behind them, so each child starts logging, then
//! fails fast (and is killed early in the Shell case).

use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Generous upper bound: both roles are expected to exit immediately.
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Mirrors `yshell_logging::set_debug_log_control(.., true)` without depending
/// on the logging crate from this integration test crate.
fn enable_debug_logging(config_dir: &Path) {
    let logs = config_dir.join("logs");
    fs::create_dir_all(&logs).expect("create debug log directory");
    fs::write(logs.join("debug-logging.enabled"), b"1\n").expect("write debug log control file");
}

fn free_port() -> String {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind free port");
    listener
        .local_addr()
        .expect("local addr")
        .port()
        .to_string()
}

/// Runs a role binary that must terminate by itself (it has no Host to talk to).
fn run_role(exe: &str, args: &[&OsStr], context: &str) -> Output {
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("failed to run {context}: {error}"));

    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        if child.try_wait().expect("try_wait on role binary").is_some() {
            return child
                .wait_with_output()
                .unwrap_or_else(|error| panic!("failed to collect {context} output: {error}"));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{context} did not exit within {EXIT_TIMEOUT:?}");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn session_worker_writes_detailed_debug_log_when_enabled() {
    let config = tempfile::tempdir().expect("tempdir");
    enable_debug_logging(config.path());
    let ipc_port = free_port();

    let output = run_role(
        env!("CARGO_BIN_EXE_yshell-session-worker"),
        &[
            OsStr::new("--config-dir"),
            config.path().as_os_str(),
            OsStr::new("--session-worker"),
            OsStr::new("debug-log-session"),
            OsStr::new("--generation"),
            OsStr::new("7"),
            OsStr::new("--parent-pid"),
            OsStr::new("0"),
            OsStr::new("--ipc-port"),
            OsStr::new(&ipc_port),
            OsStr::new("--ipc-token"),
            OsStr::new("test-token"),
        ],
        "yshell-session-worker",
    );
    assert!(
        !output.status.success(),
        "a SessionWorker without a Host IPC listener must fail, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let log_path = config
        .path()
        .join("logs")
        .join("yshell-session-worker-debug.log");
    let log = fs::read_to_string(&log_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", log_path.display()));
    assert!(
        log.contains("role=\"session_worker\""),
        "role missing from log:\n{log}"
    );
    assert!(
        log.contains("process logging initialized"),
        "worker startup record missing:\n{log}"
    );
    assert!(
        log.contains("debug-log-session"),
        "session id missing from log:\n{log}"
    );
    assert!(
        log.contains("could not reach the Host IPC"),
        "IPC failure missing from log:\n{log}"
    );
}

#[test]
fn mcp_daemon_writes_detailed_debug_log_when_enabled() {
    let config = tempfile::tempdir().expect("tempdir");
    enable_debug_logging(config.path());

    let output = run_role(
        env!("CARGO_BIN_EXE_yshell-mcp-daemon"),
        &[
            OsStr::new("--config-dir"),
            config.path().as_os_str(),
            OsStr::new("--mcp-daemon"),
            OsStr::new("--mcp-token"),
            OsStr::new("test-token"),
            OsStr::new("--generation"),
            OsStr::new("3"),
            OsStr::new("--parent-pid"),
            OsStr::new("0"),
        ],
        "yshell-mcp-daemon",
    );
    assert!(
        !output.status.success(),
        "an MCP daemon without a Host bridge must fail, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let log_path = config
        .path()
        .join("logs")
        .join("yshell-mcp-daemon-debug.log");
    let log = fs::read_to_string(&log_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", log_path.display()));
    assert!(
        log.contains("role=\"mcp_daemon\""),
        "role missing from log:\n{log}"
    );
    assert!(
        log.contains("process logging initialized"),
        "daemon startup record missing:\n{log}"
    );
    assert!(
        log.contains("mcp daemon starting"),
        "daemon startup event missing:\n{log}"
    );
    assert!(
        log.contains("could not connect to the Host bridge"),
        "bridge failure missing from log:\n{log}"
    );
}

#[test]
fn gui_shell_writes_debug_log_before_attaching() {
    let config = tempfile::tempdir().expect("tempdir");
    enable_debug_logging(config.path());

    // The Shell has no Host to attach to, but its log must already exist by the
    // time the attach retry loop starts. Kill it instead of waiting it out.
    let mut child = Command::new(env!("CARGO_BIN_EXE_yshell-shell"))
        .args([
            OsStr::new("--config-dir"),
            config.path().as_os_str(),
            OsStr::new("--attach"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn yshell-shell");
    let log_path = config.path().join("logs").join("yshell-shell-debug.log");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut log = String::new();
    while Instant::now() < deadline {
        log = fs::read_to_string(&log_path).unwrap_or_default();
        if log.contains("shell process starting") {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        log.contains("process logging initialized"),
        "shell startup record missing:\n{log}"
    );
    assert!(
        log.contains("shell process starting"),
        "shell startup event missing:\n{log}"
    );
    assert!(
        log.contains("role=\"shell\""),
        "shell role missing from log:\n{log}"
    );
}

#[test]
fn session_worker_follows_a_runtime_debug_toggle() {
    let config = tempfile::tempdir().expect("tempdir");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind worker IPC listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let ipc_port = listener
        .local_addr()
        .expect("local addr")
        .port()
        .to_string();

    let mut child = Command::new(env!("CARGO_BIN_EXE_yshell-session-worker"))
        .args([
            OsStr::new("--config-dir"),
            config.path().as_os_str(),
            OsStr::new("--session-worker"),
            OsStr::new("runtime-toggle-session"),
            OsStr::new("--generation"),
            OsStr::new("1"),
            OsStr::new("--parent-pid"),
            OsStr::new("0"),
            OsStr::new("--ipc-port"),
            OsStr::new(&ipc_port),
            OsStr::new("--ipc-token"),
            OsStr::new("test-token"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn yshell-session-worker");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "SessionWorker never connected to the IPC listener"
                );
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => panic!("accept SessionWorker connection: {error}"),
        }
    };
    stream
        .set_nonblocking(false)
        .expect("blocking worker IPC stream");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut reader = BufReader::new(stream.try_clone().expect("clone IPC stream"));
    let mut auth = String::new();
    reader.read_line(&mut auth).expect("read worker auth token");
    assert_eq!(auth.trim_end(), "test-token");

    let log_path = config
        .path()
        .join("logs")
        .join("yshell-session-worker-debug.log");
    assert!(
        !log_path.exists(),
        "debug logging is still off at this point, so the worker must not write logs"
    );

    // The Host enables debug logging while this worker is already running: the
    // control file appears and the worker's next event must reach its log.
    enable_debug_logging(config.path());
    stream
        .write_all(b"not-a-connect\n")
        .expect("send an invalid connect command");
    stream.flush().expect("flush invalid command");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().expect("try_wait on worker").is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "SessionWorker did not exit after the invalid connect command"
        );
        thread::sleep(Duration::from_millis(25));
    }

    let log = fs::read_to_string(&log_path).expect("read runtime-enabled worker debug log");
    assert!(
        log.contains("received an invalid connect command"),
        "worker must start logging after a runtime toggle:\n{log}"
    );
}

#[test]
fn role_processes_do_not_write_debug_logs_when_disabled() {
    let config = tempfile::tempdir().expect("tempdir");
    let ipc_port = free_port();

    let _ = run_role(
        env!("CARGO_BIN_EXE_yshell-session-worker"),
        &[
            OsStr::new("--config-dir"),
            config.path().as_os_str(),
            OsStr::new("--session-worker"),
            OsStr::new("debug-log-off"),
            OsStr::new("--generation"),
            OsStr::new("1"),
            OsStr::new("--parent-pid"),
            OsStr::new("0"),
            OsStr::new("--ipc-port"),
            OsStr::new(&ipc_port),
            OsStr::new("--ipc-token"),
            OsStr::new("test-token"),
        ],
        "yshell-session-worker",
    );

    assert!(
        !config.path().join("logs").exists(),
        "disabled debug logging must not create debug log files"
    );
}
