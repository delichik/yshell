//! SSH authentication configuration.

/// Supported SSH authentication mechanisms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    Password {
        username: String,
        password: String,
    },
    PrivateKey {
        username: String,
        key_path: String,
        passphrase: Option<String>,
    },
    Agent {
        username: String,
    },
    KeyboardInteractive {
        username: String,
    },
}

impl AuthMethod {
    pub fn username(&self) -> &str {
        match self {
            Self::Password { username, .. }
            | Self::PrivateKey { username, .. }
            | Self::Agent { username }
            | Self::KeyboardInteractive { username } => username,
        }
    }
}
