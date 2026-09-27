//! SSH client connection adapters.

use std::time::Duration;

use crate::auth::{AuthAttempt, AuthMethod, AuthMethods, KeyboardInteractivePrompter};
use crate::channel::ShellSession;
use crate::error::{SshError, SshResult};
use crate::fake::FakeSshAdapter;
use crate::forwarding::TunnelConfig;
use crate::host_key::{HostKeyPolicy, KnownHosts};
use crate::proxy::ProxyConfig;
use crate::pty::PtyConfig;
use crate::real::RealSshAdapter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConnectionConfig {
    pub host: String,
    pub port: u16,
    pub auth: AuthMethod,
    pub host_key_policy: HostKeyPolicy,
    pub known_hosts: KnownHosts,
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
            known_hosts: KnownHosts::new(),
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

    /// Server advertised authentication methods learned for this session
    /// (`None` = the server did not return a list).
    fn auth_methods(&self, session: &Self::Session) -> SshResult<Option<AuthMethods>>;

    /// Perform one authentication attempt.
    ///
    /// Failures are classified via [`SshError::auth_problem`] and carry the
    /// server method list via [`SshError::auth_methods`], so the caller can
    /// retry with another method inside one auth window.
    fn authenticate_with(&self, session: &mut Self::Session, attempt: AuthAttempt)
        -> SshResult<()>;

    /// Run keyboard-interactive authentication driven by an interactive
    /// prompter; the prompter is invoked once per server challenge round and
    /// may cancel.
    fn authenticate_keyboard_interactive(
        &self,
        session: &mut Self::Session,
        username: &str,
        prompter: &mut dyn KeyboardInteractivePrompter,
    ) -> SshResult<()>;
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
            Self::Real => "native-ssh",
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

    /// Server advertised authentication methods learned for this session.
    pub fn auth_methods(&self, session: &A::Session) -> SshResult<Option<AuthMethods>> {
        self.adapter.auth_methods(session)
    }

    /// Perform one authentication attempt; failures carry `auth_problem` and
    /// `auth_methods` for in-window retries.
    pub fn authenticate_with(
        &self,
        session: &mut A::Session,
        attempt: AuthAttempt,
    ) -> SshResult<()> {
        self.adapter.authenticate_with(session, attempt)
    }

    /// Keyboard-interactive authentication with an interactive prompter
    /// (multi-round, cancellable).
    pub fn authenticate_keyboard_interactive(
        &self,
        session: &mut A::Session,
        username: &str,
        prompter: &mut dyn KeyboardInteractivePrompter,
    ) -> SshResult<()> {
        self.adapter
            .authenticate_keyboard_interactive(session, username, prompter)
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
    use crate::error::SshErrorKind;

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
