//! Shell IPC jobs drained on the Host supervisor loop (same pattern as McpInbox).

use std::collections::VecDeque;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::shell_ipc::WireTerminalSnapshot;

pub enum ShellJob {
    Attach {
        pid: u32,
        window_count: u32,
        reply: Sender<Result<(u64, u32), String>>,
    },
    Detach {
        pid: u32,
    },
    SetWindowCount {
        pid: u32,
        count: u32,
    },
    Heartbeat {
        pid: u32,
        window_count: u32,
    },
    UiCall {
        method: String,
        args: Value,
        reply: Sender<Result<Value, String>>,
    },
    HostTick {
        reply: Sender<Result<(bool, u32, bool), String>>,
    },
    GetProjection {
        window_id: u64,
        reply: Sender<Result<Value, String>>,
    },
    GetTerminalSnapshot {
        window_id: u64,
        secondary: bool,
        reply: Sender<Result<Option<WireTerminalSnapshot>, String>>,
    },
}

#[derive(Clone, Default)]
pub struct ShellJobInbox {
    inner: Arc<Mutex<VecDeque<ShellJob>>>,
}

impl ShellJobInbox {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, job: ShellJob) {
        if let Ok(mut queue) = self.inner.lock() {
            queue.push_back(job);
        }
    }

    pub fn try_pop(&self) -> Option<ShellJob> {
        self.inner.lock().ok()?.pop_front()
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().map(|q| q.is_empty()).unwrap_or(true)
    }
}

pub fn oneshot<T>() -> (Sender<T>, mpsc::Receiver<T>) {
    mpsc::channel()
}
