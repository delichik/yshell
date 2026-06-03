//! Logging panel view model.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntryItem {
    pub level: LogLevel,
    pub source: String,
    pub message: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoggingModel {
    pub entries: Vec<LogEntryItem>,
    pub filter: String,
}
