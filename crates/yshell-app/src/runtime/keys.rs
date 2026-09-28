//! Active terminal input: keys, clipboard, search, selection, scrolling and resize.

use crate::{
    error::AppError, error::AppResult, session_runtime::SessionRuntime,
    session_runtime::TerminalViewportMetrics,
};
use yshell_core::{CommandDispatcher, SessionCommand};
use yshell_terminal::{GridPoint, TerminalSnapshot};

use super::*;

impl AppRuntime {
    pub fn send_active_terminal_input(&mut self, input: &str) -> AppResult<AppProjection> {
        self.send_active_terminal_bytes(input.as_bytes())
    }

    pub fn send_active_terminal_key(
        &mut self,
        text: &str,
        ctrl: bool,
        alt: bool,
        shift: bool,
        meta: bool,
    ) -> AppResult<AppProjection> {
        let bytes = yshell_terminal::encode_key(text, ctrl, alt, shift, meta);
        let projection = self.send_active_terminal_bytes(&bytes)?;
        // N9：控制键（Ctrl+C 等）广播后给状态栏 chip 一次非阻塞提示（不弹窗）。
        // IME 组合在提交前不会到达这里，因此只有提交后的文本才会触发。
        if self.note_input_sync_control_broadcast(text, ctrl, alt, shift, meta, &bytes) {
            return Ok(self.projection());
        }
        Ok(projection)
    }

    pub fn send_active_terminal_bytes(&mut self, bytes: &[u8]) -> AppResult<AppProjection> {
        if bytes.is_empty() {
            return Ok(self.projection());
        }
        let session_key = self.active_session_key()?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        self.dispatcher
            .dispatch(SessionCommand::SendTerminalInput {
                session_id,
                bytes: bytes.to_vec(),
            })
            .map_err(AppError::from_error)?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let _ = runtime
            .write_terminal_input(bytes)
            .map_err(AppError::from_error)?;
        // N9：唯一汇聚点的扇出——源写成功后把同一批字节按稳定顺序写入目标
        // （目标只写输入；目标回显由各自轮询进各自网格，不再回灌）。
        self.broadcast_synced_input(bytes);
        // D4：正常按键/输入不写状态栏（此前每个字符都刷成 "Sent N bytes…"）。
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub fn copy_active_terminal_visible_text(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let visible = self
            .sessions
            .get(&session_key)
            .map(SessionRuntime::visible_text)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        self.terminal_clipboard = visible;
        self.status_text = if self.terminal_clipboard.trim().is_empty() {
            "Copied visible terminal view, but it was empty.".to_owned()
        } else {
            format!(
                "Copied {} characters from the visible terminal view into the app clipboard.",
                self.terminal_clipboard.chars().count()
            )
        };
        Ok(self.projection())
    }

    pub fn paste_terminal_clipboard(&mut self) -> AppResult<AppProjection> {
        if self.terminal_clipboard.is_empty() {
            self.status_text = "Terminal clipboard is empty, so nothing was pasted.".to_owned();
            return Ok(self.projection());
        }
        let mut payload = self.terminal_clipboard.clone();
        if !payload.ends_with('\n') {
            payload.push('\n');
        }
        self.send_active_terminal_input(&payload)
    }

    pub fn paste_text_into_terminal(&mut self, text: &str) -> AppResult<AppProjection> {
        if text.is_empty() {
            self.status_text = "Clipboard is empty, so nothing was pasted.".to_owned();
            return Ok(self.projection());
        }
        self.terminal_clipboard = text.to_owned();
        // Paste sends the clipboard bytes verbatim; terminals do not append a
        // newline, so pasted text waits for the user to press Enter.
        self.send_active_terminal_bytes(text.as_bytes())
    }

    pub fn clear_active_terminal(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        runtime.clear_visible_terminal();
        self.terminal_search_query.clear();
        self.terminal_search_matches.clear();
        self.terminal_search_current_index = None;
        self.status_text = "Cleared the visible terminal view.".to_owned();
        Ok(self.projection())
    }

    pub fn find_in_active_terminal(&mut self, query: &str) -> AppResult<AppProjection> {
        self.terminal_search_query = query.to_owned();
        let session_key = self.active_session_key()?;
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let matches = runtime.find_visible_text(query);
        self.terminal_search_matches = matches;
        self.terminal_search_current_index = if self.terminal_search_matches.is_empty() {
            None
        } else {
            Some(0)
        };
        self.status_text = self.terminal_search_summary_legacy_text();
        Ok(self.projection())
    }

    pub fn select_next_terminal_match(&mut self) -> AppResult<AppProjection> {
        self.rotate_terminal_match(true);
        self.status_text = self.terminal_search_summary_legacy_text();
        Ok(self.projection())
    }

    pub fn select_previous_terminal_match(&mut self) -> AppResult<AppProjection> {
        self.rotate_terminal_match(false);
        self.status_text = self.terminal_search_summary_legacy_text();
        Ok(self.projection())
    }

    pub fn terminal_clipboard_text(&self) -> &str {
        &self.terminal_clipboard
    }

    #[must_use]
    pub fn active_terminal_frame_id(&self) -> u64 {
        self.active_terminal_runtime()
            .map(SessionRuntime::frame_id)
            .unwrap_or(0)
    }

    #[must_use]
    pub fn active_terminal_scroll_offset(&self) -> u64 {
        self.active_terminal_runtime()
            .map(|runtime| runtime.viewport_offset() as u64)
            .unwrap_or(0)
    }

    #[must_use]
    pub fn active_terminal_scroll_geometry(&self) -> (usize, usize) {
        self.active_terminal_viewport_metrics()
            .map(|metrics| {
                // `top_absolute_row + viewport_offset` reconstructs the scrollback length
                // without reaching into the session runtime.
                let scrollback = metrics
                    .top_absolute_row
                    .saturating_add(metrics.viewport_offset);
                (
                    scrollback.saturating_add(usize::from(metrics.rows)),
                    usize::from(metrics.rows),
                )
            })
            .unwrap_or((0, 0))
    }

    #[must_use]
    pub fn active_terminal_selection_active(&self) -> bool {
        self.active_terminal_runtime()
            .map(SessionRuntime::terminal_selection_active)
            .unwrap_or(false)
    }

    #[must_use]
    pub fn active_terminal_selection_text(&self) -> String {
        self.active_terminal_runtime()
            .map(SessionRuntime::terminal_selection_text)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn active_terminal_viewport_metrics(&self) -> Option<TerminalViewportMetrics> {
        self.active_terminal_runtime()
            .map(SessionRuntime::terminal_viewport_metrics)
    }

    #[must_use]
    pub fn active_terminal_render_snapshot(&self) -> Option<TerminalSnapshot<'_>> {
        self.active_terminal_runtime()
            .and_then(SessionRuntime::terminal_render_snapshot)
    }

    pub fn scroll_active_terminal(&mut self, delta: i32) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            self.status_text = "No active terminal to scroll.".to_owned();
            return Ok(self.projection());
        };
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let changed = runtime.scroll_terminal(delta);
        self.status_text = if runtime.viewport_offset() == 0 {
            "Terminal scrolled to the live bottom.".to_owned()
        } else if changed {
            format!(
                "Terminal scrolled back {} line(s) into scrollback.",
                runtime.viewport_offset()
            )
        } else {
            "Terminal is already at the top of the scrollback buffer.".to_owned()
        };
        Ok(self.projection())
    }

    pub fn scroll_active_terminal_to_line(
        &mut self,
        line_from_top: i32,
    ) -> AppResult<AppProjection> {
        let Some(current_top) = self
            .active_terminal_viewport_metrics()
            .map(|metrics| metrics.top_absolute_row)
        else {
            self.status_text = "No active terminal to scroll.".to_owned();
            return Ok(self.projection());
        };
        let target = usize::try_from(line_from_top.max(0)).unwrap_or(usize::MAX);
        // The viewport top is `scrollback_len - viewport_offset`, so the required
        // delta is simply the distance to the requested absolute line; the
        // session runtime clamps the resulting offset to `[0, scrollback_len]`.
        let delta = if target >= current_top {
            -i32::try_from(target - current_top).unwrap_or(i32::MAX)
        } else {
            i32::try_from(current_top - target).unwrap_or(i32::MAX)
        };
        self.scroll_active_terminal(delta)
    }

    pub fn scroll_active_terminal_to_bottom(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            self.status_text = "No active terminal to scroll.".to_owned();
            return Ok(self.projection());
        };
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        if runtime.scroll_terminal_to_bottom() {
            self.status_text = "Terminal scrolled to the live bottom.".to_owned();
        }
        Ok(self.projection())
    }

    pub fn begin_active_terminal_selection(
        &mut self,
        column: u16,
        absolute_row: u16,
    ) -> AppResult<AppProjection> {
        self.update_active_terminal_selection_with(column, absolute_row, |runtime, point| {
            runtime.begin_terminal_selection(point)
        })
    }

    pub fn update_active_terminal_selection(
        &mut self,
        column: u16,
        absolute_row: u16,
    ) -> AppResult<AppProjection> {
        self.update_active_terminal_selection_with(column, absolute_row, |runtime, point| {
            runtime.update_terminal_selection(point)
        })
    }

    pub fn select_word_in_active_terminal(
        &mut self,
        column: u16,
        absolute_row: u16,
    ) -> AppResult<AppProjection> {
        self.update_active_terminal_selection_with(column, absolute_row, |runtime, point| {
            runtime.select_word_at(point)
        })
    }

    pub fn select_all_active_terminal(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            self.status_text = "No active terminal to select.".to_owned();
            return Ok(self.projection());
        };
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        if runtime.select_all_terminal() {
            self.status_text = format!(
                "Selected {} characters from the terminal.",
                runtime.terminal_selection_text().chars().count()
            );
        }
        Ok(self.projection())
    }

    pub fn copy_active_terminal_selection(&mut self) -> AppResult<AppProjection> {
        self.active_session_key()?;
        let text = self.active_terminal_selection_text();
        if text.is_empty() {
            self.status_text = "No terminal selection to copy.".to_owned();
            return Ok(self.projection());
        }
        self.terminal_clipboard = text;
        self.status_text = format!(
            "Copied {} characters from the terminal selection.",
            self.terminal_clipboard.chars().count()
        );
        Ok(self.projection())
    }

    pub(crate) fn update_active_terminal_selection_with(
        &mut self,
        column: u16,
        absolute_row: u16,
        update: impl FnOnce(&mut SessionRuntime, GridPoint) -> bool,
    ) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            return Ok(self.projection());
        };
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let _ = update(
            runtime,
            GridPoint {
                column,
                row: absolute_row,
            },
        );
        Ok(self.projection())
    }

    pub(crate) fn active_terminal_runtime(&self) -> Option<&SessionRuntime> {
        self.active_session_id
            .as_ref()
            .and_then(|session_key| self.sessions.get(session_key))
    }

    pub fn resize_active_terminal(&mut self, columns: u16, rows: u16) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        self.dispatcher
            .dispatch(SessionCommand::ResizeTerminal {
                session_id,
                columns,
                rows,
            })
            .map_err(AppError::from_error)?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let _ = runtime
            .resize_shell_pty(columns, rows)
            .map_err(AppError::from_error)?;
        // 只有已连接的会话才用"已调整终端尺寸"更新状态栏：断开/失败的会话
        // （例如密码错误的连接失败）需要把失败原因留在状态栏直到用户下一步操作。
        let session_connected = self
            .sessions
            .get(&session_key)
            .is_some_and(|session| session.state == yshell_core::SessionState::Connected);
        if session_connected {
            self.set_status_kind(
                "terminal-resized",
                format!("Resized runtime terminal to {}x{}.", columns, rows),
                format!("{columns}x{rows}"),
                String::new(),
            );
        }
        Ok(self.projection())
    }

    pub fn sync_active_terminal_size_passive(
        &mut self,
        columns: u16,
        rows: u16,
    ) -> AppResult<Option<AppProjection>> {
        let Some(session_key) = self.active_session_id.clone() else {
            return Ok(None);
        };
        let current_size = self
            .sessions
            .get(&session_key)
            .map(SessionRuntime::current_pty_size)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        if current_size.columns == columns && current_size.rows == rows {
            return Ok(None);
        }
        self.resize_active_terminal(columns, rows).map(Some)
    }

    pub(crate) fn refresh_terminal_search_for_active_session(&mut self) {
        let query = self.terminal_search_query.clone();
        if query.trim().is_empty() {
            self.terminal_search_matches.clear();
            self.terminal_search_current_index = None;
            return;
        }
        let matches = self
            .active_terminal_runtime()
            .map(|runtime| runtime.find_visible_text(&query))
            .unwrap_or_default();
        self.terminal_search_current_index = if matches.is_empty() { None } else { Some(0) };
        self.terminal_search_matches = matches;
    }

    pub(crate) fn rotate_terminal_match(&mut self, forward: bool) {
        if self.terminal_search_matches.is_empty() {
            return;
        }
        let len = self.terminal_search_matches.len();
        let current = self.terminal_search_current_index.unwrap_or(0);
        let next = if forward {
            (current + 1) % len
        } else if current == 0 {
            len - 1
        } else {
            current - 1
        };
        self.terminal_search_current_index = Some(next);
    }

    pub(crate) fn terminal_search_summary_parts(&self) -> (&'static str, i32, i32) {
        if self.terminal_search_query.trim().is_empty() {
            return ("prompt", 0, 0);
        }
        if self.terminal_search_matches.is_empty() {
            return ("empty", 0, 0);
        }
        let count = i32::try_from(self.terminal_search_matches.len()).unwrap_or(i32::MAX);
        let current =
            i32::try_from(self.terminal_search_current_index.unwrap_or(0) + 1).unwrap_or(i32::MAX);
        ("found", count, current)
    }

    pub(crate) fn terminal_search_summary_legacy_text(&self) -> String {
        match self.terminal_search_summary_parts().0 {
            "prompt" => "Find in terminal".to_owned(),
            "empty" => format!(
                "No matches for `{}` in the visible terminal.",
                self.terminal_search_query
            ),
            _ => format!(
                "Found {} match(es) for `{}` in the visible terminal. Showing {}/{}.",
                self.terminal_search_matches.len(),
                self.terminal_search_query,
                self.terminal_search_current_index.unwrap_or(0) + 1,
                self.terminal_search_matches.len()
            ),
        }
    }
}
