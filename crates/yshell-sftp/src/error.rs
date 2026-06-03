//! SFTP error types.

use std::fmt;

pub type SftpResult<T> = Result<T, SftpError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpError {
    pub kind: SftpErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SftpErrorKind {
    NotFound,
    AlreadyExists,
    PermissionDenied,
    InvalidPath,
    TransferCancelled,
    Backend,
}

impl SftpError {
    pub fn new(kind: SftpErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for SftpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for SftpError {}
