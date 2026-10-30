//! MCP stdio bridge and local IPC protocol for controlling a running YShell GUI.
//!
//! The GUI process hosts a loopback TCP endpoint described by
//! [`endpoint_path`] when the user enables MCP in Settings. `yshell --mcp` is
//! the host-facing MCP server entrypoint (stdio JSON-RPC); it forwards tool
//! calls to that endpoint. Users do not run `--mcp` by hand.

mod endpoint;
mod ipc;
mod protocol;
mod stdio;

pub use endpoint::{
    clear_endpoint, endpoint_path, load_endpoint, write_endpoint, EndpointInfo, ENDPOINT_FILE_NAME,
};
pub use ipc::{handle_connection, read_message, write_message, IpcError};
pub use protocol::{
    ConnectedSessionInfo, IpcErrorBody, IpcRequest, IpcResponse, IpcResult, OutputMode,
    SavedSessionInfo, MCP_BOUNDARIES_TEXT, MCP_HOST_CONFIG_SNIPPET, METHOD_CONNECT_SAVED,
    METHOD_DISCONNECT_SESSION, METHOD_DUPLICATE_SESSION, METHOD_GET_MCP_INFO, METHOD_GET_SESSION,
    METHOD_LIST_CONNECTED, METHOD_LIST_SAVED, METHOD_LIST_SESSIONS, METHOD_RELEASE_CONTROL,
    METHOD_REQUEST_CONTROL, METHOD_SESSION_INPUT, METHOD_SESSION_OUTPUT, METHOD_SESSION_RUN,
    METHOD_SESSION_SEND_KEY,
};
pub use stdio::{run_stdio_server, StdioServerError};
