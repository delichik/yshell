//! Logging initialization for `YShell`.

use std::{error::Error, fmt};

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
///
/// This should be called once by the application during startup. Tests can use
/// [`try_init_tracing_for_tests`] when multiple test cases may race to install a
/// subscriber.
pub fn init_tracing(config: &LoggingConfig) -> Result<(), InitLoggingError> {
    let env_filter =
        EnvFilter::try_new(&config.env_filter).map_err(InitLoggingError::InvalidFilter)?;
    let fmt_layer = tracing_fmt::layer()
        .with_ansi(config.ansi)
        .with_target(true)
        .compact();

    if config.compact {
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
}
