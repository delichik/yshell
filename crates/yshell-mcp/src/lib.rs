//! MCP stdio bridge and local IPC protocol for controlling a running YShell GUI.
//!
//! The GUI process hosts a loopback TCP endpoint described by
//! [`endpoint_path`]. `yshell --mcp` speaks MCP over stdio and forwards tool
//! calls to that endpoint.

mod endpoint;
mod ipc;
mod protocol;
mod stdio;

pub use endpoint::{
    clear_endpoint, endpoint_path, load_endpoint, write_endpoint, EndpointInfo, ENDPOINT_FILE_NAME,
};
pub use ipc::{handle_connection, read_message, write_message, IpcError};
pub use protocol::{
    ConnectedSessionInfo, ControlToken, IpcErrorBody, IpcRequest, IpcResponse, IpcResult,
    OutputMode, SavedSessionInfo, SessionOutput, METHOD_CONNECT_SAVED, METHOD_DISCONNECT_SESSION,
    METHOD_DUPLICATE_SESSION, METHOD_LIST_CONNECTED, METHOD_LIST_SAVED, METHOD_RELEASE_CONTROL,
    METHOD_REQUEST_CONTROL, METHOD_SESSION_INPUT, METHOD_SESSION_OUTPUT, METHOD_CLIENT_HELLO,
};
pub use stdio::{run_stdio_server, StdioServerError};
