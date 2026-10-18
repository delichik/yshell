//! The local PTY process and its [`ShellSession`] implementation.

use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use portable_pty::{
    native_pty_system, Child, CommandBuilder, MasterPty, PtySize as PortablePtySize,
};
use yshell_ssh::{PtySize, ShellSession, SshError, SshErrorKind, SshResult};

use crate::config::LocalPtyConfig;

/// Number of output chunks buffered between the reader thread and the UI.
///
/// The queue is bounded on purpose: if the UI stops polling, the reader blocks,
/// the kernel PTY buffer fills up and the child eventually blocks on write
/// instead of the process growing without bound.
const OUTPUT_QUEUE_CHUNKS: usize = 256;

/// Maximum time [`LocalPty::disconnect`] waits for the reader thread to stop.
const READER_JOIN_TIMEOUT: Duration = Duration::from_millis(500);

const READ_BUFFER_BYTES: usize = 8 * 1024;
const READER_THREAD_NAME: &str = "yshell-local-pty-reader";

/// How a local shell process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalExitStatus {
    code: u32,
    signal: Option<String>,
}

impl LocalExitStatus {
    /// The numeric exit code. On Unix a signal-terminated process reports the
    /// shell's fallback code (usually 1); inspect [`Self::signal`] instead.
    pub fn code(&self) -> u32 {
        self.code
    }

    /// The signal name on Unix, if the process was terminated by a signal.
    pub fn signal(&self) -> Option<&str> {
        self.signal.as_deref()
    }

    /// True when the process exited with code 0 and was not signalled.
    pub fn success(&self) -> bool {
        self.code == 0 && self.signal.is_none()
    }

    fn from_pty_status(status: portable_pty::ExitStatus) -> Self {
        Self {
            code: status.exit_code(),
            signal: status.signal().map(str::to_owned),
        }
    }
}

impl fmt::Display for LocalExitStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.signal.as_deref() {
            Some(signal) => write!(formatter, "terminated by {signal}"),
            None => write!(formatter, "exited with code {}", self.code),
        }
    }
}

/// A local shell process attached to a PTY.
///
/// Output is pumped by a dedicated reader thread into a bounded channel, so
/// [`ShellSession::poll_output`] never blocks. Dropping the value (or calling
/// [`ShellSession::disconnect`]) kills the child process, closes the PTY and
/// waits a bounded amount of time for the reader thread to finish.
pub struct LocalPty {
    config: LocalPtyConfig,
    size: PtySize,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    output: Option<Receiver<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    reader_done: Option<Receiver<()>>,
    connected: Arc<AtomicBool>,
    exit_status: Option<LocalExitStatus>,
    closed: bool,
}

impl LocalPty {
    /// Starts `config.program` inside a new PTY.
    ///
    /// The configuration is validated before spawning: the program must not be
    /// empty and an explicit `cwd` must be an existing directory. Platform and
    /// shell resolution failures are returned as [`SshError`] with the shell
    /// path in the message so the UI can display them.
    pub fn spawn(config: LocalPtyConfig) -> SshResult<Self> {
        validate_config(&config)?;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(portable_size(config.size))
            .map_err(|error| {
                SshError::new(
                    SshErrorKind::Io,
                    format!("failed to open a local PTY: {error}"),
                )
            })?;
        let portable_pty::PtyPair { master, slave } = pair;

        let writer = master.take_writer().map_err(|error| {
            SshError::new(
                SshErrorKind::Io,
                format!("failed to open the local PTY input stream: {error}"),
            )
        })?;
        let reader = master.try_clone_reader().map_err(|error| {
            SshError::new(
                SshErrorKind::Io,
                format!("failed to open the local PTY output stream: {error}"),
            )
        })?;

        let mut command = CommandBuilder::new(&config.program);
        command.args(&config.args);
        if let Some(cwd) = &config.cwd {
            command.cwd(cwd);
        }
        for (key, value) in &config.env {
            command.env(key, value);
        }

        let mut child = slave.spawn_command(command).map_err(|error| {
            SshError::new(
                SshErrorKind::Io,
                format!(
                    "failed to start local shell `{}`: {error}",
                    config.program.display()
                ),
            )
        })?;
        // The slave handle must be closed in the parent, otherwise the reader
        // never observes EOF after the child exits.
        drop(slave);

        let (output_sender, output) = mpsc::sync_channel(OUTPUT_QUEUE_CHUNKS);
        let (done_sender, reader_done) = mpsc::channel();
        let connected = Arc::new(AtomicBool::new(true));
        let reader_connected = Arc::clone(&connected);
        let reader = thread::Builder::new()
            .name(READER_THREAD_NAME.to_owned())
            .spawn(move || {
                pump_output(reader, output_sender, reader_connected);
                let _ = done_sender.send(());
            });

        let reader = match reader {
            Ok(handle) => handle,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SshError::new(
                    SshErrorKind::Io,
                    format!("failed to start the local PTY reader thread: {error}"),
                ));
            }
        };

        let size = config.size;
        Ok(Self {
            config,
            size,
            master: Some(master),
            writer: Some(writer),
            child: Some(child),
            output: Some(output),
            reader: Some(reader),
            reader_done: Some(reader_done),
            connected,
            exit_status: None,
            closed: false,
        })
    }

    /// Starts the platform default shell in the user's home directory.
    pub fn spawn_default() -> SshResult<Self> {
        Self::spawn(LocalPtyConfig::default())
    }

    /// Starts a local shell and erases its concrete type.
    ///
    /// This is the hand-off point for the session runtime, which stores
    /// `Box<dyn ShellSession>` for both SSH and local backends.
    pub fn spawn_boxed(config: LocalPtyConfig) -> SshResult<Box<dyn ShellSession>> {
        Ok(Box::new(Self::spawn(config)?))
    }

    /// The configuration this shell was started with.
    pub fn config(&self) -> &LocalPtyConfig {
        &self.config
    }

    /// The current PTY size in character cells.
    pub fn size(&self) -> PtySize {
        self.size
    }

    /// The child process id, when the platform exposes one.
    pub fn process_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(|child| child.process_id())
    }

    /// Exit status once the process has been reaped.
    ///
    /// Reaping happens while polling output and during disconnect/drop. A shell
    /// that exited on its own is also reported as disconnected through
    /// [`ShellSession::is_connected`] once the PTY reaches EOF.
    pub fn exit_status(&self) -> Option<&LocalExitStatus> {
        self.exit_status.as_ref()
    }

    /// True after [`ShellSession::disconnect`] or drop has run.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// A short shell name for tab titles (`bash`, `pwsh`, `cmd`).
    pub fn shell_name(&self) -> String {
        self.config.shell_name()
    }

    /// Polls the child process without blocking and records its exit status.
    fn refresh_child_state(&mut self) {
        if self.exit_status.is_some() {
            return;
        }
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                tracing::debug!(
                    target: "yshell::local",
                    code = status.exit_code(),
                    signal = status.signal().unwrap_or("none"),
                    "local shell exited"
                );
                self.exit_status = Some(LocalExitStatus::from_pty_status(status));
                self.connected.store(false, Ordering::Release);
            }
            Ok(None) => {}
            Err(error) => {
                tracing::debug!(
                    target: "yshell::local",
                    %error,
                    "failed to query the local shell process state"
                );
            }
        }
    }

    /// Kills the child, closes the PTY and joins the reader with a bounded wait.
    fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.connected.store(false, Ordering::Release);

        self.reap_child();

        // Dropping the writer on Unix sends EOT; dropping the master/writer is
        // also what unblocks the reader on Windows ConPTY.
        drop(self.writer.take());
        drop(self.master.take());
        // Dropping the receiver unblocks a reader that is waiting on a full
        // output queue.
        drop(self.output.take());
        self.join_reader();
    }

    fn reap_child(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };

        if self.exit_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.exit_status = Some(LocalExitStatus::from_pty_status(status));
                }
                Ok(None) => {
                    if let Err(error) = child.kill() {
                        tracing::debug!(
                            target: "yshell::local",
                            %error,
                            "failed to kill the local shell process"
                        );
                    }
                }
                Err(error) => {
                    tracing::debug!(
                        target: "yshell::local",
                        %error,
                        "failed to query the local shell process before killing it"
                    );
                }
            }
        }

        if self.exit_status.is_none() {
            match child.wait() {
                Ok(status) => {
                    self.exit_status = Some(LocalExitStatus::from_pty_status(status));
                }
                Err(error) => {
                    tracing::debug!(
                        target: "yshell::local",
                        %error,
                        "failed to reap the local shell process"
                    );
                }
            }
        }
    }

    fn join_reader(&mut self) {
        let Some(done) = self.reader_done.take() else {
            let _ = self.reader.take();
            return;
        };
        match done.recv_timeout(READER_JOIN_TIMEOUT) {
            Ok(()) => {
                if let Some(handle) = self.reader.take() {
                    let _ = handle.join();
                }
            }
            Err(error) => {
                // A stuck reader must not hang tab close or application exit.
                tracing::debug!(
                    target: "yshell::local",
                    timeout_ms = READER_JOIN_TIMEOUT.as_millis(),
                    %error,
                    "local PTY reader did not stop in time; detaching it"
                );
                let _ = self.reader.take();
            }
        }
    }
}

impl fmt::Debug for LocalPty {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalPty")
            .field("program", &self.config.program)
            .field("args", &self.config.args)
            .field("cwd", &self.config.cwd)
            .field("size", &self.size)
            .field("connected", &self.is_connected())
            .field("closed", &self.closed)
            .field("exit_status", &self.exit_status)
            .finish_non_exhaustive()
    }
}

impl Drop for LocalPty {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl ShellSession for LocalPty {
    fn is_connected(&self) -> bool {
        !self.closed && self.connected.load(Ordering::Acquire)
    }

    fn poll_output(&mut self) -> SshResult<Vec<u8>> {
        self.refresh_child_state();
        let Some(output) = self.output.as_ref() else {
            return Ok(Vec::new());
        };
        match output.try_recv() {
            Ok(bytes) => Ok(bytes),
            Err(TryRecvError::Empty) => Ok(Vec::new()),
            Err(TryRecvError::Disconnected) => {
                self.connected.store(false, Ordering::Release);
                Ok(Vec::new())
            }
        }
    }

    fn write_input(&mut self, bytes: &[u8]) -> SshResult<()> {
        if self.closed {
            return Err(closed_error("write to"));
        }
        self.refresh_child_state();
        if !self.is_connected() {
            return Err(not_running_error("write to"));
        }
        let Some(writer) = self.writer.as_mut() else {
            return Err(closed_error("write to"));
        };
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|error| {
                SshError::new(
                    SshErrorKind::Io,
                    format!("failed to write to the local shell: {error}"),
                )
            })
    }

    fn resize_pty(&mut self, size: PtySize) -> SshResult<()> {
        if self.closed {
            return Err(closed_error("resize"));
        }
        self.refresh_child_state();
        if !self.is_connected() {
            return Err(not_running_error("resize"));
        }
        let Some(master) = self.master.as_ref() else {
            return Err(closed_error("resize"));
        };
        master.resize(portable_size(size)).map_err(|error| {
            SshError::new(
                SshErrorKind::Io,
                format!(
                    "failed to resize the local PTY to {}x{}: {error}",
                    size.columns, size.rows
                ),
            )
        })?;
        self.size = size;
        Ok(())
    }

    fn disconnect(&mut self) -> SshResult<()> {
        self.shutdown();
        Ok(())
    }
}

fn pump_output(
    mut reader: Box<dyn Read + Send>,
    output: SyncSender<Vec<u8>>,
    connected: Arc<AtomicBool>,
) {
    let mut buffer = [0u8; READ_BUFFER_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                if output.send(buffer[..count].to_vec()).is_err() {
                    // The session was closed and the receiver dropped.
                    break;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => {
                tracing::debug!(
                    target: "yshell::local",
                    %error,
                    "local PTY reader stopped"
                );
                break;
            }
        }
    }
    connected.store(false, Ordering::Release);
}

fn validate_config(config: &LocalPtyConfig) -> SshResult<()> {
    if config.program.as_os_str().is_empty() {
        return Err(SshError::new(
            SshErrorKind::Configuration,
            "local shell program path must not be empty",
        ));
    }
    if let Some(cwd) = &config.cwd {
        if !cwd.is_dir() {
            return Err(SshError::new(
                SshErrorKind::Configuration,
                format!(
                    "local shell working directory `{}` does not exist or is not a directory",
                    cwd.display()
                ),
            ));
        }
    }
    Ok(())
}

fn portable_size(size: PtySize) -> PortablePtySize {
    PortablePtySize {
        rows: size.rows,
        cols: size.columns,
        pixel_width: size.pixel_width,
        pixel_height: size.pixel_height,
    }
}

fn closed_error(operation: &str) -> SshError {
    SshError::new(
        SshErrorKind::Channel,
        format!("cannot {operation} the local shell: the session is closed"),
    )
}

fn not_running_error(operation: &str) -> SshError {
    SshError::new(
        SshErrorKind::Channel,
        format!("cannot {operation} the local shell: the process is not running"),
    )
}
