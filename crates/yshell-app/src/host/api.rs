//! Host mailbox commands / events (process-agnostic).
//!
//! Phase 1 keeps everything in-process; later phases serialize these across IPC.
//! See `docs/prototypes/process-architecture.md`.

use serde::{Deserialize, Serialize};

/// Commands Shell / MCP / workers send toward the Host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostCommand {
    /// UI window count changed (zero → Host should shut down).
    SetWindowCount { count: u32 },
    /// Request application quit (after UI confirmation).
    RequestShutdown,
    /// Register or refresh a logical session worker.
    WorkerStarted { session_id: String, pid: u32 },
    /// Worker exited (clean or faulted).
    WorkerExited { session_id: String },
    /// MCP daemon process started.
    McpStarted { pid: u32 },
    /// MCP daemon exited.
    McpExited,
    /// Heartbeat from a child process.
    Heartbeat {
        generation: u64,
        role: String,
        pid: u32,
    },
}

/// Events Host fans out to Shell / interested parties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostEvent {
    ShutdownStarted,
    WorkerFaulted { session_id: String },
    McpStatus { ready: bool, detail: String },
}

/// Process role for CLI / supervision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRole {
    Host,
    Shell,
    SessionWorker,
    McpDaemon,
}

impl ProcessRole {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Shell => "shell",
            Self::SessionWorker => "session_worker",
            Self::McpDaemon => "mcp_daemon",
        }
    }
}
