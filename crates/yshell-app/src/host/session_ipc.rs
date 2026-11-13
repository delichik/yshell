//! SessionWorker IPC: Host ↔ out-of-process SSH/PTY worker.
//!
//! Protocol: newline-delimited JSON over loopback TCP. Host binds, spawns
//! `--session-worker`, accepts one connection, then drives Connect / Input /
//! Resize / Disconnect while the worker owns the live SSH session.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use yshell_ssh::{
    with_connection_stage_hook, AuthMethod, ChannelKind, ForwardingKind, HostKeyFingerprint,
    HostKeyPolicy, KnownHosts, ProxyConfig, PtyConfig, PtySize, ShellSession, SshConnectionConfig,
    SshError, SshErrorKind, SshTransport, TunnelConfig,
};

use crate::error::{AppError, AppResult};

use super::liveness::current_pid;

const CONNECT_ACCEPT_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostToWorker {
    Connect {
        config: Box<WireSshConfig>,
        open_shell: bool,
    },
    Input {
        data_b64: String,
    },
    Resize {
        columns: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    },
    Disconnect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerToHost {
    Progress {
        stage: String,
    },
    Connected,
    Output {
        data_b64: String,
    },
    Failed {
        message: String,
        stage: Option<String>,
        kind: Option<String>,
    },
    Exited,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireSshConfig {
    pub host: String,
    pub port: u16,
    pub auth: WireAuth,
    pub host_key_policy: WireHostKeyPolicy,
    pub known_hosts: Vec<WireKnownHost>,
    pub proxy: WireProxy,
    pub pty: WirePty,
    pub tunnels: Vec<WireTunnel>,
    pub connect_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireAuth {
    Password {
        username: String,
        password: String,
    },
    PrivateKey {
        username: String,
        key_path: String,
        passphrase: Option<String>,
    },
    Agent {
        username: String,
    },
    KeyboardInteractive {
        username: String,
        secret: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireHostKeyPolicy {
    Strict,
    TrustOnFirstUse,
    AcceptAnyForTesting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireKnownHost {
    pub key: String,
    pub algorithm: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireProxy {
    None,
    Socks4 {
        address: String,
        username: Option<String>,
    },
    Socks4a {
        address: String,
        username: Option<String>,
    },
    Socks5 {
        address: String,
        username: Option<String>,
        password: Option<String>,
        resolve_dns_by_proxy: bool,
    },
    HttpConnect {
        address: String,
        username: Option<String>,
        password: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WirePty {
    pub term: String,
    pub columns: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub modes: Vec<(String, u32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireTunnel {
    pub id: String,
    pub kind: WireForwardingKind,
    pub listen_host: String,
    pub listen_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireForwardingKind {
    Local,
    Remote,
    Dynamic,
}

impl From<&SshConnectionConfig> for WireSshConfig {
    fn from(config: &SshConnectionConfig) -> Self {
        Self {
            host: config.host.clone(),
            port: config.port,
            auth: WireAuth::from(&config.auth),
            host_key_policy: WireHostKeyPolicy::from(&config.host_key_policy),
            known_hosts: config
                .known_hosts
                .snapshot()
                .into_iter()
                .map(|(key, fp)| WireKnownHost {
                    key,
                    algorithm: fp.algorithm,
                    fingerprint: fp.fingerprint,
                })
                .collect(),
            proxy: WireProxy::from(&config.proxy),
            pty: WirePty {
                term: config.pty.term.clone(),
                columns: config.pty.size.columns,
                rows: config.pty.size.rows,
                pixel_width: config.pty.size.pixel_width,
                pixel_height: config.pty.size.pixel_height,
                modes: config.pty.modes.clone(),
            },
            tunnels: config
                .tunnels
                .iter()
                .map(|tunnel| WireTunnel {
                    id: tunnel.id.clone(),
                    kind: match tunnel.kind {
                        ForwardingKind::Local => WireForwardingKind::Local,
                        ForwardingKind::Remote => WireForwardingKind::Remote,
                        ForwardingKind::Dynamic => WireForwardingKind::Dynamic,
                    },
                    listen_host: tunnel.listen_host.clone(),
                    listen_port: tunnel.listen_port,
                    target_host: tunnel.target_host.clone(),
                    target_port: tunnel.target_port,
                })
                .collect(),
            connect_timeout_ms: u64::try_from(config.connect_timeout.as_millis())
                .unwrap_or(u64::MAX),
        }
    }
}

impl From<&AuthMethod> for WireAuth {
    fn from(auth: &AuthMethod) -> Self {
        match auth {
            AuthMethod::Password { username, password } => Self::Password {
                username: username.clone(),
                password: password.clone(),
            },
            AuthMethod::PrivateKey {
                username,
                key_path,
                passphrase,
            } => Self::PrivateKey {
                username: username.clone(),
                key_path: key_path.clone(),
                passphrase: passphrase.clone(),
            },
            AuthMethod::Agent { username } => Self::Agent {
                username: username.clone(),
            },
            AuthMethod::KeyboardInteractive { username, secret } => Self::KeyboardInteractive {
                username: username.clone(),
                secret: secret.clone(),
            },
        }
    }
}

impl From<&HostKeyPolicy> for WireHostKeyPolicy {
    fn from(policy: &HostKeyPolicy) -> Self {
        match policy {
            HostKeyPolicy::Strict => Self::Strict,
            HostKeyPolicy::TrustOnFirstUse => Self::TrustOnFirstUse,
            HostKeyPolicy::AcceptAnyForTesting => Self::AcceptAnyForTesting,
        }
    }
}

impl From<&ProxyConfig> for WireProxy {
    fn from(proxy: &ProxyConfig) -> Self {
        match proxy {
            ProxyConfig::None => Self::None,
            ProxyConfig::Socks4 { address, username } => Self::Socks4 {
                address: address.clone(),
                username: username.clone(),
            },
            ProxyConfig::Socks4a { address, username } => Self::Socks4a {
                address: address.clone(),
                username: username.clone(),
            },
            ProxyConfig::Socks5 {
                address,
                username,
                password,
                resolve_dns_by_proxy,
            } => Self::Socks5 {
                address: address.clone(),
                username: username.clone(),
                password: password.clone(),
                resolve_dns_by_proxy: *resolve_dns_by_proxy,
            },
            ProxyConfig::HttpConnect {
                address,
                username,
                password,
            } => Self::HttpConnect {
                address: address.clone(),
                username: username.clone(),
                password: password.clone(),
            },
        }
    }
}

impl WireSshConfig {
    pub fn into_ssh(self) -> SshConnectionConfig {
        let mut config = SshConnectionConfig::new(self.host, self.port, self.auth.into_auth());
        config.host_key_policy = match self.host_key_policy {
            WireHostKeyPolicy::Strict => HostKeyPolicy::Strict,
            WireHostKeyPolicy::TrustOnFirstUse => HostKeyPolicy::TrustOnFirstUse,
            WireHostKeyPolicy::AcceptAnyForTesting => HostKeyPolicy::AcceptAnyForTesting,
        };
        let mut entries = BTreeMap::new();
        for entry in self.known_hosts {
            entries.insert(
                entry.key,
                HostKeyFingerprint {
                    algorithm: entry.algorithm,
                    fingerprint: entry.fingerprint,
                },
            );
        }
        config.known_hosts = KnownHosts::from_snapshot(entries);
        config.proxy = self.proxy.into_proxy();
        config.pty = PtyConfig {
            term: self.pty.term,
            size: PtySize {
                columns: self.pty.columns,
                rows: self.pty.rows,
                pixel_width: self.pty.pixel_width,
                pixel_height: self.pty.pixel_height,
            },
            modes: self.pty.modes,
        };
        config.tunnels = self
            .tunnels
            .into_iter()
            .map(|tunnel| TunnelConfig {
                id: tunnel.id,
                kind: match tunnel.kind {
                    WireForwardingKind::Local => ForwardingKind::Local,
                    WireForwardingKind::Remote => ForwardingKind::Remote,
                    WireForwardingKind::Dynamic => ForwardingKind::Dynamic,
                },
                listen_host: tunnel.listen_host,
                listen_port: tunnel.listen_port,
                target_host: tunnel.target_host,
                target_port: tunnel.target_port,
            })
            .collect();
        config.connect_timeout = Duration::from_millis(self.connect_timeout_ms);
        config
    }
}

impl WireAuth {
    fn into_auth(self) -> AuthMethod {
        match self {
            Self::Password { username, password } => AuthMethod::Password { username, password },
            Self::PrivateKey {
                username,
                key_path,
                passphrase,
            } => AuthMethod::PrivateKey {
                username,
                key_path,
                passphrase,
            },
            Self::Agent { username } => AuthMethod::Agent { username },
            Self::KeyboardInteractive { username, secret } => {
                AuthMethod::KeyboardInteractive { username, secret }
            }
        }
    }
}

impl WireProxy {
    fn into_proxy(self) -> ProxyConfig {
        match self {
            Self::None => ProxyConfig::None,
            Self::Socks4 { address, username } => ProxyConfig::Socks4 { address, username },
            Self::Socks4a { address, username } => ProxyConfig::Socks4a { address, username },
            Self::Socks5 {
                address,
                username,
                password,
                resolve_dns_by_proxy,
            } => ProxyConfig::Socks5 {
                address,
                username,
                password,
                resolve_dns_by_proxy,
            },
            Self::HttpConnect {
                address,
                username,
                password,
            } => ProxyConfig::HttpConnect {
                address,
                username,
                password,
            },
        }
    }
}

/// Owned child returned to Host for Job / run-state tracking.
#[derive(Debug)]
pub struct SessionWorkerChild {
    pub session_id: String,
    pub pid: u32,
    pub child: Child,
}

/// Host-side shell that proxies I/O to a SessionWorker process.
pub struct WorkerShellSession {
    connected: Arc<AtomicBool>,
    writer: Mutex<TcpStream>,
    output_rx: Mutex<Receiver<Vec<u8>>>,
}

impl std::fmt::Debug for WorkerShellSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerShellSession")
            .field("connected", &self.connected.load(Ordering::SeqCst))
            .finish()
    }
}

impl ShellSession for WorkerShellSession {
    fn kind(&self) -> ChannelKind {
        ChannelKind::Shell
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn poll_output(&mut self) -> Result<Vec<u8>, SshError> {
        let rx = self
            .output_rx
            .lock()
            .map_err(|_| SshError::new(SshErrorKind::Channel, "worker output lock poisoned"))?;
        let mut out = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            out.extend_from_slice(&chunk);
        }
        if out.is_empty() && !self.connected.load(Ordering::SeqCst) {
            // Drain finished; surface disconnect as empty poll (caller checks is_connected).
        }
        Ok(out)
    }

    fn write_input(&mut self, bytes: &[u8]) -> Result<(), SshError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "session worker disconnected",
            ));
        }
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| SshError::new(SshErrorKind::Channel, "worker write lock poisoned"))?;
        write_msg(
            &mut *writer,
            &HostToWorker::Input {
                data_b64: B64.encode(bytes),
            },
        )
        .map_err(|error| SshError::new(SshErrorKind::Channel, error.to_string()))
    }

    fn resize_pty(&mut self, size: PtySize) -> Result<(), SshError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "session worker disconnected",
            ));
        }
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| SshError::new(SshErrorKind::Channel, "worker write lock poisoned"))?;
        write_msg(
            &mut *writer,
            &HostToWorker::Resize {
                columns: size.columns,
                rows: size.rows,
                pixel_width: size.pixel_width,
                pixel_height: size.pixel_height,
            },
        )
        .map_err(|error| SshError::new(SshErrorKind::Channel, error.to_string()))
    }

    fn disconnect(&mut self) -> Result<(), SshError> {
        self.connected.store(false, Ordering::SeqCst);
        if let Ok(mut writer) = self.writer.lock() {
            let _ = write_msg(&mut *writer, &HostToWorker::Disconnect);
        }
        Ok(())
    }
}

impl Drop for WorkerShellSession {
    fn drop(&mut self) {
        let _ = self.disconnect();
    }
}

/// Progress callback used while the worker is connecting.
pub type ProgressFn = Box<dyn FnMut(String) + Send>;

/// Spawn a SessionWorker, drive Connect, return a live [`WorkerShellSession`].
pub fn connect_via_session_worker(
    config_dir: &Path,
    session_id: &str,
    generation: u64,
    ssh_config: &SshConnectionConfig,
    open_shell: bool,
    cancel: &AtomicBool,
    mut on_progress: ProgressFn,
) -> Result<(WorkerShellSession, SessionWorkerChild), SshError> {
    if !open_shell {
        return Err(SshError::configuration(
            "session worker requires open_shell (SFTP-only stays in-process)",
        ));
    }

    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
        SshError::new(
            SshErrorKind::Configuration,
            format!("cannot bind session-worker ipc: {error}"),
        )
    })?;
    let port = listener
        .local_addr()
        .map_err(|error| {
            SshError::new(
                SshErrorKind::Configuration,
                format!("session-worker ipc local_addr: {error}"),
            )
        })?
        .port();
    let token = format!(
        "sw-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );

    let child = spawn_worker_process(config_dir, session_id, generation, port, &token)?;
    let pid = child.id();
    let worker_child = SessionWorkerChild {
        session_id: session_id.to_owned(),
        pid,
        child,
    };

    listener
        .set_nonblocking(true)
        .map_err(|error| SshError::new(SshErrorKind::Configuration, error.to_string()))?;

    let deadline = Instant::now() + CONNECT_ACCEPT_TIMEOUT;
    let stream = loop {
        if cancel.load(Ordering::SeqCst) {
            terminate_child_pid(pid);
            return Err(SshError::new(SshErrorKind::Timeout, "connect cancelled"));
        }
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    terminate_child_pid(pid);
                    return Err(SshError::new(
                        SshErrorKind::Configuration,
                        "timed out waiting for session worker to connect",
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                terminate_child_pid(pid);
                return Err(SshError::new(
                    SshErrorKind::Configuration,
                    format!("session worker accept failed: {error}"),
                ));
            }
        }
    };

    stream
        .set_nodelay(true)
        .map_err(|error| SshError::new(SshErrorKind::Configuration, error.to_string()))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| SshError::new(SshErrorKind::Configuration, error.to_string()))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| SshError::new(SshErrorKind::Configuration, error.to_string()))?;

    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|error| SshError::new(SshErrorKind::Configuration, error.to_string()))?,
    );
    let mut writer = stream;

    // Auth line from worker.
    let mut auth = String::new();
    read_line_deadline(
        &mut reader,
        &mut auth,
        Instant::now() + CONNECT_ACCEPT_TIMEOUT,
        cancel,
    )?;
    if auth.trim_end() != token {
        terminate_child_pid(pid);
        return Err(SshError::new(
            SshErrorKind::Configuration,
            "session worker auth token mismatch",
        ));
    }

    write_msg(
        &mut writer,
        &HostToWorker::Connect {
            config: Box::new(WireSshConfig::from(ssh_config)),
            open_shell,
        },
    )
    .map_err(|error| SshError::new(SshErrorKind::Configuration, error.to_string()))?;

    let handshake_deadline = Instant::now() + CONNECT_HANDSHAKE_TIMEOUT;
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = write_msg(&mut writer, &HostToWorker::Disconnect);
            terminate_child_pid(pid);
            return Err(SshError::new(SshErrorKind::Timeout, "connect cancelled"));
        }
        let mut line = String::new();
        match read_line_deadline(&mut reader, &mut line, handshake_deadline, cancel) {
            Ok(()) => {}
            Err(error) => {
                terminate_child_pid(pid);
                return Err(error);
            }
        }
        let msg: WorkerToHost = serde_json::from_str(line.trim_end()).map_err(|error| {
            SshError::new(
                SshErrorKind::Configuration,
                format!("invalid session worker message: {error}"),
            )
        })?;
        match msg {
            WorkerToHost::Progress { stage } => on_progress(stage),
            WorkerToHost::Connected => break,
            WorkerToHost::Failed {
                message,
                stage,
                kind,
            } => {
                terminate_child_pid(pid);
                let mut error = SshError::new(parse_error_kind(kind.as_deref()), message);
                if let Some(stage) = wire_stage_static(stage.as_deref()) {
                    error = error.with_connection_stage(stage);
                }
                return Err(error);
            }
            WorkerToHost::Exited => {
                terminate_child_pid(pid);
                return Err(SshError::new(
                    SshErrorKind::Channel,
                    "session worker exited before connect finished",
                ));
            }
            WorkerToHost::Output { .. } => {
                // Ignore premature output during handshake.
            }
        }
    }

    let (output_tx, output_rx) = mpsc::channel();
    let connected = Arc::new(AtomicBool::new(true));
    let reader_connected = Arc::clone(&connected);
    let reader_thread = thread::Builder::new()
        .name(format!("yshell-sw-read-{session_id}"))
        .spawn(move || reader_loop(reader, output_tx, reader_connected));
    if reader_thread.is_err() {
        connected.store(false, Ordering::SeqCst);
        terminate_child_pid(pid);
        return Err(SshError::new(
            SshErrorKind::Configuration,
            "failed to start session worker reader thread",
        ));
    }

    let _ = writer.set_read_timeout(None);
    Ok((
        WorkerShellSession {
            connected,
            writer: Mutex::new(writer),
            output_rx: Mutex::new(output_rx),
        },
        worker_child,
    ))
}

fn reader_loop(
    mut reader: BufReader<TcpStream>,
    output_tx: Sender<Vec<u8>>,
    connected: Arc<AtomicBool>,
) {
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let Ok(msg) = serde_json::from_str::<WorkerToHost>(line.trim_end()) else {
            continue;
        };
        match msg {
            WorkerToHost::Output { data_b64 } => {
                if let Ok(bytes) = B64.decode(data_b64.as_bytes()) {
                    if output_tx.send(bytes).is_err() {
                        break;
                    }
                }
            }
            WorkerToHost::Exited | WorkerToHost::Failed { .. } => break,
            WorkerToHost::Progress { .. } | WorkerToHost::Connected => {}
        }
    }
    connected.store(false, Ordering::SeqCst);
}

fn spawn_worker_process(
    config_dir: &Path,
    session_id: &str,
    generation: u64,
    ipc_port: u16,
    ipc_token: &str,
) -> Result<Child, SshError> {
    let exe = super::roles::role_exe(super::api::ProcessRole::SessionWorker);
    let mut cmd = Command::new(&exe);
    cmd.arg("--config-dir")
        .arg(config_dir)
        .arg("--session-worker")
        .arg(session_id)
        .arg("--generation")
        .arg(generation.to_string())
        .arg("--parent-pid")
        .arg(current_pid().to_string())
        .arg("--ipc-port")
        .arg(ipc_port.to_string())
        .arg("--ipc-token")
        .arg(ipc_token)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    super::spawn::configure_child_spawn(&mut cmd);
    cmd.spawn().map_err(|error| {
        SshError::new(
            SshErrorKind::Configuration,
            format!(
                "failed to spawn session worker at {}: {error}",
                exe.display()
            ),
        )
    })
}

fn write_msg(writer: &mut impl Write, msg: &HostToWorker) -> AppResult<()> {
    serde_json::to_writer(&mut *writer, msg)
        .map_err(|error| AppError::new(format!("session ipc encode: {error}")))?;
    writer.write_all(b"\n").map_err(AppError::from_error)?;
    writer.flush().map_err(AppError::from_error)?;
    Ok(())
}

fn send_worker_msg(
    writer: &std::sync::Arc<std::sync::Mutex<TcpStream>>,
    msg: &WorkerToHost,
) -> AppResult<()> {
    let mut writer = writer
        .lock()
        .map_err(|_| AppError::new("session worker ipc lock poisoned"))?;
    write_worker_msg(&mut *writer, msg)
}

fn write_worker_msg(writer: &mut impl Write, msg: &WorkerToHost) -> AppResult<()> {
    serde_json::to_writer(&mut *writer, msg)
        .map_err(|error| AppError::new(format!("session ipc encode: {error}")))?;
    writer.write_all(b"\n").map_err(AppError::from_error)?;
    writer.flush().map_err(AppError::from_error)?;
    Ok(())
}

fn read_line_deadline(
    reader: &mut BufReader<TcpStream>,
    line: &mut String,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<(), SshError> {
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(SshError::new(SshErrorKind::Timeout, "connect cancelled"));
        }
        if Instant::now() >= deadline {
            return Err(SshError::new(
                SshErrorKind::Configuration,
                "timed out reading from session worker",
            ));
        }
        line.clear();
        match reader.read_line(line) {
            Ok(0) => {
                return Err(SshError::new(
                    SshErrorKind::Channel,
                    "session worker closed during handshake",
                ));
            }
            Ok(_) => return Ok(()),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(SshError::new(
                    SshErrorKind::Channel,
                    format!("session worker read failed: {error}"),
                ));
            }
        }
    }
}

fn parse_error_kind(kind: Option<&str>) -> SshErrorKind {
    match kind {
        Some("dns") => SshErrorKind::Dns,
        Some("tcp_connect") | Some("tcp-connect") => SshErrorKind::TcpConnect,
        Some("proxy") => SshErrorKind::Proxy,
        Some("handshake") => SshErrorKind::Handshake,
        Some("host_key_rejected") | Some("host-key-rejected") => SshErrorKind::HostKeyRejected,
        Some("authentication") => SshErrorKind::Authentication,
        Some("channel") => SshErrorKind::Channel,
        Some("cancelled") | Some("timeout") => SshErrorKind::Timeout,
        Some("configuration") => SshErrorKind::Configuration,
        Some("io") => SshErrorKind::Io,
        Some("unsupported") => SshErrorKind::Unsupported,
        _ => SshErrorKind::Channel,
    }
}

fn error_kind_wire(kind: SshErrorKind) -> &'static str {
    match kind {
        SshErrorKind::Dns => "dns",
        SshErrorKind::TcpConnect => "tcp_connect",
        SshErrorKind::Proxy => "proxy",
        SshErrorKind::Handshake => "handshake",
        SshErrorKind::HostKeyRejected => "host_key_rejected",
        SshErrorKind::Authentication => "authentication",
        SshErrorKind::Channel => "channel",
        SshErrorKind::Timeout => "timeout",
        SshErrorKind::Configuration => "configuration",
        SshErrorKind::Io => "io",
        SshErrorKind::Unsupported => "unsupported",
    }
}

fn wire_stage_static(stage: Option<&str>) -> Option<&'static str> {
    match stage? {
        "tcp" | "tcp-connect" | "prepared" | "proxy-negotiation" => Some("tcp-connect"),
        "handshake" | "ssh-handshake" => Some("ssh-handshake"),
        "host-key" | "host-key-verification" => Some("host-key-verification"),
        "auth" | "authentication" => Some("authentication"),
        "shell" | "pty-request" | "shell-open" => Some("shell-open"),
        "connected" => Some("connected"),
        "failed" => Some("failed"),
        _ => Some("failed"),
    }
}

fn terminate_child_pid(pid: u32) {
    let _ = super::liveness::terminate_pid(pid);
}

/// Session-worker process entry: dial Host IPC and own the SSH session.
pub fn run_session_worker_ipc(
    _config_dir: PathBuf,
    _session_id: String,
    _generation: u64,
    parent_pid: u32,
    ipc_port: u16,
    ipc_token: String,
) -> AppResult<()> {
    if ipc_port == 0 || ipc_token.is_empty() {
        return Err(AppError::new(
            "session-worker requires --ipc-port and --ipc-token",
        ));
    }

    let stream = TcpStream::connect(("127.0.0.1", ipc_port)).map_err(AppError::from_error)?;
    stream.set_nodelay(true).map_err(AppError::from_error)?;
    let mut writer = stream.try_clone().map_err(AppError::from_error)?;
    writeln!(writer, "{ipc_token}").map_err(AppError::from_error)?;
    writer.flush().map_err(AppError::from_error)?;
    // One lock for every write: the progress thread and the main worker loop
    // share this socket clone, and interleaved writes would corrupt NDJSON.
    let writer = std::sync::Arc::new(std::sync::Mutex::new(writer));
    let mut reader = BufReader::new(stream);

    // First command must be Connect.
    let mut line = String::new();
    reader.read_line(&mut line).map_err(AppError::from_error)?;
    let connect: HostToWorker = serde_json::from_str(line.trim_end())
        .map_err(|error| AppError::new(format!("invalid host connect: {error}")))?;
    let (config, open_shell) = match connect {
        HostToWorker::Connect { config, open_shell } => (config.into_ssh(), open_shell),
        other => {
            return Err(AppError::new(format!(
                "session worker expected Connect, got {other:?}"
            )));
        }
    };

    let (progress_tx, progress_rx) = mpsc::channel::<String>();
    let writer_for_progress = std::sync::Arc::clone(&writer);
    let progress_thread = thread::Builder::new()
        .name("yshell-sw-progress".into())
        .spawn(move || {
            while let Ok(stage) = progress_rx.recv() {
                if send_worker_msg(&writer_for_progress, &WorkerToHost::Progress { stage }).is_err()
                {
                    break;
                }
            }
        });

    let connect_result = with_connection_stage_hook(
        move |stage| {
            let _ = progress_tx.send(crate::connect_jobs::overlay_stage_id(stage).to_owned());
        },
        || {
            let transport = SshTransport::connect(&config)?;
            if !open_shell {
                return Ok((transport, None));
            }
            match transport.open_shell(&config.pty) {
                Ok(shell) => Ok((transport, Some(shell))),
                Err(error) => {
                    transport.close();
                    Err(error)
                }
            }
        },
    );
    drop(progress_thread);

    let (transport, mut shell) = match connect_result {
        Ok(pair) => {
            send_worker_msg(&writer, &WorkerToHost::Connected)?;
            pair
        }
        Err(error) => {
            send_worker_msg(
                &writer,
                &WorkerToHost::Failed {
                    message: error.to_string(),
                    stage: error.connection_stage.map(str::to_owned),
                    kind: Some(error_kind_wire(error.kind).to_owned()),
                },
            )?;
            return Ok(());
        }
    };

    let Some(mut shell) = shell.take() else {
        // SFTP-only should not use this path; close and exit.
        transport.close();
        send_worker_msg(&writer, &WorkerToHost::Exited)?;
        return Ok(());
    };

    // I/O loop: Host commands on reader; shell output polled.
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(20)))
        .map_err(AppError::from_error)?;

    loop {
        // Parent liveness is a cheap secondary check; the Host IPC socket EOF
        // below is the authoritative "Host is gone" signal.
        if parent_pid != 0 && !super::liveness::process_is_alive(parent_pid) {
            break;
        }
        // Drain host commands (non-blocking via read timeout).
        loop {
            let mut cmd_line = String::new();
            match reader.read_line(&mut cmd_line) {
                Ok(0) => {
                    let _ = shell.disconnect();
                    transport.close();
                    let _ = send_worker_msg(&writer, &WorkerToHost::Exited);
                    return Ok(());
                }
                Ok(_) => {
                    let Ok(cmd) = serde_json::from_str::<HostToWorker>(cmd_line.trim_end()) else {
                        continue;
                    };
                    match cmd {
                        HostToWorker::Input { data_b64 } => {
                            if let Ok(bytes) = B64.decode(data_b64.as_bytes()) {
                                if shell.write_input(&bytes).is_err() {
                                    break;
                                }
                            }
                        }
                        HostToWorker::Resize {
                            columns,
                            rows,
                            pixel_width,
                            pixel_height,
                        } => {
                            let _ = shell.resize_pty(PtySize {
                                columns,
                                rows,
                                pixel_width,
                                pixel_height,
                            });
                        }
                        HostToWorker::Disconnect => {
                            let _ = shell.disconnect();
                            transport.close();
                            let _ = send_worker_msg(&writer, &WorkerToHost::Exited);
                            return Ok(());
                        }
                        HostToWorker::Connect { .. } => {}
                    }
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => {
                    let _ = shell.disconnect();
                    transport.close();
                    return Ok(());
                }
            }
        }

        if !shell.is_connected() {
            break;
        }
        match shell.poll_output() {
            Ok(bytes) if !bytes.is_empty() => {
                if send_worker_msg(
                    &writer,
                    &WorkerToHost::Output {
                        data_b64: B64.encode(&bytes),
                    },
                )
                .is_err()
                {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
        thread::sleep(Duration::from_millis(5));
    }

    let _ = shell.disconnect();
    transport.close();
    let _ = send_worker_msg(&writer, &WorkerToHost::Exited);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use yshell_ssh::AuthMethod;

    #[test]
    fn wire_ssh_config_roundtrips_password() {
        let mut config = SshConnectionConfig::new(
            "example.com",
            22,
            AuthMethod::Password {
                username: "u".into(),
                password: "p".into(),
            },
        );
        config.host_key_policy = HostKeyPolicy::AcceptAnyForTesting;
        config.pty.size.columns = 120;
        let wire = WireSshConfig::from(&config);
        let back = wire.into_ssh();
        assert_eq!(back.host, "example.com");
        assert_eq!(back.port, 22);
        assert_eq!(back.pty.size.columns, 120);
        assert_eq!(back.username(), "u");
    }
}
