//! Line-delimited JSON IPC between `yshell --mcp` and the GUI process.

use serde::{Deserialize, Serialize};

pub const METHOD_GET_MCP_INFO: &str = "get_mcp_info";
pub const METHOD_LIST_SAVED: &str = "list_saved_sessions";
pub const METHOD_LIST_CONNECTED: &str = "list_connected_sessions";
pub const METHOD_LIST_SESSIONS: &str = "list_sessions";
pub const METHOD_GET_SESSION: &str = "get_session";
pub const METHOD_REQUEST_CONTROL: &str = "request_control";
pub const METHOD_RELEASE_CONTROL: &str = "release_control";
pub const METHOD_SESSION_INPUT: &str = "session_input";
pub const METHOD_SESSION_OUTPUT: &str = "session_output";
pub const METHOD_SESSION_RUN: &str = "session_run";
pub const METHOD_SESSION_SEND_KEY: &str = "session_send_key";
pub const METHOD_CONNECT_SAVED: &str = "connect_saved";
pub const METHOD_DISCONNECT_SESSION: &str = "disconnect_session";
pub const METHOD_DUPLICATE_SESSION: &str = "duplicate_session";

/// Product boundaries shown in Settings and returned by `get_mcp_info`.
pub const MCP_BOUNDARIES_TEXT: &str = "Can: list saved profiles and open terminal tabs; after you approve control, read/write terminals, run commands and wait for output, send keys (e.g. Ctrl+C), disconnect or duplicate tabs.\nCannot: fill passwords or accept host keys (do that in the YShell GUI); control SFTP or tunnels; run without the GUI; let multiple agents hold control at once.";

/// Host-facing stdio MCP config snippet (any MCP client).
pub const MCP_HOST_CONFIG_SNIPPET: &str = r#"{
  "mcpServers": {
    "yshell": {
      "command": "yshell",
      "args": ["--mcp"]
    }
  }
}"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    /// Shared secret from the endpoint file; required on every request.
    pub token: String,
    /// Opaque id for this MCP bridge process (used for lock ownership).
    pub client_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<IpcResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<IpcErrorBody>,
}

impl IpcResponse {
    #[must_use]
    pub fn ok(id: u64, result: IpcResult) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    #[must_use]
    pub fn err(id: u64, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(IpcErrorBody {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResult {
    Ok,
    McpInfo {
        enabled: bool,
        status: String,
        control: String,
        boundaries: String,
        host_config: String,
    },
    SavedSessions {
        sessions: Vec<SavedSessionInfo>,
    },
    ConnectedSessions {
        sessions: Vec<ConnectedSessionInfo>,
    },
    SessionInfo {
        session: ConnectedSessionInfo,
    },
    /// Control granted; ownership is the bridge `client_id`, not a separate token.
    ControlGranted,
    SessionOutput {
        text: String,
        mode: String,
    },
    SessionRun {
        text: String,
        timed_out: bool,
        state: String,
        blocker: String,
        tab_id: String,
    },
    TabOpened {
        tab_id: String,
        session_id: Option<String>,
        state: String,
        blocker: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSessionInfo {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub folder: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectedSessionInfo {
    pub tab_id: String,
    pub session_id: String,
    pub title: String,
    pub state: String,
    pub connected: bool,
    pub kind: String,
    /// `none` | `password` | `host_key` | `connecting` | `disconnected` | `failed`
    pub blocker: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    SinceInput,
    LastLine,
    LastN,
    Visible,
}

impl OutputMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "since_input" => Some(Self::SinceInput),
            "last_line" => Some(Self::LastLine),
            "last_n" => Some(Self::LastN),
            "visible" => Some(Self::Visible),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SinceInput => "since_input",
            Self::LastLine => "last_line",
            Self::LastN => "last_n",
            Self::Visible => "visible",
        }
    }
}
