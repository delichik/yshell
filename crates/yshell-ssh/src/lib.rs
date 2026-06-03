//! SSH protocol adapter skeleton for YShell.

pub mod auth;
pub mod channel;
pub mod client;
pub mod error;
pub mod forwarding;
pub mod host_key;
pub mod proxy;
pub mod proxy_http_connect;
pub mod proxy_socks4;
pub mod proxy_socks5;
pub mod pty;

pub use client::{SshClient, SshConnectionConfig};
pub use error::{SshError, SshResult};
