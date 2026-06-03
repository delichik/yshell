//! Real SSH transport integration skeleton.
//!
//! This module intentionally keeps the "real" path explicit even before the
//! actual network transport is wired, so the app/runtime can target a stable
//! replacement boundary instead of assuming the fake adapter is the only path.

use std::collections::VecDeque;
use std::time::Duration;

use crate::{
    channel::ShellSession,
    client::{ExecOutput, ShellAdapter, SshAdapter, SshConnectionConfig},
    error::{SshError, SshErrorKind, SshResult},
    host_key::HostKeyPolicy,
    proxy::ProxyConfig,
    pty::PtySize,
    AuthMethod, PtyConfig,
};

#[derive(Debug, Clone, Default)]
pub struct RealSshAdapter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealAuthStrategy {
    Password,
    PrivateKey { key_path: String, has_passphrase: bool },
    Agent,
    KeyboardInteractive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealConnectionPlan {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_strategy: RealAuthStrategy,
    pub host_key_policy: HostKeyPolicy,
    pub proxy: ProxyConfig,
    pub pty: PtyConfig,
    pub connect_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealConnectionStage {
    Prepared,
    ProxyNegotiation,
    TcpConnect,
    SshHandshake,
    HostKeyVerification,
    Authentication,
    PtyRequest,
    ShellOpen,
    Connected,
    Disconnected,
    Failed,
}

impl RealConnectionStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::ProxyNegotiation => "proxy-negotiation",
            Self::TcpConnect => "tcp-connect",
            Self::SshHandshake => "ssh-handshake",
            Self::HostKeyVerification => "host-key-verification",
            Self::Authentication => "authentication",
            Self::PtyRequest => "pty-request",
            Self::ShellOpen => "shell-open",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealConnectionSnapshot {
    pub stage: RealConnectionStage,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealConnectionAttempt {
    pub plan: RealConnectionPlan,
    pub stage: RealConnectionStage,
    pub history: Vec<RealConnectionSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealShellSessionPlaceholder {
    pub attempt: RealConnectionAttempt,
    pending_output: VecDeque<Vec<u8>>,
}

impl RealSshAdapter {
    pub fn plan_connection(&self, config: &SshConnectionConfig) -> SshResult<RealConnectionPlan> {
        config.validate()?;
        Ok(RealConnectionPlan {
            host: config.host.clone(),
            port: config.port,
            username: config.username().to_owned(),
            auth_strategy: RealAuthStrategy::from_auth_method(&config.auth),
            host_key_policy: config.host_key_policy.clone(),
            proxy: config.proxy.clone(),
            pty: config.pty.clone(),
            connect_timeout: config.connect_timeout,
        })
    }

    pub fn scaffold_shell_session(
        &self,
        config: &SshConnectionConfig,
    ) -> SshResult<RealShellSessionPlaceholder> {
        Ok(RealShellSessionPlaceholder {
            attempt: RealConnectionAttempt::prepared(self.plan_connection(config)?),
            pending_output: VecDeque::new(),
        })
    }
}

impl RealAuthStrategy {
    fn from_auth_method(auth: &AuthMethod) -> Self {
        match auth {
            AuthMethod::Password { .. } => Self::Password,
            AuthMethod::PrivateKey {
                key_path,
                passphrase,
                ..
            } => Self::PrivateKey {
                key_path: key_path.clone(),
                has_passphrase: passphrase.is_some(),
            },
            AuthMethod::Agent { .. } => Self::Agent,
            AuthMethod::KeyboardInteractive { .. } => Self::KeyboardInteractive,
        }
    }
}

impl SshAdapter for RealSshAdapter {
    type Session = RealConnectionAttempt;

    fn connect(&self, config: &SshConnectionConfig) -> SshResult<Self::Session> {
        Ok(RealConnectionAttempt::begin(self.plan_connection(config)?))
    }

    fn exec(&self, session: &mut Self::Session, _command: &str) -> SshResult<ExecOutput> {
        Err(SshError::new(
            SshErrorKind::Unsupported,
            format!(
                "real SSH transport adapter does not support exec yet; current stage is {}",
                session.stage.label()
            ),
        ))
    }

    fn disconnect(&self, mut session: Self::Session) -> SshResult<()> {
        session.stage = RealConnectionStage::Disconnected;
        Ok(())
    }
}

impl ShellAdapter for RealSshAdapter {
    type Shell = RealShellSessionPlaceholder;

    fn open_shell(&self, config: &SshConnectionConfig) -> SshResult<Self::Shell> {
        let mut shell = self.scaffold_shell_session(config)?;
        shell.begin_connection_attempt();
        Ok(shell)
    }
}

impl ShellSession for RealShellSessionPlaceholder {
    fn is_connected(&self) -> bool {
        self.attempt.stage == RealConnectionStage::Connected
    }

    fn poll_output(&mut self) -> SshResult<Vec<u8>> {
        Ok(self.pending_output.pop_front().unwrap_or_default())
    }

    fn write_input(&mut self, bytes: &[u8]) -> SshResult<()> {
        let printable = String::from_utf8_lossy(bytes).replace('\r', "\\r");
        self.pending_output.push_back(
            format!(
                "real-shell scaffold buffered input at stage `{}`: {printable}\n",
                self.attempt.stage.label()
            )
            .into_bytes(),
        );
        self.pending_output.push_back(
            b"Network transport is not implemented yet, so input was not sent to a remote host.\n"
                .to_vec(),
        );
        Ok(())
    }

    fn resize_pty(&mut self, size: PtySize) -> SshResult<()> {
        self.attempt.plan.pty.size = size;
        self.pending_output.push_back(
            format!(
                "real-shell scaffold updated planned PTY to {}x{} while waiting at stage `{}`.\n",
                size.columns,
                size.rows,
                self.attempt.stage.label()
            )
            .into_bytes(),
        );
        Ok(())
    }

    fn disconnect(&mut self) -> SshResult<()> {
        self.attempt.stage = RealConnectionStage::Disconnected;
        self.pending_output.push_back(
            b"real-shell scaffold marked the session as disconnected before any live transport existed.\n"
                .to_vec(),
        );
        Ok(())
    }
}

impl RealShellSessionPlaceholder {
    fn begin_connection_attempt(&mut self) {
        for line in self.attempt.begin_transcript_lines() {
            let mut bytes = line.into_bytes();
            bytes.push(b'\n');
            self.pending_output.push_back(bytes);
        }
    }
}

impl RealAuthStrategy {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::PrivateKey { .. } => "private-key",
            Self::Agent => "agent",
            Self::KeyboardInteractive => "keyboard-interactive",
        }
    }
}

impl HostKeyPolicy {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::TrustOnFirstUse => "tofu",
            Self::AcceptAnyForTesting => "accept-any-for-testing",
        }
    }
}

impl ProxyConfig {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Socks4 { .. } => "socks4",
            Self::Socks5 { .. } => "socks5",
            Self::HttpConnect { .. } => "http-connect",
        }
    }
}

impl RealConnectionAttempt {
    pub fn prepared(plan: RealConnectionPlan) -> Self {
        let mut attempt = Self {
            plan,
            stage: RealConnectionStage::Prepared,
            history: Vec::new(),
        };
        attempt.record_snapshot(
            RealConnectionStage::Prepared,
            "Connection plan captured before any network activity.",
        );
        attempt
    }

    pub fn begin(plan: RealConnectionPlan) -> Self {
        let mut attempt = Self::prepared(plan);
        let next_stage = attempt.next_pending_stage();
        attempt.stage = next_stage;
        attempt.record_snapshot(
            next_stage,
            attempt.stage_note(next_stage).to_owned(),
        );
        attempt
    }

    pub fn begin_transcript_lines(&mut self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "Real SSH backend selected for {}@{}:{}",
                self.plan.username, self.plan.host, self.plan.port
            ),
            format!(
                "Connection plan: auth={} host-key={} proxy={} term={} size={}x{} timeout={}s",
                self.plan.auth_strategy.label(),
                self.plan.host_key_policy.label(),
                self.plan.proxy.label(),
                self.plan.pty.term,
                self.plan.pty.size.columns,
                self.plan.pty.size.rows,
                self.plan.connect_timeout.as_secs()
            ),
        ];
        if self.stage == RealConnectionStage::Prepared {
            let next_stage = self.next_pending_stage();
            self.stage = next_stage;
            self.record_snapshot(
                next_stage,
                self.stage_note(next_stage).to_owned(),
            );
        }
        lines.push(format!(
            "Real transport scaffold advanced to stage `{}`. {}",
            self.stage.label(),
            self.stage_note(self.stage)
        ));
        lines
    }

    fn record_snapshot(&mut self, stage: RealConnectionStage, note: impl Into<String>) {
        self.history.push(RealConnectionSnapshot {
            stage,
            note: note.into(),
        });
    }

    fn stage_note(&self, stage: RealConnectionStage) -> &'static str {
        match stage {
            RealConnectionStage::Prepared => {
                "Connection plan captured before any network activity."
            }
            RealConnectionStage::ProxyNegotiation => {
                "Proxy negotiation must complete before TCP connect can begin."
            }
            RealConnectionStage::TcpConnect => {
                "TCP socket creation is the next missing transport step."
            }
            RealConnectionStage::SshHandshake => {
                "SSH identification exchange and key negotiation are pending."
            }
            RealConnectionStage::HostKeyVerification => {
                "Presented host key must be checked against the configured policy."
            }
            RealConnectionStage::Authentication => {
                "Authentication must run using the configured auth strategy."
            }
            RealConnectionStage::PtyRequest => {
                "Remote PTY allocation must complete before interactive shell open."
            }
            RealConnectionStage::ShellOpen => {
                "Shell channel open is the next step after transport and auth succeed."
            }
            RealConnectionStage::Connected => "Transport and shell are live.",
            RealConnectionStage::Disconnected => "Connection attempt or shell was explicitly disconnected.",
            RealConnectionStage::Failed => "Connection attempt reached a failed terminal state.",
        }
    }

    fn next_pending_stage(&self) -> RealConnectionStage {
        match self.plan.proxy {
            ProxyConfig::None => RealConnectionStage::TcpConnect,
            _ => RealConnectionStage::ProxyNegotiation,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AuthMethod, ProxyConfig, ShellClient, ShellSession, SshAdapter, SshConnectionConfig,
    };

    use super::{RealAuthStrategy, RealConnectionStage, RealSshAdapter};

    #[test]
    fn real_backend_now_returns_a_non_connected_stage_scaffold() {
        let client = ShellClient::with_real_backend();
        let config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );

        let mut shell = client.open_shell(&config).expect("real shell scaffold");
        assert!(!shell.is_connected());
        assert_eq!(shell.attempt.stage, RealConnectionStage::TcpConnect);

        let first_bytes = shell.poll_output().expect("banner");
        let second_bytes = shell.poll_output().expect("plan");
        let third_bytes = shell.poll_output().expect("stage");
        let first_chunk = String::from_utf8_lossy(&first_bytes);
        let second_chunk = String::from_utf8_lossy(&second_bytes);
        let third_chunk = String::from_utf8_lossy(&third_bytes);
        assert!(first_chunk.contains("Real SSH backend selected"));
        assert!(second_chunk.contains("auth=agent"));
        assert!(third_chunk.contains("tcp-connect"));
    }

    #[test]
    fn real_adapter_can_build_connection_plan_before_transport_exists() {
        let adapter = RealSshAdapter;
        let config = SshConnectionConfig::new(
            "example.test",
            2222,
            AuthMethod::PrivateKey {
                username: "alice".to_owned(),
                key_path: "/tmp/id_ed25519".to_owned(),
                passphrase: Some("secret".to_owned()),
            },
        );

        let plan = adapter.plan_connection(&config).expect("plan connection");
        assert_eq!(plan.host, "example.test");
        assert_eq!(plan.port, 2222);
        assert_eq!(plan.username, "alice");
        assert_eq!(
            plan.auth_strategy,
            RealAuthStrategy::PrivateKey {
                key_path: "/tmp/id_ed25519".to_owned(),
                has_passphrase: true,
            }
        );
    }

    #[test]
    fn real_adapter_exposes_prepared_stage_scaffold() {
        let adapter = RealSshAdapter;
        let config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );

        let mut shell = adapter
            .scaffold_shell_session(&config)
            .expect("scaffold shell session");
        assert_eq!(shell.attempt.stage, RealConnectionStage::Prepared);
        assert!(!shell.is_connected());

        assert!(shell.poll_output().expect("prepared output").is_empty());
    }

    #[test]
    fn real_scaffold_disconnects_without_transport_error() {
        let client = ShellClient::with_real_backend();
        let config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );

        let mut shell = client.open_shell(&config).expect("real shell scaffold");
        shell.disconnect().expect("disconnect scaffold");
        assert_eq!(shell.attempt.stage, RealConnectionStage::Disconnected);
        let _ = shell.poll_output().expect("banner");
        let _ = shell.poll_output().expect("plan");
        let _ = shell.poll_output().expect("stage");
        let disconnect_bytes = shell.poll_output().expect("disconnect chunk");
        let disconnect_chunk = String::from_utf8_lossy(&disconnect_bytes);
        assert!(disconnect_chunk.contains("disconnected"));
    }

    #[test]
    fn real_backend_uses_proxy_negotiation_stage_when_proxy_is_configured() {
        let client = ShellClient::with_real_backend();
        let mut config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );
        config.proxy = ProxyConfig::Socks5 {
            address: "127.0.0.1:1080".to_owned(),
            username: None,
            password: None,
        };

        let mut shell = client.open_shell(&config).expect("real shell scaffold");
        assert_eq!(shell.attempt.stage, RealConnectionStage::ProxyNegotiation);
        let _ = shell.poll_output().expect("banner");
        let plan_bytes = shell.poll_output().expect("plan");
        let stage_bytes = shell.poll_output().expect("stage");
        let plan_chunk = String::from_utf8_lossy(&plan_bytes);
        let stage_chunk = String::from_utf8_lossy(&stage_bytes);
        assert!(plan_chunk.contains("proxy=socks5"));
        assert!(stage_chunk.contains("proxy-negotiation"));
    }

    #[test]
    fn real_connect_returns_stage_aware_connection_attempt() {
        let adapter = RealSshAdapter;
        let config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );

        let attempt = adapter.connect(&config).expect("real connection attempt");
        assert_eq!(attempt.stage, RealConnectionStage::TcpConnect);
        assert_eq!(attempt.plan.username, "alice");
        assert_eq!(attempt.history.len(), 2);
        assert!(attempt.history[1].note.contains("TCP socket creation"));
    }

    #[test]
    fn real_connect_uses_proxy_stage_when_proxy_exists() {
        let adapter = RealSshAdapter;
        let mut config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );
        config.proxy = ProxyConfig::HttpConnect {
            address: "127.0.0.1:8080".to_owned(),
            username: None,
            password: None,
        };

        let attempt = adapter.connect(&config).expect("real connection attempt");
        assert_eq!(attempt.stage, RealConnectionStage::ProxyNegotiation);
        assert_eq!(attempt.plan.proxy.label(), "http-connect");
        assert_eq!(attempt.history.len(), 2);
        assert!(attempt.history[1].note.contains("Proxy negotiation"));
    }
}
