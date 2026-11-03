//! Minimal MCP stdio server that forwards tools to the GUI IPC endpoint.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::endpoint::load_endpoint;
use crate::ipc;
use crate::protocol::{
    IpcRequest, IpcResult, OutputMode, MCP_BOUNDARIES_TEXT, METHOD_CONNECT_SAVED,
    METHOD_DISCONNECT_SESSION, METHOD_DUPLICATE_SESSION, METHOD_GET_MCP_INFO, METHOD_GET_SESSION,
    METHOD_LIST_SAVED, METHOD_LIST_SESSIONS, METHOD_RELEASE_CONTROL, METHOD_REQUEST_CONTROL,
    METHOD_SESSION_INPUT, METHOD_SESSION_OUTPUT, METHOD_SESSION_RUN, METHOD_SESSION_SEND_KEY,
};

#[derive(Debug)]
pub enum StdioServerError {
    Io(io::Error),
    Endpoint(String),
    Protocol(String),
}

impl std::fmt::Display for StdioServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Endpoint(message) | Self::Protocol(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for StdioServerError {}

impl From<io::Error> for StdioServerError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// Run an MCP server on stdin/stdout until EOF.
///
/// This is the host-facing entrypoint (`yshell --mcp`). The YShell GUI must be
/// running with MCP enabled in Settings, or endpoint discovery fails clearly.
pub fn run_stdio_server(config_dir: &Path) -> Result<(), StdioServerError> {
    let endpoint = load_endpoint(config_dir).map_err(|error| {
        StdioServerError::Endpoint(format!(
            "YShell MCP endpoint not found at {} ({error}). Open the YShell app and turn on MCP in Settings → AI / MCP, then retry.",
            crate::endpoint_path(config_dir).display()
        ))
    })?;
    let client_id = format!(
        "mcp-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let next_id = AtomicU64::new(1);

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let lines = stdin.lock().lines();

    for line in lines {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(line)
            .map_err(|error| StdioServerError::Protocol(format!("invalid MCP JSON: {error}")))?;
        if message.get("method").and_then(Value::as_str) == Some("notifications/initialized")
            || message.get("method").and_then(Value::as_str) == Some("notifications/cancelled")
        {
            continue;
        }
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "yshell",
                        "version": env!("CARGO_PKG_VERSION"),
                        "description": format!(
                            "YShell terminal control via the running GUI. {}",
                            MCP_BOUNDARIES_TEXT
                        )
                    }
                }
            }),
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": tool_descriptors() }
            }),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match dispatch_tool(&endpoint, &client_id, &next_id, name, &arguments) {
                    Ok(value) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": value.to_string() }],
                            "isError": false
                        }
                    }),
                    Err(message) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{
                                "type": "text",
                                "text": json!({ "error": message }).to_string()
                            }],
                            "isError": true
                        }
                    }),
                }
            }
            other if !other.is_empty() => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {other}") }
            }),
            _ => continue,
        };

        writeln!(
            stdout,
            "{}",
            serde_json::to_string(&response)
                .map_err(|e| { StdioServerError::Protocol(e.to_string()) })?
        )?;
        stdout.flush()?;
    }
    Ok(())
}

fn tool_descriptors() -> Vec<Value> {
    vec![
        tool(
            "get_mcp_info",
            "YShell MCP status, product boundaries, and host config hint. No control required. Terminal sessions only — not SFTP/tunnels; passwords and host keys stay in the GUI.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "list_saved_sessions",
            "List saved session profiles from the running YShell app.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "list_sessions",
            "List open terminal tabs. Each entry includes state and blocker (none|password|host_key|connecting|…). SFTP tabs are out of scope.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "list_connected_sessions",
            "Alias of list_sessions.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "get_session",
            "Get one terminal tab by tab_id, including state and blocker.",
            json!({
                "type": "object",
                "properties": { "tab_id": { "type": "string" } },
                "required": ["tab_id"]
            }),
        ),
        tool(
            "request_control",
            "Ask the user to grant AI control (consent dialog unless auto-approve is on). Required before mutating tools.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "release_control",
            "Release AI control after finishing work.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "session_run",
            "Primary primitive: optionally send text (append newline by default), then wait until quiet, regex match, or timeout. Returns JSON text/timed_out/state/blocker. Requires control.",
            json!({
                "type": "object",
                "properties": {
                    "tab_id": { "type": "string" },
                    "text": { "type": "string" },
                    "append_newline": { "type": "boolean", "default": true },
                    "timeout_ms": { "type": "integer", "minimum": 100, "default": 15000 },
                    "until_quiet_ms": { "type": "integer", "minimum": 0, "default": 400 },
                    "until_regex": { "type": "string" }
                },
                "required": ["tab_id"]
            }),
        ),
        tool(
            "session_input",
            "Send text to a terminal tab without waiting (advanced). Prefer session_run. Requires control. Does not steal focus sync broadcast.",
            json!({
                "type": "object",
                "properties": {
                    "tab_id": { "type": "string" },
                    "text": { "type": "string" },
                    "append_newline": { "type": "boolean", "default": true }
                },
                "required": ["tab_id", "text"]
            }),
        ),
        tool(
            "session_output",
            "One-shot terminal read (advanced; no wait). Modes: since_input, last_line, last_n, visible. Prefer session_run. Requires control.",
            json!({
                "type": "object",
                "properties": {
                    "tab_id": { "type": "string" },
                    "mode": {
                        "type": "string",
                        "enum": ["since_input", "last_line", "last_n", "visible"]
                    },
                    "n": { "type": "integer", "minimum": 1 }
                },
                "required": ["tab_id", "mode"]
            }),
        ),
        tool(
            "session_send_key",
            "Send a key to a tab: enter, tab, escape, backspace, up/down/left/right, or ctrl+c / ctrl+d / alt+…. Requires control.",
            json!({
                "type": "object",
                "properties": {
                    "tab_id": { "type": "string" },
                    "key": { "type": "string" }
                },
                "required": ["tab_id", "key"]
            }),
        ),
        tool(
            "connect_saved",
            "Open a saved profile as a new tab. Returns tab_id plus state/blocker — password and host-key prompts stay in the GUI. Requires control.",
            json!({
                "type": "object",
                "properties": { "profile_id": { "type": "string" } },
                "required": ["profile_id"]
            }),
        ),
        tool(
            "disconnect_session",
            "Disconnect the session for a tab. Requires control.",
            json!({
                "type": "object",
                "properties": { "tab_id": { "type": "string" } },
                "required": ["tab_id"]
            }),
        ),
        tool(
            "duplicate_session",
            "Duplicate a tab's connection into a new tab. Returns state/blocker. Requires control.",
            json!({
                "type": "object",
                "properties": { "tab_id": { "type": "string" } },
                "required": ["tab_id"]
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn dispatch_tool(
    endpoint: &crate::EndpointInfo,
    client_id: &str,
    next_id: &AtomicU64,
    name: &str,
    arguments: &Value,
) -> Result<Value, String> {
    let method = match name {
        "get_mcp_info" => METHOD_GET_MCP_INFO,
        "list_saved_sessions" => METHOD_LIST_SAVED,
        "list_sessions" | "list_connected_sessions" => METHOD_LIST_SESSIONS,
        "get_session" => METHOD_GET_SESSION,
        "request_control" => METHOD_REQUEST_CONTROL,
        "release_control" => METHOD_RELEASE_CONTROL,
        "session_input" => METHOD_SESSION_INPUT,
        "session_output" => METHOD_SESSION_OUTPUT,
        "session_run" => METHOD_SESSION_RUN,
        "session_send_key" => METHOD_SESSION_SEND_KEY,
        "connect_saved" => METHOD_CONNECT_SAVED,
        "disconnect_session" => METHOD_DISCONNECT_SESSION,
        "duplicate_session" => METHOD_DUPLICATE_SESSION,
        other => return Err(format!("unknown tool: {other}")),
    };

    if name == "session_output" {
        let mode = arguments
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if OutputMode::parse(mode).is_none() {
            return Err(format!("invalid mode `{mode}`"));
        }
    }

    let request = IpcRequest {
        id: next_id.fetch_add(1, Ordering::SeqCst),
        method: method.to_owned(),
        params: arguments.clone(),
        token: endpoint.token.clone(),
        client_id: client_id.to_owned(),
    };
    let response = ipc::call(endpoint, &request).map_err(|error| error.to_string())?;
    if let Some(error) = response.error {
        return Err(format!("{}: {}", error.code, error.message));
    }
    let Some(result) = response.result else {
        return Err("empty IPC result".to_owned());
    };
    Ok(result_json(&result))
}

fn result_json(result: &IpcResult) -> Value {
    match result {
        IpcResult::Ok => json!({ "ok": true }),
        IpcResult::ControlGranted => json!({ "ok": true, "control": true }),
        IpcResult::McpInfo {
            enabled,
            status,
            control,
            boundaries,
            host_config,
        } => json!({
            "enabled": enabled,
            "status": status,
            "control": control,
            "boundaries": boundaries,
            "host_config": host_config,
        }),
        IpcResult::SavedSessions { sessions } => json!({ "sessions": sessions }),
        IpcResult::ConnectedSessions { sessions } => json!({ "sessions": sessions }),
        IpcResult::SessionInfo { session } => json!({ "session": session }),
        IpcResult::SessionOutput { text, mode } => json!({ "text": text, "mode": mode }),
        IpcResult::SessionRun {
            text,
            timed_out,
            state,
            blocker,
            tab_id,
        } => json!({
            "text": text,
            "timed_out": timed_out,
            "state": state,
            "blocker": blocker,
            "tab_id": tab_id,
        }),
        IpcResult::TabOpened {
            tab_id,
            session_id,
            state,
            blocker,
        } => json!({
            "tab_id": tab_id,
            "session_id": session_id,
            "state": state,
            "blocker": blocker,
        }),
    }
}
