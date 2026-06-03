//! SSH client connection skeleton.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConnectionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
}

#[derive(Debug, Default)]
pub struct SshClient;

impl SshClient {
    pub fn new() -> Self {
        Self
    }
}
