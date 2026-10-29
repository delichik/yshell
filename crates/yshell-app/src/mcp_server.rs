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
use std::time::{SystemTime, UNIX_EPOCH};

use yshell_mcp::{
    clear_endpoint, handle_connection, write_endpoint, EndpointInfo, IpcRequest, IpcResponse,
    IpcResult, METHOD_REQUEST_CONTROL,
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
        reply: mpsc::Sender<Result<String, String>>,
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
}

impl McpServerHandle {
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

/// Bind loopback TCP, publish `mcp-endpoint.json`, and accept connections.
pub fn start_mcp_server(
    config_dir: PathBuf,
    endpoint_token: String,
    inbox: McpInbox,
) -> AppResult<McpServerHandle> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(AppError::from_error)?;
    let port = listener
        .local_addr()
        .map_err(AppError::from_error)?
        .port();
    let info = EndpointInfo::loopback(port, endpoint_token);
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
                thread::sleep(std::time::Duration::from_millis(50));
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
            Ok(Ok(control_token)) => {
                IpcResponse::ok(id, IpcResult::ControlGranted { control_token })
            }
            Ok(Err(message)) => IpcResponse::err(id, "control_denied", message),
            Err(_) => IpcResponse::err(id, "control_denied", "control request cancelled"),
        };
    }

    let (tx, rx) = mpsc::channel();
    let id = request.id;
    inbox.push(McpJob::Handle {
        request,
        reply: tx,
    });
    rx.recv()
        .unwrap_or_else(|_| IpcResponse::err(id, "unavailable", "MCP UI dispatch failed"))
}
