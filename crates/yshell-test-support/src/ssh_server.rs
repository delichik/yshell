//! Placeholder test SSH server hooks.

/// Placeholder handle for a future embedded test SSH server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSshServer {
    host: String,
    port: u16,
}

impl TestSshServer {
    /// Creates a placeholder server handle without starting network services.
    pub fn placeholder(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    /// Returns the host name/address.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Returns the port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }
}
