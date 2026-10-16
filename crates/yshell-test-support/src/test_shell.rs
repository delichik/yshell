//! In-memory interactive shell for offline unit tests.
//!
//! Not a product transport backend — only used by app/runtime tests that need a
//! connected [`ShellSession`] without opening a TCP/SSH connection.

use std::collections::VecDeque;
use std::fmt;

use yshell_ssh::{
    channel::ShellSession, error::SshError, error::SshErrorKind, error::SshResult, pty::PtySize,
    proxy::ProxyState, AuthMethod, ShellAdapter, SshConnectionConfig,
};

/// Deterministic shell used by offline app unit tests.
#[derive(Clone)]
pub struct TestShellSession {
    pub host: String,
    pub username: String,
    pub proxy_state: ProxyState,
    pub pty_size: PtySize,
    pub written_inputs: Vec<Vec<u8>>,
    connected: bool,
    pending_output: VecDeque<Vec<u8>>,
}

impl fmt::Debug for TestShellSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestShellSession")
            .field("host", &self.host)
            .field("username", &self.username)
            .field("connected", &self.connected)
            .field("pty_size", &self.pty_size)
            .finish()
    }
}

impl TestShellSession {
    /// Build a connected test shell from an SSH connection config.
    pub fn open(config: &SshConnectionConfig) -> SshResult<Self> {
        config.validate()?;
        let mut pending_output = VecDeque::new();
        pending_output.push_back(
            format!(
                "Connecting to {}:{} as {}\n",
                config.host,
                config.port,
                config.username()
            )
            .into_bytes(),
        );
        pending_output.push_back(
            format!(
                "Test shell established with TERM={} and size={}x{}\n",
                config.pty.term, config.pty.size.columns, config.pty.size.rows
            )
            .into_bytes(),
        );
        pending_output.push_back(b"$ ".to_vec());
        Ok(Self {
            host: config.host.clone(),
            username: config.username().to_owned(),
            proxy_state: match config.proxy.address() {
                Some(address) => ProxyState::Connected {
                    address: address.to_owned(),
                },
                None => ProxyState::Disabled,
            },
            pty_size: config.pty.size,
            written_inputs: Vec::new(),
            connected: true,
            pending_output,
        })
    }

    /// Convenience: open a boxed shell for attaching to a session runtime.
    pub fn open_boxed(config: &SshConnectionConfig) -> SshResult<Box<dyn ShellSession>> {
        Self::open(config).map(|session| Box::new(session) as Box<dyn ShellSession>)
    }
}

/// [`ShellAdapter`] that opens [`TestShellSession`] instances.
#[derive(Debug, Default, Clone, Copy)]
pub struct TestShellAdapter;

impl ShellAdapter for TestShellAdapter {
    type Shell = TestShellSession;

    fn open_shell(&self, config: &SshConnectionConfig) -> SshResult<Self::Shell> {
        TestShellSession::open(config)
    }
}

impl ShellSession for TestShellSession {
    fn is_connected(&self) -> bool {
        self.connected
    }

    fn poll_output(&mut self) -> SshResult<Vec<u8>> {
        if !self.connected {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "cannot poll output from disconnected shell session",
            ));
        }
        Ok(self.pending_output.pop_front().unwrap_or_default())
    }

    fn write_input(&mut self, bytes: &[u8]) -> SshResult<()> {
        if !self.connected {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "cannot write input into disconnected shell session",
            ));
        }
        self.written_inputs.push(bytes.to_vec());
        let printable = String::from_utf8_lossy(bytes).replace('\r', "\\r");
        self.pending_output
            .push_back(format!("test-shell received input: {printable}\n$ ").into_bytes());
        Ok(())
    }

    fn resize_pty(&mut self, size: PtySize) -> SshResult<()> {
        if !self.connected {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "cannot resize disconnected shell session",
            ));
        }
        self.pty_size = size;
        self.pending_output.push_back(
            format!("test-shell resized to {}x{}\n$ ", size.columns, size.rows).into_bytes(),
        );
        Ok(())
    }

    fn disconnect(&mut self) -> SshResult<()> {
        self.connected = false;
        Ok(())
    }
}

/// Helper config for tests that only need a disposable shell.
pub fn test_shell_config(host: &str, username: &str) -> SshConnectionConfig {
    SshConnectionConfig::new(
        host,
        22,
        AuthMethod::Agent {
            username: username.to_owned(),
        },
    )
}
