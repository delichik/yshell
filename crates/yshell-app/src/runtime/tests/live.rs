//! Live SSH/SFTP smoke tests (skipped unless an env target is configured).

use std::{fs, path::Path, thread, time::Duration};
use tempfile::tempdir;
use yshell_config::{
    parse_quick_connect, AuthMethod as ConfigAuthMethod, AuthProfile, ConfigDocument, ConfigStore,
    FolderProfile, HostKeyPolicy as ConfigHostKeyPolicy, LoggingProfile, SessionProfile,
};
use yshell_sftp::SftpClient;
use yshell_ssh::HostKeyPolicy;

use super::*;

#[test]
fn live_native_ssh_projection_smoke_when_env_target_is_set() {
    let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
        return;
    };
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.select_native_ssh_transport_backend();
    runtime.set_host_key_policy_override_for_testing(HostKeyPolicy::AcceptAnyForTesting);

    let projection = runtime
        .handle_quick_connect(&target)
        .expect("live quick connect");
    assert_eq!(projection.tab_state_text, "connected");

    let marker = "__YSHELL_APP_LIVE__";
    let _ = runtime
        .send_active_terminal_input(&format!("printf '{marker}\\n'\n"))
        .expect("send live input");

    let mut final_projection = runtime.projection();
    for _ in 0..80 {
        if final_projection.terminal_body_text.contains(marker) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
        final_projection = runtime
            .poll_active_terminal_output()
            .expect("poll live output");
    }

    assert!(final_projection.terminal_body_text.contains(marker));

    let resized = runtime
        .resize_active_terminal(90, 28)
        .expect("resize live terminal");
    assert!(resized.status_text.contains("90x28"));

    let _ = runtime
        .send_active_terminal_input("exit\n")
        .expect("send exit");
}

#[test]
fn live_terminal_color_render_when_env_target_is_set() {
    let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
        return;
    };
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.select_native_ssh_transport_backend();
    runtime.set_host_key_policy_override_for_testing(HostKeyPolicy::AcceptAnyForTesting);
    let projection = runtime
        .handle_quick_connect(&target)
        .expect("live quick connect");
    assert_eq!(projection.tab_state_text, "connected");

    let _ = runtime
        .send_active_terminal_input("printf '\\033[31mred\\033[0m plain\\n'\n")
        .expect("send ansi printf");
    let mut projection = runtime.projection();
    for _ in 0..80 {
        if projection.terminal_body_text.contains("red plain") {
            break;
        }
        thread::sleep(Duration::from_millis(50));
        projection = runtime
            .poll_active_terminal_output()
            .expect("poll live output");
    }
    assert!(projection.terminal_body_text.contains("red plain"));

    let mut renderer = yshell_terminal::TerminalRenderer::new();
    let snapshot = runtime
        .active_terminal_render_snapshot()
        .expect("render snapshot");
    let frame = renderer.render(&snapshot);
    let red_pixels = frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[0] > 150 && pixel[1] < 90 && pixel[2] < 90)
        .count();
    assert!(
        red_pixels > 40,
        "expected ANSI red pixels in the live frame, got {red_pixels}"
    );

    let _ = runtime
        .send_active_terminal_input("exit\n")
        .expect("send exit");
}

#[test]
fn live_native_sftp_projection_smoke_when_env_target_is_set() {
    let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
        return;
    };
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.select_native_ssh_transport_backend();
    runtime.set_host_key_policy_override_for_testing(HostKeyPolicy::AcceptAnyForTesting);

    let _ = runtime
        .handle_quick_connect(&target)
        .expect("live quick connect");
    let projection = runtime
        .refresh_active_sftp_listing()
        .expect("refresh live sftp");

    assert_eq!(projection.sftp_path_text, "/");
    assert_eq!(projection.sftp_listing_kind_text, "rows");
    assert!(!projection.sftp_listing_rows_text.trim().is_empty());
    assert!(projection.status_text.contains("Loaded"));
}

#[test]
fn live_native_sftp_transfer_smoke_when_env_target_is_set() {
    let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
        return;
    };
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.select_native_ssh_transport_backend();
    runtime.set_host_key_policy_override_for_testing(HostKeyPolicy::AcceptAnyForTesting);
    let _ = runtime
        .handle_quick_connect(&target)
        .expect("live quick connect");

    let local_upload_path = temp.path().join("upload.txt");
    let local_download_path = temp.path().join("download.txt");
    fs::write(&local_upload_path, b"yshell app sftp").expect("write upload");
    let remote_root = "/tmp/yshell-app-sftp-smoke";
    let remote_file = format!("{remote_root}/upload.txt");

    runtime.sftp_path = remote_root.to_owned();
    runtime.sftp_local_path = local_upload_path.display().to_string();
    runtime.sftp_remote_target = remote_file.clone();
    let _ = runtime.refresh_active_sftp_listing().ok();
    runtime.set_sftp_remote_target(&remote_file);
    let _ = runtime.open_sftp_path("/tmp");

    let session_key = runtime.active_session_key().expect("active session");
    let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
    let mut client =
        SftpClient::with_real_backend(runtime.effective_ssh_config(&runtime_session.ssh_config));
    let _ = client.mkdir(remote_root);

    runtime.sftp_path = remote_root.to_owned();
    runtime.sftp_local_path = local_upload_path.display().to_string();
    runtime.sftp_remote_target = remote_file.clone();
    let uploaded = runtime.upload_sftp_file().expect("upload");
    assert!(uploaded.status_text.contains("Uploaded"));

    runtime.sftp_local_path = local_download_path.display().to_string();
    runtime.sftp_remote_target = remote_file.clone();
    let downloaded = runtime.download_sftp_file().expect("download");
    assert!(downloaded.status_text.contains("Downloaded"));
    assert_eq!(
        fs::read(&local_download_path).expect("read download"),
        b"yshell app sftp"
    );

    let session_key = runtime.active_session_key().expect("active session");
    let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
    let mut client =
        SftpClient::with_real_backend(runtime.effective_ssh_config(&runtime_session.ssh_config));
    client.delete(&remote_file).expect("cleanup file");
    client.delete(remote_root).expect("cleanup dir");
}

#[test]
fn live_native_logging_smoke_when_env_target_is_set() {
    let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
        return;
    };
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let document = ConfigDocument {
        logging: LoggingProfile {
            enabled: true,
            directory: Some("logs".to_owned()),
            format: "sanitized".to_owned(),
        },
        ..ConfigDocument::default()
    };
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime.select_native_ssh_transport_backend();
    runtime.set_host_key_policy_override_for_testing(HostKeyPolicy::AcceptAnyForTesting);
    let _ = runtime
        .handle_quick_connect(&target)
        .expect("live quick connect");

    let marker = "__YSHELL_APP_LOGGING__";
    let _ = runtime
        .send_active_terminal_input(&format!("printf '{marker}\\n'\n"))
        .expect("send live input");
    for _ in 0..80 {
        if runtime.projection().terminal_body_text.contains(marker) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
        let _ = runtime
            .poll_active_terminal_output()
            .expect("poll live output");
    }

    let local_upload_path = temp.path().join("logging-upload.txt");
    let local_download_path = temp.path().join("logging-download.txt");
    fs::write(&local_upload_path, b"yshell logging smoke").expect("write upload");
    let remote_root = "/tmp/yshell-app-logging-smoke";
    let remote_file = format!("{remote_root}/logging-upload.txt");

    let session_key = runtime.active_session_key().expect("active session");
    let runtime_session = runtime.sessions.get(&session_key).expect("runtime session");
    let mut client =
        SftpClient::with_real_backend(runtime.effective_ssh_config(&runtime_session.ssh_config));
    let _ = client.mkdir(remote_root);

    runtime.sftp_path = remote_root.to_owned();
    runtime.sftp_local_path = local_upload_path.display().to_string();
    runtime.sftp_remote_target = remote_file.clone();
    runtime.upload_sftp_file().expect("upload");
    runtime.sftp_local_path = local_download_path.display().to_string();
    runtime.sftp_remote_target = remote_file.clone();
    runtime.download_sftp_file().expect("download");

    let _ = runtime
        .send_active_terminal_input("exit\n")
        .expect("send exit");

    client.delete(&remote_file).expect("cleanup file");
    client.delete(remote_root).expect("cleanup dir");

    let log_files = collect_log_files(&temp.path().join("logs"));
    let transcript = log_files
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(transcript.contains(marker));
    assert!(transcript.contains("operation=upload"));
    assert!(transcript.contains("operation=download"));
}

#[test]
fn live_native_shell_typing_after_sftp_smoke_when_env_target_is_set() {
    let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
        return;
    };
    let temp = tempdir().expect("tempdir");
    let (username, host, port) = parse_quick_connect(&target)
        .map(|parsed| {
            (
                parsed.username.clone().unwrap_or_else(|| "root".to_owned()),
                parsed.host.clone(),
                parsed.port,
            )
        })
        .expect("parse live target");
    let key_path = format!(
        "{}/.ssh/yshell_test_ed25519",
        std::env::var("HOME").unwrap_or_default()
    );
    if !Path::new(&key_path).exists() {
        return;
    }
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    document.auth_profiles.insert(
        "auth-key".to_owned(),
        AuthProfile {
            id: "auth-key".to_owned(),
            name: "Key".to_owned(),
            method: ConfigAuthMethod::PrivateKey {
                key_id: None,
                path: key_path,
                passphrase_secret_key: None,
            },
        },
    );
    let mut profile = SessionProfile::new("saved-key", "Key", host);
    profile.port = port;
    profile.username = Some(username);
    profile.auth_profile_id = Some("auth-key".to_owned());
    profile.host_key_policy = Some(ConfigHostKeyPolicy::AcceptAnyForTesting);
    let mut folder = FolderProfile::new(SAVED_SESSIONS_FOLDER_ID, SAVED_SESSIONS_FOLDER_NAME);
    folder.sessions.push(profile);
    document.folders.push(folder);
    store.save(&document).expect("save config");

    let mut runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
        .expect("runtime without keychain");
    runtime.select_native_ssh_transport_backend();
    let _ = runtime.open_saved_session("saved-key").expect("connect");
    let _ = runtime.resize_active_terminal(78, 36).expect("resize");
    // 模拟 UI 计时器：每 120ms 轮询 5 秒。
    for tick in 0..40 {
        thread::sleep(Duration::from_millis(120));
        if let Err(error) = runtime.poll_active_terminal_output() {
            panic!("poll error at tick {tick}: {error}");
        }
    }
    let _ = runtime.refresh_active_sftp_listing().expect("sftp");
    // 模拟 UI：逐字符写入（每个 key 事件一次 write+poll）。
    thread::sleep(Duration::from_millis(1000));
    for ch in "echo __TMP_MARK__\r".chars() {
        runtime
            .send_active_terminal_input(&ch.to_string())
            .unwrap_or_else(|error| panic!("char input {ch:?} failed: {error}"));
        thread::sleep(Duration::from_millis(30));
    }
    let mut projection = runtime.projection();
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(50));
        projection = match runtime.poll_active_terminal_output() {
            Ok(p) => p,
            Err(error) => panic!("poll error: {error}"),
        };
        if projection.terminal_body_text.contains("__TMP_MARK__") {
            break;
        }
    }
    assert!(
        projection.terminal_body_text.contains("__TMP_MARK__"),
        "marker missing"
    );
}
