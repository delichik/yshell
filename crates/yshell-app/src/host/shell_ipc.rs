//! Shell ↔ Host IPC (D38 Phase 4).
//!
//! Bridge connection authenticated with host-bridge token, then role line
//! `shell`. Requests are NDJSON; Host bridge thread enqueues [`ShellJob`]s
//! drained on the Host supervisor loop (same pattern as MCP).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use yshell_terminal::{
    FontFamilyFaces, FontSelection, PrimaryFont, TerminalColor, TerminalPalette,
};

use crate::error::{AppError, AppResult};
use crate::runtime::AppRuntime;

use std::collections::HashMap;
use std::time::Instant;

use super::bridge::load_host_bridge;
use super::liveness::current_pid;
use super::shell_inbox::ShellInbox;
use super::shell_jobs::{oneshot, ShellJob, ShellJobInbox};

/// Tracks attached Shell processes (Host side).
#[derive(Debug, Default)]
pub struct ShellRegistry {
    shells: HashMap<u32, ShellAttachment>,
    total_windows: u32,
}

#[derive(Debug, Clone)]
struct ShellAttachment {
    window_count: u32,
    last_seen: Instant,
}

impl ShellRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn attach(&mut self, pid: u32, window_count: u32) {
        self.shells.insert(
            pid,
            ShellAttachment {
                window_count,
                last_seen: Instant::now(),
            },
        );
        self.recompute_windows();
    }

    pub fn detach(&mut self, pid: u32) {
        self.shells.remove(&pid);
        self.recompute_windows();
    }

    pub fn set_window_count(&mut self, pid: u32, count: u32) {
        if let Some(shell) = self.shells.get_mut(&pid) {
            shell.window_count = count;
            shell.last_seen = Instant::now();
        }
        self.recompute_windows();
    }

    pub fn heartbeat(&mut self, pid: u32, window_count: u32) {
        self.set_window_count(pid, window_count);
    }

    pub fn prune_stale(&mut self, timeout: Duration) -> bool {
        let before = self.shells.len();
        self.shells
            .retain(|_, shell| shell.last_seen.elapsed() < timeout);
        if self.shells.len() != before {
            self.recompute_windows();
            true
        } else {
            false
        }
    }

    #[must_use]
    pub fn total_windows(&self) -> u32 {
        self.total_windows
    }

    #[must_use]
    pub fn shell_count(&self) -> usize {
        self.shells.len()
    }

    fn recompute_windows(&mut self) {
        self.total_windows = self.shells.values().map(|s| s.window_count).sum();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireTerminalCell {
    pub grapheme: String,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub wide_continuation: bool,
}

/// Viewport-relative selection carried by [`WireTerminalSnapshot`].
///
/// Coordinates are `(column, row)` in viewport space (row `0` is the top
/// visible row), exactly like `TerminalSnapshot::selection`. The Shell cannot
/// reuse `yshell_terminal::SelectionRange` directly because it has no serde
/// derive; this pair keeps the wire representation explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireSelection {
    pub start: (u16, u16),
    pub end: (u16, u16),
}

impl WireSelection {
    #[must_use]
    pub const fn new(start: (u16, u16), end: (u16, u16)) -> Self {
        Self { start, end }
    }

    /// Rebuild the renderer's selection type (viewport coordinates).
    #[must_use]
    pub const fn to_selection_range(self) -> yshell_terminal::SelectionRange {
        yshell_terminal::SelectionRange {
            start: yshell_terminal::GridPoint {
                column: self.start.0,
                row: self.start.1,
            },
            end: yshell_terminal::GridPoint {
                column: self.end.0,
                row: self.end.1,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireTerminalSnapshot {
    pub session_key: String,
    pub frame_id: u64,
    pub columns: u16,
    pub rows: u16,
    pub cursor: Option<(u16, u16)>,
    pub cells: Vec<WireTerminalCell>,
    /// Visible selection in viewport coordinates. Added after the initial wire
    /// format; `#[serde(default)]` keeps older Shell/Host payloads decodable.
    #[serde(default)]
    pub selection: Option<WireSelection>,
    /// Frozen appearance of the snapshot's session. The Host captures it when a
    /// connection is established, which bumps the frame id, so carrying it here
    /// keeps the Shell renderer in sync without a per-frame extra `UiCall`.
    /// Boxed (and serde-defaulted) so the optional payload does not inflate the
    /// `HostToShell` enum.
    #[serde(default)]
    pub appearance: Option<Box<WireTerminalAppearance>>,
}

/// Serializable color palette for a Shell-side [`yshell_terminal::TerminalRenderer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireTerminalPalette {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub cursor: [u8; 3],
    pub selection: [u8; 3],
    pub ansi: [[u8; 3]; 16],
}

fn wire_color(color: TerminalColor) -> [u8; 3] {
    [color.red, color.green, color.blue]
}

fn terminal_color([red, green, blue]: [u8; 3]) -> TerminalColor {
    TerminalColor::rgb(red, green, blue)
}

impl WireTerminalPalette {
    #[must_use]
    pub fn from_palette(palette: TerminalPalette) -> Self {
        Self {
            foreground: wire_color(palette.foreground),
            background: wire_color(palette.background),
            cursor: wire_color(palette.cursor),
            selection: wire_color(palette.selection),
            ansi: palette.ansi.map(wire_color),
        }
    }

    #[must_use]
    pub fn to_palette(self) -> TerminalPalette {
        TerminalPalette {
            foreground: terminal_color(self.foreground),
            background: terminal_color(self.background),
            cursor: terminal_color(self.cursor),
            selection: terminal_color(self.selection),
            ansi: self.ansi.map(terminal_color),
        }
    }
}

/// Serializable primary terminal font (mirrors [`yshell_terminal::PrimaryFont`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WirePrimaryFont {
    BundledDejaVu,
    Family {
        label: String,
        regular: String,
        #[serde(default)]
        bold: Option<String>,
        #[serde(default)]
        italic: Option<String>,
        #[serde(default)]
        bold_italic: Option<String>,
    },
    File {
        path: String,
    },
}

/// Serializable font choice (primary face + user fallback files).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireFontSelection {
    pub primary: WirePrimaryFont,
    #[serde(default)]
    pub fallback_files: Vec<String>,
}

impl WireFontSelection {
    #[must_use]
    pub fn from_font_selection(selection: &FontSelection) -> Self {
        let primary = match &selection.primary {
            PrimaryFont::BundledDejaVu => WirePrimaryFont::BundledDejaVu,
            PrimaryFont::Family(faces) => WirePrimaryFont::Family {
                label: faces.label.clone(),
                regular: faces.regular.display().to_string(),
                bold: faces.bold.as_ref().map(|path| path.display().to_string()),
                italic: faces.italic.as_ref().map(|path| path.display().to_string()),
                bold_italic: faces
                    .bold_italic
                    .as_ref()
                    .map(|path| path.display().to_string()),
            },
            PrimaryFont::File(path) => WirePrimaryFont::File {
                path: path.display().to_string(),
            },
        };
        Self {
            primary,
            fallback_files: selection
                .fallback_files
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        }
    }

    #[must_use]
    pub fn to_font_selection(&self) -> FontSelection {
        let primary = match &self.primary {
            WirePrimaryFont::BundledDejaVu => PrimaryFont::BundledDejaVu,
            WirePrimaryFont::Family {
                label,
                regular,
                bold,
                italic,
                bold_italic,
            } => PrimaryFont::Family(FontFamilyFaces {
                label: label.clone(),
                regular: PathBuf::from(regular),
                bold: bold.as_ref().map(PathBuf::from),
                italic: italic.as_ref().map(PathBuf::from),
                bold_italic: bold_italic.as_ref().map(PathBuf::from),
            }),
            WirePrimaryFont::File { path } => PrimaryFont::File(PathBuf::from(path)),
        };
        FontSelection {
            primary,
            fallback_files: self.fallback_files.iter().map(PathBuf::from).collect(),
        }
    }
}

/// Frozen terminal appearance of one window's active session (item B).
///
/// The Shell owns the window/DPI metrics but the Host owns the session, so the
/// configured font family/size, palette and cursor color must be projected over
/// the bridge for the Shell renderer to match the in-process renderer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireTerminalAppearance {
    pub font_size: f32,
    pub palette: WireTerminalPalette,
    pub font: WireFontSelection,
}

impl WireTerminalAppearance {
    #[must_use]
    pub fn from_runtime(appearance: &crate::runtime::TerminalAppearance) -> Self {
        Self {
            font_size: appearance.font_size,
            palette: WireTerminalPalette::from_palette(appearance.palette),
            font: WireFontSelection::from_font_selection(&appearance.font),
        }
    }

    /// Inputs for [`yshell_terminal::TerminalRenderer::apply_appearance`].
    #[must_use]
    pub fn renderer_inputs(&self) -> (TerminalPalette, FontSelection) {
        (self.palette.to_palette(), self.font.to_font_selection())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShellToHost {
    Attach {
        pid: u32,
        window_count: u32,
    },
    Detach {
        pid: u32,
    },
    SetWindowCount {
        count: u32,
    },
    Heartbeat {
        pid: u32,
        window_count: u32,
    },
    UiCall {
        id: u64,
        method: String,
        args: Value,
    },
    HostTick {
        id: u64,
    },
    GetProjection {
        id: u64,
        window_id: u64,
    },
    GetTerminalSnapshot {
        id: u64,
        window_id: u64,
        secondary: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostToShell {
    Attached {
        generation: u64,
        host_pid: u32,
    },
    UiCallResult {
        id: u64,
        ok: bool,
        value: Value,
        error: Option<String>,
    },
    HostTickResult {
        id: u64,
        dirty: bool,
        window_count: u32,
        shutdown: bool,
    },
    Projection {
        id: u64,
        window_id: u64,
        projection: Value,
    },
    TerminalSnapshot {
        id: u64,
        snapshot: Option<WireTerminalSnapshot>,
    },
    OpenWindow,
    Err {
        message: String,
    },
}

impl ShellToHost {
    /// Request id used to pair a reply; `None` for one-way notifications.
    #[must_use]
    pub fn request_id(&self) -> Option<u64> {
        match self {
            Self::UiCall { id, .. }
            | Self::HostTick { id }
            | Self::GetProjection { id, .. }
            | Self::GetTerminalSnapshot { id, .. } => Some(*id),
            Self::Attach { .. }
            | Self::Detach { .. }
            | Self::SetWindowCount { .. }
            | Self::Heartbeat { .. } => None,
        }
    }
}

impl HostToShell {
    /// Request id this reply answers; `None` for asynchronous events.
    #[must_use]
    pub fn request_id(&self) -> Option<u64> {
        match self {
            Self::UiCallResult { id, .. }
            | Self::HostTickResult { id, .. }
            | Self::Projection { id, .. }
            | Self::TerminalSnapshot { id, .. } => Some(*id),
            Self::Attached { .. } | Self::OpenWindow | Self::Err { .. } => None,
        }
    }
}

fn write_msg(writer: &mut impl Write, msg: &HostToShell) -> AppResult<()> {
    let mut body = serde_json::to_vec(msg)
        .map_err(|error| AppError::new(format!("shell ipc encode: {error}")))?;
    body.push(b'\n');
    writer.write_all(&body).map_err(AppError::from_error)?;
    writer.flush().map_err(AppError::from_error)?;
    Ok(())
}

fn write_req(writer: &mut impl Write, msg: &ShellToHost) -> AppResult<()> {
    let mut body = serde_json::to_vec(msg)
        .map_err(|error| AppError::new(format!("shell ipc encode: {error}")))?;
    body.push(b'\n');
    writer.write_all(&body).map_err(AppError::from_error)?;
    writer.flush().map_err(AppError::from_error)?;
    Ok(())
}

/// Read one NDJSON line, tolerating the read-timeout polls the server uses.
///
/// `BufReader::read_line` may keep bytes it already read when the socket read
/// times out; re-entering with the same buffer preserves them instead of
/// turning a partial request into a bogus blank line.
fn read_shell_line(reader: &mut BufReader<TcpStream>, line: &mut String) -> AppResult<bool> {
    line.clear();
    loop {
        match reader.read_line(line) {
            Ok(0) => return Ok(false),
            Ok(_) => return Ok(true),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                // Defensive: never spin if a non-blocking socket slips through.
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            Err(error) => return Err(AppError::from_error(error)),
        }
    }
}

/// Bridge worker: translate Shell NDJSON into [`ShellJob`]s.
pub fn handle_shell_stream(
    mut reader: BufReader<TcpStream>,
    mut writer: TcpStream,
    jobs: ShellJobInbox,
    shell_inbox: ShellInbox,
    generation: u64,
    host_pid: u32,
) -> AppResult<()> {
    let _ = reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(200)));
    let mut attached_pid: Option<u32> = None;
    let mut line = String::new();

    // Every exit path (EOF, socket reset, or protocol failure) must detach the
    // Shell; otherwise a crashed Shell leaves the Host waiting for the 45s
    // heartbeat prune before it can run its shutdown sequence.
    let result: AppResult<()> = (|| {
        loop {
            // Second launch / window request: Host queued an OpenWindow command.
            while let Some(cmd) = shell_inbox.try_pop() {
                match cmd {
                    super::shell_inbox::ShellCommand::OpenWindow => {
                        write_msg(&mut writer, &HostToShell::OpenWindow)?;
                    }
                }
            }
            match read_shell_line(&mut reader, &mut line) {
                Ok(false) => {
                    break;
                }
                Ok(true) => {}
                Err(error) => {
                    return Err(error);
                }
            }
            let req: ShellToHost = serde_json::from_str(line.trim_end())
                .map_err(|error| AppError::new(format!("invalid shell ipc: {error}")))?;
            match req {
                ShellToHost::Attach { pid, window_count } => {
                    attached_pid = Some(pid);
                    let (tx, rx) = oneshot();
                    jobs.push(ShellJob::Attach {
                        pid,
                        window_count,
                        reply: tx,
                    });
                    match rx.recv_timeout(Duration::from_secs(10)) {
                        Ok(Ok((gen, host))) => write_msg(
                            &mut writer,
                            &HostToShell::Attached {
                                generation: gen,
                                host_pid: host,
                            },
                        )?,
                        Ok(Err(message)) => {
                            write_msg(&mut writer, &HostToShell::Err { message })?;
                        }
                        Err(_) => write_msg(
                            &mut writer,
                            &HostToShell::Err {
                                message: "attach timed out".into(),
                            },
                        )?,
                    }
                    let _ = generation;
                    let _ = host_pid;
                }
                ShellToHost::Detach { pid } => {
                    jobs.push(ShellJob::Detach { pid });
                    attached_pid = None;
                }
                ShellToHost::SetWindowCount { count } => {
                    if let Some(pid) = attached_pid {
                        jobs.push(ShellJob::SetWindowCount { pid, count });
                    }
                }
                ShellToHost::Heartbeat { pid, window_count } => {
                    jobs.push(ShellJob::Heartbeat { pid, window_count });
                }
                ShellToHost::UiCall { id, method, args } => {
                    let (tx, rx) = oneshot();
                    jobs.push(ShellJob::UiCall {
                        method,
                        args,
                        reply: tx,
                    });
                    match rx.recv_timeout(Duration::from_secs(120)) {
                        Ok(Ok(value)) => write_msg(
                            &mut writer,
                            &HostToShell::UiCallResult {
                                id,
                                ok: true,
                                value,
                                error: None,
                            },
                        )?,
                        Ok(Err(error)) => write_msg(
                            &mut writer,
                            &HostToShell::UiCallResult {
                                id,
                                ok: false,
                                value: Value::Null,
                                error: Some(error),
                            },
                        )?,
                        Err(_) => write_msg(
                            &mut writer,
                            &HostToShell::UiCallResult {
                                id,
                                ok: false,
                                value: Value::Null,
                                error: Some("ui call timed out".into()),
                            },
                        )?,
                    }
                }
                ShellToHost::HostTick { id } => {
                    let (tx, rx) = oneshot();
                    jobs.push(ShellJob::HostTick { reply: tx });
                    match rx.recv_timeout(Duration::from_secs(30)) {
                        Ok(Ok((dirty, window_count, shutdown))) => write_msg(
                            &mut writer,
                            &HostToShell::HostTickResult {
                                id,
                                dirty,
                                window_count,
                                shutdown,
                            },
                        )?,
                        Ok(Err(message)) => {
                            write_msg(&mut writer, &HostToShell::Err { message })?;
                        }
                        Err(_) => write_msg(
                            &mut writer,
                            &HostToShell::Err {
                                message: "host tick timed out".into(),
                            },
                        )?,
                    }
                }
                ShellToHost::GetProjection { id, window_id } => {
                    let (tx, rx) = oneshot();
                    jobs.push(ShellJob::GetProjection {
                        window_id,
                        reply: tx,
                    });
                    match rx.recv_timeout(Duration::from_secs(30)) {
                        Ok(Ok(projection)) => write_msg(
                            &mut writer,
                            &HostToShell::Projection {
                                id,
                                window_id,
                                projection,
                            },
                        )?,
                        Ok(Err(message)) => {
                            write_msg(&mut writer, &HostToShell::Err { message })?;
                        }
                        Err(_) => write_msg(
                            &mut writer,
                            &HostToShell::Err {
                                message: "get projection timed out".into(),
                            },
                        )?,
                    }
                }
                ShellToHost::GetTerminalSnapshot {
                    id,
                    window_id,
                    secondary,
                } => {
                    let (tx, rx) = oneshot();
                    jobs.push(ShellJob::GetTerminalSnapshot {
                        window_id,
                        secondary,
                        reply: tx,
                    });
                    match rx.recv_timeout(Duration::from_secs(30)) {
                        Ok(Ok(snapshot)) => {
                            write_msg(&mut writer, &HostToShell::TerminalSnapshot { id, snapshot })?
                        }
                        Ok(Err(message)) => {
                            write_msg(&mut writer, &HostToShell::Err { message })?;
                        }
                        Err(_) => write_msg(
                            &mut writer,
                            &HostToShell::Err {
                                message: "get snapshot timed out".into(),
                            },
                        )?,
                    }
                }
            }
        }
        Ok(())
    })();

    if let Some(pid) = attached_pid {
        jobs.push(ShellJob::Detach { pid });
    }
    result
}

/// Serialize full [`AppProjection`] for Shell IPC (TerminalFrame previews included).
pub fn projection_json(projection: &crate::runtime::AppProjection) -> Value {
    serde_json::to_value(projection).unwrap_or(Value::Null)
}

/// Decode a Host projection payload into [`AppProjection`].
pub fn projection_from_json(value: Value) -> AppResult<crate::runtime::AppProjection> {
    serde_json::from_value(value)
        .map_err(|error| AppError::new(format!("projection decode: {error}")))
}

pub fn capture_terminal_snapshot(
    runtime: &AppRuntime,
    window_id: u64,
    secondary: bool,
) -> Option<WireTerminalSnapshot> {
    let frame_id = if secondary {
        runtime.window_secondary_terminal_frame_id(window_id)
    } else {
        runtime.window_terminal_frame_id(window_id)
    };
    let snap = if secondary {
        runtime.window_secondary_terminal_render_snapshot(window_id)?
    } else {
        runtime.window_terminal_render_snapshot(window_id)?
    };
    let session_key = if secondary {
        runtime
            .window_secondary_session(window_id)
            .unwrap_or("")
            .to_owned()
    } else {
        runtime
            .window_active_session(window_id)
            .unwrap_or("")
            .to_owned()
    };
    let rows = u16::try_from(snap.lines.len()).unwrap_or(u16::MAX);
    let columns = snap
        .lines
        .first()
        .map(|line| u16::try_from(line.len()).unwrap_or(u16::MAX))
        .unwrap_or(0);
    let appearance = if secondary {
        runtime.window_secondary_terminal_appearance(window_id)
    } else {
        runtime.window_terminal_appearance(window_id)
    }
    .map(|appearance| Box::new(WireTerminalAppearance::from_runtime(&appearance)));
    let mut cells = Vec::with_capacity(snap.lines.iter().map(|l| l.len()).sum());
    for line in &snap.lines {
        for cell in line.iter() {
            let TerminalColor { red, green, blue } = cell.foreground;
            let bg = cell.background;
            cells.push(WireTerminalCell {
                grapheme: cell.grapheme.clone(),
                fg: [red, green, blue],
                bg: [bg.red, bg.green, bg.blue],
                bold: cell.bold,
                italic: cell.italic,
                underline: cell.underline,
                wide_continuation: cell.wide_continuation,
            });
        }
    }
    Some(WireTerminalSnapshot {
        session_key,
        frame_id,
        columns,
        rows,
        cursor: snap.cursor,
        cells,
        selection: snap.selection.map(|range| {
            let range = range.normalized();
            WireSelection::new(
                (range.start.column, range.start.row),
                (range.end.column, range.end.row),
            )
        }),
        appearance,
    })
}

/// Client used by the Shell process.
///
/// Every clone shares the same connection state. When the Host dies (or the
/// stream errors), the next call redials `host-bridge.json` and re-attaches with
/// the last reported window count, so a Host restart does not require a Shell
/// restart.
pub struct HostShellClient {
    config_dir: PathBuf,
    state: Mutex<ClientState>,
    next_id: AtomicU64,
    open_window: AtomicBool,
}

#[derive(Default)]
struct ClientState {
    conn: Option<ClientConn>,
    window_count: u32,
    attached: bool,
}

struct ClientConn {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
}

/// How long the Shell waits for a slow UI call reply.
const SHELL_IPC_REPLY_TIMEOUT: Duration = Duration::from_secs(60);
/// Tick replies must be fast; a slower Host is treated as busy/lost so the UI
/// thread is not frozen for a full minute.
const SHELL_IPC_TICK_TIMEOUT: Duration = Duration::from_secs(5);
/// Projection / terminal snapshot queries.
const SHELL_IPC_QUERY_TIMEOUT: Duration = Duration::from_secs(15);
/// Attach handshake (Host may still be starting up).
const SHELL_IPC_ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

fn dial_host(config_dir: &Path) -> AppResult<ClientConn> {
    let info = load_host_bridge(config_dir).map_err(AppError::from_error)?;
    let stream =
        TcpStream::connect((info.host.as_str(), info.port)).map_err(AppError::from_error)?;
    stream.set_nodelay(true).map_err(AppError::from_error)?;
    stream
        .set_read_timeout(Some(SHELL_IPC_REPLY_TIMEOUT))
        .map_err(AppError::from_error)?;
    stream
        .set_write_timeout(Some(SHELL_IPC_REPLY_TIMEOUT))
        .map_err(AppError::from_error)?;
    {
        let mut writer = stream.try_clone().map_err(AppError::from_error)?;
        writeln!(writer, "{}", info.token).map_err(AppError::from_error)?;
        writeln!(writer, "shell").map_err(AppError::from_error)?;
        writer.flush().map_err(AppError::from_error)?;
    }
    let reader = BufReader::new(stream.try_clone().map_err(AppError::from_error)?);
    Ok(ClientConn { stream, reader })
}

impl HostShellClient {
    pub fn connect(config_dir: &Path) -> AppResult<Self> {
        let client = Self {
            config_dir: config_dir.to_path_buf(),
            state: Mutex::new(ClientState::default()),
            next_id: AtomicU64::new(1),
            open_window: AtomicBool::new(false),
        };
        {
            let mut state = client.state_lock()?;
            state.conn = Some(dial_host(&client.config_dir)?);
        }
        Ok(client)
    }

    fn state_lock(&self) -> AppResult<std::sync::MutexGuard<'_, ClientState>> {
        self.state
            .lock()
            .map_err(|_| AppError::new("shell ipc state lock poisoned"))
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Returns `true` once for every `OpenWindow` request the Host forwarded.
    ///
    /// The Shell consumes this on its timer tick; the flag is cleared on read.
    #[must_use]
    pub fn take_open_window(&self) -> bool {
        self.open_window.swap(false, Ordering::SeqCst)
    }

    /// Dial (if needed) and re-attach (if needed), returning a live connection.
    fn open_ready_conn(&self, state: &mut ClientState) -> AppResult<ClientConn> {
        if state.conn.is_none() {
            state.conn = Some(dial_host(&self.config_dir)?);
            state.attached = false;
        }
        let mut conn = state.conn.take().expect("dialed shell connection");
        if !state.attached {
            self.send_attach(&mut conn, state.window_count)?;
            state.attached = true;
        }
        Ok(conn)
    }

    fn send_attach(&self, conn: &mut ClientConn, window_count: u32) -> AppResult<()> {
        write_req(
            &mut conn.stream,
            &ShellToHost::Attach {
                pid: current_pid(),
                window_count,
            },
        )?;
        let _ = conn.stream.set_read_timeout(Some(SHELL_IPC_ATTACH_TIMEOUT));
        let _ = conn
            .reader
            .get_ref()
            .set_read_timeout(Some(SHELL_IPC_ATTACH_TIMEOUT));
        loop {
            match Self::read_message(conn)? {
                HostToShell::OpenWindow => {
                    self.open_window.store(true, Ordering::SeqCst);
                }
                HostToShell::Attached { .. } => return Ok(()),
                HostToShell::Err { message } => return Err(AppError::new(message)),
                other => {
                    return Err(AppError::new(format!("unexpected attach reply: {other:?}")));
                }
            }
        }
    }

    fn read_message(conn: &mut ClientConn) -> AppResult<HostToShell> {
        let mut line = String::new();
        match conn.reader.read_line(&mut line) {
            Ok(0) => Err(AppError::new("host shell ipc closed")),
            Ok(_) => serde_json::from_str(line.trim_end())
                .map_err(|error| AppError::new(format!("shell ipc decode: {error}"))),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Err(AppError::new(
                    "host shell ipc timed out waiting for a reply",
                ))
            }
            Err(error) => Err(AppError::from_error(error)),
        }
    }

    /// Send a request and wait for the reply carrying its id.
    ///
    /// Reconnects and re-attaches exactly once on a transport failure. Host
    /// `Err` replies are returned immediately (they are not worth retrying).
    fn request(&self, req: ShellToHost, timeout: Duration) -> AppResult<HostToShell> {
        let id = req
            .request_id()
            .ok_or_else(|| AppError::new("shell ipc request without an id"))?;
        let mut state = self.state_lock()?;
        let mut last_error = AppError::new("host shell ipc unavailable");
        for _ in 0..2 {
            let mut conn = match self.open_ready_conn(&mut state) {
                Ok(conn) => conn,
                Err(error) => {
                    last_error = error;
                    state.conn = None;
                    state.attached = false;
                    continue;
                }
            };
            if let Err(error) = write_req(&mut conn.stream, &req) {
                last_error = error;
                state.conn = None;
                state.attached = false;
                continue;
            }
            let _ = conn.stream.set_read_timeout(Some(timeout));
            let _ = conn.reader.get_ref().set_read_timeout(Some(timeout));
            loop {
                match Self::read_message(&mut conn) {
                    Ok(HostToShell::OpenWindow) => {
                        self.open_window.store(true, Ordering::SeqCst);
                    }
                    Ok(HostToShell::Err { message }) => {
                        state.conn = Some(conn);
                        return Err(AppError::new(message));
                    }
                    Ok(reply) => {
                        if reply.request_id() == Some(id) {
                            state.conn = Some(conn);
                            return Ok(reply);
                        }
                        state.conn = Some(conn);
                        return Err(AppError::new(format!(
                            "unexpected shell ipc reply: {reply:?}"
                        )));
                    }
                    Err(error) => {
                        last_error = error;
                        state.conn = None;
                        state.attached = false;
                        break;
                    }
                }
            }
        }
        Err(last_error)
    }

    /// Send a one-way notification (the Host never replies to these).
    fn send(&self, req: ShellToHost) -> AppResult<()> {
        let mut state = self.state_lock()?;
        match &req {
            ShellToHost::SetWindowCount { count } => state.window_count = *count,
            ShellToHost::Heartbeat { window_count, .. } => state.window_count = *window_count,
            _ => {}
        }
        let mut last_error = AppError::new("host shell ipc unavailable");
        for _ in 0..2 {
            let mut conn = match self.open_ready_conn(&mut state) {
                Ok(conn) => conn,
                Err(error) => {
                    last_error = error;
                    state.conn = None;
                    state.attached = false;
                    continue;
                }
            };
            match write_req(&mut conn.stream, &req) {
                Ok(()) => {
                    state.conn = Some(conn);
                    return Ok(());
                }
                Err(error) => {
                    last_error = error;
                    state.conn = None;
                    state.attached = false;
                }
            }
        }
        Err(last_error)
    }

    pub fn attach(&self, window_count: u32) -> AppResult<()> {
        let mut state = self.state_lock()?;
        state.window_count = window_count;
        state.attached = false;
        let mut last_error = AppError::new("host shell ipc unavailable");
        for _ in 0..2 {
            let conn = match self.open_ready_conn(&mut state) {
                Ok(conn) => conn,
                Err(error) => {
                    last_error = error;
                    state.conn = None;
                    state.attached = false;
                    continue;
                }
            };
            state.conn = Some(conn);
            return Ok(());
        }
        Err(last_error)
    }

    pub fn detach(&self) -> AppResult<()> {
        let result = self.send(ShellToHost::Detach { pid: current_pid() });
        if let Ok(mut state) = self.state.lock() {
            state.attached = false;
            state.conn = None;
        }
        result
    }

    pub fn set_window_count(&self, count: u32) -> AppResult<()> {
        self.send(ShellToHost::SetWindowCount { count })
    }

    pub fn heartbeat(&self, window_count: u32) -> AppResult<()> {
        self.send(ShellToHost::Heartbeat {
            pid: current_pid(),
            window_count,
        })
    }

    pub fn ui_call(&self, method: &str, args: Value) -> AppResult<Value> {
        let id = self.next_id();
        match self.request(
            ShellToHost::UiCall {
                id,
                method: method.to_owned(),
                args,
            },
            SHELL_IPC_REPLY_TIMEOUT,
        )? {
            HostToShell::UiCallResult {
                ok: true, value, ..
            } => Ok(value),
            HostToShell::UiCallResult {
                ok: false, error, ..
            } => Err(AppError::new(
                error.unwrap_or_else(|| "ui call failed".into()),
            )),
            other => Err(AppError::new(format!(
                "unexpected ui_call reply: {other:?}"
            ))),
        }
    }

    pub fn host_tick(&self) -> AppResult<(bool, u32, bool)> {
        let id = self.next_id();
        match self.request(ShellToHost::HostTick { id }, SHELL_IPC_TICK_TIMEOUT)? {
            HostToShell::HostTickResult {
                dirty,
                window_count,
                shutdown,
                ..
            } => Ok((dirty, window_count, shutdown)),
            other => Err(AppError::new(format!("unexpected tick reply: {other:?}"))),
        }
    }

    pub fn get_projection(&self, window_id: u64) -> AppResult<Value> {
        let id = self.next_id();
        match self.request(
            ShellToHost::GetProjection { id, window_id },
            SHELL_IPC_QUERY_TIMEOUT,
        )? {
            HostToShell::Projection { projection, .. } => Ok(projection),
            other => Err(AppError::new(format!(
                "unexpected projection reply: {other:?}"
            ))),
        }
    }

    pub fn get_terminal_snapshot(
        &self,
        window_id: u64,
        secondary: bool,
    ) -> AppResult<Option<WireTerminalSnapshot>> {
        let id = self.next_id();
        match self.request(
            ShellToHost::GetTerminalSnapshot {
                id,
                window_id,
                secondary,
            },
            SHELL_IPC_QUERY_TIMEOUT,
        )? {
            HostToShell::TerminalSnapshot { snapshot, .. } => Ok(snapshot),
            other => Err(AppError::new(format!(
                "unexpected snapshot reply: {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    use tempfile::TempDir;

    use crate::host::service::HostService;
    use crate::runtime::AppRuntime;

    fn wait_for_bridge(config_dir: &Path) -> HostShellClient {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if load_host_bridge(config_dir).is_ok() {
                if let Ok(client) = HostShellClient::connect(config_dir) {
                    return client;
                }
            }
            assert!(
                Instant::now() < deadline,
                "host bridge never became available"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// P0 regression: one-way notifications must return immediately. Before the
    /// fix the client waited for a reply that the Host never wrote, deadlocking
    /// the Shell before its first window was shown.
    #[test]
    fn attach_and_one_way_notifications_do_not_block() {
        let temp = TempDir::new().expect("tempdir");
        let config_dir = temp.path().to_path_buf();
        let stop = Arc::new(AtomicBool::new(false));
        let host_dir = config_dir.clone();
        let host_stop = Arc::clone(&stop);
        let host = std::thread::spawn(move || {
            let runtime = AppRuntime::new_with_keychain(host_dir, None).expect("runtime");
            let service = HostService::start(runtime).expect("host");
            while !host_stop.load(Ordering::SeqCst) {
                let _ = service.borrow_mut().supervisor_tick();
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = service.borrow_mut().begin_shutdown();
        });

        let client = wait_for_bridge(&config_dir);
        let started = Instant::now();
        client.attach(1).expect("attach");
        client.set_window_count(1).expect("set window count");
        client.heartbeat(1).expect("heartbeat");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "one-way shell notifications must not wait for a reply"
        );

        let (_, window_count, shutdown) = client.host_tick().expect("tick");
        assert!(!shutdown, "one live window must not request shutdown");
        assert_eq!(window_count, 1);
        let projection = client.get_projection(0).expect("projection");
        assert!(projection.is_object(), "projection must be a JSON object");

        stop.store(true, Ordering::SeqCst);
        let _ = client.detach();
        host.join().expect("host thread");
    }

    /// Regression: a Shell that vanishes without calling `detach` (crash, kill,
    /// network reset) must still make the Host run its shutdown sequence instead
    /// of waiting for the 45s heartbeat prune.
    #[test]
    fn shell_connection_loss_detaches_and_shuts_down_host() {
        let temp = TempDir::new().expect("tempdir");
        let config_dir = temp.path().to_path_buf();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
        let host_dir = config_dir.clone();
        let host = std::thread::spawn(move || {
            let runtime = AppRuntime::new_with_keychain(host_dir, None).expect("runtime");
            let service = HostService::start(runtime).expect("host");
            loop {
                let shutdown = service
                    .borrow_mut()
                    .supervisor_tick()
                    .expect("supervisor tick");
                if shutdown {
                    let _ = shutdown_tx.send(());
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });

        let client = wait_for_bridge(&config_dir);
        client.attach(1).expect("attach");
        client.set_window_count(1).expect("window count");
        // No `detach`: drop the socket like a killed Shell would.
        drop(client);

        shutdown_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("Host must shut down when the Shell connection drops");
        host.join().expect("host thread");
    }

    /// The selection field was added after the first wire format; an older
    /// payload (no `selection`) must still decode with an empty selection.
    #[test]
    fn terminal_snapshot_selection_is_backward_compatible() {
        let legacy = serde_json::json!({
            "session_key": "s1",
            "frame_id": 7,
            "columns": 2,
            "rows": 1,
            "cursor": [1, 0],
            "cells": [
                {
                    "grapheme": "a",
                    "fg": [1, 2, 3],
                    "bg": [4, 5, 6],
                    "bold": false,
                    "italic": false,
                    "underline": false,
                    "wide_continuation": false
                }
            ]
        });
        let snapshot: WireTerminalSnapshot =
            serde_json::from_value(legacy).expect("legacy snapshot decodes");
        assert_eq!(snapshot.selection, None);
        assert_eq!(snapshot.columns, 2);
    }

    #[test]
    fn terminal_snapshot_selection_round_trips() {
        let snapshot = WireTerminalSnapshot {
            session_key: "s1".into(),
            frame_id: 3,
            columns: 4,
            rows: 2,
            cursor: None,
            cells: Vec::new(),
            selection: Some(WireSelection::new((1, 0), (3, 1))),
            appearance: None,
        };
        let encoded = serde_json::to_value(&snapshot).expect("snapshot encodes");
        assert_eq!(encoded["selection"]["start"], serde_json::json!([1, 0]));
        let decoded: WireTerminalSnapshot = serde_json::from_value(encoded).expect("decodes");
        assert_eq!(decoded.selection, snapshot.selection);
        let range = decoded
            .selection
            .expect("selection present")
            .to_selection_range()
            .normalized();
        assert!(range.contains(1, 0));
        assert!(range.contains(3, 1));
        assert!(!range.contains(0, 0));
    }

    /// Item B: the snapshot carries the frozen appearance so the Shell picks it
    /// up on the frame bump that the Host capture performs.
    #[test]
    fn terminal_snapshot_carries_the_frozen_appearance() {
        let appearance =
            WireTerminalAppearance::from_runtime(&crate::runtime::TerminalAppearance {
                palette: TerminalPalette::DEFAULT,
                font: FontSelection::default(),
                font_size: 15.0,
                rejected_fonts: Vec::new(),
            });
        let snapshot = WireTerminalSnapshot {
            session_key: "s1".into(),
            frame_id: 1,
            columns: 1,
            rows: 1,
            cursor: None,
            cells: Vec::new(),
            selection: None,
            appearance: Some(Box::new(appearance.clone())),
        };
        let encoded = serde_json::to_value(&snapshot).expect("snapshot encodes");
        let decoded: WireTerminalSnapshot = serde_json::from_value(encoded).expect("decodes");
        assert_eq!(decoded.appearance.as_deref(), Some(&appearance));
    }

    /// A reversed drag selection is normalized when captured so the Shell
    /// paints the same cells the Host copies.
    #[test]
    fn captured_selection_is_normalized_when_wire_encoded() {
        let reversed = yshell_terminal::SelectionRange {
            start: yshell_terminal::GridPoint { column: 5, row: 2 },
            end: yshell_terminal::GridPoint { column: 1, row: 0 },
        };
        let normalized = reversed.normalized();
        let wire = WireSelection::new(
            (normalized.start.column, normalized.start.row),
            (normalized.end.column, normalized.end.row),
        );
        assert_eq!(wire.start, (1, 0));
        assert_eq!(wire.end, (5, 2));
    }

    /// Item B: the appearance projected over the bridge must rebuild exactly the
    /// renderer inputs the Host session froze.
    #[test]
    fn terminal_appearance_round_trips_renderer_inputs() {
        let appearance = crate::runtime::TerminalAppearance {
            palette: TerminalPalette::DEFAULT,
            font: FontSelection::default(),
            font_size: 18.5,
            rejected_fonts: Vec::new(),
        };
        let wire = WireTerminalAppearance::from_runtime(&appearance);
        let encoded = serde_json::to_value(&wire).expect("appearance encodes");
        let decoded: WireTerminalAppearance =
            serde_json::from_value(encoded).expect("appearance decodes");
        assert_eq!(decoded, wire);
        let (palette, font) = decoded.renderer_inputs();
        assert_eq!(palette, appearance.palette);
        assert_eq!(font, appearance.font);
        assert!((decoded.font_size - 18.5).abs() < f32::EPSILON);
    }

    /// Family and file primary fonts survive the JSON projection (the Shell
    /// rebuilds `FontSelection` without Host types).
    #[test]
    fn terminal_appearance_font_variants_round_trip() {
        let family = FontSelection {
            primary: PrimaryFont::Family(FontFamilyFaces {
                label: "Consolas".into(),
                regular: PathBuf::from(r"C:\Windows\Fonts\consola.ttf"),
                bold: Some(PathBuf::from(r"C:\Windows\Fonts\consolab.ttf")),
                italic: None,
                bold_italic: None,
            }),
            fallback_files: vec![PathBuf::from(r"C:\fonts\fallback.ttf")],
        };
        let wire = WireFontSelection::from_font_selection(&family);
        let encoded = serde_json::to_value(&wire).expect("font encodes");
        let decoded: WireFontSelection = serde_json::from_value(encoded).expect("font decodes");
        assert_eq!(decoded.to_font_selection(), family);

        let file = FontSelection::with_primary_file(r"C:\fonts\custom.ttf");
        let wire = WireFontSelection::from_font_selection(&file);
        let encoded = serde_json::to_value(&wire).expect("font encodes");
        let decoded: WireFontSelection = serde_json::from_value(encoded).expect("font decodes");
        assert_eq!(decoded.to_font_selection(), file);
    }
}
