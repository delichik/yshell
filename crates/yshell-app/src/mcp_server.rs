//! Loopback IPC server hosted by the GUI process for `yshell --mcp`.
//!
//! Worker threads never touch `AppRuntime` directly. They enqueue jobs onto
//! [`McpInbox`]; the Slint timer tick drains them on the UI thread.

use std::collections::VecDeque;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use yshell_mcp::{
    clear_endpoint, handle_connection, write_endpoint, EndpointInfo, IpcRequest, IpcResponse,
    IpcResult, METHOD_REQUEST_CONTROL, METHOD_SESSION_RUN,
};

use crate::error::{AppError, AppResult};
use crate::runtime::AppRuntime;

/// Job waiting for the UI thread.
pub enum McpJob {
    Handle {
        request: IpcRequest,
        reply: mpsc::Sender<IpcResponse>,
    },
    BeginControl {
        client_id: String,
        endpoint_token: String,
        reply: mpsc::Sender<Result<(), String>>,
    },
    ReleaseClient {
        client_id: String,
    },
}

/// Shared inbox drained from the window timer tick.
#[derive(Clone, Default)]
pub struct McpInbox {
    inner: Arc<Mutex<VecDeque<McpJob>>>,
}

impl McpInbox {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, job: McpJob) {
        if let Ok(mut queue) = self.inner.lock() {
            queue.push_back(job);
        }
    }

    /// Process all pending jobs on the UI thread.
    pub fn drain(&self, runtime: &mut AppRuntime) {
        let jobs: Vec<McpJob> = {
            let Ok(mut queue) = self.inner.lock() else {
                return;
            };
            queue.drain(..).collect()
        };
        for job in jobs {
            match job {
                McpJob::Handle { request, reply } => {
                    let response = runtime.handle_mcp_ipc(request);
                    let _ = reply.send(response);
                }
                McpJob::BeginControl {
                    client_id,
                    endpoint_token,
                    reply,
                } => {
                    if !runtime
                        .mcp_endpoint_token
                        .as_deref()
                        .is_some_and(|expected| expected == endpoint_token)
                    {
                        let _ = reply.send(Err("invalid MCP endpoint token".to_owned()));
                        continue;
                    }
                    if !runtime.mcp_enabled() {
                        let _ = reply.send(Err("MCP is disabled in Settings → MCP".to_owned()));
                        continue;
                    }
                    runtime.mcp_begin_request_control(client_id, reply);
                }
                McpJob::ReleaseClient { client_id } => {
                    runtime.release_mcp_control_for_client(&client_id);
                }
            }
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .map(|queue| queue.is_empty())
            .unwrap_or(true)
    }
}

/// Stops the accept loop and clears the endpoint file.
pub struct McpServerHandle {
    shutdown: Arc<AtomicBool>,
    config_dir: PathBuf,
    pub port: u16,
    /// Bind host used at start (`127.0.0.1` or `0.0.0.0`).
    pub bind_host: String,
    /// Endpoint token published for this listener.
    pub token: String,
}

impl McpServerHandle {
    /// Whether this running listener still matches the desired bind/token.
    #[must_use]
    pub fn matches(&self, bind_host: &str, port: u16, token: &str) -> bool {
        self.bind_host == bind_host && self.port == port && self.token == token
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = clear_endpoint(&self.config_dir);
    }
}

impl Drop for McpServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Bind TCP on `bind_host:port`, publish `mcp-endpoint.json`, and accept connections.
///
/// `endpoint_host` is what `yshell --mcp` connects to (normally `127.0.0.1` even
/// when the acceptor listens on `0.0.0.0` for LAN).
pub fn start_mcp_server(
    config_dir: PathBuf,
    bind_host: &str,
    port: u16,
    endpoint_host: &str,
    endpoint_token: String,
    inbox: McpInbox,
) -> AppResult<McpServerHandle> {
    let listener = TcpListener::bind((bind_host, port)).map_err(AppError::from_error)?;
    let bound_port = listener.local_addr().map_err(AppError::from_error)?.port();
    let info = EndpointInfo {
        host: endpoint_host.to_owned(),
        port: bound_port,
        token: endpoint_token.clone(),
    };
    write_endpoint(&config_dir, &info).map_err(AppError::from_error)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let accept_shutdown = Arc::clone(&shutdown);
    thread::Builder::new()
        .name("yshell-mcp-accept".into())
        .spawn(move || accept_loop(listener, accept_shutdown, inbox))
        .map_err(AppError::from_error)?;

    Ok(McpServerHandle {
        shutdown,
        config_dir,
        port: bound_port,
        bind_host: bind_host.to_owned(),
        token: endpoint_token,
    })
}

/// Mint a fresh endpoint token for this GUI session.
#[must_use]
pub fn new_endpoint_token() -> String {
    format!(
        "tok-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    )
}

fn accept_loop(listener: TcpListener, shutdown: Arc<AtomicBool>, inbox: McpInbox) {
    if let Err(error) = listener.set_nonblocking(true) {
        tracing::warn!(error = %error, "mcp listener nonblocking failed");
        return;
    }
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let inbox = inbox.clone();
                let shutdown = Arc::clone(&shutdown);
                let _ = thread::Builder::new()
                    .name("yshell-mcp-conn".into())
                    .spawn(move || {
                        let last_client = Arc::new(Mutex::new(None::<String>));
                        let last_client_for_handler = Arc::clone(&last_client);
                        let inbox_for_handler = inbox.clone();
                        let result = handle_connection(stream, move |request| {
                            if let Ok(mut guard) = last_client_for_handler.lock() {
                                *guard = Some(request.client_id.clone());
                            }
                            dispatch_via_inbox(&inbox_for_handler, request)
                        });
                        if let Ok(guard) = last_client.lock() {
                            if let Some(client_id) = guard.clone() {
                                inbox.push(McpJob::ReleaseClient { client_id });
                            }
                        }
                        if let Err(error) = result {
                            tracing::debug!(error = %error, "mcp connection closed");
                        }
                        let _ = shutdown;
                    });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                tracing::warn!(error = %error, "mcp accept failed");
                break;
            }
        }
    }
}

fn dispatch_via_inbox(inbox: &McpInbox, request: IpcRequest) -> IpcResponse {
    if request.method == METHOD_REQUEST_CONTROL {
        let (tx, rx) = mpsc::channel();
        let id = request.id;
        inbox.push(McpJob::BeginControl {
            client_id: request.client_id,
            endpoint_token: request.token,
            reply: tx,
        });
        return match rx.recv() {
            Ok(Ok(())) => IpcResponse::ok(id, IpcResult::ControlGranted),
            Ok(Err(message)) => IpcResponse::err(id, "control_denied", message),
            Err(_) => IpcResponse::err(id, "control_denied", "control request cancelled"),
        };
    }

    if request.method == METHOD_SESSION_RUN {
        return dispatch_session_run(inbox, request);
    }

    let (tx, rx) = mpsc::channel();
    let id = request.id;
    inbox.push(McpJob::Handle { request, reply: tx });
    rx.recv()
        .unwrap_or_else(|_| IpcResponse::err(id, "unavailable", "MCP UI dispatch failed"))
}

fn sync_handle(inbox: &McpInbox, request: IpcRequest) -> IpcResponse {
    let (tx, rx) = mpsc::channel();
    let id = request.id;
    inbox.push(McpJob::Handle { request, reply: tx });
    rx.recv()
        .unwrap_or_else(|_| IpcResponse::err(id, "unavailable", "MCP UI dispatch failed"))
}

/// Wait loop runs on the IPC worker; each poll/write is a UI-thread job.
fn dispatch_session_run(inbox: &McpInbox, request: IpcRequest) -> IpcResponse {
    let id = request.id;
    let tab_id = request
        .params
        .get("tab_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    let text = request
        .params
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    let append_newline = request
        .params
        .get("append_newline")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let timeout_ms = request
        .params
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(15_000)
        .clamp(100, 120_000);
    let until_quiet_ms = request
        .params
        .get("until_quiet_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(400)
        .min(60_000);
    let until_regex = request
        .params
        .get("until_regex")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .filter(|s| !s.is_empty());

    if !text.is_empty() {
        let write = IpcRequest {
            id: request.id.wrapping_add(1),
            method: yshell_mcp::METHOD_SESSION_INPUT.to_owned(),
            params: serde_json::json!({
                "tab_id": tab_id,
                "text": text,
                "append_newline": append_newline,
            }),
            token: request.token.clone(),
            client_id: request.client_id.clone(),
        };
        let response = sync_handle(inbox, write);
        if let Some(error) = response.error {
            return IpcResponse::err(id, error.code, error.message);
        }
    }

    let regex = match until_regex.as_deref() {
        Some(pattern) => match regex::Regex::new(pattern) {
            Ok(re) => Some(re),
            Err(error) => {
                return IpcResponse::err(id, "invalid_regex", error.to_string());
            }
        },
        None => None,
    };

    let timeout = Duration::from_millis(timeout_ms);
    let quiet = Duration::from_millis(until_quiet_ms);
    let start = Instant::now();
    let mut last_text = String::new();
    let mut last_change = Instant::now();
    let mut saw_output = false;
    let mut timed_out = false;
    let mut poll_id = request.id.wrapping_add(2);

    // Brief settle after write so the first poll is not empty-by-race.
    if !text.is_empty() {
        thread::sleep(Duration::from_millis(50));
    }

    loop {
        let poll = IpcRequest {
            id: poll_id,
            method: yshell_mcp::METHOD_SESSION_OUTPUT.to_owned(),
            params: serde_json::json!({
                "tab_id": tab_id,
                "mode": if text.is_empty() { "visible" } else { "since_input" },
            }),
            token: request.token.clone(),
            client_id: request.client_id.clone(),
        };
        poll_id = poll_id.wrapping_add(1);
        let response = sync_handle(inbox, poll);
        if let Some(error) = response.error {
            return IpcResponse::err(id, error.code, error.message);
        }
        let text_now = match response.result {
            Some(IpcResult::SessionOutput { text, .. }) => text,
            other => {
                return IpcResponse::err(
                    id,
                    "output_failed",
                    format!("unexpected poll result: {other:?}"),
                );
            }
        };
        if text_now != last_text {
            if !text_now.is_empty() {
                saw_output = true;
            }
            last_text = text_now;
            last_change = Instant::now();
        }

        if let Some(re) = regex.as_ref() {
            if re.is_match(&last_text) {
                break;
            }
        } else if until_quiet_ms > 0
            && (saw_output || text.is_empty())
            && last_change.elapsed() >= quiet
        {
            // Quiet after change, or (no input) after one settle window.
            if saw_output || start.elapsed() >= quiet {
                break;
            }
        }

        if start.elapsed() >= timeout {
            timed_out = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let info = IpcRequest {
        id: poll_id,
        method: yshell_mcp::METHOD_GET_SESSION.to_owned(),
        params: serde_json::json!({ "tab_id": tab_id }),
        token: request.token.clone(),
        client_id: request.client_id.clone(),
    };
    let (state, blocker) = match sync_handle(inbox, info).result {
        Some(IpcResult::SessionInfo { session }) => (session.state, session.blocker),
        _ => ("unknown".to_owned(), "none".to_owned()),
    };

    IpcResponse::ok(
        id,
        IpcResult::SessionRun {
            text: last_text,
            timed_out,
            state,
            blocker,
            tab_id,
        },
    )
}
