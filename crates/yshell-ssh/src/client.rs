//! SSH client connection adapters.

use std::time::Duration;

use crate::auth::AuthMethod;
use crate::error::{SshError, SshErrorKind, SshResult};
use crate::forwarding::TunnelConfig;
use crate::host_key::HostKeyPolicy;
use crate::proxy::{ProxyConfig, ProxyState};
use crate::pty::PtyConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConnectionConfig {
    pub host: String,
    pub port: u16,
    pub auth: AuthMethod,
    pub host_key_policy: HostKeyPolicy,
    pub proxy: ProxyConfig,
    pub pty: PtyConfig,
    pub tunnels: Vec<TunnelConfig>,
    pub connect_timeout: Duration,
}

impl SshConnectionConfig {
    pub fn new(host: impl Into<String>, port: u16, auth: AuthMethod) -> Self {
        Self {
            host: host.into(),
            port,
            auth,
            host_key_policy: HostKeyPolicy::Strict,
            proxy: ProxyConfig::None,
            pty: PtyConfig::default(),
            tunnels: Vec::new(),
            connect_timeout: Duration::from_secs(30),
        }
    }

    pub fn username(&self) -> &str {
        self.auth.username()
    }

    pub fn validate(&self) -> SshResult<()> {
        if self.host.trim().is_empty() {
            return Err(SshError::configuration("ssh host must not be empty"));
        }
        if self.port == 0 {
            return Err(SshError::configuration(
                "ssh port must be greater than zero",
            ));
        }
        if self.username().trim().is_empty() {
            return Err(SshError::configuration("ssh username must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: i32,
}

pub trait SshAdapter {
    type Session;

    fn connect(&self, config: &SshConnectionConfig) -> SshResult<Self::Session>;
    fn exec(&self, session: &mut Self::Session, command: &str) -> SshResult<ExecOutput>;
    fn disconnect(&self, session: Self::Session) -> SshResult<()>;
}

#[derive(Debug, Default)]
pub struct SshClient<A = FakeSshAdapter> {
    adapter: A,
}

impl SshClient<FakeSshAdapter> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<A> SshClient<A>
where
    A: SshAdapter,
{
    pub fn with_adapter(adapter: A) -> Self {
        Self { adapter }
    }

    pub fn connect(&self, config: &SshConnectionConfig) -> SshResult<A::Session> {
        self.adapter.connect(config)
    }

    pub fn exec(&self, session: &mut A::Session, command: &str) -> SshResult<ExecOutput> {
        self.adapter.exec(session, command)
    }

    pub fn disconnect(&self, session: A::Session) -> SshResult<()> {
        self.adapter.disconnect(session)
    }
}

#[derive(Debug, Clone, Default)]
pub struct FakeSshAdapter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeSshSession {
    pub host: String,
    pub username: String,
    pub proxy_state: ProxyState,
    pub executed_commands: Vec<String>,
    connected: bool,
}

impl SshAdapter for FakeSshAdapter {
    type Session = FakeSshSession;

    fn connect(&self, config: &SshConnectionConfig) -> SshResult<Self::Session> {
        config.validate()?;
        Ok(FakeSshSession {
            host: config.host.clone(),
            username: config.username().to_owned(),
            proxy_state: match config.proxy.address() {
                Some(address) => ProxyState::Connected {
                    address: address.to_owned(),
                },
                None => ProxyState::Disabled,
            },
            executed_commands: Vec::new(),
            connected: true,
        })
    }

    fn exec(&self, session: &mut Self::Session, command: &str) -> SshResult<ExecOutput> {
        if !session.connected {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "cannot execute command on disconnected session",
            ));
        }
        session.executed_commands.push(command.to_owned());
        Ok(ExecOutput {
            stdout: format!("fake ssh executed: {command}\n").into_bytes(),
            stderr: Vec::new(),
            exit_status: 0,
        })
    }

    fn disconnect(&self, mut session: Self::Session) -> SshResult<()> {
        session.connected = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_adapter_connects_and_executes_deterministically() {
        let client = SshClient::new();
        let config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );
        let mut session = client.connect(&config).expect("connect");
        let output = client.exec(&mut session, "uptime").expect("exec");
        assert_eq!(session.host, "example.test");
        assert_eq!(output.stdout, b"fake ssh executed: uptime\n");
    }

    #[test]
    fn validates_required_connection_fields() {
        let config = SshConnectionConfig::new(
            "",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );
        assert_eq!(
            config.validate().unwrap_err().kind,
            SshErrorKind::Configuration
        );
    }
}
