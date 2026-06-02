use std::{
    collections::HashMap,
    io::{Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::config::SessionProtocol;

const OUTPUT_EVENT: &str = "terminal://output";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Idle,
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRuntime {
    pub runtime_id: String,
    pub profile_id: Option<String>,
    pub kind: SessionProtocol,
    pub status: RuntimeStatus,
    pub pane_id: String,
    pub tab_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalOutputEvent {
    runtime_id: String,
    data: String,
}

struct RuntimeHandle {
    runtime: TerminalRuntime,
    stdin: Option<Arc<Mutex<ChildStdin>>>,
    child: Option<Child>,
}

#[derive(Default)]
pub struct RuntimeRegistry {
    runtimes: Mutex<HashMap<String, RuntimeHandle>>,
}

impl RuntimeRegistry {
    pub fn open_local(
        &self,
        app_handle: AppHandle,
        tab_id: String,
        pane_id: String,
    ) -> Result<TerminalRuntime, String> {
        let runtime = TerminalRuntime {
            runtime_id: Uuid::new_v4().to_string(),
            profile_id: None,
            kind: SessionProtocol::Local,
            status: RuntimeStatus::Connected,
            pane_id,
            tab_id,
            title: default_shell_title(),
        };

        let mut command = default_shell_command();
        command
            .env("TERM", "xterm-256color")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to spawn local shell: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .map(|stdin| Arc::new(Mutex::new(stdin)))
            .ok_or_else(|| "failed to open local shell stdin".to_string())?;

        if let Some(stdout) = child.stdout.take() {
            spawn_output_reader(app_handle.clone(), runtime.runtime_id.clone(), stdout);
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_output_reader(app_handle, runtime.runtime_id.clone(), stderr);
        }

        self.runtimes
            .lock()
            .map_err(|_| "terminal runtime registry lock poisoned".to_string())?
            .insert(
                runtime.runtime_id.clone(),
                RuntimeHandle {
                    runtime: runtime.clone(),
                    stdin: Some(stdin),
                    child: Some(child),
                },
            );
        Ok(runtime)
    }

    pub fn open_ssh_placeholder(
        &self,
        tab_id: String,
        pane_id: String,
        title: String,
    ) -> Result<TerminalRuntime, String> {
        let runtime = TerminalRuntime {
            runtime_id: Uuid::new_v4().to_string(),
            profile_id: None,
            kind: SessionProtocol::Ssh,
            status: RuntimeStatus::Connecting,
            pane_id,
            tab_id,
            title,
        };
        self.runtimes
            .lock()
            .map_err(|_| "terminal runtime registry lock poisoned".to_string())?
            .insert(
                runtime.runtime_id.clone(),
                RuntimeHandle {
                    runtime: runtime.clone(),
                    stdin: None,
                    child: None,
                },
            );
        Ok(runtime)
    }

    pub fn write(&self, runtime_id: &str, data: &str) -> Result<(), String> {
        let stdin = {
            let runtimes = self
                .runtimes
                .lock()
                .map_err(|_| "terminal runtime registry lock poisoned".to_string())?;
            runtimes
                .get(runtime_id)
                .and_then(|handle| handle.stdin.clone())
                .ok_or_else(|| format!("terminal runtime {runtime_id} does not accept input"))?
        };

        let mut stdin = stdin
            .lock()
            .map_err(|_| "terminal stdin lock poisoned".to_string())?;
        stdin
            .write_all(data.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("failed to write terminal input: {error}"))
    }

    pub fn resize(&self, runtime_id: &str, _cols: u16, _rows: u16) -> Result<(), String> {
        let runtimes = self
            .runtimes
            .lock()
            .map_err(|_| "terminal runtime registry lock poisoned".to_string())?;
        let Some(handle) = runtimes.get(runtime_id) else {
            return Err(format!("terminal runtime {runtime_id} not found"));
        };
        match &handle.runtime.kind {
            SessionProtocol::Local | SessionProtocol::Ssh => Ok(()),
        }
    }

    pub fn close(&self, runtime_id: &str) -> Result<(), String> {
        let handle = self
            .runtimes
            .lock()
            .map_err(|_| "terminal runtime registry lock poisoned".to_string())?
            .remove(runtime_id);

        if let Some(mut handle) = handle {
            if let Some(mut child) = handle.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        Ok(())
    }
}

fn spawn_output_reader<R>(app_handle: AppHandle, runtime_id: String, mut reader: R)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => {
                    let data = String::from_utf8_lossy(&buffer[..bytes_read]).into_owned();
                    let _ = app_handle.emit(
                        OUTPUT_EVENT,
                        TerminalOutputEvent {
                            runtime_id: runtime_id.clone(),
                            data,
                        },
                    );
                }
                Err(_) => break,
            }
        }
    });
}

#[cfg(windows)]
fn default_shell_command() -> Command {
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "powershell.exe".to_string());
    Command::new(shell)
}

#[cfg(not(windows))]
fn default_shell_command() -> Command {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    Command::new(shell)
}

#[cfg(windows)]
fn default_shell_title() -> String {
    std::env::var("COMSPEC").unwrap_or_else(|_| "PowerShell".to_string())
}

#[cfg(not(windows))]
fn default_shell_title() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "Local Shell".to_string())
}
