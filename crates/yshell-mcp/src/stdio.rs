//! Minimal MCP stdio server that forwards tools to the GUI IPC endpoint.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::endpoint::load_endpoint;
use crate::ipc;
use crate::protocol::{
    IpcRequest, IpcResult, OutputMode, METHOD_CONNECT_SAVED, METHOD_DISCONNECT_SESSION,
    METHOD_DUPLICATE_SESSION, METHOD_LIST_CONNECTED, METHOD_LIST_SAVED, METHOD_RELEASE_CONTROL,
    METHOD_REQUEST_CONTROL, METHOD_SESSION_INPUT, METHOD_SESSION_OUTPUT,
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
pub fn run_stdio_server(config_dir: &Path) -> Result<(), StdioServerError> {
    let endpoint = load_endpoint(config_dir).map_err(|error| {
        StdioServerError::Endpoint(format!(
            "YShell GUI MCP endpoint not found at {}: {error}. Start the YShell app first.",
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
    let mut lines = stdin.lock().lines();

    while let Some(line) = lines.next() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(line).map_err(|error| {
            StdioServerError::Protocol(format!("invalid MCP JSON: {error}"))
        })?;
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
                    "serverInfo": { "name": "yshell", "version": env!("CARGO_PKG_VERSION") }
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
                match dispatch_tool(
                    &endpoint,
                    &client_id,
                    &next_id,
                    name,
                    &arguments,
                ) {
                    Ok(text) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": text }],
                            "isError": false
                        }
                    }),
                    Err(message) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": message }],
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

        writeln!(stdout, "{}", serde_json::to_string(&response).map_err(|e| {
            StdioServerError::Protocol(e.to_string())
        })?)?;
        stdout.flush()?;
    }
    Ok(())
}

fn tool_descriptors() -> Vec<Value> {
    vec![
        tool(
            "list_saved_sessions",
            "List saved session profiles from the running YShell app.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "list_connected_sessions",
            "List open terminal tabs/sessions in the running YShell app.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "request_control",
            "Ask the user to grant AI control of YShell (shows a consent dialog unless auto-approve is on). Required before input/output/connect/disconnect/duplicate.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "release_control",
            "Release AI control of YShell after finishing work.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "session_input",
            "Send text to a terminal tab. Requires control lock.",
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
            "Read terminal text. Modes: since_input, last_line, last_n, visible. Requires control lock.",
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
            "connect_saved",
            "Open a saved session profile as a new tab. Requires control lock. Auth prompts still appear in the GUI.",
            json!({
                "type": "object",
                "properties": { "profile_id": { "type": "string" } },
                "required": ["profile_id"]
            }),
        ),
        tool(
            "disconnect_session",
            "Disconnect the session for a tab. Requires control lock.",
            json!({
                "type": "object",
                "properties": { "tab_id": { "type": "string" } },
                "required": ["tab_id"]
            }),
        ),
        tool(
            "duplicate_session",
            "Duplicate a tab's connection into a new tab. Requires control lock.",
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
) -> Result<String, String> {
    let method = match name {
        "list_saved_sessions" => METHOD_LIST_SAVED,
        "list_connected_sessions" => METHOD_LIST_CONNECTED,
        "request_control" => METHOD_REQUEST_CONTROL,
        "release_control" => METHOD_RELEASE_CONTROL,
        "session_input" => METHOD_SESSION_INPUT,
        "session_output" => METHOD_SESSION_OUTPUT,
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
    Ok(format_result(&result))
}

fn format_result(result: &IpcResult) -> String {
    match result {
        IpcResult::Ok => "ok".to_owned(),
        IpcResult::ControlGranted { control_token } => {
            format!("control granted; token={control_token}")
        }
        IpcResult::SavedSessions { sessions } => serde_json::to_string_pretty(sessions)
            .unwrap_or_else(|_| "[]".to_owned()),
        IpcResult::ConnectedSessions { sessions } => serde_json::to_string_pretty(sessions)
            .unwrap_or_else(|_| "[]".to_owned()),
        IpcResult::SessionOutput { text, mode } => {
            format!("mode={mode}\n{text}")
        }
        IpcResult::TabOpened { tab_id, session_id } => {
            format!(
                "tab_id={tab_id}; session_id={}",
                session_id.as_deref().unwrap_or("")
            )
        }
    }
}
