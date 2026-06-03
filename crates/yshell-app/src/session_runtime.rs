//! Runtime-owned session state for one live or pending shell session.

use yshell_config::{QuickConnectTarget, SessionProfile};
use yshell_core::{SessionId, SessionState, TabId};
use yshell_ssh::{AuthMethod, ShellSession, SshConnectionConfig};
use yshell_terminal::{TerminalGrid, TerminalParser};

#[derive(Debug)]
pub enum SessionSource {
    QuickConnect,
    SavedSession { profile_id: String },
    Draft,
}

#[derive(Debug)]
pub struct SessionRuntime {
    session_id: SessionId,
    tab_id: TabId,
    pub display_name: String,
    pub source: SessionSource,
    pub ssh_config: SshConnectionConfig,
    pub terminal_parser: TerminalParser,
    pub terminal_grid: TerminalGrid,
    pub shell_session: Option<Box<dyn ShellSession>>,
    pub state: SessionState,
}

impl SessionRuntime {
    pub fn from_quick_connect(target: QuickConnectTarget, ordinal: usize) -> Self {
        let username = target.username.clone().unwrap_or_else(|| "user".to_owned());
        let display_name = format!(
            "{}@{}:{}",
            target.username.as_deref().unwrap_or("<default>"),
            target.host,
            target.port
        );
        let ssh_config = SshConnectionConfig::new(
            target.host.clone(),
            target.port,
            AuthMethod::Agent { username },
        );
        let mut runtime = Self {
            session_id: SessionId::new(format!("quick-connect-{ordinal}")),
            tab_id: TabId::new(format!("tab-quick-connect-{ordinal}")),
            display_name,
            source: SessionSource::QuickConnect,
            ssh_config,
            terminal_parser: TerminalParser::new(),
            terminal_grid: TerminalGrid::new(120, 32),
            shell_session: None,
            state: SessionState::Connecting,
        };
        runtime.append_status_line("Quick Connect accepted. Preparing runtime session.");
        runtime
    }

    pub fn from_profile(profile: &SessionProfile, ordinal: usize) -> Self {
        let username = profile
            .username
            .clone()
            .unwrap_or_else(|| "user".to_owned());
        let ssh_config = SshConnectionConfig::new(
            profile.host.clone(),
            profile.port,
            AuthMethod::Agent { username },
        );
        let mut runtime = Self {
            session_id: SessionId::new(profile.id.clone()),
            tab_id: TabId::new(format!("tab-saved-{ordinal}")),
            display_name: profile.name.clone(),
            source: SessionSource::SavedSession {
                profile_id: profile.id.clone(),
            },
            ssh_config,
            terminal_parser: TerminalParser::new(),
            terminal_grid: TerminalGrid::new(120, 32),
            shell_session: None,
            state: SessionState::Idle,
        };
        runtime.append_status_line("Saved session loaded into runtime inventory.");
        runtime
    }

    pub fn draft(ordinal: usize) -> Self {
        let mut runtime = Self {
            session_id: SessionId::new(format!("draft-{ordinal}")),
            tab_id: TabId::new(format!("tab-draft-{ordinal}")),
            display_name: "New Session".to_owned(),
            source: SessionSource::Draft,
            ssh_config: SshConnectionConfig::new(
                "example.com",
                22,
                AuthMethod::Agent {
                    username: "user".to_owned(),
                },
            ),
            terminal_parser: TerminalParser::new(),
            terminal_grid: TerminalGrid::new(120, 32),
            shell_session: None,
            state: SessionState::Idle,
        };
        runtime.append_status_line("Draft session created. Waiting for connection details.");
        runtime
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn tab_id(&self) -> &TabId {
        &self.tab_id
    }

    pub fn host_label(&self) -> String {
        format!("{}:{}", self.ssh_config.host, self.ssh_config.port)
    }

    pub fn username_label(&self) -> &str {
        self.ssh_config.username()
    }

    pub fn state_label(&self) -> &'static str {
        match self.state {
            SessionState::Idle => "idle",
            SessionState::Connecting => "connecting",
            SessionState::Connected => "connected",
            SessionState::Disconnected => "disconnected",
            SessionState::Failed => "failed",
        }
    }

    pub fn set_state(&mut self, state: SessionState) {
        self.state = state;
    }

    pub fn append_status_line(&mut self, message: &str) {
        let mut line = String::from(message);
        line.push('\n');
        self.terminal_parser
            .advance(&mut self.terminal_grid, line.as_bytes());
    }

    pub fn visible_text(&self) -> String {
        let lines = self.terminal_grid.visible_lines();
        if lines.iter().all(|line| line.is_empty()) {
            String::new()
        } else {
            lines.join("\n")
        }
    }

    pub fn sidebar_summary(&self) -> String {
        let source = match &self.source {
            SessionSource::QuickConnect => "quick-connect",
            SessionSource::SavedSession { .. } => "saved",
            SessionSource::Draft => "draft",
        };
        format!(
            "{} [{}] {}",
            self.display_name,
            self.state_label(),
            source
        )
    }

    pub fn attach_shell_session(&mut self, shell_session: Box<dyn ShellSession>) {
        self.shell_session = Some(shell_session);
    }

    pub fn poll_shell_output(&mut self) -> Result<Vec<Vec<u8>>, yshell_ssh::SshError> {
        let Some(shell_session) = self.shell_session.as_mut() else {
            return Ok(Vec::new());
        };
        let mut chunks = Vec::new();
        loop {
            let bytes = shell_session.poll_output()?;
            if bytes.is_empty() {
                break;
            }
            self.terminal_parser.advance(&mut self.terminal_grid, &bytes);
            chunks.push(bytes);
        }
        Ok(chunks)
    }

    pub fn write_terminal_input(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<Vec<u8>>, yshell_ssh::SshError> {
        let Some(shell_session) = self.shell_session.as_mut() else {
            return Ok(Vec::new());
        };
        shell_session.write_input(bytes)?;
        self.poll_shell_output()
    }

    pub fn resize_shell_pty(
        &mut self,
        columns: u16,
        rows: u16,
    ) -> Result<Vec<Vec<u8>>, yshell_ssh::SshError> {
        let Some(shell_session) = self.shell_session.as_mut() else {
            return Ok(Vec::new());
        };
        let size = yshell_ssh::PtySize {
            columns,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        };
        shell_session.resize_pty(size)?;
        self.poll_shell_output()
    }

    pub fn disconnect_shell(&mut self) -> Result<(), yshell_ssh::SshError> {
        let Some(shell_session) = self.shell_session.as_mut() else {
            return Ok(());
        };
        shell_session.disconnect()
    }

    pub fn to_session_profile(&self, profile_id: String) -> SessionProfile {
        let mut profile = SessionProfile::new(
            profile_id,
            self.display_name.clone(),
            self.ssh_config.host.clone(),
        );
        profile.port = self.ssh_config.port;
        profile.username = Some(self.ssh_config.username().to_owned());
        profile
    }
}
