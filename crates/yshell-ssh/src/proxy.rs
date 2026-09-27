//! Proxy configuration and state for SSH transports.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProxyConfig {
    #[default]
    None,
    Socks4 {
        address: String,
        username: Option<String>,
    },
    Socks4a {
        address: String,
        username: Option<String>,
    },
    Socks5 {
        address: String,
        username: Option<String>,
        password: Option<String>,
        resolve_dns_by_proxy: bool,
    },
    HttpConnect {
        address: String,
        username: Option<String>,
        password: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyState {
    Disabled,
    Connecting { address: String },
    Connected { address: String },
    Failed { address: String, reason: String },
}

impl ProxyConfig {
    pub fn address(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Socks4 { address, .. }
            | Self::Socks4a { address, .. }
            | Self::Socks5 { address, .. }
            | Self::HttpConnect { address, .. } => Some(address),
        }
    }
}
