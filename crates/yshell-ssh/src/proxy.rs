//! Proxy configuration placeholders.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyConfig {
    None,
    Socks4 { address: String },
    Socks5 { address: String },
    HttpConnect { address: String },
}
