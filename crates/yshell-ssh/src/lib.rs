//! SSH protocol adapter for YShell.

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

pub use auth::AuthMethod;
pub use client::{
    ExecOutput, FakeSshAdapter, FakeSshSession, SshAdapter, SshClient, SshConnectionConfig,
};
pub use error::{SshError, SshErrorKind, SshResult};
pub use forwarding::{ForwardingKind, TunnelConfig, TunnelState};
pub use host_key::{HostKeyDecision, HostKeyFingerprint, HostKeyPolicy, KnownHosts};
pub use proxy::{ProxyConfig, ProxyState};
pub use pty::{PtyConfig, PtySize};
