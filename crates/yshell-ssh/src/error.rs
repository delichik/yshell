//! SSH error types.

use std::fmt;

pub type SshResult<T> = Result<T, SshError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshError {
    pub kind: SshErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshErrorKind {
    Configuration,
    Dns,
    TcpConnect,
    Proxy,
    Handshake,
    Authentication,
    HostKeyRejected,
    Channel,
    Timeout,
    Io,
    Unsupported,
}

impl SshError {
    pub fn new(kind: SshErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn configuration(message: impl Into<String>) -> Self {
        Self::new(SshErrorKind::Configuration, message)
    }
}

impl fmt::Display for SshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for SshError {}
