use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::config::SessionProtocol;

const OUTPUT_EVENT: &str = "terminal://output";
const STATUS_EVENT: &str = "terminal://status";
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 30;

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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStatusEvent {
    pub runtime_id: String,
    pub status: RuntimeStatus,
}

type PtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;

struct RuntimeHandle {
    runtime: TerminalRuntime,
    writer: Option<PtyWriter>,
    master: Option<Box<dyn MasterPty + Send>>,
    child: Option<Box<dyn Child + Send>>,
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
        cols: Option<u16>,
        rows: Option<u16>,
        shell: Option<String>,
        working_directory: Option<String>,
    ) -> Result<TerminalRuntime, String> {
        let runtime = TerminalRuntime {
            runtime_id: Uuid::new_v4().to_string(),
            profile_id: None,
            kind: SessionProtocol::Local,
            status: RuntimeStatus::Connected,
            pane_id,
            tab_id,
            title: shell.clone().unwrap_or_else(default_shell_title),
        };

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows.unwrap_or(DEFAULT_ROWS),
                cols: cols.unwrap_or(DEFAULT_COLS),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("failed to open local PTY: {error}"))?;

        let mut command = default_shell_command(shell, working_directory);
        command.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("failed to spawn local shell in PTY: {error}"))?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("failed to clone PTY reader: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("failed to open PTY writer: {error}"))?;
        spawn_output_reader(app_handle, runtime.runtime_id.clone(), reader);

        self.runtimes
            .lock()
            .map_err(|_| "terminal runtime registry lock poisoned".to_string())?
            .insert(
                runtime.runtime_id.clone(),
                RuntimeHandle {
                    runtime: runtime.clone(),
                    writer: Some(Arc::new(Mutex::new(writer))),
                    master: Some(pair.master),
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
                    writer: None,
                    master: None,
                    child: None,
                },
            );
        Ok(runtime)
    }

    pub fn write(&self, runtime_id: &str, data: &str) -> Result<(), String> {
        let writer = {
            let runtimes = self
                .runtimes
                .lock()
                .map_err(|_| "terminal runtime registry lock poisoned".to_string())?;
            runtimes
                .get(runtime_id)
                .and_then(|handle| handle.writer.clone())
                .ok_or_else(|| format!("terminal runtime {runtime_id} does not accept input"))?
        };

        let mut writer = writer
            .lock()
            .map_err(|_| "terminal writer lock poisoned".to_string())?;
        writer
            .write_all(data.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| format!("failed to write terminal input: {error}"))
    }

    pub fn resize(&self, runtime_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let runtimes = self
            .runtimes
            .lock()
            .map_err(|_| "terminal runtime registry lock poisoned".to_string())?;
        let Some(handle) = runtimes.get(runtime_id) else {
            return Err(format!("terminal runtime {runtime_id} not found"));
        };

        match (&handle.runtime.kind, &handle.master) {
            (SessionProtocol::Local, Some(master)) => master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|error| format!("failed to resize PTY: {error}")),
            (SessionProtocol::Ssh, _) => Ok(()),
            (SessionProtocol::Local, None) => {
                Err(format!("terminal runtime {runtime_id} has no PTY master"))
            }
        }
    }

    pub fn close(&self, runtime_id: &str) -> Result<(), String> {
        let handle = self
            .runtimes
            .lock()
            .map_err(|_| "terminal runtime registry lock poisoned".to_string())?
            .remove(runtime_id);

        Self::terminate_handle(handle);
        Ok(())
    }

    pub fn close_all(&self) -> Result<(), String> {
        let handles = self
            .runtimes
            .lock()
            .map_err(|_| "terminal runtime registry lock poisoned".to_string())?
            .drain()
            .map(|(_, handle)| handle)
            .collect::<Vec<_>>();

        for handle in handles {
            Self::terminate_handle(Some(handle));
        }
        Ok(())
    }

    fn terminate_handle(handle: Option<RuntimeHandle>) {
        if let Some(mut handle) = handle {
            if let Some(mut child) = handle.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        Ok(())
    }
}

impl Drop for RuntimeRegistry {
    fn drop(&mut self) {
        let _ = self.close_all();
    }
}

fn spawn_output_reader(
    app_handle: AppHandle,
    runtime_id: String,
    mut reader: Box<dyn Read + Send>,
) {
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
        let _ = app_handle.emit(
            STATUS_EVENT,
            TerminalStatusEvent {
                runtime_id,
                status: RuntimeStatus::Disconnected,
            },
        );
    });
}

fn default_shell_command(
    shell: Option<String>,
    working_directory: Option<String>,
) -> CommandBuilder {
    let mut command = CommandBuilder::new(shell.unwrap_or_else(default_shell_path));
    if let Some(directory) = working_directory {
        command.cwd(directory);
    } else if let Ok(directory) = std::env::current_dir() {
        command.cwd(directory);
    }
    command
}

#[cfg(windows)]
fn default_shell_path() -> String {
    std::env::var("COMSPEC").unwrap_or_else(|_| "powershell.exe".to_string())
}

#[cfg(not(windows))]
fn default_shell_path() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

fn default_shell_title() -> String {
    default_shell_path()
}
