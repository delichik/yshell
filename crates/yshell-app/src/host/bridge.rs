//! Loopback bridge: out-of-process MCP daemon and second-instance Shell attach
//! forward into the Host ([`McpInbox`] / [`ShellInbox`], drained on the UI tick).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use yshell_mcp::{IpcRequest, IpcResponse};

use crate::error::{AppError, AppResult};
use crate::mcp_server::{McpInbox, McpJob};

use super::liveness::current_pid;
use super::shell_inbox::{ShellCommand, ShellInbox};
use super::shell_jobs::ShellJobInbox;

pub const HOST_BRIDGE_FILE_NAME: &str = "host-bridge.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostBridgeInfo {
    pub host: String,
    pub port: u16,
    pub token: String,
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
enum BridgeRequest {
    McpHandle {
        request: IpcRequest,
    },
    McpBeginControl {
        client_id: String,
        endpoint_token: String,
    },
    McpReleaseClient {
        client_id: String,
    },
    /// Second `yshell` launch: ask the running Shell to open/focus a window.
    OpenWindow,
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
enum BridgeResponse {
    McpHandle { response: IpcResponse },
    McpBeginControl { ok: bool, error: Option<String> },
    McpReleaseClient { ok: bool },
    OpenWindow { ok: bool },
    Pong { generation: u64 },
    Err { message: String },
}

/// Accepts bridge connections from `--mcp-daemon` and enqueues [`McpJob`]s.
pub struct HostBridgeHandle {
    shutdown: Arc<AtomicBool>,
    config_dir: PathBuf,
    pub info: HostBridgeInfo,
}

impl HostBridgeHandle {
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = clear_host_bridge(&self.config_dir);
        // Wake accept loop.
        let _ = TcpStream::connect((self.info.host.as_str(), self.info.port));
    }
}

impl Drop for HostBridgeHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn start_host_bridge(
    config_dir: PathBuf,
    generation: u64,
    inbox: McpInbox,
    shell_inbox: ShellInbox,
    shell_jobs: ShellJobInbox,
) -> AppResult<HostBridgeHandle> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(AppError::from_error)?;
    let port = listener.local_addr().map_err(AppError::from_error)?.port();
    let token = format!(
        "host-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let info = HostBridgeInfo {
        host: "127.0.0.1".to_owned(),
        port,
        token: token.clone(),
        generation,
    };
    write_host_bridge(&config_dir, &info).map_err(AppError::from_error)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let shared = BridgeConnShared {
        shutdown: Arc::clone(&shutdown),
        expected_token: token,
        inbox,
        shell_inbox,
        shell_jobs,
        generation,
        host_pid: current_pid(),
    };
    thread::Builder::new()
        .name("yshell-host-bridge".into())
        .spawn(move || accept_loop(listener, shared))
        .map_err(AppError::from_error)?;

    Ok(HostBridgeHandle {
        shutdown,
        config_dir,
        info,
    })
}

#[derive(Clone)]
struct BridgeConnShared {
    shutdown: Arc<AtomicBool>,
    expected_token: String,
    inbox: McpInbox,
    shell_inbox: ShellInbox,
    shell_jobs: ShellJobInbox,
    generation: u64,
    host_pid: u32,
}

fn accept_loop(listener: TcpListener, shared: BridgeConnShared) {
    let _ = listener.set_nonblocking(true);
    while !shared.shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let shared = shared.clone();
                let _ = thread::Builder::new()
                    .name("yshell-host-bridge-conn".into())
                    .spawn(move || {
                        if let Err(error) = handle_bridge_conn(stream, &shared) {
                            if !shared.shutdown.load(Ordering::SeqCst) {
                                tracing::debug!(error = %error, "host bridge connection ended");
                            }
                        }
                    });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                tracing::warn!(error = %error, "host bridge accept failed");
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn handle_bridge_conn(stream: TcpStream, shared: &BridgeConnShared) -> AppResult<()> {
    // The listener is non-blocking; accepted sockets inherit that on Windows,
    // which would make the NDJSON reader spin on WouldBlock forever.
    let _ = stream.set_nonblocking(false);
    let expected_token = shared.expected_token.as_str();
    let inbox = &shared.inbox;
    let shell_inbox = &shared.shell_inbox;
    let shell_jobs = &shared.shell_jobs;
    let generation = shared.generation;
    let host_pid = shared.host_pid;
    stream.set_nodelay(true).map_err(AppError::from_error)?;
    stream
        .set_read_timeout(None)
        .map_err(AppError::from_error)?;
    let mut reader = BufReader::new(stream.try_clone().map_err(AppError::from_error)?);
    let mut writer = stream;

    // First line: auth token.
    let mut auth = String::new();
    reader.read_line(&mut auth).map_err(AppError::from_error)?;
    if auth.trim_end() != expected_token {
        write_bridge(
            &mut writer,
            &BridgeResponse::Err {
                message: "invalid host bridge token".to_owned(),
            },
        )?;
        return Err(AppError::new("invalid host bridge token"));
    }

    // Second line: optional role. `shell` → Shell IPC; otherwise treat as MCP JSON.
    let mut role_or_req = String::new();
    let n = reader
        .read_line(&mut role_or_req)
        .map_err(AppError::from_error)?;
    if n == 0 {
        return Ok(());
    }
    if role_or_req.trim_end() == "shell" {
        // Hand the already-authenticated connection (including any bytes the
        // BufReader read ahead of the role line) to Shell IPC. Do not rebuild
        // the stream: `BufReader::into_inner` would drop a pipelined request.
        return super::shell_ipc::handle_shell_stream(
            reader,
            writer,
            shell_jobs.clone(),
            shell_inbox.clone(),
            generation,
            host_pid,
        );
    }

    // MCP path: first payload line already read.
    let mut first = Some(role_or_req);
    loop {
        let line = if let Some(line) = first.take() {
            line
        } else {
            let mut line = String::new();
            let n = reader.read_line(&mut line).map_err(AppError::from_error)?;
            if n == 0 {
                break;
            }
            line
        };
        let request: BridgeRequest = serde_json::from_str(line.trim_end())
            .map_err(|error| AppError::new(format!("invalid host bridge request: {error}")))?;
        let response = match request {
            BridgeRequest::McpHandle { request } => {
                let (tx, rx) = mpsc::channel();
                inbox.push(McpJob::Handle { request, reply: tx });
                match rx.recv_timeout(Duration::from_secs(600)) {
                    Ok(response) => BridgeResponse::McpHandle { response },
                    Err(_) => BridgeResponse::Err {
                        message: "timed out waiting for Host MCP handler".to_owned(),
                    },
                }
            }
            BridgeRequest::McpBeginControl {
                client_id,
                endpoint_token,
            } => {
                let (tx, rx) = mpsc::channel();
                inbox.push(McpJob::BeginControl {
                    client_id,
                    endpoint_token,
                    reply: tx,
                });
                match rx.recv_timeout(Duration::from_secs(600)) {
                    Ok(Ok(())) => BridgeResponse::McpBeginControl {
                        ok: true,
                        error: None,
                    },
                    Ok(Err(error)) => BridgeResponse::McpBeginControl {
                        ok: false,
                        error: Some(error),
                    },
                    Err(_) => BridgeResponse::McpBeginControl {
                        ok: false,
                        error: Some("timed out waiting for MCP control consent".to_owned()),
                    },
                }
            }
            BridgeRequest::McpReleaseClient { client_id } => {
                inbox.push(McpJob::ReleaseClient { client_id });
                BridgeResponse::McpReleaseClient { ok: true }
            }
            BridgeRequest::OpenWindow => {
                shell_inbox.push(ShellCommand::OpenWindow);
                BridgeResponse::OpenWindow { ok: true }
            }
            BridgeRequest::Ping => BridgeResponse::Pong { generation },
        };
        write_bridge(&mut writer, &response)?;
    }
    Ok(())
}

fn write_bridge(writer: &mut impl Write, response: &BridgeResponse) -> AppResult<()> {
    serde_json::to_writer(&mut *writer, response)
        .map_err(|error| AppError::new(format!("host bridge encode failed: {error}")))?;
    writer.write_all(b"\n").map_err(AppError::from_error)?;
    writer.flush().map_err(AppError::from_error)?;
    Ok(())
}

pub fn write_host_bridge(config_dir: &Path, info: &HostBridgeInfo) -> std::io::Result<PathBuf> {
    fs::create_dir_all(config_dir)?;
    let path = config_dir.join(HOST_BRIDGE_FILE_NAME);
    let body = serde_json::to_vec_pretty(info)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(&path, body)?;
    Ok(path)
}

pub fn load_host_bridge(config_dir: &Path) -> std::io::Result<HostBridgeInfo> {
    let path = config_dir.join(HOST_BRIDGE_FILE_NAME);
    let bytes = fs::read(&path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

pub fn clear_host_bridge(config_dir: &Path) -> std::io::Result<()> {
    let path = config_dir.join(HOST_BRIDGE_FILE_NAME);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Client used by `--mcp-daemon` to forward jobs into the Host.
pub struct HostBridgeClient {
    info: HostBridgeInfo,
}

impl HostBridgeClient {
    pub fn connect(config_dir: &Path) -> AppResult<Self> {
        let info = load_host_bridge(config_dir).map_err(AppError::from_error)?;
        Ok(Self { info })
    }

    #[must_use]
    pub fn info(&self) -> &HostBridgeInfo {
        &self.info
    }

    fn open(&self) -> AppResult<(BufReader<TcpStream>, TcpStream)> {
        let stream = TcpStream::connect((self.info.host.as_str(), self.info.port))
            .map_err(AppError::from_error)?;
        stream.set_nodelay(true).map_err(AppError::from_error)?;
        let mut writer = stream.try_clone().map_err(AppError::from_error)?;
        writeln!(writer, "{}", self.info.token).map_err(AppError::from_error)?;
        writer.flush().map_err(AppError::from_error)?;
        Ok((BufReader::new(stream), writer))
    }

    pub fn mcp_handle(&self, request: IpcRequest) -> AppResult<IpcResponse> {
        let (mut reader, mut writer) = self.open()?;
        serde_json::to_writer(&mut writer, &BridgeRequest::McpHandle { request })
            .map_err(|error| AppError::new(format!("bridge encode: {error}")))?;
        writer.write_all(b"\n").map_err(AppError::from_error)?;
        writer.flush().map_err(AppError::from_error)?;
        let mut line = String::new();
        reader.read_line(&mut line).map_err(AppError::from_error)?;
        let response: BridgeResponse = serde_json::from_str(line.trim_end())
            .map_err(|error| AppError::new(format!("bridge decode: {error}")))?;
        match response {
            BridgeResponse::McpHandle { response } => Ok(response),
            BridgeResponse::Err { message } => Err(AppError::new(message)),
            other => Err(AppError::new(format!(
                "unexpected bridge response: {other:?}"
            ))),
        }
    }

    pub fn mcp_begin_control(&self, client_id: String, endpoint_token: String) -> AppResult<()> {
        let (mut reader, mut writer) = self.open()?;
        serde_json::to_writer(
            &mut writer,
            &BridgeRequest::McpBeginControl {
                client_id,
                endpoint_token,
            },
        )
        .map_err(|error| AppError::new(format!("bridge encode: {error}")))?;
        writer.write_all(b"\n").map_err(AppError::from_error)?;
        writer.flush().map_err(AppError::from_error)?;
        let mut line = String::new();
        reader.read_line(&mut line).map_err(AppError::from_error)?;
        let response: BridgeResponse = serde_json::from_str(line.trim_end())
            .map_err(|error| AppError::new(format!("bridge decode: {error}")))?;
        match response {
            BridgeResponse::McpBeginControl { ok: true, .. } => Ok(()),
            BridgeResponse::McpBeginControl {
                ok: false,
                error: Some(message),
            } => Err(AppError::new(message)),
            BridgeResponse::McpBeginControl {
                ok: false,
                error: None,
            } => Err(AppError::new("MCP control denied")),
            BridgeResponse::Err { message } => Err(AppError::new(message)),
            other => Err(AppError::new(format!(
                "unexpected bridge response: {other:?}"
            ))),
        }
    }

    pub fn mcp_release_client(&self, client_id: String) -> AppResult<()> {
        let (mut reader, mut writer) = self.open()?;
        serde_json::to_writer(&mut writer, &BridgeRequest::McpReleaseClient { client_id })
            .map_err(|error| AppError::new(format!("bridge encode: {error}")))?;
        writer.write_all(b"\n").map_err(AppError::from_error)?;
        writer.flush().map_err(AppError::from_error)?;
        let mut line = String::new();
        reader.read_line(&mut line).map_err(AppError::from_error)?;
        let _response: BridgeResponse = serde_json::from_str(line.trim_end())
            .map_err(|error| AppError::new(format!("bridge decode: {error}")))?;
        Ok(())
    }

    pub fn open_window(&self) -> AppResult<()> {
        let (mut reader, mut writer) = self.open()?;
        serde_json::to_writer(&mut writer, &BridgeRequest::OpenWindow)
            .map_err(|error| AppError::new(format!("bridge encode: {error}")))?;
        writer.write_all(b"\n").map_err(AppError::from_error)?;
        writer.flush().map_err(AppError::from_error)?;
        let mut line = String::new();
        reader.read_line(&mut line).map_err(AppError::from_error)?;
        let response: BridgeResponse = serde_json::from_str(line.trim_end())
            .map_err(|error| AppError::new(format!("bridge decode: {error}")))?;
        match response {
            BridgeResponse::OpenWindow { ok: true } => Ok(()),
            BridgeResponse::Err { message } => Err(AppError::new(message)),
            other => Err(AppError::new(format!(
                "unexpected bridge response: {other:?}"
            ))),
        }
    }
}

/// If a live Host already owns this config dir, ask it to open a window and
/// return `Ok(true)` so the new process can exit. Otherwise `Ok(false)`.
///
/// Liveness is the bridge itself: connecting and authenticating proves a Host
/// is serving this config dir (and its token), so no pid file is needed.
pub fn try_notify_existing_instance(config_dir: &Path) -> AppResult<bool> {
    let Ok(client) = HostBridgeClient::connect(config_dir) else {
        return Ok(false);
    };
    match client.open_window() {
        Ok(()) => {
            tracing::info!("notified existing yshell instance to open a window");
            Ok(true)
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "existing instance bridge unreachable; starting a new Host"
            );
            Ok(false)
        }
    }
}
