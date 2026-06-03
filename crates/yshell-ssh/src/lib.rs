//! SSH protocol adapter for YShell.

pub mod auth;
pub mod channel;
pub mod client;
pub mod error;
pub mod fake;
pub mod forwarding;
pub mod host_key;
pub mod proxy;
pub mod proxy_http_connect;
pub mod proxy_socks4;
pub mod proxy_socks5;
pub mod pty;
pub mod real;

pub use auth::AuthMethod;
pub use channel::{ChannelKind, ShellSession};
pub use client::{
    ExecOutput, ShellAdapter, ShellClient, SshAdapter, SshClient, SshConnectionConfig,
    TransportBackend,
};
pub use error::{SshError, SshErrorKind, SshResult};
pub use fake::{FakeShellSession, FakeSshAdapter, FakeSshSession};
pub use forwarding::{ForwardingKind, TunnelConfig, TunnelState};
pub use host_key::{HostKeyDecision, HostKeyFingerprint, HostKeyPolicy, KnownHosts};
pub use proxy::{ProxyConfig, ProxyState};
pub use pty::{PtyConfig, PtySize};
pub use real::{
    RealAuthStrategy, RealConnectionAttempt, RealConnectionPlan, RealConnectionSnapshot,
    RealConnectionStage, RealShellSessionPlaceholder, RealSshAdapter,
};
