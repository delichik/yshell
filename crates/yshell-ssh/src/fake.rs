//! Fake SSH transport used to exercise the app/runtime pipeline before real transport exists.

use std::collections::VecDeque;

use crate::{
    channel::ShellSession,
    client::{ExecOutput, ShellAdapter, SshAdapter, SshConnectionConfig},
    error::{SshError, SshErrorKind, SshResult},
    proxy::ProxyState,
    pty::PtySize,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeShellSession {
    pub host: String,
    pub username: String,
    pub proxy_state: ProxyState,
    pub pty_size: PtySize,
    pub written_inputs: Vec<Vec<u8>>,
    connected: bool,
    pending_output: VecDeque<Vec<u8>>,
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

impl ShellAdapter for FakeSshAdapter {
    type Shell = FakeShellSession;

    fn open_shell(&self, config: &SshConnectionConfig) -> SshResult<Self::Shell> {
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
                "Fake shell established with TERM={} and size={}x{}\n",
                config.pty.term,
                config.pty.size.columns,
                config.pty.size.rows
            )
            .into_bytes(),
        );
        pending_output.push_back(b"$ ".to_vec());
        Ok(FakeShellSession {
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
}

impl ShellSession for FakeShellSession {
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
            .push_back(format!("fake-shell received input: {printable}\n$ ").into_bytes());
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
            format!("fake-shell resized to {}x{}\n$ ", size.columns, size.rows).into_bytes(),
        );
        Ok(())
    }

    fn disconnect(&mut self) -> SshResult<()> {
        self.connected = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{AuthMethod, ShellClient, ShellSession, SshClient, SshConnectionConfig};

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
    fn fake_shell_session_streams_output_and_accepts_input() {
        let client = ShellClient::new();
        let config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );
        let mut shell = client.open_shell(&config).expect("open shell");

        assert!(String::from_utf8_lossy(&shell.poll_output().expect("banner")).contains("Connecting"));
        assert!(String::from_utf8_lossy(&shell.poll_output().expect("term")).contains("Fake shell established"));

        shell.write_input(b"pwd\n").expect("write input");
        assert!(String::from_utf8_lossy(&shell.poll_output().expect("prompt")).contains('$'));
        assert!(String::from_utf8_lossy(&shell.poll_output().expect("echo")).contains("fake-shell received input"));
    }
}
