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
pub mod real;

pub use auth::{
    AuthAttempt, AuthAttemptKind, AuthMethod, AuthMethods, AuthProblemKind,
    KeyboardInteractiveChallenge, KeyboardInteractivePrompter, KeyboardInteractiveResponse, Prompt,
    ScriptedPrompter, SingleSecretPrompter,
};
pub use channel::{ChannelKind, ShellSession};
pub use client::{
    ExecOutput, ShellAdapter, ShellClient, SshAdapter, SshClient, SshConnectionConfig,
};
pub use error::{HostKeyProblem, SshError, SshErrorKind, SshResult};
pub use forwarding::{ForwardingKind, TunnelConfig, TunnelState};
pub use host_key::{HostKeyDecision, HostKeyFingerprint, HostKeyPolicy, KnownHosts};
pub use proxy::{probe_proxy_connectivity, ProxyConfig, ProxyProbeReport, ProxyState};
pub use pty::{PtyConfig, PtySize};
pub use real::{
    with_connection_stage_hook, RealAuthStrategy, RealConnectionAttempt, RealConnectionPlan,
    RealConnectionSnapshot, RealConnectionStage, RealShellSessionPlaceholder, RealSshAdapter,
    RealTransportAction, RealTransportActionOutcome, RealTransportActionRecord,
    RealTransportExecutionReport, SshTransport,
};
