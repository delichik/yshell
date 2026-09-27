//! Runtime-owned session state for one live or pending shell session.

use std::borrow::Cow;
use std::io;
use std::path::{Path, PathBuf};

use yshell_config::{LoggingProfile, QuickConnectTarget, SessionProfile, TerminalProfile};
use yshell_core::{SessionId, SessionState, TabId};
use yshell_logging::{
    LogPathContext, PathTemplate, Redactor, RotationPolicy, SessionLogOptions, SessionLogger,
    TranscriptDirection, TranscriptFormat, TransferLogger,
};
use yshell_ssh::{AuthMethod, PtySize, ShellSession, SshConnectionConfig, TransportBackend};
use yshell_terminal::{
    GridPoint, SearchMatch, SearchQuery, SelectionRange, TerminalGrid, TerminalParser,
    TerminalSnapshot, DEFAULT_SCROLLBACK_LINES, DEFAULT_SCROLLBACK_MAX_CELLS,
};

#[derive(Debug)]
pub enum SessionSource {
    QuickConnect,
    SavedSession {
        profile_id: String,
    },
    /// 草稿会话（旧 `+` 行为）。N2 起 `+` 走 Quick Connect 页 / Session Editor，
    /// 暂无可达 UI 入口；保留变体以维持既有生命周期代码与测试（后续可整体清理）。
    #[allow(dead_code)]
    Draft,
}

/// Everything the app layer needs to map pixels to grid cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalViewportMetrics {
    pub columns: u16,
    pub rows: u16,
    /// Absolute line shown in the first visible row.
    pub top_absolute_row: usize,
    /// Lines scrolled back from the bottom (0 = live view).
    pub viewport_offset: usize,
}

/// N6：会话日志输出类型（REC 指示与菜单启用条件）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionLoggingMode {
    /// 没有日志输出。
    #[default]
    Off,
    /// 配置驱动的自动日志（`configure_logging` 打开）。
    Auto,
    /// 运行时手动开启到指定文件（弹窗 → [`SessionRuntime::start_logging`]）。
    Manual,
}

impl SessionLoggingMode {
    /// N6：投影/状态文案用的稳定 id（`off`/`auto`/`manual`）。
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

/// N6：运行时手动日志选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualLoggingOptions {
    /// 勾选"记录本地输入"时，输入按 [`TranscriptDirection::Input`] 写入日志。
    pub include_input: bool,
}

#[derive(Debug)]
pub struct SessionRuntime {
    session_id: SessionId,
    tab_id: TabId,
    pub display_name: String,
    pub source: SessionSource,
    pub ssh_config: SshConnectionConfig,
    pub terminal_parser: TerminalParser,
    pub terminal_grid: TerminalGrid,
    pub shell_session: Option<Box<dyn ShellSession>>,
    pub state: SessionState,
    pub transport_backend: TransportBackend,
    session_logger: Option<SessionLogger>,
    transfer_logger: Option<TransferLogger>,
    logging_notice: Option<String>,
    /// N6：手动日志选项；`Some` = 当前 `session_logger` 由运行时手动开启。
    ///
    /// 手动日志优先：开启时暂停配置驱动的自动日志，且本会话内不再自动恢复。
    manual_logging: Option<ManualLoggingOptions>,
    /// N6：手动日志开启过（含停止后）就不再自动打开配置驱动的自动日志。
    auto_logging_suppressed: bool,
    /// N6：最近一次日志文件路径（停止/写失败后仍可用于"打开日志文件"）。
    last_logging_path: Option<PathBuf>,
    terminal_limits: (usize, usize),
    viewport_offset: usize,
    selection: Option<SelectionRange>,
    frame_id: u64,
}

impl SessionRuntime {
    pub fn from_quick_connect(target: QuickConnectTarget, ordinal: usize) -> Self {
        let username = target.username.clone().unwrap_or_else(|| "user".to_owned());
        let display_name = format!(
            "{}@{}:{}",
            target.username.as_deref().unwrap_or("<default>"),
            target.host,
            target.port
        );
        let ssh_config = SshConnectionConfig::new(
            target.host.clone(),
            target.port,
            AuthMethod::Agent { username },
        );
        let mut runtime = Self {
            session_id: SessionId::new(format!("quick-connect-{ordinal}")),
            tab_id: TabId::new(format!("tab-quick-connect-{ordinal}")),
            display_name,
            source: SessionSource::QuickConnect,
            ssh_config,
            terminal_parser: TerminalParser::new(),
            terminal_grid: TerminalGrid::new(120, 32),
            shell_session: None,
            state: SessionState::Connecting,
            transport_backend: TransportBackend::Fake,
            session_logger: None,
            transfer_logger: None,
            logging_notice: None,
            manual_logging: None,
            auto_logging_suppressed: false,
            last_logging_path: None,
            terminal_limits: (DEFAULT_SCROLLBACK_LINES, DEFAULT_SCROLLBACK_MAX_CELLS),
            viewport_offset: 0,
            selection: None,
            frame_id: 0,
        };
        runtime.append_status_line("Quick Connect accepted. Preparing runtime session.");
        runtime
    }

    pub fn from_profile(profile: &SessionProfile, ordinal: usize) -> Self {
        let username = profile
            .username
            .clone()
            .unwrap_or_else(|| "user".to_owned());
        let ssh_config = SshConnectionConfig::new(
            profile.host.clone(),
            profile.port,
            AuthMethod::Agent { username },
        );
        let mut runtime = Self {
            // N0：同一 saved session 允许重复打开（重复连接），因此每个运行时实例
            // 都必须有唯一 session id（日志目录、dispatcher、SFTP 状态都按 id 归属）；
            // profile id 仍保存在 `SessionSource::SavedSession` 里。
            session_id: SessionId::new(format!("{}-{ordinal}", profile.id)),
            tab_id: TabId::new(format!("tab-saved-{ordinal}")),
            display_name: profile.name.clone(),
            source: SessionSource::SavedSession {
                profile_id: profile.id.clone(),
            },
            ssh_config,
            terminal_parser: TerminalParser::new(),
            terminal_grid: TerminalGrid::new(120, 32),
            shell_session: None,
            state: SessionState::Idle,
            transport_backend: TransportBackend::Fake,
            session_logger: None,
            transfer_logger: None,
            logging_notice: None,
            manual_logging: None,
            auto_logging_suppressed: false,
            last_logging_path: None,
            terminal_limits: (DEFAULT_SCROLLBACK_LINES, DEFAULT_SCROLLBACK_MAX_CELLS),
            viewport_offset: 0,
            selection: None,
            frame_id: 0,
        };
        runtime.append_status_line("Saved session loaded into runtime inventory.");
        runtime
    }

    /// 旧 `+` 行为：新建一个草稿终端标签。
    ///
    /// N2 起 `+` 走 `handle_new_tab_default()`（Quick Connect 页 / Session Editor），
    /// 草稿标签暂无可达 UI 入口；保留实现与测试覆盖，便于后续恢复"临时会话"入口。
    #[allow(dead_code)]
    pub fn draft(ordinal: usize) -> Self {
        let mut runtime = Self {
            session_id: SessionId::new(format!("draft-{ordinal}")),
            tab_id: TabId::new(format!("tab-draft-{ordinal}")),
            display_name: "New Session".to_owned(),
            source: SessionSource::Draft,
            ssh_config: SshConnectionConfig::new(
                "example.com",
                22,
                AuthMethod::Agent {
                    username: "user".to_owned(),
                },
            ),
            terminal_parser: TerminalParser::new(),
            terminal_grid: TerminalGrid::new(120, 32),
            shell_session: None,
            state: SessionState::Idle,
            transport_backend: TransportBackend::Fake,
            session_logger: None,
            transfer_logger: None,
            logging_notice: None,
            manual_logging: None,
            auto_logging_suppressed: false,
            last_logging_path: None,
            terminal_limits: (DEFAULT_SCROLLBACK_LINES, DEFAULT_SCROLLBACK_MAX_CELLS),
            viewport_offset: 0,
            selection: None,
            frame_id: 0,
        };
        runtime.append_status_line("Draft session created. Waiting for connection details.");
        runtime
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn tab_id(&self) -> &TabId {
        &self.tab_id
    }

    pub fn host_label(&self) -> String {
        format!("{}:{}", self.ssh_config.host, self.ssh_config.port)
    }

    pub fn username_label(&self) -> &str {
        self.ssh_config.username()
    }

    pub fn state_label(&self) -> &'static str {
        match self.state {
            SessionState::Idle => "idle",
            SessionState::Connecting => "connecting",
            SessionState::Connected => "connected",
            SessionState::Disconnected => "disconnected",
            SessionState::Failed => "failed",
        }
    }

    pub fn set_state(&mut self, state: SessionState) {
        self.state = state;
    }

    pub fn append_status_line(&mut self, message: &str) {
        let mut line = String::from(message);
        line.push('\n');
        self.terminal_parser
            .advance(&mut self.terminal_grid, line.as_bytes());
        self.frame_id = self.frame_id.wrapping_add(1);
    }

    pub fn visible_text(&self) -> String {
        let lines = self.terminal_grid.visible_lines();
        if lines.iter().all(|line| line.is_empty()) {
            String::new()
        } else {
            lines.join("\n")
        }
    }

    pub fn visible_lines(&self) -> Vec<String> {
        self.terminal_grid.visible_lines()
    }

    pub fn cursor_position(&self) -> (u16, u16) {
        (
            self.terminal_grid.cursor_column,
            self.terminal_grid.cursor_row,
        )
    }

    pub fn find_visible_text(&self, query: &str) -> Vec<SearchMatch> {
        SearchQuery::new(query).find_in_grid(&self.terminal_grid)
    }

    pub fn clear_visible_terminal(&mut self) {
        let columns = self.terminal_grid.columns;
        let rows = self.terminal_grid.rows;
        let (line_limit, cell_limit) = self.terminal_limits;
        self.terminal_parser = TerminalParser::new();
        self.terminal_grid = TerminalGrid::with_limits(columns, rows, line_limit, cell_limit);
        self.viewport_offset = 0;
        self.selection = None;
        self.frame_id = self.frame_id.wrapping_add(1);
        self.append_status_line("Terminal view cleared. Live shell remains attached.");
    }

    /// Current frame revision used by the app to decide when to re-render.
    #[must_use]
    pub const fn frame_id(&self) -> u64 {
        self.frame_id
    }

    /// Lines scrolled back from the live bottom (0 = live view).
    #[must_use]
    pub const fn viewport_offset(&self) -> usize {
        self.viewport_offset
    }

    #[must_use]
    pub fn terminal_selection_active(&self) -> bool {
        self.selection
            .map(|selection| {
                let range = selection.normalized();
                range.start != range.end
            })
            .unwrap_or(false)
    }

    /// Grid geometry plus the absolute line at the top of the viewport.
    #[must_use]
    pub fn terminal_viewport_metrics(&self) -> TerminalViewportMetrics {
        TerminalViewportMetrics {
            columns: self.terminal_grid.columns,
            rows: self.terminal_grid.rows,
            top_absolute_row: self
                .terminal_grid
                .scrollback_len()
                .saturating_sub(self.viewport_offset),
            viewport_offset: self.viewport_offset,
        }
    }

    /// Scroll the viewport; positive `delta` moves back into scrollback.
    ///
    /// Returns true when the offset changed.
    pub fn scroll_terminal(&mut self, delta: i32) -> bool {
        let maximum = self.terminal_grid.scrollback_len() as i64;
        let next = (self.viewport_offset as i64 + i64::from(delta)).clamp(0, maximum) as usize;
        if next == self.viewport_offset {
            return false;
        }
        self.viewport_offset = next;
        self.frame_id = self.frame_id.wrapping_add(1);
        true
    }

    pub fn scroll_terminal_to_bottom(&mut self) -> bool {
        if self.viewport_offset == 0 {
            return false;
        }
        self.viewport_offset = 0;
        self.frame_id = self.frame_id.wrapping_add(1);
        true
    }

    /// Replace the whole selection (absolute rows); `None` clears it.
    pub fn set_terminal_selection(&mut self, selection: Option<SelectionRange>) -> bool {
        self.selection = selection;
        self.frame_id = self.frame_id.wrapping_add(1);
        self.selection.is_some()
    }

    /// Start a drag selection at an absolute grid point.
    pub fn begin_terminal_selection(&mut self, point: GridPoint) -> bool {
        self.set_terminal_selection(Some(SelectionRange {
            start: point,
            end: point,
        }))
    }

    /// Extend the active drag selection to an absolute grid point.
    pub fn update_terminal_selection(&mut self, point: GridPoint) -> bool {
        let start = self
            .selection
            .map(|selection| selection.start)
            .unwrap_or(point);
        self.set_terminal_selection(Some(SelectionRange { start, end: point }))
    }

    /// Select the word under an absolute grid point; clears on whitespace.
    pub fn select_word_at(&mut self, point: GridPoint) -> bool {
        self.selection = SelectionRange::word_at(&self.terminal_grid, point);
        self.frame_id = self.frame_id.wrapping_add(1);
        self.selection.is_some()
    }

    pub fn select_all_terminal(&mut self) -> bool {
        self.selection = SelectionRange::select_all(&self.terminal_grid);
        self.frame_id = self.frame_id.wrapping_add(1);
        self.selection.is_some()
    }

    #[must_use]
    pub fn terminal_selection_text(&self) -> String {
        self.selection
            .map(|selection| selection.copy_text_absolute(&self.terminal_grid))
            .unwrap_or_default()
    }

    /// Slice the grid for the renderer: viewport lines plus cursor/selection
    /// in viewport coordinates.
    ///
    /// Visible rows borrow the live grid; rows that come from the compressed
    /// scrollback are expanded into owned buffers.
    #[must_use]
    pub fn terminal_render_snapshot(&self) -> Option<TerminalSnapshot<'_>> {
        let rows = usize::from(self.terminal_grid.rows);
        if rows == 0 || self.terminal_grid.columns == 0 {
            return None;
        }
        let scrollback = self.terminal_grid.scrollback_len();
        let top = scrollback.saturating_sub(self.viewport_offset);
        let mut lines = Vec::with_capacity(rows);
        for row in 0..rows {
            let absolute = top + row;
            if absolute < scrollback {
                lines.push(Cow::Owned(
                    self.terminal_grid.line_cells(absolute).unwrap_or_default(),
                ));
            } else {
                let visible_row = u16::try_from(absolute - scrollback).unwrap_or(u16::MAX);
                lines.push(match self.terminal_grid.visible_cells(visible_row) {
                    Some(cells) => Cow::Borrowed(cells),
                    None => Cow::Owned(Vec::new()),
                });
            }
        }
        let cursor = if self.viewport_offset == 0 {
            Some((
                self.terminal_grid.cursor_column,
                self.terminal_grid.cursor_row,
            ))
        } else {
            None
        };
        Some(TerminalSnapshot {
            lines,
            cursor,
            selection: self.viewport_selection(top),
        })
    }

    fn viewport_selection(&self, top_absolute_row: usize) -> Option<SelectionRange> {
        let selection = self.selection?.normalized();
        let rows = usize::from(self.terminal_grid.rows);
        let columns = usize::from(self.terminal_grid.columns);
        if rows == 0 || columns == 0 {
            return None;
        }
        let bottom = top_absolute_row + rows - 1;
        let start_row = usize::from(selection.start.row);
        let end_row = usize::from(selection.end.row);
        if end_row < top_absolute_row || start_row > bottom {
            return None;
        }
        let start_clamped = start_row.max(top_absolute_row);
        let end_clamped = end_row.min(bottom);
        Some(SelectionRange {
            start: GridPoint {
                column: if start_row < top_absolute_row {
                    0
                } else {
                    selection.start.column
                },
                row: u16::try_from(start_clamped - top_absolute_row).unwrap_or(u16::MAX),
            },
            end: GridPoint {
                column: if end_row > bottom {
                    self.terminal_grid.columns.saturating_sub(1)
                } else {
                    selection.end.column
                },
                row: u16::try_from(end_clamped - top_absolute_row).unwrap_or(u16::MAX),
            },
        })
    }

    fn clamp_view_state(&mut self) {
        let scrollback = self.terminal_grid.scrollback_len();
        if self.viewport_offset > scrollback {
            self.viewport_offset = scrollback;
        }
        let total = self.terminal_grid.total_lines();
        let Some(selection) = self.selection else {
            return;
        };
        if total == 0 {
            self.selection = None;
            return;
        }
        let last = u16::try_from(total - 1).unwrap_or(u16::MAX);
        let range = selection.normalized();
        let last_column = self.terminal_grid.columns.saturating_sub(1);
        self.selection = Some(SelectionRange {
            start: GridPoint {
                column: range.start.column.min(last_column),
                row: range.start.row.min(last),
            },
            end: GridPoint {
                column: range.end.column.min(last_column),
                row: range.end.row.min(last),
            },
        });
    }

    pub fn sidebar_summary(&self) -> String {
        let source = match &self.source {
            SessionSource::QuickConnect => "quick-connect",
            SessionSource::SavedSession { .. } => "saved",
            SessionSource::Draft => "draft",
        };
        format!("{} [{}] {}", self.display_name, self.state_label(), source)
    }

    pub fn attach_shell_session(&mut self, shell_session: Box<dyn ShellSession>) {
        self.shell_session = Some(shell_session);
    }

    /// Apply the resolved terminal profile to the grid.
    ///
    /// The caps are remembered so grid rebuilds (clearing the view or future
    /// PTY rebuild paths) keep the configured scrollback limits.
    pub fn configure_terminal_limits(&mut self, terminal: &TerminalProfile) {
        self.terminal_limits = (terminal.scrollback_lines, terminal.scrollback_max_cells);
        self.terminal_grid
            .set_scrollback_limits(terminal.scrollback_lines, terminal.scrollback_max_cells);
        self.clamp_view_state();
        self.frame_id = self.frame_id.wrapping_add(1);
    }

    pub fn configure_logging(&mut self, config_dir: &Path, logging: &LoggingProfile) {
        // N6：手动日志优先。手动日志开启过（含已停止）后，本会话不再自动打开
        // 配置驱动的自动日志，保持当前输出不变（避免与手动输出抢占同一文件）。
        if self.auto_logging_suppressed {
            return;
        }
        self.session_logger = None;
        self.transfer_logger = None;
        self.logging_notice = None;

        if !logging.enabled {
            return;
        }

        let log_root = logging_root(config_dir, logging);
        let session_context = LogPathContext::now(self.session_id.as_str(), "terminal");
        let session_log_path = log_root.join(
            PathTemplate::new("{session_id}/{kind}-{timestamp}.log").render(&session_context),
        );
        let transfer_context = LogPathContext::now(self.session_id.as_str(), "transfer");
        let transfer_log_path = log_root.join(
            PathTemplate::new("{session_id}/{kind}-{timestamp}.log").render(&transfer_context),
        );
        let transcript_format = transcript_format(logging);

        match SessionLogger::open(
            &session_log_path,
            transcript_format,
            Redactor::default(),
            RotationPolicy::disabled(),
        ) {
            Ok(logger) => {
                self.session_logger = Some(logger);
            }
            Err(error) => {
                self.set_logging_notice(format!(
                    "session transcript logging could not start at `{}`: {error}",
                    session_log_path.display()
                ));
            }
        }

        match TransferLogger::open(&transfer_log_path, Redactor::default()) {
            Ok(logger) => {
                self.transfer_logger = Some(logger);
            }
            Err(error) => {
                self.set_logging_notice(format!(
                    "transfer logging could not start at `{}`: {error}",
                    transfer_log_path.display()
                ));
            }
        }
    }

    pub fn poll_shell_output(&mut self) -> Result<Vec<Vec<u8>>, yshell_ssh::SshError> {
        let Some(shell_session) = self.shell_session.as_mut() else {
            return Ok(Vec::new());
        };
        let mut chunks = Vec::new();
        loop {
            let bytes = shell_session.poll_output()?;
            if bytes.is_empty() {
                break;
            }
            chunks.push(bytes);
        }
        for bytes in &chunks {
            self.terminal_parser.advance(&mut self.terminal_grid, bytes);
            self.record_transcript_chunk(TranscriptDirection::Output, bytes);
        }
        if !chunks.is_empty() {
            self.frame_id = self.frame_id.wrapping_add(1);
            self.clamp_view_state();
        }
        Ok(chunks)
    }

    pub fn shell_is_connected(&self) -> Option<bool> {
        self.shell_session
            .as_ref()
            .map(|session| session.is_connected())
    }

    pub fn write_terminal_input(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<Vec<u8>>, yshell_ssh::SshError> {
        let Some(shell_session) = self.shell_session.as_mut() else {
            return Ok(Vec::new());
        };
        shell_session.write_input(bytes)?;
        // N6：勾选"记录本地输入"的手动日志按 Input 方向记录（自动日志不记输入，
        // 沿用 G0 语义）。
        self.record_transcript_chunk(TranscriptDirection::Input, bytes);
        self.poll_shell_output()
    }

    pub fn resize_shell_pty(
        &mut self,
        columns: u16,
        rows: u16,
    ) -> Result<Vec<Vec<u8>>, yshell_ssh::SshError> {
        let size = PtySize {
            columns,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        };
        self.ssh_config.pty.size = size;
        // The renderer grid must follow the PTY size even before a shell is
        // attached, otherwise the first frame would not match the viewport.
        self.terminal_grid.resize(columns, rows);
        self.clamp_view_state();
        self.frame_id = self.frame_id.wrapping_add(1);
        let Some(shell_session) = self.shell_session.as_mut() else {
            return Ok(Vec::new());
        };
        shell_session.resize_pty(size)?;
        self.poll_shell_output()
    }

    pub fn current_pty_size(&self) -> PtySize {
        self.ssh_config.pty.size
    }

    pub fn disconnect_shell(&mut self) -> Result<(), yshell_ssh::SshError> {
        let Some(shell_session) = self.shell_session.as_mut() else {
            return Ok(());
        };
        shell_session.disconnect()
    }

    pub fn record_transfer(
        &mut self,
        operation: &str,
        local_path: &Path,
        remote_path: &Path,
        bytes: u64,
    ) {
        let result = if let Some(logger) = self.transfer_logger.as_mut() {
            logger
                .record_transfer(operation, local_path, remote_path, bytes)
                .err()
        } else {
            None
        };
        if let Some(error) = result {
            self.set_logging_notice(format!(
                "transfer logging failed for `{operation}`: {error}"
            ));
        }
    }

    pub fn take_logging_notice(&mut self) -> Option<String> {
        self.logging_notice.take()
    }

    pub fn to_session_profile(&self, profile_id: String) -> SessionProfile {
        let mut profile = SessionProfile::new(
            profile_id,
            self.display_name.clone(),
            self.ssh_config.host.clone(),
        );
        profile.port = self.ssh_config.port;
        profile.username = Some(self.ssh_config.username().to_owned());
        profile.host_key_policy = Some(crate::runtime::runtime_host_key_policy_to_config(
            &self.ssh_config.host_key_policy,
        ));
        profile
    }

    fn record_transcript_chunk(&mut self, direction: TranscriptDirection, bytes: &[u8]) {
        // 输出始终写入当前日志；输入只在手动日志勾选"记录本地输入"时写入。
        if direction == TranscriptDirection::Input && !self.manual_logging_includes_input() {
            return;
        }
        let failed = match self.session_logger.as_mut() {
            Some(logger) => logger.record(direction, bytes).is_err(),
            None => false,
        };
        if failed {
            self.stop_logging_after_write_failure();
        }
    }

    /// N6：写失败 → 自动停止日志（保留已写内容），并把 `last_error` 折进日志
    /// 提示（状态栏在下一个 tick 折叠显示）。
    fn stop_logging_after_write_failure(&mut self) {
        let (path_text, last_error) = match self.session_logger.as_ref() {
            Some(logger) => (
                logger.path().display().to_string(),
                logger.last_error().map(ToString::to_string),
            ),
            None => (String::new(), None),
        };
        // 释放失败的文件句柄；写入内容已在每次 record 时 flush 落盘。
        self.session_logger = None;
        self.manual_logging = None;
        if !path_text.is_empty() {
            self.last_logging_path = Some(PathBuf::from(&path_text));
        }
        let detail = last_error.unwrap_or_else(|| "unknown write failure".to_owned());
        // 写失败是当前最需要反馈的信息：覆盖可能存在的旧提示。
        self.logging_notice = Some(format!(
            "session logging stopped after a write failure at `{path_text}`: {detail}"
        ));
    }

    fn set_logging_notice(&mut self, notice: String) {
        if self.logging_notice.is_none() {
            self.logging_notice = Some(notice);
        }
    }
}

/// N6：运行时手动日志开关（弹窗 → 会话；右键菜单/状态栏/REC 接线在 Phase 2）。
///
/// 设计：`docs/product/yshell-next-n6-logging-ui.md` §3。手动日志优先——同一
/// 会话同一时刻只有一个 [`SessionLogger`] 输出，开启手动日志会暂停自动日志，
/// 且停止后不自动恢复（见 `auto_logging_suppressed`）。
///
/// Phase 1 只交付模块与单测，接线前该 impl 允许 dead_code（Phase 2 接线完成
/// 后删除此属性）。
#[allow(dead_code)]
impl SessionRuntime {
    /// N6：运行时开启到指定文件的手动日志。
    ///
    /// 新文件先打开、成功后再替换当前输出，因此打开失败时既有的（自动）日志
    /// 不受影响。返回实际打开的日志文件路径。
    pub fn start_logging(
        &mut self,
        options: SessionLogOptions,
        include_input: bool,
    ) -> io::Result<PathBuf> {
        let logger = SessionLogger::open_with(options)?;
        let path = logger.path().clone();
        if let Some(previous) = self.session_logger.take() {
            if let Err(error) = previous.close() {
                self.set_logging_notice(format!(
                    "previous logging output could not be closed cleanly: {error}"
                ));
            }
        }
        self.manual_logging = Some(ManualLoggingOptions { include_input });
        self.auto_logging_suppressed = true;
        self.last_logging_path = Some(path.clone());
        self.session_logger = Some(logger);
        Ok(path)
    }

    /// N6：停止当前日志输出（手动或自动）并 flush。
    ///
    /// * `Ok(None)`：当前没有活动日志输出。
    /// * `Ok(Some(path))`：已 flush 并关闭，返回日志文件路径。
    /// * `Err(error)`：flush/关闭失败；已写内容保留，文件路径见
    ///   [`Self::last_logging_path`]。
    pub fn stop_logging(&mut self) -> io::Result<Option<PathBuf>> {
        self.manual_logging = None;
        let Some(logger) = self.session_logger.take() else {
            return Ok(None);
        };
        let path = logger.path().clone();
        self.last_logging_path = Some(path.clone());
        logger.close().map(|()| Some(path))
    }

    /// N6：当前是否有日志输出（自动或手动）。
    #[must_use]
    pub const fn logging_active(&self) -> bool {
        self.session_logger.is_some()
    }

    /// N6：当前输出是否由手动日志开启。
    #[must_use]
    pub const fn manual_logging_active(&self) -> bool {
        self.manual_logging.is_some()
    }

    /// N6：当前输出类型（REC 指示 / 菜单启用条件）。
    #[must_use]
    pub const fn logging_mode(&self) -> SessionLoggingMode {
        if self.manual_logging.is_some() {
            SessionLoggingMode::Manual
        } else if self.session_logger.is_some() {
            SessionLoggingMode::Auto
        } else {
            SessionLoggingMode::Off
        }
    }

    /// N6：活动日志文件路径。
    #[must_use]
    pub fn logging_path(&self) -> Option<&Path> {
        self.session_logger
            .as_ref()
            .map(|logger| logger.path().as_path())
    }

    /// N6：最近一次日志文件路径（停止/写失败后保留）。
    #[must_use]
    pub fn last_logging_path(&self) -> Option<&Path> {
        self.last_logging_path.as_deref()
    }

    /// N6：手动日志期间是否记录本地输入（R-106）。
    #[must_use]
    pub const fn manual_logging_includes_input(&self) -> bool {
        match &self.manual_logging {
            Some(options) => options.include_input,
            None => false,
        }
    }
}

fn logging_root(config_dir: &Path, logging: &LoggingProfile) -> PathBuf {
    let configured = logging
        .directory
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("logs"));
    if configured.is_absolute() {
        configured
    } else {
        config_dir.join(configured)
    }
}

fn transcript_format(logging: &LoggingProfile) -> TranscriptFormat {
    if logging.format.eq_ignore_ascii_case("raw") {
        TranscriptFormat::Raw
    } else {
        TranscriptFormat::Sanitized
    }
}

#[cfg(test)]
mod tests {
    use yshell_ssh::ShellClient;
    use yshell_terminal::TerminalCell;

    use super::*;

    fn cells_text(cells: &[TerminalCell]) -> String {
        cells
            .iter()
            .filter(|cell| !cell.wide_continuation)
            .map(|cell| cell.grapheme.as_str())
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    fn runtime_with_output(columns: u16, rows: u16, output: &str) -> SessionRuntime {
        let mut runtime = SessionRuntime::draft(1);
        runtime
            .resize_shell_pty(columns, rows)
            .expect("resize draft grid");
        // Start from an empty grid: shrinking the screen now saves rows above
        // the cursor into scrollback, and these tests measure only the output
        // they feed themselves.
        runtime.terminal_grid = TerminalGrid::with_limits(
            columns,
            rows,
            DEFAULT_SCROLLBACK_LINES,
            DEFAULT_SCROLLBACK_MAX_CELLS,
        );
        runtime.terminal_parser = TerminalParser::new();
        runtime
            .terminal_parser
            .advance(&mut runtime.terminal_grid, output.as_bytes());
        runtime
    }

    #[test]
    fn resize_shell_pty_resizes_the_render_grid_without_a_shell() {
        let mut runtime = SessionRuntime::draft(1);
        runtime
            .terminal_parser
            .advance(&mut runtime.terminal_grid, b"hello\nworld");
        let before = runtime.frame_id();

        runtime.resize_shell_pty(140, 40).expect("resize");

        assert_eq!(runtime.terminal_grid.columns, 140);
        assert_eq!(runtime.terminal_grid.rows, 40);
        assert!(runtime
            .terminal_grid
            .line_text(0)
            .contains("Draft session created"));
        assert_eq!(runtime.terminal_grid.line_text(1), "hello");
        assert_eq!(runtime.terminal_grid.line_text(2), "world");
        assert!(runtime.frame_id() > before);
        assert_eq!(runtime.current_pty_size().columns, 140);
        assert_eq!(runtime.current_pty_size().rows, 40);
    }

    #[test]
    fn resize_shell_pty_resizes_the_grid_with_a_fake_shell_attached() {
        let mut runtime = SessionRuntime::draft(1);
        let shell = ShellClient::new()
            .open_shell(&runtime.ssh_config)
            .expect("open fake shell");
        runtime.attach_shell_session(Box::new(shell));
        runtime.poll_shell_output().expect("banner");

        runtime.resize_shell_pty(64, 20).expect("resize");

        assert_eq!(runtime.terminal_grid.columns, 64);
        assert_eq!(runtime.terminal_grid.rows, 20);
        assert!(runtime
            .visible_text()
            .contains("fake-shell resized to 64x20"));
    }

    #[test]
    fn scrollback_offsets_clamp_and_drive_the_render_snapshot() {
        let mut runtime = runtime_with_output(10, 2, "one\ntwo\nthree\nfour");

        assert_eq!(runtime.terminal_grid.scrollback_len(), 2);
        assert!(runtime.scroll_terminal(1));
        assert_eq!(runtime.viewport_offset(), 1);
        let snapshot = runtime.terminal_render_snapshot().expect("snapshot");
        assert_eq!(cells_text(&snapshot.lines[0]), "two");
        assert_eq!(cells_text(&snapshot.lines[1]), "three");
        assert_eq!(snapshot.cursor, None);
        assert!(runtime.scroll_terminal(-5), "scrolls back to the bottom");
        assert_eq!(runtime.viewport_offset(), 0);
        assert!(!runtime.scroll_terminal(-1), "already at the bottom");
        assert!(runtime.scroll_terminal(99));
        assert_eq!(runtime.viewport_offset(), 2);
        assert!(!runtime.scroll_terminal(1), "already clamped at the top");
        assert!(runtime.scroll_terminal_to_bottom());
        let snapshot = runtime.terminal_render_snapshot().expect("snapshot");
        assert_eq!(snapshot.cursor, Some((4, 1)));
    }

    #[test]
    fn absolute_selection_text_spans_scrollback_and_viewport() {
        let mut runtime = runtime_with_output(10, 2, "one\ntwo\nthree\nfour");

        runtime.begin_terminal_selection(GridPoint { column: 0, row: 1 });
        runtime.update_terminal_selection(GridPoint { column: 2, row: 2 });

        assert!(runtime.terminal_selection_active());
        assert_eq!(runtime.terminal_selection_text(), "two\nthr");

        runtime.select_all_terminal();
        assert_eq!(runtime.terminal_selection_text(), "one\ntwo\nthree\nfour");

        runtime.scroll_terminal(1);
        let snapshot = runtime.terminal_render_snapshot().expect("snapshot");
        let selection = snapshot.selection.expect("viewport selection");
        assert_eq!(selection.start, GridPoint { column: 0, row: 0 });
        assert_eq!(selection.end.row, 1);
        assert_eq!(selection.end.column, 9);
    }

    #[test]
    fn viewport_metrics_describe_the_scrolled_window() {
        let mut runtime = runtime_with_output(10, 2, "one\ntwo\nthree\nfour");
        let metrics = runtime.terminal_viewport_metrics();
        assert_eq!(metrics.columns, 10);
        assert_eq!(metrics.rows, 2);
        assert_eq!(metrics.top_absolute_row, 2);
        assert_eq!(metrics.viewport_offset, 0);
        assert!(!runtime.terminal_selection_active());

        runtime.scroll_terminal(1);
        let metrics = runtime.terminal_viewport_metrics();
        assert_eq!(metrics.top_absolute_row, 1);
        assert_eq!(metrics.viewport_offset, 1);
    }

    #[test]
    fn word_selection_uses_absolute_lines() {
        let mut runtime = runtime_with_output(16, 2, "git commit\nsecond line");

        let word = runtime.select_word_at(GridPoint { column: 2, row: 1 });
        assert!(word);
        assert_eq!(runtime.terminal_selection_text(), "second");
        assert!(!runtime.select_word_at(GridPoint { column: 3, row: 0 }));
        assert!(!runtime.terminal_selection_active());
    }

    #[test]
    fn configure_terminal_limits_applies_immediately_and_survives_clear() {
        let mut runtime = SessionRuntime::draft(1);
        runtime.configure_terminal_limits(&TerminalProfile {
            scrollback_lines: 2,
            scrollback_max_cells: 100_000,
            ..TerminalProfile::default()
        });

        let mut output = String::new();
        for index in 0..40 {
            output.push_str(&format!("line {index}\n"));
        }
        runtime
            .terminal_parser
            .advance(&mut runtime.terminal_grid, output.as_bytes());

        assert_eq!(runtime.terminal_grid.scrollback_len(), 2);
        assert!(runtime
            .terminal_grid
            .line_text_absolute(0)
            .expect("oldest line")
            .starts_with("line "));
        assert!(runtime
            .terminal_grid
            .line_text_absolute(1)
            .expect("newest line")
            .starts_with("line "));

        runtime.clear_visible_terminal();

        assert_eq!(runtime.terminal_grid.scrollback_limits(), (2, 100_000));
        assert_eq!(runtime.terminal_grid.scrollback_len(), 0);

        runtime.resize_shell_pty(96, 24).expect("resize draft grid");

        assert_eq!(runtime.terminal_grid.scrollback_limits(), (2, 100_000));
    }

    #[test]
    fn selection_and_render_snapshot_span_scrollback_after_resize() {
        let mut runtime = runtime_with_output(10, 3, "one\ntwo\nthree\nfour\nfive");
        assert_eq!(runtime.terminal_grid.scrollback_len(), 2);

        runtime.resize_shell_pty(6, 2).expect("resize");

        assert_eq!(runtime.terminal_grid.scrollback_len(), 3);
        assert_eq!(
            runtime.terminal_grid.line_text_absolute(0).as_deref(),
            Some("one")
        );

        runtime.select_all_terminal();
        assert_eq!(
            runtime.terminal_selection_text(),
            "one\ntwo\nthree\nfour\nfive"
        );

        runtime.scroll_terminal(2);
        let snapshot = runtime.terminal_render_snapshot().expect("snapshot");
        assert_eq!(cells_text(&snapshot.lines[0]), "two");
        assert_eq!(cells_text(&snapshot.lines[1]), "three");
    }
}
