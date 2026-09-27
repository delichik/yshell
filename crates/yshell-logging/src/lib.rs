//! Logging initialization and transcript logging for `YShell`.

use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use time::{error::IndeterminateOffset, OffsetDateTime};

use tracing_subscriber::{
    fmt as tracing_fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

/// Options used to initialize the process-wide tracing subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggingConfig {
    env_filter: String,
    ansi: bool,
    compact: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            env_filter: "yshell=info,warn".to_owned(),
            ansi: true,
            compact: false,
        }
    }
}

impl LoggingConfig {
    /// Creates a config with a custom tracing [`EnvFilter`] directive string.
    #[must_use]
    pub fn with_env_filter(env_filter: impl Into<String>) -> Self {
        Self {
            env_filter: env_filter.into(),
            ..Self::default()
        }
    }

    /// Enables or disables ANSI color in formatted logs.
    #[must_use]
    pub const fn with_ansi(mut self, ansi: bool) -> Self {
        self.ansi = ansi;
        self
    }

    /// Enables compact formatting.
    #[must_use]
    pub const fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
}

/// Error returned when tracing cannot be initialized.
#[derive(Debug)]
pub enum InitLoggingError {
    /// The configured env filter string was invalid.
    InvalidFilter(tracing_subscriber::filter::ParseError),
    /// A global tracing subscriber was already installed.
    AlreadyInitialized(tracing_subscriber::util::TryInitError),
}

impl fmt::Display for InitLoggingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFilter(error) => write!(f, "invalid tracing filter: {error}"),
            Self::AlreadyInitialized(error) => {
                write!(f, "tracing was already initialized: {error}")
            }
        }
    }
}

impl Error for InitLoggingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidFilter(error) => Some(error),
            Self::AlreadyInitialized(error) => Some(error),
        }
    }
}

/// Initializes the process-wide tracing subscriber.
pub fn init_tracing(config: &LoggingConfig) -> Result<(), InitLoggingError> {
    let env_filter =
        EnvFilter::try_new(&config.env_filter).map_err(InitLoggingError::InvalidFilter)?;

    if config.compact {
        let fmt_layer = tracing_fmt::layer()
            .with_ansi(config.ansi)
            .with_target(true)
            .compact();
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .try_init()
            .map_err(InitLoggingError::AlreadyInitialized)
    } else {
        let fmt_layer = tracing_fmt::layer()
            .with_ansi(config.ansi)
            .with_target(true);
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .try_init()
            .map_err(InitLoggingError::AlreadyInitialized)
    }
}

/// Initializes tracing for tests, treating an existing subscriber as success.
pub fn try_init_tracing_for_tests() -> Result<(), InitLoggingError> {
    match init_tracing(&LoggingConfig::default().with_ansi(false)) {
        Ok(()) | Err(InitLoggingError::AlreadyInitialized(_)) => Ok(()),
        Err(error) => Err(error),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathTemplate(String);

impl PathTemplate {
    #[must_use]
    pub fn new(template: impl Into<String>) -> Self {
        Self(template.into())
    }

    #[must_use]
    pub fn render(&self, context: &LogPathContext) -> PathBuf {
        let rendered = self
            .0
            .replace(
                "{session_id}",
                &sanitize_path_component(&context.session_id),
            )
            .replace("{kind}", &sanitize_path_component(&context.kind))
            .replace("{timestamp}", &context.timestamp.to_string());
        PathBuf::from(rendered)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPathContext {
    pub session_id: String,
    pub kind: String,
    pub timestamp: u64,
}

impl LogPathContext {
    #[must_use]
    pub fn now(session_id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            kind: kind.into(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redactor {
    replacements: Vec<(String, String)>,
}

impl Redactor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            replacements: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        let secret = secret.into();
        if !secret.is_empty() {
            self.replacements.push((secret, "[REDACTED]".to_owned()));
        }
        self
    }

    #[must_use]
    pub fn redact(&self, input: &str) -> String {
        self.replacements
            .iter()
            .fold(input.to_owned(), |output, (needle, replacement)| {
                output.replace(needle, replacement)
            })
    }
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptFormat {
    Raw,
    Sanitized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptDirection {
    Input,
    Output,
}

impl TranscriptDirection {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationPolicy {
    pub max_bytes: u64,
    pub keep: usize,
}

impl RotationPolicy {
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            max_bytes: u64::MAX,
            keep: 0,
        }
    }
}

/// How [`SessionLogger::open_with`] treats an existing log file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogMode {
    /// Keep existing content and append new records.
    #[default]
    Append,
    /// Discard existing content and start from an empty file.
    Truncate,
}

/// Options for [`SessionLogger::open_with`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLogOptions {
    /// Destination path; missing parent directories are created.
    pub path: PathBuf,
    /// `Raw` writes payload bytes untouched; `Sanitized` strips ANSI escape
    /// sequences and then applies the logger redactor.
    pub format: TranscriptFormat,
    /// When enabled, every non-empty line starts with a `[HH:MM:SS]` local
    /// time timestamp. When the platform cannot determine the local offset,
    /// the timestamp falls back to UTC.
    pub timestamps: bool,
    /// Whether to append to or truncate an existing file.
    pub mode: LogMode,
}

impl SessionLogOptions {
    /// Creates options with `Raw` format, timestamps disabled and append mode.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: TranscriptFormat::Raw,
            timestamps: false,
            mode: LogMode::Append,
        }
    }

    /// Selects the transcript format.
    #[must_use]
    pub fn with_format(mut self, format: TranscriptFormat) -> Self {
        self.format = format;
        self
    }

    /// Enables or disables line timestamps.
    #[must_use]
    pub fn with_timestamps(mut self, timestamps: bool) -> Self {
        self.timestamps = timestamps;
        self
    }

    /// Selects append or truncate mode.
    #[must_use]
    pub fn with_mode(mut self, mode: LogMode) -> Self {
        self.mode = mode;
        self
    }
}

#[derive(Debug)]
pub struct SessionLogger {
    path: PathBuf,
    file: File,
    format: TranscriptFormat,
    redactor: Redactor,
    rotation: RotationPolicy,
    timestamps: bool,
    legacy: bool,
    line_start: bool,
    ansi: AnsiStripper,
    last_error: Option<io::Error>,
}

impl SessionLogger {
    /// Opens a session log with the original signature.
    ///
    /// This keeps the historical line-per-record format (direction marker,
    /// escaped payload, trailing newline) while delegating construction to
    /// [`SessionLogger::open_with`], so legacy loggers still gain
    /// [`SessionLogger::flush`], [`SessionLogger::close`],
    /// [`SessionLogger::last_error`] and the `Drop` fallback flush.
    pub fn open(
        path: impl Into<PathBuf>,
        format: TranscriptFormat,
        redactor: Redactor,
        rotation: RotationPolicy,
    ) -> io::Result<Self> {
        let mut logger = Self::open_with(SessionLogOptions::new(path).with_format(format))?;
        logger.redactor = redactor;
        logger.rotation = rotation;
        logger.legacy = true;
        Ok(logger)
    }

    /// Opens a session log from explicit [`SessionLogOptions`].
    ///
    /// [`SessionLogger::record`] chunks are treated as one byte stream: the
    /// optional local-time `[HH:MM:SS]` timestamp (UTC when the local offset
    /// is indeterminate) and the direction marker are written at the start of
    /// each line only, so chunks split mid-line are stitched back together
    /// without injecting prefixes.
    pub fn open_with(options: SessionLogOptions) -> io::Result<Self> {
        let SessionLogOptions {
            path,
            format,
            timestamps,
            mode,
        } = options;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        if let Some(parent) = parent {
            fs::create_dir_all(parent)?;
        }
        let mut open_options = OpenOptions::new();
        open_options.create(true);
        match mode {
            LogMode::Append => {
                open_options.append(true);
            }
            LogMode::Truncate => {
                open_options.write(true).truncate(true);
            }
        }
        let file = open_options.open(&path)?;
        Ok(Self {
            path,
            file,
            format,
            redactor: Redactor::default(),
            rotation: RotationPolicy::disabled(),
            timestamps,
            legacy: false,
            line_start: true,
            ansi: AnsiStripper::default(),
            last_error: None,
        })
    }

    /// Appends one transcript chunk.
    pub fn record(&mut self, direction: TranscriptDirection, bytes: &[u8]) -> io::Result<()> {
        if let Err(error) = self.rotate_if_needed(bytes.len() as u64) {
            return Err(self.note_error(error));
        }
        if self.legacy {
            return self.record_legacy(direction, bytes);
        }
        match self.format {
            TranscriptFormat::Raw => self.write_stream(direction, bytes),
            TranscriptFormat::Sanitized => {
                let mut sanitized = Vec::with_capacity(bytes.len());
                self.ansi.strip_into(bytes, &mut sanitized);
                let text = String::from_utf8_lossy(&sanitized);
                let redacted = self.redactor.redact(&text);
                self.write_stream(direction, redacted.as_bytes())
            }
        }
    }

    /// Flushes pending content to the operating system.
    pub fn flush(&mut self) -> io::Result<()> {
        let result = self.file.flush();
        self.check_result(result)
    }

    /// Flushes and closes the log file.
    pub fn close(mut self) -> io::Result<()> {
        self.flush()
    }

    /// Returns the most recent write failure, if any.
    ///
    /// The stored error is replaced only by a newer failure; successful writes
    /// leave it untouched so callers can inspect failures after the fact.
    #[must_use]
    pub fn last_error(&self) -> Option<&io::Error> {
        self.last_error.as_ref()
    }

    #[must_use]
    pub const fn path(&self) -> &PathBuf {
        &self.path
    }

    fn record_legacy(&mut self, direction: TranscriptDirection, bytes: &[u8]) -> io::Result<()> {
        let content = String::from_utf8_lossy(bytes);
        let content = match self.format {
            TranscriptFormat::Raw => content.into_owned(),
            TranscriptFormat::Sanitized => self.redactor.redact(&content),
        };
        let line = format!("{}\t{}\n", direction.as_str(), content.escape_default());
        let result = self.file.write_all(line.as_bytes());
        self.check_result(result)?;
        let result = self.file.flush();
        self.check_result(result)
    }

    fn write_stream(&mut self, direction: TranscriptDirection, payload: &[u8]) -> io::Result<()> {
        if payload.is_empty() {
            return Ok(());
        }
        let mut line = Vec::with_capacity(payload.len() + 32);
        let mut index = 0;
        while index < payload.len() {
            if self.line_start {
                if payload[index] == b'\n' {
                    line.push(b'\n');
                    index += 1;
                    continue;
                }
                if self.timestamps {
                    line.extend_from_slice(hms_prefix(local_now_or_utc()).as_bytes());
                }
                line.extend_from_slice(direction.as_str().as_bytes());
                line.push(b'\t');
                self.line_start = false;
            }
            let start = index;
            while index < payload.len() && payload[index] != b'\n' {
                index += 1;
            }
            line.extend_from_slice(&payload[start..index]);
            if index < payload.len() {
                line.push(b'\n');
                index += 1;
                self.line_start = true;
            }
        }
        let result = self.file.write_all(&line);
        self.check_result(result)?;
        let result = self.file.flush();
        self.check_result(result)
    }

    fn check_result(&mut self, result: io::Result<()>) -> io::Result<()> {
        match result {
            Ok(()) => Ok(()),
            Err(error) => Err(self.note_error(error)),
        }
    }

    fn note_error(&mut self, error: io::Error) -> io::Error {
        let reported = io::Error::new(error.kind(), error.to_string());
        self.last_error = Some(error);
        reported
    }

    fn rotate_if_needed(&mut self, incoming: u64) -> io::Result<()> {
        if self.rotation.max_bytes == u64::MAX || self.rotation.keep == 0 {
            return Ok(());
        }
        let len = self.file.metadata()?.len();
        if len + incoming <= self.rotation.max_bytes {
            return Ok(());
        }
        self.file.flush()?;
        for index in (1..=self.rotation.keep).rev() {
            let source = rotated_path(&self.path, index);
            let destination = rotated_path(&self.path, index + 1);
            if source.exists() {
                if index == self.rotation.keep {
                    fs::remove_file(&source)?;
                } else {
                    fs::rename(&source, &destination)?;
                }
            }
        }
        if self.path.exists() {
            fs::rename(&self.path, rotated_path(&self.path, 1))?;
        }
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.line_start = true;
        Ok(())
    }
}

impl Drop for SessionLogger {
    fn drop(&mut self) {
        if let Err(error) = self.file.flush() {
            self.last_error = Some(error);
        }
    }
}

#[derive(Debug)]
pub struct TransferLogger {
    inner: SessionLogger,
}

impl TransferLogger {
    pub fn open(path: impl Into<PathBuf>, redactor: Redactor) -> io::Result<Self> {
        Ok(Self {
            inner: SessionLogger::open(
                path,
                TranscriptFormat::Sanitized,
                redactor,
                RotationPolicy::disabled(),
            )?,
        })
    }

    #[must_use]
    pub const fn path(&self) -> &PathBuf {
        self.inner.path()
    }

    pub fn record_transfer(
        &mut self,
        operation: &str,
        local_path: &Path,
        remote_path: &Path,
        bytes: u64,
    ) -> io::Result<()> {
        let line = format!(
            "operation={operation} local={} remote={} bytes={bytes}",
            local_path.display(),
            remote_path.display()
        );
        self.inner
            .record(TranscriptDirection::Output, line.as_bytes())
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum AnsiState {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    OscEscape,
    ControlString,
    ControlStringEscape,
}

/// Streaming ANSI escape sequence stripper.
///
/// The state is kept across [`SessionLogger::record`] calls so escape
/// sequences split between chunks are still removed instead of leaking their
/// remaining bytes into the sanitized transcript.
#[derive(Debug, Default)]
struct AnsiStripper {
    state: AnsiState,
}

impl AnsiStripper {
    fn strip_into(&mut self, input: &[u8], output: &mut Vec<u8>) {
        for &byte in input {
            self.state = match self.state {
                AnsiState::Ground => {
                    if byte == 0x1B {
                        AnsiState::Escape
                    } else {
                        output.push(byte);
                        AnsiState::Ground
                    }
                }
                AnsiState::Escape => match byte {
                    b'[' => AnsiState::Csi,
                    b']' => AnsiState::Osc,
                    b'P' | b'X' | b'^' | b'_' => AnsiState::ControlString,
                    0x20..=0x2F => AnsiState::EscapeIntermediate,
                    0x1B => AnsiState::Escape,
                    0x30..=0x7E => AnsiState::Ground,
                    _ => {
                        output.push(byte);
                        AnsiState::Ground
                    }
                },
                AnsiState::EscapeIntermediate => match byte {
                    0x20..=0x2F => AnsiState::EscapeIntermediate,
                    0x1B => AnsiState::Escape,
                    0x30..=0x7E => AnsiState::Ground,
                    _ => {
                        output.push(byte);
                        AnsiState::Ground
                    }
                },
                AnsiState::Csi => {
                    if (0x40..=0x7E).contains(&byte) {
                        AnsiState::Ground
                    } else {
                        AnsiState::Csi
                    }
                }
                AnsiState::Osc => match byte {
                    0x07 => AnsiState::Ground,
                    0x1B => AnsiState::OscEscape,
                    _ => AnsiState::Osc,
                },
                AnsiState::OscEscape => match byte {
                    b'\\' | 0x07 => AnsiState::Ground,
                    0x1B => AnsiState::OscEscape,
                    _ => AnsiState::Osc,
                },
                AnsiState::ControlString => {
                    if byte == 0x1B {
                        AnsiState::ControlStringEscape
                    } else {
                        AnsiState::ControlString
                    }
                }
                AnsiState::ControlStringEscape => match byte {
                    b'\\' => AnsiState::Ground,
                    0x1B => AnsiState::ControlStringEscape,
                    _ => AnsiState::ControlString,
                },
            };
        }
    }
}

/// Formats `time` as `[HH:MM:SS]`, using the offset carried by `time`.
fn hms_prefix(time: OffsetDateTime) -> String {
    format!(
        "[{:02}:{:02}:{:02}]",
        time.hour(),
        time.minute(),
        time.second()
    )
}

/// Resolves a local-time lookup, falling back to UTC when the local offset is
/// indeterminate.
fn resolve_local_now(local: Result<OffsetDateTime, IndeterminateOffset>) -> OffsetDateTime {
    local.unwrap_or_else(|_| OffsetDateTime::now_utc())
}

/// Returns the current time in the local timezone.
///
/// Platforms that cannot determine the local offset (for example an
/// indeterminate-offset lookup) fall back to UTC instead of failing or
/// dropping the timestamp.
fn local_now_or_utc() -> OffsetDateTime {
    resolve_local_now(OffsetDateTime::now_local())
}

fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => character,
            _ => '_',
        })
        .collect()
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.display(), index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_filter() {
        let error = init_tracing(&LoggingConfig::with_env_filter("[not-a-filter"))
            .expect_err("invalid filter should fail before subscriber install");

        assert!(matches!(error, InitLoggingError::InvalidFilter(_)));
    }

    #[test]
    fn test_initializer_is_idempotent() {
        try_init_tracing_for_tests().expect("first init should succeed");
        try_init_tracing_for_tests().expect("second init should be ignored");
    }

    #[test]
    fn path_template_sanitizes_components() {
        let template = PathTemplate::new("logs/{session_id}/{kind}-{timestamp}.log");
        let rendered = template.render(&LogPathContext {
            session_id: "prod/session:1".to_owned(),
            kind: "terminal".to_owned(),
            timestamp: 42,
        });

        assert_eq!(
            rendered,
            PathBuf::from("logs/prod_session_1/terminal-42.log")
        );
    }

    #[test]
    fn sanitized_transcript_redacts_sensitive_input_and_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.log");
        let redactor = Redactor::new().with_secret("hunter2");
        let mut logger = SessionLogger::open(
            &path,
            TranscriptFormat::Sanitized,
            redactor,
            RotationPolicy::disabled(),
        )
        .expect("open logger");

        logger
            .record(TranscriptDirection::Input, b"password=hunter2")
            .expect("input");
        logger
            .record(TranscriptDirection::Output, b"echo hunter2")
            .expect("output");

        let log = fs::read_to_string(path).expect("read log");
        assert!(!log.contains("hunter2"));
        assert!(log.contains("[REDACTED]"));
        assert!(log.contains("input"));
        assert!(log.contains("output"));
    }

    #[test]
    fn raw_transcript_preserves_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("raw.log");
        let mut logger = SessionLogger::open(
            &path,
            TranscriptFormat::Raw,
            Redactor::new().with_secret("hunter2"),
            RotationPolicy::disabled(),
        )
        .expect("open logger");

        logger
            .record(TranscriptDirection::Input, b"password=hunter2")
            .expect("input");

        assert!(fs::read_to_string(path).expect("read").contains("hunter2"));
    }

    #[test]
    fn rotates_session_log_when_threshold_is_crossed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rotate.log");
        let mut logger = SessionLogger::open(
            &path,
            TranscriptFormat::Raw,
            Redactor::new(),
            RotationPolicy {
                max_bytes: 10,
                keep: 2,
            },
        )
        .expect("open logger");

        logger
            .record(TranscriptDirection::Output, b"first long line")
            .expect("first");
        logger
            .record(TranscriptDirection::Output, b"second long line")
            .expect("second");

        assert!(path.exists());
        assert!(rotated_path(&path, 1).exists());
    }

    fn assert_timestamped(line: &str) -> &str {
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

    fn open_stream(path: &Path, format: TranscriptFormat, timestamps: bool) -> SessionLogger {
        SessionLogger::open_with(
            SessionLogOptions::new(path)
                .with_format(format)
                .with_timestamps(timestamps),
        )
        .expect("open session log")
    }

    #[test]
    fn hms_prefix_formats_injected_local_times() {
        use time::macros::datetime;

        assert_eq!(
            hms_prefix(datetime!(2026-09-27 14:30:45 +08:00)),
            "[14:30:45]"
        );
        assert_eq!(
            hms_prefix(datetime!(2026-09-27 00:00:00 UTC)),
            "[00:00:00]"
        );
        assert_eq!(
            hms_prefix(datetime!(2026-09-27 23:59:59 -07:00)),
            "[23:59:59]"
        );
    }

    #[test]
    fn local_timestamp_falls_back_to_utc_when_offset_is_indeterminate() {
        use time::UtcOffset;

        let fallback = resolve_local_now(Err(IndeterminateOffset));
        assert_eq!(fallback.offset(), UtcOffset::UTC);

        let injected = OffsetDateTime::now_utc();
        assert_eq!(resolve_local_now(Ok(injected)), injected);
    }

    #[test]
    fn local_timestamps_appear_at_line_starts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("local-timestamps.log");
        let mut logger = open_stream(&path, TranscriptFormat::Sanitized, true);

        logger
            .record(
                TranscriptDirection::Output,
                b"\x1b[32m$ uptime\x1b[0m\r\n 14:02 up 3 days,  2 users\r\n",
            )
            .expect("record sample output");

        let log = fs::read_to_string(&path).expect("read log");
        for line in log.lines() {
            assert_timestamped(line);
        }
        // Run with `--nocapture` to print the local-time sample transcript.
        eprintln!("local timestamp sample:\n{log}");
    }

    #[test]
    fn session_log_options_default_to_raw_append_without_timestamps() {
        let options = SessionLogOptions::new("session.log");
        assert_eq!(options.path, PathBuf::from("session.log"));
        assert_eq!(options.format, TranscriptFormat::Raw);
        assert!(!options.timestamps);
        assert_eq!(options.mode, LogMode::Append);
    }

    #[test]
    fn timestamps_prefix_only_line_starts_across_chunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("stamped.log");
        let mut logger = open_stream(&path, TranscriptFormat::Raw, true);

        logger
            .record(TranscriptDirection::Output, b"first")
            .expect("first chunk");
        logger
            .record(TranscriptDirection::Output, b" line\nsecond")
            .expect("second chunk");
        logger
            .record(TranscriptDirection::Output, b" line\n")
            .expect("third chunk");

        let log = fs::read_to_string(&path).expect("read log");
        assert!(log.ends_with('\n'));
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(assert_timestamped(lines[0]), "output\tfirst line");
        assert_eq!(assert_timestamped(lines[1]), "output\tsecond line");
    }

    #[test]
    fn timestamps_off_keeps_plain_stream() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plain.log");
        let mut logger = open_stream(&path, TranscriptFormat::Raw, false);

        logger
            .record(TranscriptDirection::Input, b"ls\npwd\n")
            .expect("record");

        let log = fs::read_to_string(&path).expect("read log");
        assert_eq!(log, "input\tls\ninput\tpwd\n");
    }

    #[test]
    fn raw_format_preserves_payload_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("raw-bytes.log");
        let mut logger = open_stream(&path, TranscriptFormat::Raw, false);
        let payload = b"\x1b[31mred\x1b[0m\x00\xff";

        logger
            .record(TranscriptDirection::Output, payload)
            .expect("record");

        let bytes = fs::read(&path).expect("read log");
        let mut expected = b"output\t".to_vec();
        expected.extend_from_slice(payload);
        assert_eq!(bytes, expected, "raw mode must keep payload bytes intact");
    }

    #[test]
    fn sanitized_format_strips_escape_sequences() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sanitized.log");
        let mut logger = open_stream(&path, TranscriptFormat::Sanitized, false);

        logger
            .record(
                TranscriptDirection::Output,
                b"plain \x1b[1;38;2;12;34;56mred-ish\x1b[0m",
            )
            .expect("record");

        let log = fs::read_to_string(&path).expect("read log");
        assert_eq!(log, "output\tplain red-ish");
        assert!(!log.contains('\u{1b}'));
    }

    #[test]
    fn sanitized_format_strips_osc_and_control_strings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sanitized-osc.log");
        let mut logger = open_stream(&path, TranscriptFormat::Sanitized, false);

        logger
            .record(
                TranscriptDirection::Output,
                b"\x1b]0;title\x07visible\x1b]8;;http://example\x1b\\link\x1b]8;;\x1b\\\x1bPignored\x1b\\",
            )
            .expect("record");

        let log = fs::read_to_string(&path).expect("read log");
        assert_eq!(log, "output\tvisiblelink");
    }

    #[test]
    fn sanitized_format_keeps_state_across_chunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sanitized-split.log");
        let mut logger = open_stream(&path, TranscriptFormat::Sanitized, true);

        logger
            .record(TranscriptDirection::Output, b"\x1b[3")
            .expect("partial escape");
        logger
            .record(TranscriptDirection::Output, b"1mred\x1b[0m")
            .expect("rest of escape");

        let log = fs::read_to_string(&path).expect("read log");
        assert_eq!(assert_timestamped(&log), "output\tred");
    }

    #[test]
    fn sanitized_timestamps_follow_escape_removal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sanitized-stamped.log");
        let mut logger = open_stream(&path, TranscriptFormat::Sanitized, true);

        logger
            .record(TranscriptDirection::Output, b"\x1b[32mok\x1b[0m\ndone")
            .expect("record");

        let log = fs::read_to_string(&path).expect("read log");
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(assert_timestamped(lines[0]), "output\tok");
        assert_eq!(assert_timestamped(lines[1]), "output\tdone");
    }

    #[test]
    fn append_mode_keeps_existing_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("append.log");
        fs::write(&path, b"previous\n").expect("seed log");
        let mut logger = SessionLogger::open_with(
            SessionLogOptions::new(&path)
                .with_mode(LogMode::Append)
                .with_timestamps(true),
        )
        .expect("open logger");

        logger
            .record(TranscriptDirection::Output, b"tail")
            .expect("record");

        let log = fs::read_to_string(&path).expect("read log");
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "previous");
        assert_eq!(assert_timestamped(lines[1]), "output\ttail");
    }

    #[test]
    fn truncate_mode_replaces_existing_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("truncate.log");
        fs::write(&path, b"previous\n").expect("seed log");
        let mut logger =
            SessionLogger::open_with(SessionLogOptions::new(&path).with_mode(LogMode::Truncate))
                .expect("open logger");

        logger
            .record(TranscriptDirection::Output, b"tail")
            .expect("record");

        assert_eq!(
            fs::read_to_string(&path).expect("read log"),
            "output\ttail"
        );
    }

    #[test]
    fn open_with_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/logs/session.log");
        let mut logger = open_stream(&path, TranscriptFormat::Raw, false);

        logger
            .record(TranscriptDirection::Output, b"ok")
            .expect("record");

        assert_eq!(fs::read_to_string(&path).expect("read log"), "output\tok");
    }

    #[test]
    fn record_with_empty_payload_keeps_line_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.log");
        let mut logger = open_stream(&path, TranscriptFormat::Raw, true);

        logger
            .record(TranscriptDirection::Output, b"")
            .expect("empty record");
        logger
            .record(TranscriptDirection::Output, b"first")
            .expect("record");

        let log = fs::read_to_string(&path).expect("read log");
        assert_eq!(assert_timestamped(&log), "output\tfirst");
    }

    #[test]
    fn close_persists_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("closed.log");
        let mut logger = open_stream(&path, TranscriptFormat::Raw, true);

        logger
            .record(TranscriptDirection::Output, b"before close\n")
            .expect("record");
        logger.close().expect("close");

        let log = fs::read_to_string(&path).expect("read after close");
        assert_eq!(assert_timestamped(&log), "output\tbefore close\n");
    }

    #[test]
    fn drop_flushes_content_without_close() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dropped.log");
        {
            let mut logger = open_stream(&path, TranscriptFormat::Raw, true);
            logger
                .record(TranscriptDirection::Output, b"before drop\n")
                .expect("record");
        }

        let log = fs::read_to_string(&path).expect("read after drop");
        assert_eq!(assert_timestamped(&log), "output\tbefore drop\n");
    }

    #[test]
    fn flush_reports_success() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("flushed.log");
        let mut logger = open_stream(&path, TranscriptFormat::Raw, false);

        logger
            .record(TranscriptDirection::Output, b"flushed")
            .expect("record");
        logger.flush().expect("flush");

        assert!(logger.last_error().is_none());
        assert_eq!(
            fs::read_to_string(&path).expect("read log"),
            "output\tflushed"
        );
    }

    #[test]
    fn legacy_open_keeps_framed_line_format() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("legacy.log");
        let mut logger = SessionLogger::open(
            &path,
            TranscriptFormat::Raw,
            Redactor::new(),
            RotationPolicy::disabled(),
        )
        .expect("open legacy logger");

        logger
            .record(TranscriptDirection::Input, b"ls\npwd")
            .expect("record");

        let log = fs::read_to_string(&path).expect("read log");
        assert_eq!(log, "input\tls\\npwd\n");
        assert!(logger.last_error().is_none());
    }

    #[test]
    fn open_with_fails_when_target_is_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = SessionLogger::open_with(SessionLogOptions::new(dir.path()))
            .expect_err("a directory is not a valid log file");

        assert!(
            matches!(
                error.kind(),
                io::ErrorKind::IsADirectory | io::ErrorKind::PermissionDenied
            ),
            "unexpected error kind: {:?}",
            error.kind()
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_with_rejects_read_only_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("read-only.log");
        fs::write(&path, b"existing").expect("seed log");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).expect("chmod");

        if OpenOptions::new().write(true).open(&path).is_ok() {
            // Privileged users (or CAP_DAC_OVERRIDE) bypass permission bits.
            return;
        }

        let error = SessionLogger::open_with(SessionLogOptions::new(&path))
            .expect_err("read-only file must not open for writing");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(unix)]
    #[test]
    fn write_failure_is_tracked_by_last_error() {
        let full = Path::new("/dev/full");
        if !full.exists() {
            return;
        }
        let mut logger =
            SessionLogger::open_with(SessionLogOptions::new(full)).expect("open /dev/full");
        assert!(logger.last_error().is_none());

        let error = logger
            .record(TranscriptDirection::Output, b"payload")
            .expect_err("writing to /dev/full must fail");
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);

        let last = logger.last_error().expect("failure recorded");
        assert_eq!(last.kind(), io::ErrorKind::StorageFull);
        assert!(!last.to_string().is_empty());
    }
}
