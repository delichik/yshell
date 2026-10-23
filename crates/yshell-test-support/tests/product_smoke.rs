//! Product QA smoke/e2e acceptance for the Milestone 0 backend loop.
//!
//! Run with:
//! `cargo test -p yshell-test-support --test product_smoke -- --nocapture`

use std::fs;

use yshell_config::{
    parse_quick_connect, AuthMethod as ConfigAuthMethod, AuthProfile, ConfigDocument, ConfigStore,
    FolderProfile, LoggingProfile, SftpProfile,
};
use yshell_core::{
    PaneId, SessionId, SessionManager, SessionState, SplitDirection, WorkspaceLayoutError,
};
use yshell_logging::{
    Redactor, RotationPolicy, SessionLogger, TranscriptDirection, TranscriptFormat,
};
use yshell_secret::{FakeKeychain, Keychain, SecretRef, SecretString};
use yshell_sftp::{InMemorySftpBackend, FsEntry, SftpBackend, SftpClient};
use yshell_ssh::{channel::ShellSession, AuthMethod, SshConnectionConfig};
use yshell_terminal::{TerminalColor, TerminalGrid, TerminalParser};
use yshell_test_support::{TempHome, TestShellSession, TestSshServer};

const SESSION_SECRET: &str = "p@ssw0rd-never-log";

#[test]
fn product_docs_smoke_acceptance_loop() {
    let home = TempHome::new().expect("temp home");
    let store = ConfigStore::new(home.join(".config/yshell"));
    let fake_server = TestSshServer::placeholder("smoke.example.test", 2200);

    let quick = parse_quick_connect("ssh://qa@smoke.example.test:2200/srv")
        .expect("quick connect parses ssh://user@host:port/path");
    assert_eq!(quick.username.as_deref(), Some("qa"));
    assert_eq!(quick.host, fake_server.host());
    assert_eq!(quick.port, fake_server.port());

    let secret_ref = SecretRef::new("fake://yshell/smoke/session/password");
    let keychain = FakeKeychain::new("qa-master");
    keychain
        .put(secret_ref.clone(), SecretString::from(SESSION_SECRET))
        .expect("secret stored out-of-band");

    let mut document = ConfigDocument {
        sftp: SftpProfile {
            enabled: true,
            initial_directory: Some("/srv".to_owned()),
        },
        logging: LoggingProfile {
            enabled: true,
            directory: Some("logs".to_owned()),
            format: "sanitized".to_owned(),
        },
        ..ConfigDocument::default()
    };
    document.auth_profiles.insert(
        "smoke-auth".to_owned(),
        AuthProfile {
            id: "smoke-auth".to_owned(),
            name: "Smoke password ref".to_owned(),
            method: ConfigAuthMethod::Password {
                secret_key: secret_ref.as_str().to_owned(),
            },
        },
    );
    let mut profile = quick.into_session_profile("smoke-session");
    profile.name = "Smoke Session".to_owned();
    profile.auth_profile_id = Some("smoke-auth".to_owned());
    document
        .folders
        .push(FolderProfile::new("smoke-folder", "Smoke"));
    document.folders[0].sessions.push(profile);

    store.save_all(&document).expect("config saved");
    let saved_config = fs::read_to_string(store.config_file()).expect("config readable");
    assert!(saved_config.contains("secret_key"));
    assert!(saved_config.contains(secret_ref.as_str()));
    assert!(
        !saved_config.contains(SESSION_SECRET),
        "config must persist secret refs, not plaintext secrets"
    );
    assert!(
        !saved_config.contains("[[folders]]"),
        "sessions must live under sessions/, not config.toml"
    );
    let session_file = store
        .sessions_dir()
        .join("Smoke")
        .join("Smoke Session.toml");
    assert!(session_file.exists(), "session file {}", session_file.display());
    let session_body = fs::read_to_string(&session_file).expect("session readable");
    assert!(session_body.contains("smoke.example.test"));
    assert!(!session_body.contains(SESSION_SECRET));

    let loaded = store.load_or_recover().expect("config loads").document;
    let resolved = loaded
        .resolve_session("smoke-session")
        .expect("session resolves defaults and auth profile");
    assert_eq!(resolved.session.host, fake_server.host());
    assert_eq!(resolved.session.port, fake_server.port());
    assert_eq!(resolved.session.username.as_deref(), Some("qa"));
    assert_eq!(resolved.sftp.initial_directory.as_deref(), Some("/srv"));
    assert_eq!(resolved.logging.format, "sanitized");

    let mut manager = SessionManager::new();
    let session_id = SessionId::new(&resolved.session.id);
    let tab_id = yshell_core::TabId::new("smoke-tab");
    manager.open_tab(tab_id.clone(), session_id.clone());
    manager
        .set_state(&session_id, SessionState::Connecting)
        .expect("state changed to connecting");
    manager
        .set_state(&session_id, SessionState::Connected)
        .expect("state changed to connected");
    let tab = manager.tab_mut(&tab_id).expect("tab is tracked");
    let second = tab
        .layout_mut()
        .split(
            PaneId::new(1),
            SplitDirection::Horizontal,
            0.5,
            Some(session_id.clone()),
        )
        .expect("split right/down modeled");
    let third = tab
        .layout_mut()
        .split(
            second,
            SplitDirection::Vertical,
            0.5,
            Some(session_id.clone()),
        )
        .expect("recursive split modeled");
    tab.layout_mut()
        .split(
            third,
            SplitDirection::Horizontal,
            0.5,
            Some(session_id.clone()),
        )
        .expect("fourth pane modeled");
    assert_eq!(tab.layout().leaf_count(), 4);
    assert_eq!(
        tab.layout_mut()
            .split(PaneId::new(1), SplitDirection::Horizontal, 0.5, None),
        Err(WorkspaceLayoutError::TooManyLeaves),
        "first-version layout must cap recursive panes at four leaves"
    );

    let mut grid = TerminalGrid::new(24, 3);
    let mut parser = TerminalParser::new();
    parser.advance(
        &mut grid,
        b"plain \x1b[1;38;2;12;34;56mred-ish\x1b[0m\nwide: \xe5\xa5\xbd",
    );
    let styled_cell = grid.cell(6, 0).expect("styled output cell");
    assert_eq!(styled_cell.grapheme, "r");
    assert!(styled_cell.bold);
    assert_eq!(styled_cell.foreground, TerminalColor::rgb(12, 34, 56));
    assert_eq!(grid.line_text(1), "wide: å¥?);

    let ssh_config = SshConnectionConfig::new(
        &resolved.session.host,
        resolved.session.port,
        AuthMethod::Password {
            username: resolved.session.username.clone().expect("username"),
            password: keychain
                .get(&secret_ref)
                .expect("secret retrieved")
                .expose_secret()
                .to_owned(),
        },
    );
    let mut shell = TestShellSession::open(&ssh_config).expect("test shell opens");
    assert_eq!(shell.host, fake_server.host());
    shell
        .write_input(b"printf smoke\n")
        .expect("test shell accepts input");
    let mut shell_output = Vec::new();
    for _ in 0..8 {
        let chunk = shell.poll_output().expect("poll test shell");
        if chunk.is_empty() {
            break;
        }
        shell_output.extend_from_slice(&chunk);
    }
    let shell_text = String::from_utf8_lossy(&shell_output);
    assert!(shell_text.contains("test-shell received input: printf smoke\n"));

    let mut sftp_backend = InMemorySftpBackend::default();
    sftp_backend.mkdir("/srv").expect("mkdir /srv");
    sftp_backend.insert(FsEntry::file("/srv/readme.txt", 12));
    let mut sftp = SftpClient::with_backend(sftp_backend);
    let listing = sftp.list_dir("/srv").expect("list /srv");
    assert_eq!(listing.entries.len(), 1);
    sftp.chmod("/srv/readme.txt", 0o600).expect("chmod");
    sftp.rename("/srv/readme.txt", "/srv/renamed.txt")
        .expect("rename");
    assert_eq!(
        sftp.list_dir("/srv").expect("list renamed").entries[0].name,
        "renamed.txt"
    );

    let log_path = home.join("logs/smoke-session.log");
    let redactor = Redactor::new().with_secret(SESSION_SECRET);
    let mut logger = SessionLogger::open(
        &log_path,
        TranscriptFormat::Sanitized,
        redactor,
        RotationPolicy::disabled(),
    )
    .expect("logger opens");
    logger
        .record(
            TranscriptDirection::Input,
            format!("password={SESSION_SECRET}").as_bytes(),
        )
        .expect("secret input logged safely");
    logger
        .record(TranscriptDirection::Output, &shell_output)
        .expect("output logged safely");
    let log = fs::read_to_string(log_path).expect("log readable");
    assert!(log.contains("[REDACTED]"));
    assert!(log.contains("test-shell received input"));
    assert!(
        !log.contains(SESSION_SECRET),
        "sanitized session logs must not leak plaintext secrets"
    );

    shell.disconnect().expect("test shell disconnects");
    manager.close_tab(&tab_id).expect("tab closes");
    assert_eq!(manager.tab_len(), 0);
}
