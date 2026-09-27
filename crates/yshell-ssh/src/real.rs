//! Real SSH transport integration skeleton.
//!
//! This module intentionally keeps the "real" path explicit even before the
//! actual network transport is wired, so the app/runtime can target a stable
//! replacement boundary instead of assuming the fake adapter is the only path.

use std::collections::VecDeque;
use std::io;
use std::io::Read;
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use ssh2::{Channel, KeyboardInteractivePrompt, Prompt, Session};

use crate::{
    channel::ShellSession,
    client::{ExecOutput, ShellAdapter, SshAdapter, SshConnectionConfig},
    error::{SshError, SshErrorKind, SshResult},
    forwarding::TunnelConfig,
    host_key::{HostKeyPolicy, KnownHosts},
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
    pub auth: AuthMethod,
    pub auth_strategy: RealAuthStrategy,
    pub host_key_policy: HostKeyPolicy,
    pub known_hosts: KnownHosts,
    pub proxy: ProxyConfig,
    pub tunnels: Vec<TunnelConfig>,
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
pub enum RealTransportAction {
    NegotiateProxy { protocol: &'static str, address: String },
    OpenTcpSocket { address: String },
    PerformSshHandshake { server: String },
    VerifyHostKey { server: String, policy: HostKeyPolicy },
    Authenticate { username: String, strategy: RealAuthStrategy },
    RequestPty { term: String, columns: u16, rows: u16 },
    OpenShell,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealTransportActionOutcome {
    Pending,
    Blocked,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealTransportActionRecord {
    pub action: RealTransportAction,
    pub outcome: RealTransportActionOutcome,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealTransportExecutionReport {
    pub stage: RealConnectionStage,
    pub action: RealTransportAction,
    pub record: Option<RealTransportActionRecord>,
    pub summary: String,
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
    pub action_log: Vec<RealTransportActionRecord>,
}

#[derive(Debug)]
pub struct RealShellSessionPlaceholder {
    pub attempt: RealConnectionAttempt,
    pending_output: VecDeque<Vec<u8>>,
    transport: RealShellTransport,
}

#[derive(Debug)]
enum RealShellTransport {
    Scaffold,
    Native(RealNativeShellSession),
}

struct RealNativeShellSession {
    session: Session,
    channel: Channel,
    closed: bool,
    disconnect_reported: bool,
}

impl RealSshAdapter {
    pub fn plan_connection(&self, config: &SshConnectionConfig) -> SshResult<RealConnectionPlan> {
        config.validate()?;
        Ok(RealConnectionPlan {
            host: config.host.clone(),
            port: config.port,
            username: config.username().to_owned(),
            auth: config.auth.clone(),
            auth_strategy: RealAuthStrategy::from_auth_method(&config.auth),
            host_key_policy: config.host_key_policy.clone(),
            known_hosts: config.known_hosts.clone(),
            proxy: config.proxy.clone(),
            tunnels: config.tunnels.clone(),
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
            transport: RealShellTransport::Scaffold,
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

#[derive(Debug, Clone)]
struct SecretOnlyKeyboardInteractivePrompt {
    secret: String,
    prompt_log: Vec<String>,
}

impl SecretOnlyKeyboardInteractivePrompt {
    fn new(secret: String) -> Self {
        Self {
            secret,
            prompt_log: Vec::new(),
        }
    }

    fn describe_prompts(&self) -> String {
        if self.prompt_log.is_empty() {
            "none".to_owned()
        } else {
            self.prompt_log.join(" | ")
        }
    }
}

impl KeyboardInteractivePrompt for SecretOnlyKeyboardInteractivePrompt {
    fn prompt<'a>(&mut self, username: &str, instructions: &str, prompts: &[Prompt<'a>]) -> Vec<String> {
        self.prompt_log.clear();
        if !instructions.trim().is_empty() {
            self.prompt_log
                .push(format!("instructions={}", instructions.trim()));
        }
        self.prompt_log.extend(prompts.iter().map(|prompt| {
            format!("user={username} prompt=`{}` echo={}", prompt.text.trim(), prompt.echo)
        }));
        prompts
            .iter()
            .enumerate()
            .map(|(index, prompt)| {
                if index == 0 || (!prompt.echo && looks_like_secret_prompt(&prompt.text)) {
                    self.secret.clone()
                } else {
                    String::new()
                }
            })
            .collect()
    }
}

fn looks_like_secret_prompt(text: &str) -> bool {
    let normalized = text.trim().to_ascii_lowercase();
    normalized.contains("password")
        || normalized.contains("passcode")
        || normalized.contains("otp")
        || normalized.contains("token")
        || normalized.contains("verification code")
}

impl SshAdapter for RealSshAdapter {
    type Session = RealConnectionAttempt;

    fn connect(&self, config: &SshConnectionConfig) -> SshResult<Self::Session> {
        RealConnectionAttempt::begin(self.plan_connection(config)?)
    }

    fn exec(&self, session: &mut Self::Session, command: &str) -> SshResult<ExecOutput> {
        if command.trim().is_empty() {
            return Err(SshError::new(
                SshErrorKind::Configuration,
                "exec command must not be empty",
            ));
        }
        let native_session = connect_authenticated_native_session(session)?;
        execute_native_exec(native_session, session, command)
    }

    fn disconnect(&self, mut session: Self::Session) -> SshResult<()> {
        session.mark_disconnected("Connection attempt was closed before live transport existed.");
        Ok(())
    }
}

impl ShellAdapter for RealSshAdapter {
    type Shell = RealShellSessionPlaceholder;

    fn open_shell(&self, config: &SshConnectionConfig) -> SshResult<Self::Shell> {
        let mut shell = self.scaffold_shell_session(config)?;
        shell.begin_connection_attempt()?;
        Ok(shell)
    }
}

impl ShellSession for RealShellSessionPlaceholder {
    fn is_connected(&self) -> bool {
        match &self.transport {
            RealShellTransport::Scaffold => self.attempt.stage == RealConnectionStage::Connected,
            RealShellTransport::Native(session) => session.is_connected(),
        }
    }

    fn poll_output(&mut self) -> SshResult<Vec<u8>> {
        self.refresh_transport_state();
        match &mut self.transport {
            RealShellTransport::Scaffold => Ok(self.pending_output.pop_front().unwrap_or_default()),
            RealShellTransport::Native(session) => {
                session.drain_into(&mut self.pending_output)?;
                Ok(self.pending_output.pop_front().unwrap_or_default())
            }
        }
    }

    fn write_input(&mut self, bytes: &[u8]) -> SshResult<()> {
        match &mut self.transport {
            RealShellTransport::Scaffold => {
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
            RealShellTransport::Native(session) => session.write_input(bytes),
        }
    }

    fn resize_pty(&mut self, size: PtySize) -> SshResult<()> {
        self.attempt.plan.pty.size = size;
        match &mut self.transport {
            RealShellTransport::Scaffold => self.pending_output.push_back(
                format!(
                    "real-shell scaffold updated planned PTY to {}x{} while waiting at stage `{}`.\n",
                    size.columns,
                    size.rows,
                    self.attempt.stage.label()
                )
                .into_bytes(),
            ),
            RealShellTransport::Native(session) => session.resize_pty(size)?,
        }
        Ok(())
    }

    fn disconnect(&mut self) -> SshResult<()> {
        match &mut self.transport {
            RealShellTransport::Scaffold => {
                self.attempt
                    .mark_disconnected("Shell scaffold was closed before any live transport existed.");
                self.pending_output.push_back(
                    b"real-shell scaffold marked the session as disconnected before any live transport existed.\n"
                        .to_vec(),
                );
                Ok(())
            }
            RealShellTransport::Native(session) => {
                session.disconnect()?;
                self.attempt
                    .mark_disconnected("Native SSH shell session was explicitly disconnected by the runtime.");
                self.pending_output
                    .push_back(b"native SSH shell session was disconnected by the runtime.\n".to_vec());
                Ok(())
            }
        }
    }
}

impl RealShellSessionPlaceholder {
    fn begin_connection_attempt(&mut self) -> SshResult<()> {
        for line in self.attempt.begin_transcript_lines() {
            let mut bytes = line.into_bytes();
            bytes.push(b'\n');
            self.pending_output.push_back(bytes);
        }
        let report = self.attempt.execute_current_action()?;
        self.pending_output.push_back(format!("Transport action: {}\n", report.summary()).into_bytes());
        if self.attempt.stage != report.stage {
            self.pending_output.push_back(
                format!(
                    "Real transport is now waiting at stage `{}`. {}\n",
                    self.attempt.stage.label(),
                    self.attempt.stage_note(self.attempt.stage)
                )
                .into_bytes(),
            );
        }
        self.try_start_live_shell()?;
        Ok(())
    }

    fn try_start_live_shell(&mut self) -> SshResult<()> {
        if self.attempt.stage != RealConnectionStage::SshHandshake {
            return Ok(());
        }
        if !matches!(self.attempt.plan.proxy, ProxyConfig::None) {
            self.pending_output.push_back(
                b"Live shell handoff is not available while proxy negotiation is still scaffold-only.\n"
                    .to_vec(),
            );
            return Ok(());
        }
        if !self.attempt.plan.tunnels.is_empty() {
            self.pending_output.push_back(
                b"Native SSH shell startup does not yet provision tunnel forwards. Remove tunnel config or stay on the fake backend for now.\n"
                    .to_vec(),
            );
            return Ok(());
        }

        self.pending_output.push_back(
            b"Opening a native SSH session inside yshell-ssh.\n".to_vec(),
        );
        let session = RealNativeShellSession::connect(&mut self.attempt)?;
        self.pending_output.push_back(
            format!(
                "Real transport is now waiting at stage `{}`. {}\n",
                self.attempt.stage.label(),
                self.attempt.stage_note(self.attempt.stage)
            )
            .into_bytes(),
        );
        self.transport = RealShellTransport::Native(session);
        Ok(())
    }

    fn refresh_transport_state(&mut self) {
        let native_disconnected = match &mut self.transport {
            RealShellTransport::Native(session) => {
                if session.is_connected() {
                    None
                } else {
                    session.take_disconnect_notice()
                }
            }
            _ => None,
        };
        if let Some(notice) = native_disconnected {
            self.pending_output.push_back(notice.clone().into_bytes());
            self.attempt
                .mark_disconnected("Native SSH shell session closed.");
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
            Self::Socks4a { .. } => "socks4a",
            Self::Socks5 { .. } => "socks5",
            Self::HttpConnect { .. } => "http-connect",
        }
    }
}

impl RealTransportAction {
    pub fn summary(&self) -> String {
        match self {
            Self::NegotiateProxy { protocol, address } => {
                format!("proxy:{protocol}@{address}")
            }
            Self::OpenTcpSocket { address } => format!("tcp-connect:{address}"),
            Self::PerformSshHandshake { server } => format!("ssh-handshake:{server}"),
            Self::VerifyHostKey { server, policy } => {
                format!("host-key:{}@{}", policy.label(), server)
            }
            Self::Authenticate { username, strategy } => {
                format!("auth:{}@{}", strategy.label(), username)
            }
            Self::RequestPty {
                term,
                columns,
                rows,
            } => format!("pty:{}:{}x{}", term, columns, rows),
            Self::OpenShell => "shell-open".to_owned(),
            Self::None => "none".to_owned(),
        }
    }
}

impl RealTransportActionOutcome {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

impl RealTransportActionRecord {
    pub fn summary(&self) -> String {
        format!(
            "{}:{} ({})",
            self.outcome.label(),
            self.action.summary(),
            self.detail
        )
    }
}

impl RealTransportExecutionReport {
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

impl RealConnectionAttempt {
    pub fn prepared(plan: RealConnectionPlan) -> Self {
        let mut attempt = Self {
            plan,
            stage: RealConnectionStage::Prepared,
            history: Vec::new(),
            action_log: Vec::new(),
        };
        attempt.record_snapshot(
            RealConnectionStage::Prepared,
            "Connection plan captured before any network activity.",
        );
        attempt
    }

    pub fn begin(plan: RealConnectionPlan) -> SshResult<Self> {
        let mut attempt = Self::prepared(plan);
        let next_stage = attempt.next_pending_stage();
        attempt.advance_to(next_stage, attempt.stage_note(next_stage));
        let _ = attempt.execute_current_action()?;
        Ok(attempt)
    }

    pub fn begin_transcript_lines(&mut self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "Real SSH backend selected for {}@{}:{}",
                self.plan.username, self.plan.host, self.plan.port
            ),
            format!(
                "Connection plan: auth={} host-key={} proxy={} tunnels={} term={} size={}x{} timeout={}s",
                self.plan.auth_strategy.label(),
                self.plan.host_key_policy.label(),
                self.plan.proxy.label(),
                self.plan.tunnels.len(),
                self.plan.pty.term,
                self.plan.pty.size.columns,
                self.plan.pty.size.rows,
                self.plan.connect_timeout.as_secs()
            ),
        ];
        if self.stage == RealConnectionStage::Prepared {
            let next_stage = self.next_pending_stage();
            self.advance_to(next_stage, self.stage_note(next_stage));
        }
        lines.push(format!(
            "Real transport scaffold advanced to stage `{}`. {}",
            self.stage.label(),
            self.stage_note(self.stage)
        ));
        if let Some(record) = self.latest_action_record() {
            lines.push(format!("Current transport action: {}", record.summary()));
        }
        lines
    }

    pub fn advance_to(&mut self, stage: RealConnectionStage, note: impl Into<String>) {
        self.stage = stage;
        self.record_snapshot(stage, note);
    }

    pub fn mark_failed(&mut self, note: impl Into<String>) {
        self.advance_to(RealConnectionStage::Failed, note);
    }

    pub fn mark_disconnected(&mut self, note: impl Into<String>) {
        self.advance_to(RealConnectionStage::Disconnected, note);
    }

    pub fn current_summary(&self) -> String {
        let note = self
            .history
            .last()
            .map(|snapshot| snapshot.note.as_str())
            .unwrap_or("no stage note recorded");
        let action = self
            .latest_action_record()
            .map(RealTransportActionRecord::summary)
            .unwrap_or_else(|| self.next_action().summary());
        format!(
            "stage={} action={} note={note}",
            self.stage.label(),
            action
        )
    }

    pub fn history_transcript_lines(&self) -> Vec<String> {
        self.history
            .iter()
            .map(|snapshot| format!("{}: {}", snapshot.stage.label(), snapshot.note))
            .collect()
    }

    pub fn action_transcript_lines(&self) -> Vec<String> {
        self.action_log
            .iter()
            .map(RealTransportActionRecord::summary)
            .collect()
    }

    pub fn next_action(&self) -> RealTransportAction {
        match self.stage {
            RealConnectionStage::Prepared => RealTransportAction::None,
            RealConnectionStage::ProxyNegotiation => RealTransportAction::NegotiateProxy {
                protocol: self.plan.proxy.label(),
                address: self
                    .plan
                    .proxy
                    .address()
                    .unwrap_or("<missing-proxy-address>")
                    .to_owned(),
            },
            RealConnectionStage::TcpConnect => RealTransportAction::OpenTcpSocket {
                address: format!("{}:{}", self.plan.host, self.plan.port),
            },
            RealConnectionStage::SshHandshake => RealTransportAction::PerformSshHandshake {
                server: format!("{}:{}", self.plan.host, self.plan.port),
            },
            RealConnectionStage::HostKeyVerification => RealTransportAction::VerifyHostKey {
                server: format!("{}:{}", self.plan.host, self.plan.port),
                policy: self.plan.host_key_policy.clone(),
            },
            RealConnectionStage::Authentication => RealTransportAction::Authenticate {
                username: self.plan.username.clone(),
                strategy: self.plan.auth_strategy.clone(),
            },
            RealConnectionStage::PtyRequest => RealTransportAction::RequestPty {
                term: self.plan.pty.term.clone(),
                columns: self.plan.pty.size.columns,
                rows: self.plan.pty.size.rows,
            },
            RealConnectionStage::ShellOpen => RealTransportAction::OpenShell,
            RealConnectionStage::Connected
            | RealConnectionStage::Disconnected
            | RealConnectionStage::Failed => RealTransportAction::None,
        }
    }

    pub fn record_current_action_pending(&mut self, detail: impl Into<String>) {
        self.record_action(self.next_action(), RealTransportActionOutcome::Pending, detail);
    }

    pub fn record_current_action_blocked(&mut self, detail: impl Into<String>) {
        self.record_action(self.next_action(), RealTransportActionOutcome::Blocked, detail);
    }

    pub fn record_current_action_completed(&mut self, detail: impl Into<String>) {
        self.record_action(
            self.next_action(),
            RealTransportActionOutcome::Completed,
            detail,
        );
    }

    pub fn record_current_action_failed(&mut self, detail: impl Into<String>) {
        self.record_action(self.next_action(), RealTransportActionOutcome::Failed, detail);
    }

    pub fn latest_action_record(&self) -> Option<&RealTransportActionRecord> {
        self.action_log.last()
    }

    pub fn execute_current_action_scaffold(&mut self) -> RealTransportExecutionReport {
        let action = self.next_action();
        let detail = self.scaffold_action_detail(&action);
        let record = RealTransportActionRecord {
            action: action.clone(),
            outcome: RealTransportActionOutcome::Blocked,
            detail,
        };
        let stored_record = if record.action != RealTransportAction::None {
            self.action_log.push(record.clone());
            Some(record)
        } else {
            None
        };
        let summary = match &stored_record {
            Some(record) => format!(
                "scaffold-execution stage={} {}",
                self.stage.label(),
                record.summary()
            ),
            None => format!(
                "scaffold-execution stage={} none (no transport action scheduled)",
                self.stage.label()
            ),
        };
        RealTransportExecutionReport {
            stage: self.stage,
            action,
            record: stored_record,
            summary,
        }
    }

    pub fn execute_current_action(&mut self) -> SshResult<RealTransportExecutionReport> {
        match self.next_action() {
            RealTransportAction::OpenTcpSocket { .. } => self.execute_tcp_connect_action(),
            _ => Ok(self.execute_current_action_scaffold()),
        }
    }

    fn record_action(
        &mut self,
        action: RealTransportAction,
        outcome: RealTransportActionOutcome,
        detail: impl Into<String>,
    ) {
        if action == RealTransportAction::None {
            return;
        }
        self.action_log.push(RealTransportActionRecord {
            action,
            outcome,
            detail: detail.into(),
        });
    }

    fn record_snapshot(&mut self, stage: RealConnectionStage, note: impl Into<String>) {
        self.history.push(RealConnectionSnapshot {
            stage,
            note: note.into(),
        });
    }

    fn execute_tcp_connect_action(&mut self) -> SshResult<RealTransportExecutionReport> {
        let action = self.next_action();
        let report_stage = self.stage;
        let target = format!("{}:{}", self.plan.host, self.plan.port);
        let resolved = (self.plan.host.as_str(), self.plan.port)
            .to_socket_addrs()
            .map_err(|error| {
                self.record_action(
                    action.clone(),
                    RealTransportActionOutcome::Failed,
                    format!("DNS resolution failed for `{target}`: {error}"),
                );
                self.mark_failed(format!("DNS resolution failed before TCP connect: {error}"));
                SshError::new(
                    SshErrorKind::Dns,
                    format!("failed to resolve `{target}` before TCP connect: {error}"),
                )
            })?;
        let addresses = resolved.collect::<Vec<_>>();
        if addresses.is_empty() {
            self.record_action(
                action.clone(),
                RealTransportActionOutcome::Failed,
                format!("DNS resolution for `{target}` produced no socket addresses."),
            );
            self.mark_failed("DNS resolution produced no socket addresses.");
            return Err(SshError::new(
                SshErrorKind::Dns,
                format!("failed to resolve `{target}` before TCP connect: no socket addresses"),
            ));
        }

        let mut last_error = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, self.plan.connect_timeout) {
                Ok(stream) => {
                    let peer = stream.peer_addr().unwrap_or(address);
                    let local = stream.local_addr().ok();
                    let detail = match local {
                        Some(local) => format!(
                            "TCP socket opened from `{local}` to `{peer}`. The socket was released immediately because persistent SSH transport retention is not wired yet."
                        ),
                        None => format!(
                            "TCP socket opened to `{peer}`. The socket was released immediately because persistent SSH transport retention is not wired yet."
                        ),
                    };
                    let record = RealTransportActionRecord {
                        action: action.clone(),
                        outcome: RealTransportActionOutcome::Completed,
                        detail,
                    };
                    self.action_log.push(record.clone());
                    self.advance_to(
                        RealConnectionStage::SshHandshake,
                        format!(
                            "TCP connect reached `{peer}`. SSH handshake is the next live step once persistent transport plumbing is wired."
                        ),
                    );
                    return Ok(RealTransportExecutionReport {
                        stage: report_stage,
                        action,
                        record: Some(record.clone()),
                        summary: format!(
                            "live-execution stage={} {}",
                            report_stage.label(),
                            record.summary()
                        ),
                    });
                }
                Err(error) => {
                    last_error = Some((address, error));
                }
            }
        }

        let (address, error) = last_error.expect("tcp connect attempted at least one address");
        let kind = map_tcp_connect_error_kind(&error);
        let detail = format!("TCP connect to `{address}` failed: {error}");
        let record = RealTransportActionRecord {
            action: action.clone(),
            outcome: RealTransportActionOutcome::Failed,
            detail: detail.clone(),
        };
        self.action_log.push(record.clone());
        self.mark_failed(format!("TCP connect failed for `{address}`: {error}"));
        Err(SshError::new(kind, detail))
    }

    fn scaffold_action_detail(&self, action: &RealTransportAction) -> String {
        match action {
            RealTransportAction::NegotiateProxy { protocol, address } => format!(
                "Proxy negotiation scaffold for `{protocol}` via `{address}` is defined, but no live proxy implementation is wired yet."
            ),
            RealTransportAction::OpenTcpSocket { address } => format!(
                "TCP connect scaffold for `{address}` is defined, but no live socket implementation is wired yet."
            ),
            RealTransportAction::PerformSshHandshake { server } => format!(
                "SSH handshake scaffold for `{server}` is defined, but no live handshake implementation is wired yet."
            ),
            RealTransportAction::VerifyHostKey { server, policy } => format!(
                "Host-key verification scaffold for `{}` against `{server}` is defined, but no live verification implementation is wired yet.",
                policy.label()
            ),
            RealTransportAction::Authenticate { username, strategy } => format!(
                "Authentication scaffold for `{}` using `{username}` is defined, but no live authentication implementation is wired yet.",
                strategy.label()
            ),
            RealTransportAction::RequestPty {
                term,
                columns,
                rows,
            } => format!(
                "PTY request scaffold for `{term}` at {}x{} is defined, but no live PTY implementation is wired yet.",
                columns, rows
            ),
            RealTransportAction::OpenShell => {
                "Shell-open scaffold is defined, but no live shell-channel implementation is wired yet.".to_owned()
            }
            RealTransportAction::None => {
                "No further transport action is scheduled for the current scaffold stage.".to_owned()
            }
        }
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

fn map_tcp_connect_error_kind(error: &io::Error) -> SshErrorKind {
    match error.kind() {
        io::ErrorKind::TimedOut => SshErrorKind::Timeout,
        _ => SshErrorKind::TcpConnect,
    }
}

impl std::fmt::Debug for RealNativeShellSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RealNativeShellSession")
            .field("connected", &self.is_connected())
            .finish()
    }
}

impl RealNativeShellSession {
    fn connect(attempt: &mut RealConnectionAttempt) -> SshResult<Self> {
        let session = connect_authenticated_native_session(attempt)?;
        attempt.advance_to(
            RealConnectionStage::PtyRequest,
            "Authentication succeeded. Native PTY allocation is next.",
        );

        let mut channel = session.channel_session().map_err(|error| {
            native_stage_error(
                attempt,
                RealConnectionStage::ShellOpen,
                SshErrorKind::Channel,
                format!("failed to open native SSH session channel: {error}"),
            )
        })?;
        channel
            .request_pty(
                &attempt.plan.pty.term,
                None,
                Some((
                    attempt.plan.pty.size.columns as u32,
                    attempt.plan.pty.size.rows as u32,
                    attempt.plan.pty.size.pixel_width as u32,
                    attempt.plan.pty.size.pixel_height as u32,
                )),
            )
            .map_err(|error| {
                native_stage_error(
                    attempt,
                    RealConnectionStage::PtyRequest,
                    SshErrorKind::Channel,
                    format!("failed to request native PTY: {error}"),
                )
            })?;
        attempt.record_action(
            RealTransportAction::RequestPty {
                term: attempt.plan.pty.term.clone(),
                columns: attempt.plan.pty.size.columns,
                rows: attempt.plan.pty.size.rows,
            },
            RealTransportActionOutcome::Completed,
            format!(
                "native PTY allocated for {} at {}x{}.",
                attempt.plan.pty.term, attempt.plan.pty.size.columns, attempt.plan.pty.size.rows
            ),
        );
        attempt.advance_to(
            RealConnectionStage::ShellOpen,
            "Native PTY allocated. Opening interactive shell channel.",
        );

        channel.shell().map_err(|error| {
            native_stage_error(
                attempt,
                RealConnectionStage::ShellOpen,
                SshErrorKind::Channel,
                format!("failed to start native interactive shell: {error}"),
            )
        })?;
        session.set_blocking(false);
        attempt.record_action(
            RealTransportAction::OpenShell,
            RealTransportActionOutcome::Completed,
            "native interactive shell channel is open.".to_owned(),
        );
        attempt.advance_to(
            RealConnectionStage::Connected,
            "Interactive shell is live inside the embedded native SSH transport.",
        );
        Ok(Self {
            session,
            channel,
            closed: false,
            disconnect_reported: false,
        })
    }

    fn is_connected(&self) -> bool {
        !self.closed && !self.channel.eof()
    }

    fn drain_into(&mut self, queue: &mut VecDeque<Vec<u8>>) -> SshResult<()> {
        drain_channel_stream(&mut self.channel, queue)?;
        let mut stderr = self.channel.stderr();
        drain_read_stream(&mut stderr, queue)?;
        Ok(())
    }

    fn write_input(&mut self, bytes: &[u8]) -> SshResult<()> {
        self.session.set_blocking(true);
        let result = self
            .channel
            .write_all(bytes)
            .and_then(|_| self.channel.flush())
            .map_err(|error| {
                SshError::new(
                    SshErrorKind::Io,
                    format!("failed to write bytes into native SSH shell: {error}"),
                )
            });
        self.session.set_blocking(false);
        result
    }

    fn resize_pty(&mut self, size: PtySize) -> SshResult<()> {
        self.channel
            .request_pty_size(
                size.columns as u32,
                size.rows as u32,
                Some(size.pixel_width as u32),
                Some(size.pixel_height as u32),
            )
            .map_err(|error| {
                SshError::new(
                    SshErrorKind::Channel,
                    format!(
                        "failed to resize native SSH PTY to {}x{}: {error}",
                        size.columns, size.rows
                    ),
                )
            })
    }

    fn disconnect(&mut self) -> SshResult<()> {
        self.session.set_blocking(true);
        let close_result = self
            .channel
            .close()
            .map_err(|error| SshError::new(SshErrorKind::Channel, format!("failed to close native SSH channel: {error}")));
        if close_result.is_ok() {
            let _ = self.channel.wait_close();
        }
        self.closed = true;
        self.disconnect_reported = false;
        self.session.set_blocking(false);
        close_result
    }

    fn take_disconnect_notice(&mut self) -> Option<String> {
        if self.disconnect_reported {
            return None;
        }
        self.disconnect_reported = true;
        if self.closed {
            Some("native SSH shell session was closed.\n".to_owned())
        } else if let Ok(status) = self.channel.exit_status() {
            self.closed = true;
            Some(format!("native SSH shell session exited with status {status}.\n"))
        } else {
            self.closed = true;
            Some("native SSH shell session reached EOF.\n".to_owned())
        }
    }
}

fn connect_authenticated_native_session(attempt: &mut RealConnectionAttempt) -> SshResult<Session> {
    let stream = connect_native_stream(&attempt.plan)?;
    let mut session = Session::new().map_err(|error| {
        SshError::new(
            SshErrorKind::Io,
            format!("failed to allocate native SSH session: {error}"),
        )
    })?;
    session.set_tcp_stream(stream);
    session.set_timeout(session_timeout_ms(attempt.plan.connect_timeout));
    session
        .handshake()
        .map_err(|error| {
            native_stage_error(
                attempt,
                RealConnectionStage::SshHandshake,
                SshErrorKind::Handshake,
                format!("native SSH handshake failed: {error}"),
            )
        })?;
    attempt.record_action(
        RealTransportAction::PerformSshHandshake {
            server: format!("{}:{}", attempt.plan.host, attempt.plan.port),
        },
        RealTransportActionOutcome::Completed,
        "native SSH handshake completed through the embedded ssh2 transport.".to_owned(),
    );
    attempt.advance_to(
        RealConnectionStage::HostKeyVerification,
        "Native SSH handshake completed. Host-key verification is next.",
    );

    verify_native_host_key(&session, attempt)?;
    attempt.advance_to(
        RealConnectionStage::Authentication,
        "Host key accepted. Native SSH authentication is next.",
    );

    authenticate_native_session(&session, attempt)?;
    Ok(session)
}

fn execute_native_exec(
    session: Session,
    attempt: &mut RealConnectionAttempt,
    command: &str,
) -> SshResult<ExecOutput> {
    attempt.advance_to(
        RealConnectionStage::Connected,
        format!("Running native exec command `{command}` through the embedded SSH transport."),
    );
    let mut channel = session.channel_session().map_err(|error| {
        native_stage_error(
            attempt,
            RealConnectionStage::Connected,
            SshErrorKind::Channel,
            format!("failed to open native exec channel: {error}"),
        )
    })?;
    channel.exec(command).map_err(|error| {
        native_stage_error(
            attempt,
            RealConnectionStage::Connected,
            SshErrorKind::Channel,
            format!("failed to execute native command `{command}`: {error}"),
        )
    })?;

    let mut stdout = Vec::new();
    channel.read_to_end(&mut stdout).map_err(|error| {
        native_stage_error(
            attempt,
            RealConnectionStage::Connected,
            SshErrorKind::Io,
            format!("failed to read stdout for native exec `{command}`: {error}"),
        )
    })?;

    let mut stderr = Vec::new();
    channel.stderr().read_to_end(&mut stderr).map_err(|error| {
        native_stage_error(
            attempt,
            RealConnectionStage::Connected,
            SshErrorKind::Io,
            format!("failed to read stderr for native exec `{command}`: {error}"),
        )
    })?;

    channel.wait_close().map_err(|error| {
        native_stage_error(
            attempt,
            RealConnectionStage::Connected,
            SshErrorKind::Channel,
            format!("failed while waiting for native exec channel close: {error}"),
        )
    })?;
    let exit_status = channel.exit_status().map_err(|error| {
        native_stage_error(
            attempt,
            RealConnectionStage::Connected,
            SshErrorKind::Channel,
            format!("failed to fetch native exec exit status: {error}"),
        )
    })?;
    attempt.mark_disconnected(format!(
        "Native exec command `{command}` completed with exit status {exit_status}."
    ));
    Ok(ExecOutput {
        stdout,
        stderr,
        exit_status,
    })
}

fn connect_native_stream(plan: &RealConnectionPlan) -> SshResult<TcpStream> {
    let target = format!("{}:{}", plan.host, plan.port);
    let resolved = (plan.host.as_str(), plan.port)
        .to_socket_addrs()
        .map_err(|error| {
            SshError::new(
                SshErrorKind::Dns,
                format!("failed to resolve `{target}` for native SSH session: {error}"),
            )
        })?;
    let addresses = resolved.collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(SshError::new(
            SshErrorKind::Dns,
            format!("failed to resolve `{target}` for native SSH session: no socket addresses"),
        ));
    }
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, plan.connect_timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some((address, error)),
        }
    }
    let (address, error) = last_error.expect("native connect attempted at least one address");
    Err(SshError::new(
        map_tcp_connect_error_kind(&error),
        format!("failed to open native SSH TCP stream to `{address}`: {error}"),
    ))
}

fn session_timeout_ms(timeout: Duration) -> u32 {
    timeout.as_millis().clamp(1, u32::MAX as u128) as u32
}

fn verify_native_host_key(session: &Session, attempt: &mut RealConnectionAttempt) -> SshResult<()> {
    let (key, key_type) = session.host_key().ok_or_else(|| {
        native_stage_error(
            attempt,
            RealConnectionStage::HostKeyVerification,
            SshErrorKind::Handshake,
            "native SSH session did not expose a server host key".to_owned(),
        )
    })?;
    let presented = crate::host_key::HostKeyFingerprint {
        algorithm: native_host_key_algorithm_label(key_type).to_owned(),
        fingerprint: bytes_to_hex(key),
    };
    let server = format!("{}:{}", attempt.plan.host, attempt.plan.port);
    let policy = attempt.plan.host_key_policy.clone();

    // 唯一静默信任的分支：显式测试策略 accept-any-for-testing。
    if matches!(
        attempt.plan.host_key_policy,
        HostKeyPolicy::AcceptAnyForTesting
    ) {
        attempt.record_action(
            RealTransportAction::VerifyHostKey { server, policy },
            RealTransportActionOutcome::Completed,
            format!(
                "native host key accepted by the accept-any-for-testing policy: {} {}",
                presented.algorithm, presented.fingerprint
            ),
        );
        return Ok(());
    }

    // 产品决定（2026-09-27）：TOFU 首次连接也必须弹窗确认，所以未知主机与 Strict
    // 走同一条错误路径（`HostKeyProblem::Unknown`），由 App 层呈现三选一。
    let Some(problem) = native_host_key_problem(
        &attempt.plan.known_hosts,
        &attempt.plan.host,
        attempt.plan.port,
        &presented,
    ) else {
        attempt.record_action(
            RealTransportAction::VerifyHostKey { server, policy },
            RealTransportActionOutcome::Completed,
            format!(
                "native host key trusted: {} {}",
                presented.algorithm, presented.fingerprint
            ),
        );
        return Ok(());
    };

    let host = attempt.plan.host.clone();
    let port = attempt.plan.port;
    let error = match &problem {
        crate::error::HostKeyProblem::Changed { expected, .. } => native_stage_error(
            attempt,
            RealConnectionStage::HostKeyVerification,
            SshErrorKind::HostKeyRejected,
            format!(
                "native host key mismatch: expected {} {}, got {} {}",
                expected.algorithm,
                expected.fingerprint,
                presented.algorithm,
                presented.fingerprint
            ),
        ),
        crate::error::HostKeyProblem::Unknown { .. } => native_stage_error(
            attempt,
            RealConnectionStage::HostKeyVerification,
            SshErrorKind::HostKeyRejected,
            format!(
                "no known host key for {host}:{port}; presented {} {}",
                presented.algorithm, presented.fingerprint
            ),
        ),
    };
    Err(error.with_host_key_problem(problem))
}

/// 非测试策略下的主机密钥问题（`None` = 密钥可信）。
///
/// 产品决定（2026-09-27）：`TrustOnFirstUse` 不再静默 pin 新密钥；未知主机与
/// `Strict` 一样返回 `HostKeyProblem::Unknown`，由 App 层弹信任弹窗。
/// 唯一静默信任的路径是显式测试策略 `AcceptAnyForTesting`（在调用方短路）。
fn native_host_key_problem(
    known_hosts: &KnownHosts,
    host: &str,
    port: u16,
    presented: &crate::host_key::HostKeyFingerprint,
) -> Option<crate::error::HostKeyProblem> {
    match known_hosts.get(host, port) {
        Some(expected) if expected == presented => None,
        Some(expected) => Some(crate::error::HostKeyProblem::Changed {
            host: host.to_owned(),
            port,
            presented: presented.clone(),
            expected: expected.clone(),
        }),
        None => Some(crate::error::HostKeyProblem::Unknown {
            host: host.to_owned(),
            port,
            presented: presented.clone(),
        }),
    }
}

fn authenticate_native_session(session: &Session, attempt: &mut RealConnectionAttempt) -> SshResult<()> {
    let detail = match &attempt.plan.auth_strategy {
        RealAuthStrategy::Password => "native password authentication succeeded".to_owned(),
        RealAuthStrategy::PrivateKey { key_path, .. } => {
            format!("native private-key authentication succeeded using `{key_path}`")
        }
        RealAuthStrategy::Agent => "native ssh-agent authentication succeeded".to_owned(),
        RealAuthStrategy::KeyboardInteractive => {
            "native keyboard-interactive authentication succeeded".to_owned()
        }
    };
    match &attempt.plan.auth {
        AuthMethod::Password { username, password } => {
            session.userauth_password(username, password).map_err(|error| {
                native_stage_error(
                    attempt,
                    RealConnectionStage::Authentication,
                    SshErrorKind::Authentication,
                    format!("native password authentication failed: {error}"),
                )
            })?;
        }
        AuthMethod::PrivateKey {
            username,
            key_path,
            passphrase,
        } => {
            session
                .userauth_pubkey_file(username, None, Path::new(key_path), passphrase.as_deref())
                .map_err(|error| {
                    native_stage_error(
                        attempt,
                        RealConnectionStage::Authentication,
                        SshErrorKind::Authentication,
                        format!("native private-key authentication failed: {error}"),
                    )
                })?;
        }
        AuthMethod::Agent { username } => {
            session
                .userauth_agent(username)
                .map_err(|error| {
                    native_stage_error(
                        attempt,
                        RealConnectionStage::Authentication,
                        SshErrorKind::Authentication,
                        format!("native ssh-agent authentication failed: {error}"),
                    )
                })?;
        }
        AuthMethod::KeyboardInteractive { username, secret } => {
            let mut prompter = SecretOnlyKeyboardInteractivePrompt::new(secret.clone());
            session
                .userauth_keyboard_interactive(username, &mut prompter)
                .map_err(|error| {
                    native_stage_error(
                        attempt,
                        RealConnectionStage::Authentication,
                        SshErrorKind::Authentication,
                        format!(
                            "native keyboard-interactive authentication failed: {error}; prompts={}",
                            prompter.describe_prompts()
                        ),
                    )
                })?;
        }
    }
    if !session.authenticated() {
        return Err(native_stage_error(
            attempt,
            RealConnectionStage::Authentication,
            SshErrorKind::Authentication,
            "native SSH session is still unauthenticated after auth attempt".to_owned(),
        ));
    }
    attempt.record_action(
        RealTransportAction::Authenticate {
            username: attempt.plan.username.clone(),
            strategy: attempt.plan.auth_strategy.clone(),
        },
        RealTransportActionOutcome::Completed,
        detail,
    );
    Ok(())
}

fn drain_channel_stream(channel: &mut Channel, queue: &mut VecDeque<Vec<u8>>) -> SshResult<()> {
    drain_read_stream(channel, queue)
}

fn drain_read_stream(reader: &mut impl Read, queue: &mut VecDeque<Vec<u8>>) -> SshResult<()> {
    loop {
        let mut buffer = [0u8; 4096];
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => queue.push_back(buffer[..count].to_vec()),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => {
                return Err(SshError::new(
                    SshErrorKind::Io,
                    format!("failed while reading native SSH output: {error}"),
                ))
            }
        }
    }
    Ok(())
}

fn native_host_key_algorithm_label(key_type: ssh2::HostKeyType) -> &'static str {
    match key_type {
        ssh2::HostKeyType::Unknown => "unknown",
        ssh2::HostKeyType::Rsa => "ssh-rsa",
        ssh2::HostKeyType::Dss => "ssh-dss",
        ssh2::HostKeyType::Ecdsa256 => "ecdsa-sha2-nistp256",
        ssh2::HostKeyType::Ecdsa384 => "ecdsa-sha2-nistp384",
        ssh2::HostKeyType::Ecdsa521 => "ecdsa-sha2-nistp521",
        ssh2::HostKeyType::Ed25519 => "ssh-ed25519",
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn native_stage_error(
    attempt: &mut RealConnectionAttempt,
    stage: RealConnectionStage,
    kind: SshErrorKind,
    message: String,
) -> SshError {
    let action = match stage {
        RealConnectionStage::SshHandshake => RealTransportAction::PerformSshHandshake {
            server: format!("{}:{}", attempt.plan.host, attempt.plan.port),
        },
        RealConnectionStage::HostKeyVerification => RealTransportAction::VerifyHostKey {
            server: format!("{}:{}", attempt.plan.host, attempt.plan.port),
            policy: attempt.plan.host_key_policy.clone(),
        },
        RealConnectionStage::Authentication => RealTransportAction::Authenticate {
            username: attempt.plan.username.clone(),
            strategy: attempt.plan.auth_strategy.clone(),
        },
        RealConnectionStage::PtyRequest => RealTransportAction::RequestPty {
            term: attempt.plan.pty.term.clone(),
            columns: attempt.plan.pty.size.columns,
            rows: attempt.plan.pty.size.rows,
        },
        RealConnectionStage::ShellOpen => RealTransportAction::OpenShell,
        _ => attempt.next_action(),
    };
    attempt.record_action(action, RealTransportActionOutcome::Failed, message.clone());
    attempt.mark_failed(message.clone());
    SshError::new(kind, message)
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    use crate::{
        AuthMethod, ForwardingKind, ProxyConfig, ShellClient, ShellSession, SshAdapter, SshClient,
        SshConnectionConfig, SshErrorKind, TunnelConfig,
    };

    use super::{
        native_host_key_problem, RealAuthStrategy, RealConnectionAttempt, RealConnectionStage,
        RealSshAdapter, RealTransportActionOutcome,
    };

    #[test]
    fn real_backend_open_shell_reports_handshake_failure_for_non_ssh_listener() {
        let (port, accept_handle) = start_tcp_probe_target(2);
        let client = ShellClient::with_real_backend();
        let config = localhost_agent_config(port);

        let error = client
            .open_shell(&config)
            .expect_err("non-ssh listener should fail during handshake handoff");
        accept_handle.join().expect("accept thread");
        assert_eq!(error.kind, SshErrorKind::Handshake);
        assert!(error.message.contains("native SSH handshake failed"));
    }

    #[test]
    fn tofu_unknown_host_produces_unknown_problem_for_user_confirmation() {
        // 产品决定（2026-09-27）：TrustOnFirstUse 不再静默 pin 新密钥；
        // 未知主机产生 Unknown 问题，由 App 层弹信任弹窗（Trust Once / Trust and Save）。
        let presented = crate::host_key::HostKeyFingerprint {
            algorithm: "ecdsa-sha2-nistp256".to_owned(),
            fingerprint: "aa:bb".to_owned(),
        };
        let problem = native_host_key_problem(
            &crate::host_key::KnownHosts::new(),
            "tofu.example.test",
            22,
            &presented,
        )
        .expect("unknown host must produce a problem instead of silent trust");
        assert_eq!(
            problem,
            crate::error::HostKeyProblem::Unknown {
                host: "tofu.example.test".to_owned(),
                port: 22,
                presented,
            }
        );
    }

    #[test]
    fn known_host_key_problem_reports_changed_key_and_accepts_matching_key() {
        let pinned = crate::host_key::HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "aa:bb".to_owned(),
        };
        let mut known_hosts = crate::host_key::KnownHosts::new();
        known_hosts.pin("known.example.test", 2222, pinned.clone());
        assert_eq!(
            native_host_key_problem(&known_hosts, "known.example.test", 2222, &pinned),
            None
        );

        let presented = crate::host_key::HostKeyFingerprint {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "cc:dd".to_owned(),
        };
        assert_eq!(
            native_host_key_problem(&known_hosts, "known.example.test", 2222, &presented),
            Some(crate::error::HostKeyProblem::Changed {
                host: "known.example.test".to_owned(),
                port: 2222,
                presented,
                expected: pinned,
            })
        );
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
            .expect("real shell scaffold");
        shell.disconnect().expect("disconnect scaffold");
        assert_eq!(shell.attempt.stage, RealConnectionStage::Disconnected);
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
            resolve_dns_by_proxy: true,
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
        let (port, accept_handle) = start_tcp_probe_target(1);
        let adapter = RealSshAdapter;
        let config = localhost_agent_config(port);

        let attempt = adapter.connect(&config).expect("real connection attempt");
        accept_handle.join().expect("accept thread");
        assert_eq!(attempt.stage, RealConnectionStage::SshHandshake);
        assert_eq!(attempt.plan.username, "alice");
        assert_eq!(attempt.history.len(), 3);
        assert_eq!(attempt.action_log.len(), 1);
        assert_eq!(
            attempt.latest_action_record().expect("action record").outcome,
            RealTransportActionOutcome::Completed
        );
        assert!(attempt.history[2].note.contains("SSH handshake"));
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
        assert_eq!(attempt.action_log.len(), 1);
        assert!(attempt.history[1].note.contains("Proxy negotiation"));
    }

    #[test]
    fn real_connection_attempt_can_advance_fail_and_summarize() {
        let adapter = RealSshAdapter;
        let config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );

        let mut attempt = RealConnectionAttempt::prepared(
            adapter.plan_connection(&config).expect("plan connection"),
        );
        attempt.advance_to(
            RealConnectionStage::TcpConnect,
            "Entering tcp-connect stage for summary testing.",
        );
        attempt.advance_to(
            RealConnectionStage::SshHandshake,
            "TCP socket placeholder completed; SSH handshake would start next.",
        );
        attempt.mark_failed("Synthetic handshake failure for scaffold testing.");

        assert_eq!(attempt.stage, RealConnectionStage::Failed);
        assert_eq!(attempt.history.len(), 4);
        assert!(attempt.current_summary().contains("stage=failed"));

        let transcript = attempt.history_transcript_lines();
        assert!(transcript[0].contains("prepared"));
        assert!(transcript[2].contains("ssh-handshake"));
        assert!(transcript[3].contains("Synthetic handshake failure"));
    }

    #[test]
    fn real_connection_attempt_can_mark_disconnected() {
        let adapter = RealSshAdapter;
        let config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );

        let mut attempt = RealConnectionAttempt::prepared(
            adapter.plan_connection(&config).expect("plan connection"),
        );
        attempt.advance_to(
            RealConnectionStage::TcpConnect,
            "Entering tcp-connect stage before disconnect.",
        );
        attempt.mark_disconnected("User canceled before socket creation.");
        assert_eq!(attempt.stage, RealConnectionStage::Disconnected);
        assert!(attempt.current_summary().contains("disconnected"));
    }

    #[test]
    fn real_connection_attempt_reports_next_transport_action() {
        let adapter = RealSshAdapter;
        let config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );

        let mut attempt = RealConnectionAttempt::prepared(
            adapter.plan_connection(&config).expect("plan connection"),
        );
        attempt.advance_to(
            RealConnectionStage::TcpConnect,
            "Entering tcp-connect stage for action-summary testing.",
        );
        assert_eq!(attempt.next_action().summary(), "tcp-connect:example.test:22");

        attempt.advance_to(
            RealConnectionStage::Authentication,
            "Synthetic auth stage for action-summary testing.",
        );
        attempt.record_current_action_pending("Authentication implementation will use the configured agent.");
        assert_eq!(attempt.next_action().summary(), "auth:agent@alice");
        assert!(attempt.current_summary().contains("pending:auth:agent@alice"));
    }

    #[test]
    fn proxy_stage_reports_proxy_action_summary() {
        let adapter = RealSshAdapter;
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
            resolve_dns_by_proxy: true,
        };

        let attempt = adapter.connect(&config).expect("real connection attempt");
        assert_eq!(
            attempt.next_action().summary(),
            "proxy:socks5@127.0.0.1:1080"
        );
    }

    #[test]
    fn real_connection_plan_carries_tunnel_configuration() {
        let adapter = RealSshAdapter;
        let mut config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );
        config.tunnels = vec![
            TunnelConfig {
                id: "local-db".to_owned(),
                kind: ForwardingKind::Local,
                listen_host: "127.0.0.1".to_owned(),
                listen_port: 15432,
                target_host: "db.internal".to_owned(),
                target_port: 5432,
            },
            TunnelConfig {
                id: "dynamic-socks".to_owned(),
                kind: ForwardingKind::Dynamic,
                listen_host: "127.0.0.1".to_owned(),
                listen_port: 19050,
                target_host: String::new(),
                target_port: 0,
            },
        ];

        let plan = adapter.plan_connection(&config).expect("plan connection");

        assert_eq!(plan.tunnels.len(), 2);
        assert_eq!(plan.tunnels[0].kind, ForwardingKind::Local);
        assert_eq!(plan.tunnels[1].kind, ForwardingKind::Dynamic);
    }

    #[test]
    fn real_connection_attempt_records_action_outcomes() {
        let adapter = RealSshAdapter;
        let config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );

        let mut attempt = RealConnectionAttempt::prepared(
            adapter.plan_connection(&config).expect("plan connection"),
        );
        attempt.advance_to(
            RealConnectionStage::TcpConnect,
            "Entering tcp-connect stage for action-outcome testing.",
        );
        let _ = attempt.execute_current_action_scaffold();
        assert_eq!(attempt.action_log.len(), 1);

        attempt.record_current_action_pending("TCP socket open has been scheduled.");
        attempt.record_current_action_failed("Synthetic socket failure for scaffold testing.");

        let lines = attempt.action_transcript_lines();
        assert!(lines[0].contains("blocked:tcp-connect:example.test:22"));
        assert!(lines[1].contains("pending:tcp-connect:example.test:22"));
        assert!(lines[2].contains("failed:tcp-connect:example.test:22"));
    }

    #[test]
    fn real_connection_attempt_can_execute_canonical_scaffold_action() {
        let adapter = RealSshAdapter;
        let config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );

        let mut attempt = RealConnectionAttempt::prepared(
            adapter.plan_connection(&config).expect("plan connection"),
        );
        attempt.advance_to(
            RealConnectionStage::TcpConnect,
            "Entering tcp-connect stage for scaffold execution testing.",
        );

        let report = attempt.execute_current_action_scaffold();
        let record = report.record.as_ref().expect("scaffold record");
        assert_eq!(report.stage, RealConnectionStage::TcpConnect);
        assert_eq!(report.action.summary(), "tcp-connect:example.test:22");
        assert!(report.summary().contains("scaffold-execution"));
        assert_eq!(record.outcome, RealTransportActionOutcome::Blocked);
        assert_eq!(record.action.summary(), "tcp-connect:example.test:22");
        assert!(record.detail.contains("no live socket implementation"));
        assert_eq!(attempt.action_log.len(), 1);
    }

    #[test]
    fn live_native_ssh_shell_smoke_when_env_target_is_set() {
        let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
            return;
        };
        let (username, host, port) = parse_live_ssh_target(&target);
        let client = ShellClient::with_real_backend();
        let mut config = SshConnectionConfig::new(host, port, AuthMethod::Agent { username });
        config.host_key_policy = crate::host_key::HostKeyPolicy::AcceptAnyForTesting;

        let mut shell = client.open_shell(&config).expect("live ssh shell");
        assert!(shell.is_connected());
        shell
            .write_input(b"printf '__YSHELL_LIVE_OK__\\n'\n")
            .expect("write live command");
        shell.write_input(b"exit\n").expect("write exit");

        let mut transcript = String::new();
        for _ in 0..80 {
            let bytes = shell.poll_output().expect("poll live output");
            if !bytes.is_empty() {
                transcript.push_str(&String::from_utf8_lossy(&bytes));
                if transcript.contains("__YSHELL_LIVE_OK__") {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(transcript.contains("__YSHELL_LIVE_OK__"));
    }

    #[test]
    fn real_connection_attempt_marks_failed_when_tcp_probe_cannot_connect() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind closed-port probe");
        let port = listener.local_addr().expect("listener addr").port();
        drop(listener);

        let adapter = RealSshAdapter;
        let config = localhost_agent_config(port);
        let mut attempt = RealConnectionAttempt::prepared(
            adapter.plan_connection(&config).expect("plan connection"),
        );
        attempt.advance_to(
            RealConnectionStage::TcpConnect,
            "Entering tcp-connect stage for failure testing.",
        );

        let error = attempt
            .execute_current_action()
            .expect_err("tcp connect should fail once listener is gone");
        assert_eq!(error.kind, crate::error::SshErrorKind::TcpConnect);
        assert_eq!(attempt.stage, RealConnectionStage::Failed);
        assert_eq!(
            attempt.latest_action_record().expect("action record").outcome,
            RealTransportActionOutcome::Failed
        );
        assert!(attempt.current_summary().contains("failed"));
    }

    #[test]
    fn keyboard_interactive_auth_strategy_is_selected_from_config() {
        let adapter = RealSshAdapter;
        let config = SshConnectionConfig::new(
            "127.0.0.1",
            22,
            AuthMethod::KeyboardInteractive {
                username: "alice".to_owned(),
                secret: "otp-secret".to_owned(),
            },
        );

        let plan = adapter.plan_connection(&config).expect("plan connection");

        assert_eq!(plan.username, "alice");
        assert_eq!(plan.auth_strategy, RealAuthStrategy::KeyboardInteractive);
    }

    #[test]
    fn real_exec_rejects_empty_command() {
        let adapter = RealSshAdapter;
        let config = localhost_agent_config(22);
        let mut attempt =
            RealConnectionAttempt::prepared(adapter.plan_connection(&config).expect("plan"));

        let error = adapter.exec(&mut attempt, "   ").expect_err("empty command should fail");

        assert_eq!(error.kind, SshErrorKind::Configuration);
    }

    #[test]
    fn live_native_ssh_exec_smoke_when_env_target_is_set() {
        let Some(target) = std::env::var("YSHELL_LIVE_SSH_TARGET").ok() else {
            return;
        };
        let (username, host, port) = parse_live_ssh_target(&target);
        let client = SshClient::with_real_backend();
        let mut config = SshConnectionConfig::new(host, port, AuthMethod::Agent { username });
        config.host_key_policy = crate::host_key::HostKeyPolicy::AcceptAnyForTesting;

        let mut attempt = client.connect(&config).expect("real connect");
        let output = client
            .exec(&mut attempt, "printf '__YSHELL_EXEC_OK__\\n'")
            .expect("real exec");

        assert_eq!(output.exit_status, 0);
        assert!(String::from_utf8_lossy(&output.stdout).contains("__YSHELL_EXEC_OK__"));
        assert!(output.stderr.is_empty());
    }

    fn localhost_agent_config(port: u16) -> SshConnectionConfig {
        SshConnectionConfig::new(
            "127.0.0.1",
            port,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        )
    }

    fn start_tcp_probe_target(expected_accepts: usize) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind tcp probe target");
        let port = listener.local_addr().expect("listener addr").port();
        let handle = thread::spawn(move || {
            for _ in 0..expected_accepts {
                let _ = listener.accept();
            }
        });
        (port, handle)
    }

    fn parse_live_ssh_target(target: &str) -> (String, String, u16) {
        let (username, host_port) = target
            .split_once('@')
            .unwrap_or_else(|| panic!("YSHELL_LIVE_SSH_TARGET must be user@host:port"));
        let (host, port) = host_port
            .rsplit_once(':')
            .unwrap_or_else(|| panic!("YSHELL_LIVE_SSH_TARGET must include :port"));
        let port = port.parse::<u16>().expect("live ssh port");
        (username.to_owned(), host.to_owned(), port)
    }
}
