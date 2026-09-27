//! SSH error types.

use std::fmt;

use crate::host_key::HostKeyFingerprint;

pub type SshResult<T> = Result<T, SshError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshError {
    pub kind: SshErrorKind,
    pub message: String,
    pub host_key_problem: Option<Box<HostKeyProblem>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyProblem {
    Unknown {
        host: String,
        port: u16,
        presented: HostKeyFingerprint,
    },
    Changed {
        host: String,
        port: u16,
        presented: HostKeyFingerprint,
        expected: HostKeyFingerprint,
    },
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
            host_key_problem: None,
        }
    }

    pub fn configuration(message: impl Into<String>) -> Self {
        Self::new(SshErrorKind::Configuration, message)
    }

    pub fn with_host_key_problem(mut self, problem: HostKeyProblem) -> Self {
        self.host_key_problem = Some(Box::new(problem));
        self
    }
}

impl fmt::Display for SshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for SshError {}
