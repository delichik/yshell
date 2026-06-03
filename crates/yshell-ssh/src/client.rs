//! SSH client connection adapters.

use std::time::Duration;

use crate::auth::AuthMethod;
use crate::channel::ShellSession;
use crate::error::{SshError, SshErrorKind, SshResult};
use crate::fake::FakeSshAdapter;
use crate::forwarding::TunnelConfig;
use crate::host_key::HostKeyPolicy;
use crate::proxy::ProxyConfig;
use crate::pty::PtyConfig;
use crate::real::RealSshAdapter;

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

pub trait ShellAdapter {
    type Shell: ShellSession + 'static;

    fn open_shell(&self, config: &SshConnectionConfig) -> SshResult<Self::Shell>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportBackend {
    Fake,
    Real,
}

impl TransportBackend {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fake => "fake",
            Self::Real => "real",
        }
    }
}

#[derive(Debug, Default)]
pub struct SshClient<A = FakeSshAdapter> {
    adapter: A,
}

impl SshClient<FakeSshAdapter> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_fake_backend() -> Self {
        Self::new()
    }
}

impl SshClient<RealSshAdapter> {
    pub fn with_real_backend() -> Self {
        Self {
            adapter: RealSshAdapter,
        }
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

#[derive(Debug, Default)]
pub struct ShellClient<A = FakeSshAdapter> {
    adapter: A,
}

impl ShellClient<FakeSshAdapter> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_fake_backend() -> Self {
        Self::new()
    }
}

impl ShellClient<RealSshAdapter> {
    pub fn with_real_backend() -> Self {
        Self {
            adapter: RealSshAdapter,
        }
    }
}

impl<A> ShellClient<A>
where
    A: ShellAdapter,
{
    pub fn with_adapter(adapter: A) -> Self {
        Self { adapter }
    }

    pub fn open_shell(&self, config: &SshConnectionConfig) -> SshResult<A::Shell> {
        self.adapter.open_shell(config)
    }

    pub fn open_shell_boxed(&self, config: &SshConnectionConfig) -> SshResult<Box<dyn ShellSession>>
    where
        A::Shell: 'static,
    {
        self.adapter
            .open_shell(config)
            .map(|session| Box::new(session) as Box<dyn ShellSession>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
