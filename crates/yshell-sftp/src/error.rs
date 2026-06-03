//! SFTP error types.

use std::fmt;

pub type SftpResult<T> = Result<T, SftpError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpError {
    pub message: String,
}

impl SftpError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SftpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SftpError {}
