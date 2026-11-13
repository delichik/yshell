//! Process-level smoke test for the D38 split-process default path.
//!
//! Starts a real `yshell --host` child, attaches a Shell IPC client, verifies a
//! tick/projection round-trip and then closes to zero windows: the Host must run
//! its shutdown sequence, exit, and remove `run-state.json`.
//!
//! This is the end-to-end counterpart of the in-process unit tests: it exercises
//! the actual binary, `host-bridge.json`, the loopback NDJSON protocol and the
//! single-instance lock.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use yshell_app::host::shell_ipc::HostShellClient;

fn wait_for_bridge(dir: &Path, host: &mut Child) -> HostShellClient {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = host.try_wait().expect("host try_wait") {
            panic!("Host exited before publishing its bridge (status {status})");
        }
        if dir.join("host-bridge.json").exists() {
            if let Ok(client) = HostShellClient::connect(dir) {
                return client;
            }
        }
        assert!(
            Instant::now() < deadline,
            "Host never published host-bridge.json under {}",
            dir.display()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_exit(host: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if host.try_wait().expect("host try_wait").is_some() {
            return;
        }
        if Instant::now() >= deadline {
            let _ = host.kill();
            panic!("Host did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn default_split_path_boots_host_attaches_shell_and_cleans_up() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().to_path_buf();
    let mut host = Command::new(env!("CARGO_BIN_EXE_yshell"))
        .arg("--config-dir")
        .arg(&dir)
        .arg("--host")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn yshell --host");

    let client = wait_for_bridge(&dir, &mut host);
    client.attach(1).expect("attach");
    client.set_window_count(1).expect("window count");

    let (_, window_count, shutdown) = client.host_tick().expect("host tick");
    assert_eq!(
        window_count, 1,
        "one attached window must be visible to the Host"
    );
    assert!(!shutdown, "a live Shell must not request shutdown");
    let projection = client.get_projection(0).expect("projection");
    assert!(projection.is_object(), "projection must be a JSON object");

    // Closing to zero windows must make the Host shut itself down.
    client.set_window_count(0).expect("zero windows");
    wait_for_exit(&mut host, Duration::from_secs(15));

    assert!(
        !dir.join("run-state.json").exists(),
        "run-state must be cleared by the shutdown sequence"
    );
    assert!(
        !dir.join("host-bridge.json").exists(),
        "host-bridge.json must be removed by the shutdown sequence"
    );
    // The Shell may still try to detach; it must not panic either way.
    let _ = client.detach();
}
