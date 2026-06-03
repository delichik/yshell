//! SSH error types.

use std::fmt;

pub type SshResult<T> = Result<T, SshError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshError {
    pub message: String,
}

impl SshError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SshError {}
