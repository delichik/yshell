//! Logging initialization and transcript logging for `YShell`.

use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

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

#[derive(Debug)]
pub struct SessionLogger {
    path: PathBuf,
    file: File,
    format: TranscriptFormat,
    redactor: Redactor,
    rotation: RotationPolicy,
}

impl SessionLogger {
    pub fn open(
        path: impl Into<PathBuf>,
        format: TranscriptFormat,
        redactor: Redactor,
        rotation: RotationPolicy,
    ) -> io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file,
            format,
            redactor,
            rotation,
        })
    }

    pub fn record(&mut self, direction: TranscriptDirection, bytes: &[u8]) -> io::Result<()> {
        self.rotate_if_needed(bytes.len() as u64)?;
        let content = String::from_utf8_lossy(bytes);
        let content = match self.format {
            TranscriptFormat::Raw => content.into_owned(),
            TranscriptFormat::Sanitized => self.redactor.redact(&content),
        };
        writeln!(
            self.file,
            "{}\t{}",
            direction.as_str(),
            content.escape_default()
        )?;
        self.file.flush()
    }

    #[must_use]
    pub const fn path(&self) -> &PathBuf {
        &self.path
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
        Ok(())
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
}
