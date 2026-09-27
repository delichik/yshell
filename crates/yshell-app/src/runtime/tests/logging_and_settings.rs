//! Logging settings and the settings-terminal form.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use yshell_config::{ConfigDocument, ConfigStore, LoggingProfile, TerminalProfile};
use yshell_logging::{LogMode, TranscriptFormat};
use yshell_terminal::{DEFAULT_SCROLLBACK_LINES, DEFAULT_SCROLLBACK_MAX_CELLS};

use crate::runtime::logging::{
    default_session_log_file_name, session_log_path_for, validate_log_file_name,
    SessionLoggingRequest,
};
use crate::session_runtime::SessionLoggingMode;

use super::*;

#[test]
fn global_logging_settings_are_visible_and_persisted() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let enabled = runtime.toggle_global_logging_enabled();
    assert_eq!(enabled.logging_enabled_text, "enabled");
    let raw = runtime.set_global_logging_format_raw();
    assert_eq!(raw.logging_format_text, "raw");
    let directory = runtime.update_global_logging_directory("audit/logs");
    assert_eq!(directory.logging_directory_text, "audit/logs");

    let saved = runtime
        .save_global_logging_settings()
        .expect("save logging settings");
    assert!(saved.status_text.contains("Saved global logging policy"));
    assert_eq!(saved.status_kind, "logging-saved");
    assert!(saved.status_param_1.contains("global=enabled"));
    assert_eq!(saved.status_param_2, "");

    let persisted = ConfigStore::new(temp.path())
        .load_or_recover()
        .expect("reload config")
        .document
        .logging;
    assert!(persisted.enabled);
    assert_eq!(persisted.format, "raw");
    assert_eq!(persisted.directory.as_deref(), Some("audit/logs"));
}

#[test]
fn status_kind_expires_when_text_is_overwritten() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    runtime.set_status_kind(
        "session-deleted",
        "Deleted saved session `demo` and persisted the updated config.".to_owned(),
        "demo".to_owned(),
        String::new(),
    );
    let covered = runtime.projection();
    assert_eq!(covered.status_kind, "session-deleted");
    assert_eq!(covered.status_param_1, "demo");

    // Any mutation of the append channel after the kind snapshot invalidates
    // it, so Slint falls back to the English status_text.
    runtime.status_text = format!("{} Logging notice: rotated", runtime.status_text);
    let stale = runtime.projection();
    assert_eq!(stale.status_kind, "");
    assert_eq!(stale.status_param_1, "");
    assert_eq!(stale.status_param_2, "");
}

#[test]
fn settings_terminal_save_persists_scrollback_limits() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let updated_lines = runtime.update_settings_scrollback_lines("25000");
    assert_eq!(updated_lines.settings_scrollback_lines_text, "25000");
    let updated_cells = runtime.update_settings_scrollback_max_cells("3000000");
    assert_eq!(updated_cells.settings_scrollback_max_cells_text, "3000000");

    let saved = runtime
        .save_settings_terminal()
        .expect("save terminal settings");

    assert_eq!(saved.settings_terminal_status_kind_text, "saved");
    assert_eq!(saved.settings_terminal_status_value_text, "25000");
    assert_eq!(saved.settings_terminal_status_limit_text, "3000000");
    let persisted = ConfigStore::new(temp.path())
        .load_or_recover()
        .expect("reload config")
        .document
        .terminal;
    assert_eq!(persisted.scrollback_lines, 25_000);
    assert_eq!(persisted.scrollback_max_cells, 3_000_000);
}

#[test]
fn settings_terminal_save_applies_to_existing_sessions() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let session_key = runtime.active_session_key().expect("active session");
    {
        let session = runtime
            .sessions
            .get_mut(&session_key)
            .expect("runtime session");
        assert_eq!(
            session.terminal_grid.scrollback_limits(),
            (DEFAULT_SCROLLBACK_LINES, DEFAULT_SCROLLBACK_MAX_CELLS)
        );
        for index in 0..200 {
            session.append_status_line(&format!("scrollback sample line {index}"));
        }
    }

    let _ = runtime.update_settings_scrollback_lines("120");
    let _ = runtime.update_settings_scrollback_max_cells("100000");
    runtime
        .save_settings_terminal()
        .expect("save terminal settings");

    let session = runtime.sessions.get(&session_key).expect("runtime session");
    assert_eq!(session.terminal_grid.scrollback_limits(), (120, 100_000));
    assert!(
        session.terminal_grid.scrollback_len() <= 120,
        "saving smaller limits must evict overflow"
    );
}

#[test]
fn settings_terminal_rejects_invalid_input_without_persisting() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    for value in ["", "   ", "abc", "-5", "99", "1000001"] {
        let _ = runtime.update_settings_scrollback_lines(value);
        let rejected = runtime
            .save_settings_terminal()
            .expect("validation failure is not fatal");
        assert_ne!(
            rejected.settings_terminal_status_kind_text, "saved",
            "lines value {value:?} should be rejected"
        );
        assert!(!rejected.settings_terminal_status_kind_text.is_empty());
        assert_eq!(runtime.config_document.terminal, TerminalProfile::default());
    }

    let _ = runtime.update_settings_scrollback_lines("10000");
    for value in ["", "abc", "99999", "100000001"] {
        let _ = runtime.update_settings_scrollback_max_cells(value);
        let rejected = runtime
            .save_settings_terminal()
            .expect("validation failure is not fatal");
        assert_ne!(
            rejected.settings_terminal_status_kind_text, "saved",
            "cap value {value:?} should be rejected"
        );
        assert_eq!(runtime.config_document.terminal, TerminalProfile::default());
    }

    let persisted = ConfigStore::new(temp.path())
        .load_or_recover()
        .expect("reload config")
        .document
        .terminal;
    assert_eq!(persisted, TerminalProfile::default());
}

#[test]
fn settings_terminal_reset_defaults_fills_inputs_without_persisting() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let _ = runtime.update_settings_scrollback_lines("500");
    let _ = runtime.update_settings_scrollback_max_cells("150000");
    runtime
        .save_settings_terminal()
        .expect("save custom limits");

    let projection = runtime.reset_settings_terminal_defaults();

    assert_eq!(projection.settings_scrollback_lines_text, "10000");
    assert_eq!(projection.settings_scrollback_max_cells_text, "2000000");
    assert_eq!(
        projection.settings_terminal_status_kind_text,
        "defaults-loaded"
    );
    assert_eq!(runtime.config_document.terminal.scrollback_lines, 500);
    assert_eq!(
        runtime.config_document.terminal.scrollback_max_cells,
        150_000
    );
}

#[test]
fn enabled_logging_writes_fake_terminal_transcript() {
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
    let _ = runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");

    let log_files = collect_log_files(&temp.path().join("logs"));
    assert!(!log_files.is_empty());
    let combined = log_files
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(combined.contains("Connecting to example.com:2200 as alice"));
    assert!(combined.contains("Fake shell established"));
}

// ---------------------------------------------------------------- N6 手动日志

/// N6：连接一个 fake shell 的运行时，返回（runtime, session_key）。
fn runtime_with_quick_connect(temp: &Path) -> (AppRuntime, String) {
    let mut runtime = AppRuntime::new(temp.to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let session_key = runtime.active_session_key().expect("active session");
    (runtime, session_key)
}

/// N6：把一段输出喂进会话并把 fake shell 的 pending 输出全部 poll 掉。
fn drain_shell_output(runtime: &mut AppRuntime, session_key: &str, marker: &str) {
    queue_shell_output(runtime, session_key, marker);
    runtime
        .sessions
        .get_mut(session_key)
        .expect("runtime session")
        .poll_shell_output()
        .expect("poll shell output");
}

/// N6：断言行首是 `[HH:MM:SS]`，返回去掉时间戳的整行。
fn strip_timestamp_prefix(line: &str) -> &str {
    let bytes = line.as_bytes();
    assert!(
        bytes.len() >= 11,
        "line is too short for a timestamp: {line:?}"
    );
    assert_eq!(bytes[0], b'[');
    assert_eq!(bytes[3], b':');
    assert_eq!(bytes[6], b':');
    assert_eq!(bytes[9], b']');
    for index in [1, 2, 4, 5, 7, 8] {
        assert!(
            bytes[index].is_ascii_digit(),
            "timestamp digit missing at index {index} in {line:?}"
        );
    }
    &line[10..]
}

#[test]
fn manual_logging_writes_sanitized_timestamped_transcript() {
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());
    let log_path = temp.path().join("manual-sanitized.log");

    let projection = runtime
        .start_session_logging(&session_key, &SessionLoggingRequest::new(&log_path))
        .expect("start logging");
    assert_eq!(projection.status_kind, "logging-started");
    assert!(projection.status_text.contains("sanitized"));
    assert!(runtime.session_logging_active(&session_key));
    assert_eq!(
        runtime.session_logging_mode(&session_key),
        SessionLoggingMode::Manual
    );
    assert_eq!(
        runtime.session_logging_path_text(&session_key),
        log_path.display().to_string()
    );

    // `ls --color` 的等价物：带 ANSI 颜色的输出在 sanitized 模式被剥掉。
    drain_shell_output(&mut runtime, &session_key, "\u{1b}[31mRED\u{1b}[0m");

    let stopped = runtime
        .stop_session_logging(&session_key)
        .expect("stop logging");
    assert_eq!(stopped.status_kind, "logging-stopped");
    assert!(!runtime.session_logging_active(&session_key));
    assert_eq!(
        runtime.session_logging_mode(&session_key),
        SessionLoggingMode::Off
    );

    let log = fs::read_to_string(&log_path).expect("read log");
    eprintln!("sanitized transcript sample:\n{log}");
    assert!(
        !log.contains('\u{1b}'),
        "sanitized log must not contain ESC bytes: {log:?}"
    );
    assert!(log.contains("RED"));
    let mut output_lines = 0;
    for line in log.lines().filter(|line| !line.is_empty()) {
        let body = strip_timestamp_prefix(line);
        assert!(body.starts_with("output\t"), "unexpected line: {line:?}");
        output_lines += 1;
    }
    assert!(output_lines > 0, "expected at least one transcript line");
}

#[test]
fn manual_logging_raw_format_keeps_escape_sequences() {
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());
    let log_path = temp.path().join("manual-raw.log");

    let mut request = SessionLoggingRequest::new(&log_path);
    request.format = TranscriptFormat::Raw;
    runtime
        .start_session_logging(&session_key, &request)
        .expect("start raw logging");

    drain_shell_output(&mut runtime, &session_key, "\u{1b}[32mGREEN\u{1b}[0m");
    runtime
        .stop_session_logging(&session_key)
        .expect("stop logging");

    let log = fs::read_to_string(&log_path).expect("read log");
    assert!(
        log.contains("\u{1b}[32mGREEN\u{1b}[0m"),
        "raw mode must keep ANSI bytes: {log:?}"
    );
}

#[test]
fn manual_logging_records_local_input_only_when_enabled() {
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());

    // 设计默认：不记录本地输入。
    let default_log = temp.path().join("no-input.log");
    runtime
        .start_session_logging(&session_key, &SessionLoggingRequest::new(&default_log))
        .expect("start logging");
    runtime
        .sessions
        .get_mut(&session_key)
        .expect("runtime session")
        .write_terminal_input(b"ls -la\n")
        .expect("terminal input");
    runtime
        .stop_session_logging(&session_key)
        .expect("stop logging");
    let log = fs::read_to_string(&default_log).expect("read log");
    assert!(
        !log.contains("input\t"),
        "input must not be recorded by default: {log:?}"
    );
    assert!(log.contains("ls -la"), "output echo is still recorded");

    // 勾选"记录本地输入"（R-106）：输入按 Input 方向写入。
    let input_log = temp.path().join("with-input.log");
    let mut request = SessionLoggingRequest::new(&input_log);
    request.include_input = true;
    runtime
        .start_session_logging(&session_key, &request)
        .expect("start logging with input");
    runtime
        .sessions
        .get_mut(&session_key)
        .expect("runtime session")
        .write_terminal_input(b"pwd\n")
        .expect("terminal input");
    runtime
        .stop_session_logging(&session_key)
        .expect("stop logging");
    let log = fs::read_to_string(&input_log).expect("read log");
    let input_lines: Vec<&str> = log
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let body = strip_timestamp_prefix(line);
            body.strip_prefix("input\t")
        })
        .collect();
    assert_eq!(input_lines, vec!["pwd"]);
}

#[test]
fn manual_logging_replaces_auto_logging_without_resuming_it() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    store
        .save(&ConfigDocument {
            logging: LoggingProfile {
                enabled: true,
                directory: Some("logs".to_owned()),
                format: "sanitized".to_owned(),
            },
            ..ConfigDocument::default()
        })
        .expect("save config");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());

    // 配置驱动的自动日志已经打开（终端 transcript + 传输日志两个文件）。
    let auto_logs = collect_log_files(&temp.path().join("logs"));
    let auto_log = auto_logs
        .iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("terminal-"))
        })
        .cloned()
        .expect("one automatic terminal log expected");
    assert_eq!(
        runtime.session_logging_mode(&session_key),
        SessionLoggingMode::Auto
    );

    // 手动日志优先：开启后自动日志暂停，输出只进手动文件。
    let manual_log = temp.path().join("manual-precedence.log");
    runtime
        .start_session_logging(&session_key, &SessionLoggingRequest::new(&manual_log))
        .expect("start manual logging");
    assert_eq!(
        runtime.session_logging_mode(&session_key),
        SessionLoggingMode::Manual
    );
    let auto_before = fs::read(&auto_log).expect("read auto log");
    drain_shell_output(&mut runtime, &session_key, "manual-only-marker");
    assert_eq!(
        fs::read(&auto_log).expect("read auto log"),
        auto_before,
        "automatic logging must stay paused while manual logging is active"
    );
    let manual_content = fs::read_to_string(&manual_log).expect("read manual log");
    assert!(manual_content.contains("manual-only-marker"));

    // 停止手动日志：不自动恢复自动日志（同一会话单一日志输出）。
    runtime
        .stop_session_logging(&session_key)
        .expect("stop manual logging");
    assert_eq!(
        runtime.session_logging_mode(&session_key),
        SessionLoggingMode::Off
    );
    drain_shell_output(&mut runtime, &session_key, "after-stop-marker");
    assert_eq!(
        fs::read(&auto_log).expect("read auto log"),
        auto_before,
        "automatic logging must not resume after a manual stop"
    );
    assert!(
        !fs::read_to_string(&manual_log)
            .expect("read manual log")
            .contains("after-stop-marker"),
        "stopped manual log must not receive more output"
    );

    // 即使再次应用日志配置（连接/重连路径），本会话也不重新打开自动日志。
    let logging = runtime.config_document.logging.clone();
    runtime
        .sessions
        .get_mut(&session_key)
        .expect("runtime session")
        .configure_logging(temp.path(), &logging);
    assert!(
        !runtime.session_logging_active(&session_key),
        "configure_logging must not reopen automatic logging after a manual stop"
    );
}

#[test]
fn manual_logging_open_failure_reports_error_and_keeps_current_output() {
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());
    let directory_target = temp.path().join("not-a-file");
    fs::create_dir_all(&directory_target).expect("create directory");

    let error = runtime
        .start_session_logging(&session_key, &SessionLoggingRequest::new(&directory_target))
        .expect_err("a directory is not a valid log file");
    assert!(error.to_string().contains("could not open log file"));

    let projection = runtime.projection();
    assert_eq!(projection.status_kind, "logging-error");
    assert!(projection.status_text.contains("not-a-file"));
    assert!(!runtime.session_logging_active(&session_key));
    assert_eq!(
        runtime.session_logging_mode(&session_key),
        SessionLoggingMode::Off
    );

    assert!(runtime
        .start_session_logging(
            "missing-session",
            &SessionLoggingRequest::new(&directory_target)
        )
        .is_err());
}

#[cfg(unix)]
#[test]
fn manual_logging_write_failure_stops_and_surfaces_last_error() {
    let full = Path::new("/dev/full");
    if !full.exists() {
        return;
    }
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());

    runtime
        .start_session_logging(&session_key, &SessionLoggingRequest::new(full))
        .expect("open /dev/full");
    assert_eq!(
        runtime.session_logging_mode(&session_key),
        SessionLoggingMode::Manual
    );

    queue_shell_output(&mut runtime, &session_key, "write-failure-marker");
    runtime
        .sessions
        .get_mut(&session_key)
        .expect("runtime session")
        .poll_shell_output()
        .expect("poll output");

    // 写失败 → 自动停止并保留已写内容，`last_error` 折进状态栏提示。
    runtime.fold_logging_notice_from_session(&session_key);
    assert!(!runtime.session_logging_active(&session_key));
    assert_eq!(
        runtime.session_logging_mode(&session_key),
        SessionLoggingMode::Off
    );
    assert!(
        runtime.status_text.contains("write failure"),
        "status must report the write failure: {}",
        runtime.status_text
    );
    assert!(runtime.status_text.contains("/dev/full"));
    assert_eq!(
        runtime.session_logging_path_text(&session_key),
        "/dev/full",
        "the failed file path stays available for 'Open Log File'"
    );
}

#[test]
fn manual_logging_append_and_truncate_modes() {
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());
    let log_path = temp.path().join("modes.log");
    fs::write(&log_path, "previous\n").expect("seed log");

    let mut append = SessionLoggingRequest::new(&log_path);
    append.mode = LogMode::Append;
    runtime
        .start_session_logging(&session_key, &append)
        .expect("start append logging");
    drain_shell_output(&mut runtime, &session_key, "append-marker");
    runtime
        .stop_session_logging(&session_key)
        .expect("stop append logging");
    let log = fs::read_to_string(&log_path).expect("read log");
    assert!(log.starts_with("previous\n"));
    assert!(log.contains("append-marker"));

    let mut truncate = SessionLoggingRequest::new(&log_path);
    truncate.mode = LogMode::Truncate;
    runtime
        .start_session_logging(&session_key, &truncate)
        .expect("start truncate logging");
    drain_shell_output(&mut runtime, &session_key, "truncate-marker");
    runtime
        .stop_session_logging(&session_key)
        .expect("stop truncate logging");
    let log = fs::read_to_string(&log_path).expect("read log");
    assert!(!log.contains("previous"));
    assert!(log.contains("truncate-marker"));
}

#[test]
fn close_and_exit_entries_stop_and_flush_active_logs() {
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());
    let log_path = temp.path().join("close-path.log");

    runtime
        .start_session_logging(&session_key, &SessionLoggingRequest::new(&log_path))
        .expect("start logging");
    drain_shell_output(&mut runtime, &session_key, "close-marker");

    // 标签关闭入口：停止并返回文件路径。
    let closed = runtime
        .stop_session_logging_for_close(&session_key)
        .expect("close must report the log file");
    assert!(closed.contains("close-path.log"));
    assert!(runtime
        .stop_session_logging_for_close("missing-session")
        .is_none());
    let log = fs::read_to_string(&log_path).expect("read log");
    assert!(log.contains("close-marker"));

    // 退出入口：停止所有活动日志（幂等）。
    runtime
        .start_session_logging(
            &session_key,
            &SessionLoggingRequest::new(temp.path().join("exit-path.log")),
        )
        .expect("start logging again");
    assert_eq!(runtime.stop_all_session_logging(), 1);
    assert!(!runtime.session_logging_active(&session_key));
    assert_eq!(runtime.stop_all_session_logging(), 0);
}

#[test]
fn stop_session_logging_without_active_output_is_an_error() {
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());

    assert!(runtime.stop_session_logging(&session_key).is_err());
    assert!(runtime.stop_session_logging("missing-session").is_err());
}

#[test]
fn logging_request_defaults_match_the_design() {
    let request = SessionLoggingRequest::new("session.log");
    assert_eq!(request.path, PathBuf::from("session.log"));
    assert_eq!(request.format, TranscriptFormat::Sanitized);
    assert!(request.timestamps);
    assert!(!request.include_input);
    assert_eq!(request.mode, LogMode::Append);
}

#[test]
fn log_file_name_validation_and_default_suggestion() {
    assert!(validate_log_file_name("").is_err());
    assert!(validate_log_file_name("   ").is_err());
    assert!(validate_log_file_name(".").is_err());
    assert!(validate_log_file_name("..").is_err());
    assert!(validate_log_file_name("a/b.log").is_err());
    assert!(validate_log_file_name("a\\b.log").is_err());
    assert!(validate_log_file_name("a:b.log").is_err());
    assert!(validate_log_file_name("a*b.log").is_err());
    assert!(validate_log_file_name("a\u{1}b.log").is_err());
    assert!(validate_log_file_name("session-2026.log").is_ok());
    assert!(validate_log_file_name("  session-2026.log  ").is_ok());

    // 默认名模板 `{session}-{timestamp}.log`，会话标签里的非法字符转 `_`。
    let suggested = default_session_log_file_name("Prod web/1");
    assert!(suggested.starts_with("Prod_web_1-"), "got {suggested}");
    assert!(suggested.ends_with(".log"));
    assert!(validate_log_file_name(&suggested).is_ok());

    assert_eq!(
        session_log_path_for(Path::new("/tmp/logs"), "session.log"),
        PathBuf::from("/tmp/logs/session.log")
    );
    assert_eq!(
        session_log_path_for(Path::new("/tmp/logs"), "  session.log  "),
        PathBuf::from("/tmp/logs/session.log")
    );
}

// ------------------------------------------------- N6 Phase 2：弹窗/菜单/关闭

/// N6：弹窗默认值 = 上次目录（回退配置 logs 目录）+ `{session}-{timestamp}.log`。
#[test]
fn logging_dialog_defaults_and_start_flow() {
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());

    let opened = runtime.open_logging_dialog();
    assert!(opened.logging_dialog_visible);
    assert_eq!(opened.logging_dialog_session_text, "alice@example.com:2200");
    // 设计默认：sanitized + 时间戳开 + 不记输入 + 无覆盖确认。
    assert!(
        !opened.logging_dialog_raw_format,
        "sanitized is the default"
    );
    assert!(opened.logging_dialog_timestamps);
    assert!(!opened.logging_dialog_include_input);
    assert!(!opened.logging_dialog_input_confirmed);
    assert!(!opened.logging_dialog_overwrite_confirmed);
    assert!(!opened.logging_dialog_file_exists);
    assert!(opened.logging_dialog_file_name_error_text.is_empty());
    // 默认目录 = 配置 logs 目录（相对 config_dir 解析为绝对路径）。
    assert_eq!(
        opened.logging_dialog_directory_text,
        temp.path().join("logs").display().to_string()
    );
    assert!(opened
        .logging_dialog_file_name_text
        .starts_with("alice_example.com_2200-"));
    assert!(opened.logging_dialog_file_name_text.ends_with(".log"));

    // 改成临时目录 + 指定文件名后开始。
    let directory_text = temp.path().display().to_string();
    runtime.update_logging_dialog_directory(&directory_text);
    runtime.update_logging_dialog_file_name("dialog-session.log");
    let started = runtime
        .start_logging_from_dialog()
        .expect("start from dialog");
    assert_eq!(started.status_kind, "logging-started");
    assert!(
        !started.logging_dialog_visible,
        "successful start closes the dialog"
    );
    assert!(runtime.session_logging_active(&session_key));
    drain_shell_output(&mut runtime, &session_key, "dialog-flow-marker");
    runtime
        .stop_session_logging(&session_key)
        .expect("stop logging");
    let log = fs::read_to_string(temp.path().join("dialog-session.log")).expect("read log");
    assert!(log.contains("dialog-flow-marker"));

    // "上次使用目录"：再次打开弹窗默认 = 上次目录。
    let reopened = runtime.open_logging_dialog();
    assert!(reopened.logging_dialog_visible);
    assert_eq!(reopened.logging_dialog_directory_text, directory_text);
    runtime.close_logging_dialog();
}

#[test]
fn logging_dialog_validates_file_name_and_overwrite_confirmation() {
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());
    runtime.open_logging_dialog();
    runtime.update_logging_dialog_directory(&temp.path().display().to_string());

    // 非法文件名：内联错误 + 不开始（弹窗保持打开）。
    runtime.update_logging_dialog_file_name("bad/name.log");
    let rejected = runtime
        .start_logging_from_dialog()
        .expect("invalid name is not fatal");
    assert!(rejected.logging_dialog_visible);
    assert!(!rejected.logging_dialog_file_name_error_text.is_empty());
    assert!(!runtime.session_logging_active(&session_key));

    // 已存在文件：先要覆盖确认；确认后按 Truncate 覆盖写入。
    let existing_path = temp.path().join("existing.log");
    fs::write(&existing_path, "old content\n").expect("seed log");
    runtime.update_logging_dialog_file_name("existing.log");
    let existing = runtime.projection();
    assert!(existing.logging_dialog_file_exists);
    assert!(!existing.logging_dialog_overwrite_confirmed);

    runtime.set_logging_dialog_options(false, true, false, false, true);
    assert!(runtime.projection().logging_dialog_overwrite_confirmed);
    runtime
        .start_logging_from_dialog()
        .expect("start with overwrite");
    drain_shell_output(&mut runtime, &session_key, "overwrite-marker");
    runtime
        .stop_session_logging(&session_key)
        .expect("stop logging");
    let log = fs::read_to_string(&existing_path).expect("read log");
    assert!(!log.contains("old content"));
    assert!(log.contains("overwrite-marker"));
}

#[test]
fn logging_dialog_open_failure_keeps_dialog_and_reports_path() {
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());
    let blocker = temp.path().join("not-a-directory");
    fs::write(&blocker, "x").expect("seed file");

    runtime.open_logging_dialog();
    runtime.update_logging_dialog_directory(&blocker.display().to_string());
    runtime.update_logging_dialog_file_name("session.log");
    let projection = runtime
        .start_logging_from_dialog()
        .expect("open failure is not fatal");
    assert!(projection.logging_dialog_visible, "dialog stays open");
    assert!(
        projection
            .logging_dialog_path_error_text
            .contains("could not open log file"),
        "path error: {}",
        projection.logging_dialog_path_error_text
    );
    assert_eq!(projection.status_kind, "logging-error");
    assert!(!runtime.session_logging_active(&session_key));
}

#[test]
fn terminal_logging_menu_flags_follow_state() {
    let temp = tempdir().expect("tempdir");
    // 无会话：Start/Stop/Open 全部禁用。
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    let idle = runtime.projection();
    assert!(!idle.terminal_logging_start_enabled);
    assert!(!idle.terminal_logging_stop_enabled);
    assert!(!idle.terminal_logging_open_file_enabled);
    assert!(!idle.terminal_logging_open_folder_enabled);
    assert!(!idle.logging_active);
    assert_eq!(idle.logging_mode_text, "off");

    // 有会话、未记录：Start 可用。
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let session_key = runtime.active_session_key().expect("active session");
    let ready = runtime.projection();
    assert!(ready.terminal_logging_start_enabled);
    assert!(!ready.terminal_logging_stop_enabled);

    // 记录中：Stop/Open 可用，Start 仍打开弹窗（停止提示视图）；REC 投影为 manual。
    let log_path = temp.path().join("menu.log");
    runtime
        .start_session_logging(&session_key, &SessionLoggingRequest::new(&log_path))
        .expect("start logging");
    let logging = runtime.projection();
    assert!(logging.terminal_logging_start_enabled);
    assert!(logging.terminal_logging_stop_enabled);
    assert!(logging.terminal_logging_open_file_enabled);
    assert!(logging.terminal_logging_open_folder_enabled);
    assert!(logging.logging_active);
    assert_eq!(logging.logging_mode_text, "manual");
    assert_eq!(logging.logging_file_name_text, "menu.log".to_owned());
    assert_eq!(logging.logging_path_text, log_path.display().to_string());
    assert!(runtime.projection().tabs[0].logging, "tab REC indicator");

    // 已在记录：Start 入口打开弹窗（前端按 logging-active 显示停止提示视图）。
    let dialog = runtime.terminal_logging_start();
    assert!(dialog.logging_dialog_visible);
    assert!(dialog.logging_active);
    runtime.close_logging_dialog();
    assert!(!runtime.projection().logging_dialog_visible);

    // 停止后：Start 恢复，"打开"仍指向最近文件（已写盘）。
    runtime
        .stop_session_logging(&session_key)
        .expect("stop logging");
    let stopped = runtime.projection();
    assert!(stopped.terminal_logging_start_enabled);
    assert!(!stopped.terminal_logging_stop_enabled);
    assert!(stopped.terminal_logging_open_file_enabled);
    assert!(!stopped.logging_active);
    assert_eq!(stopped.logging_mode_text, "off");
    assert!(!stopped.tabs[0].logging);
    assert_eq!(
        runtime.active_session_logging_path_text(),
        log_path.display().to_string()
    );
}

#[test]
fn terminal_logging_start_without_session_reports_status() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

    let projection = runtime.terminal_logging_start();
    assert_eq!(projection.status_kind, "logging-no-session");
    assert!(!projection.logging_dialog_visible);
    assert!(!projection.status_text.is_empty());
}

#[test]
fn closing_tab_stops_logging_and_reports_log_path() {
    let temp = tempdir().expect("tempdir");
    let (mut runtime, session_key) = runtime_with_quick_connect(temp.path());
    let tab_id = runtime.active_tab_id.clone().expect("active tab");
    let log_path = temp.path().join("close-flow.log");

    runtime
        .start_session_logging(&session_key, &SessionLoggingRequest::new(&log_path))
        .expect("start logging");
    drain_shell_output(&mut runtime, &session_key, "close-flow-marker");

    // 活动连接：先确认再关闭；关闭路径强制停止日志并提示文件位置。
    let pending = runtime.request_close_tab(&tab_id).expect("request close");
    assert!(pending.close_tabs_confirm_visible);
    let projection = runtime.confirm_close_tabs().expect("confirm close");
    assert!(
        projection.status_text.contains("close-flow.log"),
        "status must mention the log file: {}",
        projection.status_text
    );
    assert!(!runtime.sessions.contains_key(&session_key));
    let log = fs::read_to_string(&log_path).expect("read log");
    assert!(log.contains("close-flow-marker"), "log flushed on close");
}
