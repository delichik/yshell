//! Terminal tabs: entries, activation, close requests (single and batch) and the
//! session polling cursor.

use crate::{
    error::AppError,
    error::AppResult,
    session_runtime::{SessionRuntime, SessionSource},
};
use yshell_core::{CommandDispatcher, SessionCommand, SessionState};

use super::*;

/// 分屏时为第二 pane 再开同源会话的来源。
enum SiblingSplitSource {
    Saved(String),
    QuickConnect {
        host: String,
        port: u16,
        username: String,
    },
    Unsupported,
}

impl AppRuntime {
    pub fn poll_all_terminal_outputs(&mut self) -> AppResult<Option<AppProjection>> {
        let active_session = self.active_session_id.clone();
        let mut active_changed = false;
        let mut background_state_changed = false;
        let mut polled = 0usize;

        if let Some(session_key) = active_session.as_deref() {
            let outcome = self.poll_session_output(session_key)?;
            active_changed = outcome.output_changed || outcome.state_changed;
            polled += 1;
        }

        if polled < MAX_POLL_SESSIONS_PER_TICK {
            let candidates: Vec<String> = self
                .tabs
                .iter()
                .filter_map(|tab| tab.session_id().map(str::to_owned))
                .collect();
            let session_count = candidates.len();
            if session_count > 0 {
                let mut cursor = self.terminal_poll_cursor % session_count;
                let mut visited = 0usize;
                while visited < session_count && polled < MAX_POLL_SESSIONS_PER_TICK {
                    let session_key = candidates[cursor].clone();
                    cursor = (cursor + 1) % session_count;
                    visited += 1;
                    if active_session.as_deref() == Some(session_key.as_str()) {
                        continue;
                    }
                    // 后台标签只更新 grid/parser；状态/未读变化才需要刷新标签条投影。
                    // 后台会话的瞬时错误不打断整个 tick（活动会话的错误照旧上报）。
                    if let Ok(outcome) = self.poll_session_output(&session_key) {
                        background_state_changed |= outcome.state_changed;
                        if outcome.output_changed && self.mark_tab_unread_for_session(&session_key)
                        {
                            background_state_changed = true;
                        }
                    }
                    polled += 1;
                }
                self.terminal_poll_cursor = cursor % session_count;
            }
        }

        // N9：掉线/关闭的源或目标在这里自动收敛（标签级状态机只读标签与会话表）。
        let sync_changed = self.reconcile_input_sync();

        if active_changed || background_state_changed || sync_changed {
            Ok(Some(self.projection()))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn poll_session_output(
        &mut self,
        session_key: &str,
    ) -> AppResult<SessionPollOutcome> {
        // SFTP tabs have no shell channel; terminal polling must not classify
        // their (intentionally empty) shell slot as a closed terminal. Their
        // connection liveness is checked separately by
        // `AppRuntime::poll_sftp_connection` (a real SFTP probe — the
        // libssh2 `authenticated()` flag stays true after a TCP drop).
        if self
            .sessions
            .get(session_key)
            .is_some_and(SessionRuntime::is_sftp)
        {
            return Ok(SessionPollOutcome::default());
        }
        let Some((previous_terminal, previous_state)) = self
            .sessions
            .get(session_key)
            .map(|runtime| (runtime.visible_text(), runtime.state))
        else {
            return Ok(SessionPollOutcome::default());
        };
        let is_active = self.active_session_id.as_deref() == Some(session_key);
        let (chunks_empty, terminal_changed, state_changed, shell_closed) = {
            let runtime = self
                .sessions
                .get_mut(session_key)
                .ok_or_else(|| AppError::new("runtime session is missing"))?;
            let poll_result = runtime.poll_shell_output();
            let shell_connected = runtime.shell_is_connected().unwrap_or(false);
            // shell 已经掉线时 poll 返回错误是预期路径：按 shell-closed 处理而不是让
            // 整个 tick 失败。非阻塞读路径上 WouldBlock 不会冒泡，因此 Connected
            // 状态下的 poll I/O 错误也视为传输已死（否则会每帧 WARN 刷屏）。
            let mut shell_closed =
                previous_state == yshell_core::SessionState::Connected && !shell_connected;
            let chunks = match poll_result {
                Ok(chunks) => chunks,
                Err(_) if shell_closed => Vec::new(),
                Err(_) if previous_state == yshell_core::SessionState::Disconnected => Vec::new(),
                Err(error) if previous_state == yshell_core::SessionState::Connected => {
                    tracing::warn!(
                        target: "yshell::app",
                        "live shell transport failed while polling output; marking session disconnected: {error}"
                    );
                    shell_closed = true;
                    Vec::new()
                }
                Err(error) => return Err(AppError::from_error(error)),
            };
            if shell_closed {
                runtime.set_state(yshell_core::SessionState::Disconnected);
                runtime
                    .log_runtime_event("Live shell closed while the app was polling for output.");
            }
            (
                chunks.is_empty(),
                runtime.visible_text() != previous_terminal,
                runtime.state != previous_state,
                shell_closed,
            )
        };
        let output_changed = !chunks_empty || terminal_changed;
        if !output_changed && !state_changed {
            return Ok(SessionPollOutcome::default());
        }
        if shell_closed {
            let sftp_status = self.sync_sftp_lifecycle_for_session(session_key, false);
            if is_active {
                tracing::info!(
                    target: "yshell::app",
                    "active live shell closed; {sftp_status}"
                );
                self.set_status_kind(
                    "session-live-shell-closed",
                    format!(
                        "The active live shell closed while the terminal was refreshing. {sftp_status}"
                    ),
                    String::new(),
                    String::new(),
                );
            }
        }
        self.fold_logging_notice_from_session(session_key);
        Ok(SessionPollOutcome {
            output_changed,
            state_changed,
        })
    }

    pub fn activate_tab(&mut self, tab_id: &str) -> AppResult<AppProjection> {
        let Some(index) = self.tab_index(tab_id) else {
            return Err(AppError::new(format!("tab `{tab_id}` was not found")));
        };
        // New Tab pages have no runtime session; terminal/SFTP tabs do.
        let session_id = self.tabs[index].session_id().map(str::to_owned);
        if let Some(session_id) = &session_id {
            if !self.sessions.contains_key(session_id) {
                return Err(AppError::new(format!(
                    "runtime session for tab `{tab_id}` is missing"
                )));
            }
        }
        self.park_active_sftp_tab();
        let window_id = self.window_of_tab(tab_id);
        self.activate_tab_in_window(tab_id);
        if window_id.is_none_or(|id| self.focused_window == Some(id)) {
            self.active_tab_id = Some(tab_id.to_owned());
            self.active_session_id = session_id.clone();
        }
        self.tabs[index].unread = 0;
        if !self.focus_pane_for_tab(tab_id) {
            self.assign_tab_to_focused_pane(tab_id);
        }
        self.refresh_terminal_search_for_active_session();
        let title = self.tab_display_title(tab_id);
        let Some(session_id) = session_id else {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.sftp_listing = SftpListingState::RefreshNeedsSession;
            self.set_status_kind(
                "tab-activated-new",
                format!("Activated tab `{title}`."),
                title,
                String::new(),
            );
            return Ok(self.projection());
        };
        if self.tabs[index].is_sftp() {
            self.restore_sftp_tab(tab_id);
            let sftp_status = self.sync_sftp_lifecycle_for_session(&session_id, false);
            if matches!(self.sftp_listing, SftpListingState::ConnectReal) {
                // First activation of this SFTP tab: defer the listing to the
                // next UI tick so tab switching stays instant.
                self.sftp_listing_pending = true;
                self.set_status_kind(
                    "tab-activated-loading-sftp",
                    format!("Activated tab `{title}`. {sftp_status} Loading SFTP listing…"),
                    title,
                    String::new(),
                );
            } else {
                self.set_status_kind(
                    "tab-activated",
                    format!("Activated tab `{title}`. {sftp_status}"),
                    title,
                    String::new(),
                );
            }
            return Ok(self.projection());
        }
        // Terminal/New Tab pages no longer show the docked SFTP panel.
        self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
        self.sftp_listing = SftpListingState::RefreshNeedsSession;
        self.set_status_kind(
            "tab-activated",
            format!("Activated tab `{title}`."),
            title,
            String::new(),
        );
        Ok(self.projection())
    }

    pub fn request_close_tab(&mut self, tab_id: &str) -> AppResult<AppProjection> {
        let Some(index) = self.tab_index(tab_id) else {
            return Err(AppError::new(format!("tab `{tab_id}` was not found")));
        };
        if self.confirm_close_session_enabled()
            && self.tabs[index]
                .session_id()
                .is_some_and(|session_id| self.session_has_active_connection(session_id))
        {
            let title = self.tab_display_title(tab_id);
            self.pending_close_tabs = Some(PendingCloseTabs {
                tab_ids: vec![tab_id.to_owned()],
                single: true,
                active_connections: 1,
            });
            self.set_status_kind(
                "tab-close-confirm",
                format!("Confirm closing tab `{title}`: the session is still active."),
                title,
                String::new(),
            );
            return Ok(self.projection());
        }
        let title = self.tab_display_title(tab_id);
        let stopped_log_path = self.close_tab_immediate(tab_id);
        let (text, log_path) = match stopped_log_path {
            Some(path) => (
                format!("Closed tab `{title}`. Stopped logging; log file: `{path}`."),
                path,
            ),
            None => (format!("Closed tab `{title}`."), String::new()),
        };
        self.set_status_kind("tab-closed", text, title, log_path);
        // TAB-6：关闭标签不改变 Quick Connect 覆盖层状态；零标签 = 空工作区（D1）。
        Ok(self.projection())
    }

    pub fn request_close_tabs(&mut self, scope: &str) -> AppResult<AppProjection> {
        let targets = self.close_scope_targets(scope)?;
        if targets.is_empty() {
            self.set_status_kind(
                "tabs-close-none",
                "No tabs matched the close request.".to_owned(),
                String::new(),
                String::new(),
            );
            return Ok(self.projection());
        }
        let active_connections = targets
            .iter()
            .filter(|tab_id| {
                self.tabs
                    .iter()
                    .find(|tab| &tab.tab_id == *tab_id)
                    .and_then(|tab| tab.session_id())
                    .is_some_and(|session_id| self.session_has_active_connection(session_id))
            })
            .count();
        // 关闭顺序固定为右→左，保证剩余标签顺序稳定。
        let mut close_order = targets;
        close_order.reverse();
        let target_count = close_order.len();
        if active_connections > 0 && self.confirm_close_session_enabled() {
            self.pending_close_tabs = Some(PendingCloseTabs {
                tab_ids: close_order,
                single: false,
                active_connections,
            });
            self.set_status_kind(
                "tabs-close-confirm",
                format!(
                    "Confirm closing {target_count} tab(s): {active_connections} still active connection(s)."
                ),
                target_count.to_string(),
                active_connections.to_string(),
            );
            return Ok(self.projection());
        }
        let mut stopped_log_path = None;
        for tab_id in &close_order {
            if let Some(path) = self.close_tab_immediate(tab_id) {
                stopped_log_path = Some(path);
            }
        }
        if active_connections > 0 {
            tracing::info!(
                target: "yshell::app",
                "closed {target_count} tab(s) without confirmation; {active_connections} active connection(s) disconnected"
            );
            let mut text = format!(
                "Closed {target_count} tab(s); {active_connections} active connection(s) were disconnected without confirmation."
            );
            if let Some(path) = &stopped_log_path {
                text.push_str(&format!(" Stopped logging; log file: `{path}`."));
            }
            self.set_status_kind(
                "tabs-closed-active",
                text,
                target_count.to_string(),
                active_connections.to_string(),
            );
        } else {
            let (text, log_path) = match stopped_log_path {
                Some(path) => (
                    format!("Closed {target_count} tab(s). Stopped logging; log file: `{path}`."),
                    path,
                ),
                None => (format!("Closed {target_count} tab(s)."), String::new()),
            };
            self.set_status_kind("tabs-closed", text, target_count.to_string(), log_path);
        }
        // TAB-6：关闭标签不改变 Quick Connect 覆盖层状态；零标签 = 空工作区（D1）。
        Ok(self.projection())
    }

    /// 确认关闭；`dont_ask_again` 为真时把"关闭会话前确认"持久化为关闭，
    /// 用户可在 Settings 里重新开启。
    pub fn confirm_close_tabs_with(&mut self, dont_ask_again: bool) -> AppResult<AppProjection> {
        let Some(pending) = self.pending_close_tabs.take() else {
            return Ok(self.projection());
        };
        let single_title = if pending.single {
            pending
                .tab_ids
                .first()
                .map(|tab_id| self.tab_display_title(tab_id))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let mut closed = 0usize;
        let mut stopped_log_path = None;
        for tab_id in &pending.tab_ids {
            if self.tab_index(tab_id).is_some() {
                if let Some(path) = self.close_tab_immediate(tab_id) {
                    stopped_log_path = Some(path);
                }
                closed += 1;
            }
        }
        let base_status = if pending.single {
            format!("Closed tab `{single_title}`.")
        } else {
            format!("Closed {closed} tab(s).")
        };
        // 先落盘「不再询问」，再写状态栏文案（setter 会覆盖 status_text）。
        if dont_ask_again {
            self.set_confirm_close_session(false)?;
        }
        let (text, log_path) = match stopped_log_path {
            Some(path) => (
                format!("{base_status} Stopped logging; log file: `{path}`."),
                path,
            ),
            None => (base_status, String::new()),
        };
        if pending.single {
            self.set_status_kind("tab-closed", text, single_title, log_path);
        } else {
            self.set_status_kind("tabs-closed", text, closed.to_string(), log_path);
        }
        Ok(self.projection())
    }

    pub fn cancel_close_tabs(&mut self) -> AppProjection {
        self.pending_close_tabs = None;
        self.set_status_kind(
            "tab-close-canceled",
            "Canceled the tab close confirmation.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    pub(crate) fn tab_index(&self, tab_id: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.tab_id == tab_id)
    }

    /// D18：标签右键菜单的连接状态（针对 `tab_menu_tab_id`）。
    ///
    /// 返回 `(reconnect_enabled, disconnect_enabled)`；没有会话时都为 `false`。
    /// WS-A 的标签菜单用这两个旗标启用 Reconnect/Disconnect。
    pub(crate) fn tab_menu_connection_flags(&self) -> (bool, bool) {
        let session_key = self
            .tab_menu_tab_id
            .as_deref()
            .and_then(|tab_id| self.tab_index(tab_id))
            .and_then(|index| self.tabs[index].session_id());
        match session_key.and_then(|key| self.sessions.get(key)) {
            Some(runtime) => (
                Self::session_reconnectable(runtime),
                matches!(
                    runtime.state,
                    SessionState::Connected | SessionState::Connecting
                ),
            ),
            None => (false, false),
        }
    }

    /// 会话是否处于"可重连"状态（没连上、已断开或失败）。
    ///
    /// TAB-4：本地终端没有可重连的 SSH 连接，永远不算可重连。
    fn session_reconnectable(runtime: &SessionRuntime) -> bool {
        !runtime.is_local()
            && matches!(
                runtime.state,
                SessionState::Idle | SessionState::Disconnected | SessionState::Failed
            )
    }

    /// 标签是否可以直接重连（与右键菜单的 Reconnect 同一套判定）。
    pub(crate) fn tab_reconnectable(&self, tab_id: &str) -> bool {
        let Some(index) = self.tab_index(tab_id) else {
            return false;
        };
        let Some(session_id) = self.tabs[index].session_id() else {
            return false;
        };
        self.sessions
            .get(session_id)
            .is_some_and(Self::session_reconnectable)
    }

    /// D18：重连被右键的标签（复用会话级 reconnect）。
    pub fn reconnect_tab_session(&mut self, tab_id: &str) -> AppResult<AppProjection> {
        let session_key = self.tab_session_key(tab_id)?;
        self.reconnect_session_by_key(&session_key)
    }

    /// D18：断开被右键的标签（复用会话级 disconnect）。
    ///
    /// N9：断开后立即收敛同步发送按键状态（源断开 → 停止；目标断开 → 移出），
    /// 状态栏 chip 与角标随投影一起更新。
    pub fn disconnect_tab_session(&mut self, tab_id: &str) -> AppResult<AppProjection> {
        let session_key = self.tab_session_key(tab_id)?;
        match self.disconnect_session_by_key(&session_key) {
            Ok(_) => {
                self.reconcile_input_sync();
                Ok(self.projection())
            }
            Err(error) => Err(error),
        }
    }

    /// 标签对应的会话 key（未知标签报错）。
    fn tab_session_key(&self, tab_id: &str) -> AppResult<String> {
        self.tab_index(tab_id)
            .and_then(|index| self.tabs[index].session_id().map(str::to_owned))
            .ok_or_else(|| AppError::new(format!("tab `{tab_id}` does not have a runtime session")))
    }

    pub(crate) fn tab_display_title(&self, tab_id: &str) -> String {
        let Some(index) = self.tab_index(tab_id) else {
            return String::new();
        };
        if let Some(custom) = self.tabs[index].custom_title.as_ref() {
            return custom.clone();
        }
        if self.tabs[index].is_new_tab() {
            return "New Tab".to_owned();
        }
        self.tabs[index]
            .session_id()
            .and_then(|session_id| self.sessions.get(session_id))
            .map(|runtime| runtime.display_name.clone())
            .unwrap_or_default()
    }

    /// Input text / inline error of a specific New Tab page.
    pub(crate) fn new_tab_parts_for(&self, tab_id: &str) -> (String, String) {
        self.tab_index(tab_id)
            .filter(|index| self.tabs[*index].is_new_tab())
            .map(|index| {
                (
                    self.tabs[index].quick_connect_input.clone(),
                    self.tabs[index].quick_connect_error.clone(),
                )
            })
            .unwrap_or_default()
    }

    /// WP4：重命名标签显示名（仅内存；空串清除覆盖、回到会话名）。
    pub fn rename_tab(&mut self, tab_id: &str, title: &str) -> AppResult<AppProjection> {
        let Some(index) = self.tab_index(tab_id) else {
            return Err(AppError::new(format!("tab `{tab_id}` was not found")));
        };
        let trimmed = title.trim();
        self.tabs[index].custom_title = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        };
        let title = self.tab_display_title(tab_id);
        self.set_status_kind(
            "tab-renamed",
            format!("Renamed tab to `{title}`."),
            title,
            String::new(),
        );
        Ok(self.projection())
    }

    /// 双击标签：已断开/未连接的标签直接重连，其它情况复制同源 session。
    ///
    /// 断开的标签里没有可用的终端内容，复制它没有意义；用户双击的意图是
    /// "把它连回去"。可重连的判定与右键菜单的 Reconnect 同一套。
    pub fn tab_double_click(&mut self, tab_id: &str) -> AppResult<AppProjection> {
        if self.tab_index(tab_id).is_none() {
            return Err(AppError::new(format!("tab `{tab_id}` was not found")));
        }
        if self.tab_reconnectable(tab_id) {
            // `reconnect_session_by_key` 会顺带激活该标签（与右键 Reconnect 一致）。
            return self.reconnect_tab_session(tab_id);
        }
        self.duplicate_tab(tab_id)
    }

    /// 双击标签：复制同源 session（新开独立标签 + runtime）。
    pub fn duplicate_tab(&mut self, tab_id: &str) -> AppResult<AppProjection> {
        if self.tab_index(tab_id).is_none() {
            return Err(AppError::new(format!("tab `{tab_id}` was not found")));
        }
        let before = self.tabs.len();
        self.open_sibling_session_for_split(tab_id)?;
        if self.tabs.len() <= before {
            self.set_status_kind(
                "tab-duplicate-auth-needed",
                "Authenticate the new session to finish duplicating.".to_owned(),
                String::new(),
                String::new(),
            );
            return Ok(self.projection());
        }
        let new_tab = self
            .active_tab_id
            .clone()
            .ok_or_else(|| AppError::new("duplicated tab is missing"))?;
        self.assign_tab_to_focused_pane(&new_tab);
        let title = self.tab_display_title(&new_tab);
        self.set_status_kind(
            "tab-duplicated",
            format!("Duplicated session as `{title}`."),
            title,
            String::new(),
        );
        Ok(self.projection())
    }

    /// 把标签移入指定分屏叶子（`pane_id`；已分屏时）；未分屏则忽略。
    /// 若源叶因此变空，自动合并邻叶。
    pub fn move_tab_to_pane(&mut self, tab_id: &str, pane_id: u64) -> AppResult<AppProjection> {
        if self.tab_index(tab_id).is_none() {
            return Err(AppError::new(format!("tab `{tab_id}` was not found")));
        }
        let window_id = self
            .window_of_tab(tab_id)
            .or(self.focused_window)
            .or_else(|| self.main_window_id())
            .ok_or_else(|| AppError::new("no window is available"))?;
        {
            let window = self
                .windows
                .get_mut(&window_id)
                .ok_or_else(|| AppError::new("window registration is missing"))?;
            let Some(split) = window.split.as_mut() else {
                return Ok(self.projection());
            };
            split
                .move_tab_to_pane(tab_id, pane_id)
                .map_err(AppError::new)?;
            window.active_tab = Some(tab_id.to_owned());
        }
        self.park_active_sftp_tab();
        self.compact_window_split(window_id)?;
        if self.focused_window == Some(window_id) {
            self.active_tab_id = Some(tab_id.to_owned());
            self.active_session_id = self
                .tab_index(tab_id)
                .and_then(|index| self.tabs[index].session_id().map(str::to_owned));
            self.refresh_active_view_mirrors();
        }
        if let Some(index) = self.tab_index(tab_id) {
            self.tabs[index].unread = 0;
        }
        self.refresh_terminal_search_for_active_session();
        self.set_status_kind(
            "tab-moved-pane",
            format!("Moved tab to pane {pane_id}."),
            pane_id.to_string(),
            String::new(),
        );
        Ok(self.projection())
    }

    /// 拖到 terminal 分区：未分屏则创建两叶；已分屏则拆分目标叶（最多 10 叶）。
    ///
    /// `target_pane_id`：悬停叶 id；未分屏传 `0`。
    /// `direction`：`up|down|left|right`，可选 `:small|:equal|:major`。
    pub fn place_tab_split(
        &mut self,
        tab_id: &str,
        target_pane_id: u64,
        direction: &str,
    ) -> AppResult<AppProjection> {
        let (orientation, dragged_is_first, ratio) = Self::parse_split_drop_spec(direction)?;
        if self.tab_index(tab_id).is_none() {
            return Err(AppError::new(format!("tab `{tab_id}` was not found")));
        }
        let window_id = self
            .window_of_tab(tab_id)
            .or(self.focused_window)
            .or_else(|| self.main_window_id())
            .ok_or_else(|| AppError::new("no window is available"))?;

        let already_split = self
            .windows
            .get(&window_id)
            .is_some_and(|window| window.split.is_some());
        if already_split {
            let target = {
                let window = self
                    .windows
                    .get(&window_id)
                    .ok_or_else(|| AppError::new("window registration is missing"))?;
                let split = window
                    .split
                    .as_ref()
                    .ok_or_else(|| AppError::new("workspace is not split"))?;
                if target_pane_id != 0 && split.find_pane(target_pane_id).is_some() {
                    target_pane_id
                } else {
                    split.focused_pane
                }
            };
            {
                let window = self
                    .windows
                    .get_mut(&window_id)
                    .ok_or_else(|| AppError::new("window registration is missing"))?;
                let split = window
                    .split
                    .as_mut()
                    .ok_or_else(|| AppError::new("workspace is not split"))?;
                split
                    .split_leaf_with_tab(target, orientation, ratio, dragged_is_first, tab_id)
                    .map_err(AppError::new)?;
                window.active_tab = Some(tab_id.to_owned());
            }
            self.park_active_sftp_tab();
            self.compact_window_split(window_id)?;
            if self.focused_window == Some(window_id) {
                self.active_tab_id = Some(tab_id.to_owned());
                self.active_session_id = self
                    .tab_index(tab_id)
                    .and_then(|index| self.tabs[index].session_id().map(str::to_owned));
                self.refresh_active_view_mirrors();
            }
            if let Some(index) = self.tab_index(tab_id) {
                self.tabs[index].unread = 0;
            }
            self.refresh_terminal_search_for_active_session();
            self.set_status_kind(
                "split-pane",
                format!("Split pane {target} {}.", orientation.as_str()),
                target.to_string(),
                orientation.as_str().to_owned(),
            );
            return Ok(self.projection());
        }

        // 未分屏：需要邻接标签；若只有一个终端标签则先复制一份。
        let neighbor = self.windows.get(&window_id).and_then(|window| {
            window
                .tabs
                .iter()
                .rev()
                .find(|id| id.as_str() != tab_id)
                .cloned()
        });
        let neighbor = if let Some(neighbor) = neighbor {
            neighbor
        } else {
            let before = self.tabs.len();
            self.open_sibling_session_for_split(tab_id)?;
            if self.tabs.len() <= before {
                self.set_status_kind(
                    "split-auth-drag-needed",
                    "Authenticate the new session, then drag the tab to split.".to_owned(),
                    String::new(),
                    String::new(),
                );
                return Ok(self.projection());
            }
            self.windows
                .get(&window_id)
                .and_then(|window| {
                    window
                        .tabs
                        .iter()
                        .rev()
                        .find(|id| id.as_str() != tab_id)
                        .cloned()
                })
                .ok_or_else(|| AppError::new("split needs two session tabs"))?
        };

        {
            let window = self
                .windows
                .get_mut(&window_id)
                .ok_or_else(|| AppError::new("window registration is missing"))?;
            let (first, second) = if dragged_is_first {
                (Some(tab_id.to_owned()), Some(neighbor.clone()))
            } else {
                (Some(neighbor.clone()), Some(tab_id.to_owned()))
            };
            let mut split = WorkspaceSplit::two_pane(orientation, first, second);
            if let SplitNode::Split {
                ratio: r,
                first,
                second,
                ..
            } = &mut split.root
            {
                *r = ratio;
                let remainder = if dragged_is_first { second } else { first };
                if let SplitNode::Leaf { pane, .. } = remainder.as_mut() {
                    for other in &window.tabs {
                        if other == &neighbor || other.as_str() == tab_id {
                            continue;
                        }
                        if !pane.tabs.iter().any(|id| id == other) {
                            pane.tabs.push(other.clone());
                        }
                    }
                }
            }
            // two_pane: first leaf id=1, second id=2；聚焦被拖侧。
            split.focused_pane = if dragged_is_first { 1 } else { 2 };
            window.split = Some(split);
            window.active_tab = Some(tab_id.to_owned());
        }
        if self.focused_window == Some(window_id) {
            self.active_tab_id = Some(tab_id.to_owned());
            self.active_session_id = self
                .tab_index(tab_id)
                .and_then(|index| self.tabs[index].session_id().map(str::to_owned));
        }
        for id in [tab_id, neighbor.as_str()] {
            if let Some(index) = self.tab_index(id) {
                self.tabs[index].unread = 0;
            }
        }
        self.refresh_terminal_search_for_active_session();
        self.set_status_kind(
            "split-workspace",
            format!("Split workspace {}.", orientation.as_str()),
            orientation.as_str().to_owned(),
            String::new(),
        );
        Ok(self.projection())
    }

    /// 空叶合并；若只剩一叶则取消分屏。
    fn compact_window_split(&mut self, window_id: u64) -> AppResult<()> {
        let window = self
            .windows
            .get_mut(&window_id)
            .ok_or_else(|| AppError::new("window registration is missing"))?;
        let Some(split) = window.split.as_mut() else {
            return Ok(());
        };
        match split.compact_empty_leaves() {
            CompactOutcome::Keep => Ok(()),
            CompactOutcome::Unsplit => {
                let remaining_tab = match &split.root {
                    SplitNode::Leaf { pane, .. } => pane.active_tab.clone(),
                    SplitNode::Split { .. } => split.focused_tab().map(str::to_owned),
                };
                window.split = None;
                if let Some(tab) = remaining_tab {
                    window.active_tab = Some(tab);
                }
                Ok(())
            }
        }
    }

    /// 解析拖放规格：`dir[:size]` → (朝向, 被拖是否 pane0, pane0 占比)。
    ///
    /// `size`：`small`=0.25 / `equal`=0.5（默认）/ `major`=0.7，指被拖侧；
    /// pane0 占比在 `down`/`right` 时取 `1 - share`，并夹到 0.2..=0.8。
    fn parse_split_drop_spec(spec: &str) -> AppResult<(SplitOrientation, bool, f32)> {
        let mut parts = spec.split(':');
        let dir = parts.next().unwrap_or("").trim();
        let size = parts.next().unwrap_or("equal").trim();
        if parts.next().is_some() {
            return Err(AppError::new(format!(
                "unknown split drop spec `{spec}` (expected dir[:small|equal|major])"
            )));
        }
        let share = match size {
            "" | "equal" | "half" => 0.5_f32,
            "small" | "minor" => 0.25,
            "major" | "large" => 0.7,
            other => {
                return Err(AppError::new(format!(
                    "unknown split size `{other}` (expected small/equal/major)"
                )));
            }
        };
        let (orientation, dragged_is_first) = match dir {
            "up" => (SplitOrientation::Vertical, true),
            "down" => (SplitOrientation::Vertical, false),
            "left" => (SplitOrientation::Horizontal, true),
            "right" => (SplitOrientation::Horizontal, false),
            other => {
                return Err(AppError::new(format!(
                    "unknown split direction `{other}` (expected up/down/left/right)"
                )));
            }
        };
        let pane0_ratio = if dragged_is_first { share } else { 1.0 - share };
        Ok((orientation, dragged_is_first, pane0_ratio.clamp(0.2, 0.8)))
    }

    /// WP4：后台会话有输出时给对应标签加未读（已在任一 pane 可见则跳过）。
    ///
    /// 返回是否实际递增了未读（调用方据此触发投影）。
    pub(crate) fn mark_tab_unread_for_session(&mut self, session_key: &str) -> bool {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.session_id() == Some(session_key))
        else {
            return false;
        };
        let tab_id = self.tabs[index].tab_id.clone();
        if self.tab_is_visible(&tab_id) {
            return false;
        }
        self.tabs[index].unread = self.tabs[index].unread.saturating_add(1);
        true
    }

    /// 标签是否正在窗口的活动位或分屏任一 pane 中显示。
    pub(crate) fn tab_is_visible(&self, tab_id: &str) -> bool {
        let Some(window_id) = self.window_of_tab(tab_id) else {
            return self.active_tab_id.as_deref() == Some(tab_id);
        };
        let Some(window) = self.windows.get(&window_id) else {
            return false;
        };
        if let Some(split) = &window.split {
            return split.shows_tab(tab_id);
        }
        window.active_tab.as_deref() == Some(tab_id)
    }

    /// WP4：把标签绑到聚焦 pane（无分屏时 no-op）。
    pub(crate) fn assign_tab_to_focused_pane(&mut self, tab_id: &str) {
        let Some(window_id) = self.window_of_tab(tab_id) else {
            return;
        };
        let Some(window) = self.windows.get_mut(&window_id) else {
            return;
        };
        let Some(split) = window.split.as_mut() else {
            return;
        };
        let focused = split.focused_pane;
        let _ = split.move_tab_to_pane(tab_id, focused);
        window.active_tab = Some(tab_id.to_owned());
    }

    /// 若 `tab_id` 已属于某 pane，聚焦该 pane 并激活；返回是否已处理。
    pub(crate) fn focus_pane_for_tab(&mut self, tab_id: &str) -> bool {
        let Some(window_id) = self.window_of_tab(tab_id) else {
            return false;
        };
        let Some(window) = self.windows.get_mut(&window_id) else {
            return false;
        };
        let Some(split) = window.split.as_mut() else {
            return false;
        };
        let Some(pane_id) = split.find_pane_id_for_tab(tab_id) else {
            return false;
        };
        split.focused_pane = pane_id;
        if let Some(pane) = split.find_pane_mut(pane_id) {
            pane.active_tab = Some(tab_id.to_owned());
        }
        window.active_tab = Some(tab_id.to_owned());
        true
    }

    /// WP4：Split Right / Split Down（递归拆聚焦叶，最多 10 叶）。
    pub fn split_workspace(&mut self, direction: &str) -> AppResult<AppProjection> {
        let orientation = match direction {
            "right" | "horizontal" => SplitOrientation::Horizontal,
            "down" | "vertical" => SplitOrientation::Vertical,
            other => {
                return Err(AppError::new(format!(
                    "unknown split direction `{other}` (expected right/down)"
                )));
            }
        };
        let window_id = self
            .focused_window
            .or_else(|| self.main_window_id())
            .ok_or_else(|| AppError::new("no window is available for split"))?;
        {
            let window = self
                .windows
                .get(&window_id)
                .ok_or_else(|| AppError::new("window registration is missing"))?;
            if let Some(split) = &window.split {
                if split.pane_count() >= WorkspaceSplit::MAX_LEAVES {
                    return Err(AppError::new(format!(
                        "workspace already has the maximum of {} panes",
                        WorkspaceSplit::MAX_LEAVES
                    )));
                }
            }
        }

        let original_tab = self
            .windows
            .get(&window_id)
            .and_then(|window| window.active_tab.clone())
            .or_else(|| self.active_tab_id.clone())
            .ok_or_else(|| AppError::new("open a tab before splitting the workspace"))?;

        let has_neighbor = self.windows.get(&window_id).is_some_and(|window| {
            window
                .tabs
                .iter()
                .any(|id| id.as_str() != original_tab.as_str())
        });
        if !has_neighbor {
            let tab_count_before = self.tabs.len();
            self.open_sibling_session_for_split(&original_tab)?;
            if self.tabs.len() <= tab_count_before {
                self.set_status_kind(
                    "split-auth-menu-needed",
                    "Authenticate the new session, then choose Split again.".to_owned(),
                    String::new(),
                    String::new(),
                );
                return Ok(self.projection());
            }
        }

        let active_tab = self
            .windows
            .get(&window_id)
            .and_then(|window| window.active_tab.clone())
            .or_else(|| self.active_tab_id.clone())
            .ok_or_else(|| AppError::new("open a tab before splitting the workspace"))?;
        let neighbor = self
            .windows
            .get(&window_id)
            .and_then(|window| {
                window
                    .tabs
                    .iter()
                    .rev()
                    .find(|id| id.as_str() != active_tab.as_str())
                    .cloned()
            })
            .ok_or_else(|| {
                AppError::new("split needs two session tabs; open another session first")
            })?;

        let already_split = self
            .windows
            .get(&window_id)
            .is_some_and(|window| window.split.is_some());
        if already_split {
            // 拆聚焦叶：active 进新叶（右侧/下侧），邻接留在原叶。
            let focused = self
                .windows
                .get(&window_id)
                .and_then(|w| w.split.as_ref().map(|s| s.focused_pane))
                .unwrap_or(1);
            {
                let window = self
                    .windows
                    .get_mut(&window_id)
                    .ok_or_else(|| AppError::new("window registration is missing"))?;
                let split = window
                    .split
                    .as_mut()
                    .ok_or_else(|| AppError::new("workspace is not split"))?;
                // 确保 neighbor 在聚焦叶，active 被拖到新侧（second）。
                let _ = split.move_tab_to_pane(&neighbor, focused);
                if let Some(pane) = split.find_pane_mut(focused) {
                    if !pane.tabs.iter().any(|id| id == &neighbor) {
                        pane.tabs.push(neighbor.clone());
                    }
                    pane.active_tab = Some(neighbor.clone());
                }
                split
                    .split_leaf_with_tab(focused, orientation, 0.5, false, &active_tab)
                    .map_err(AppError::new)?;
                window.active_tab = Some(active_tab.clone());
            }
            self.compact_window_split(window_id)?;
        } else {
            let window = self
                .windows
                .get_mut(&window_id)
                .ok_or_else(|| AppError::new("window registration is missing"))?;
            let mut split = WorkspaceSplit::two_pane(
                orientation,
                Some(neighbor.clone()),
                Some(active_tab.clone()),
            );
            if let SplitNode::Split { first, .. } = &mut split.root {
                if let SplitNode::Leaf { pane, .. } = first.as_mut() {
                    for tab_id in &window.tabs {
                        if tab_id == &neighbor || tab_id == &active_tab {
                            continue;
                        }
                        if !pane.tabs.iter().any(|id| id == tab_id) {
                            pane.tabs.push(tab_id.clone());
                        }
                    }
                }
            }
            window.split = Some(split);
            window.active_tab = Some(active_tab.clone());
        }
        if self.focused_window == Some(window_id) {
            self.active_tab_id = Some(active_tab.clone());
            self.active_session_id = self
                .tab_index(&active_tab)
                .and_then(|index| self.tabs[index].session_id().map(str::to_owned));
        }
        for tab_id in [&active_tab, &neighbor] {
            if let Some(index) = self.tab_index(tab_id) {
                self.tabs[index].unread = 0;
            }
        }
        self.set_status_kind(
            "split-workspace",
            format!("Split workspace {}.", orientation.as_str()),
            orientation.as_str().to_owned(),
            String::new(),
        );
        Ok(self.projection())
    }

    /// 为分屏再开一份同源独立会话（新 tab + 新 runtime）。
    fn open_sibling_session_for_split(&mut self, source_tab_id: &str) -> AppResult<()> {
        let Some(source_index) = self.tab_index(source_tab_id) else {
            return Err(AppError::new("split source tab was not found"));
        };
        if self.tabs[source_index].is_new_tab() {
            self.open_new_tab();
            return Ok(());
        }
        let sftp = self.tabs[source_index].is_sftp();
        let session_id = self.tabs[source_index]
            .session_id()
            .map(str::to_owned)
            .ok_or_else(|| {
                AppError::new("split needs a terminal session; open or connect a session first")
            })?;
        let source = self
            .sessions
            .get(&session_id)
            .map(|runtime| match &runtime.source {
                SessionSource::SavedSession { profile_id } => {
                    SiblingSplitSource::Saved(profile_id.clone())
                }
                SessionSource::QuickConnect => SiblingSplitSource::QuickConnect {
                    host: runtime.ssh_config.host.clone(),
                    port: runtime.ssh_config.port,
                    username: runtime.ssh_config.username().to_owned(),
                },
                // TAB-4：本地终端的复制/分屏暂不派生同源 SSH 会话（避免误触网络），
                // 与旧 Draft 一样走 Unsupported；`+` 可再开一个本地标签。
                SessionSource::Draft | SessionSource::Local => SiblingSplitSource::Unsupported,
            })
            .ok_or_else(|| AppError::new("active session runtime is missing"))?;
        match source {
            SiblingSplitSource::Saved(profile_id) => {
                if sftp {
                    self.open_saved_session_sftp(&profile_id)?;
                } else {
                    self.open_saved_session(&profile_id)?;
                }
            }
            SiblingSplitSource::QuickConnect {
                host,
                port,
                username,
            } => {
                let target = yshell_config::QuickConnectTarget {
                    username: Some(username),
                    host,
                    port,
                };
                let mut runtime =
                    SessionRuntime::from_quick_connect(target, self.allocate_runtime_ordinal());
                if sftp {
                    runtime.set_view_mode(crate::session_runtime::SessionViewMode::Sftp);
                }
                self.activate_runtime_session(runtime)?;
            }
            SiblingSplitSource::Unsupported => {
                return Err(AppError::new(
                    "split needs a terminal session; open or connect a session first",
                ));
            }
        }
        Ok(())
    }

    /// WP4：聚焦指定 pane（pane id）；同步窗口活动标签与未读清除。
    pub fn focus_split_pane(&mut self, pane_id: i32) -> AppResult<AppProjection> {
        let window_id = self
            .focused_window
            .or_else(|| self.main_window_id())
            .ok_or_else(|| AppError::new("no window is available"))?;
        let pane_id = u64::try_from(pane_id).unwrap_or(0);
        if !self
            .windows
            .get(&window_id)
            .is_some_and(|window| window.split.is_some())
        {
            return Ok(self.projection());
        }
        self.park_active_sftp_tab();
        let tab_id = {
            let window = self
                .windows
                .get_mut(&window_id)
                .ok_or_else(|| AppError::new("window registration is missing"))?;
            let split = window
                .split
                .as_mut()
                .ok_or_else(|| AppError::new("workspace is not split"))?;
            if split.find_pane(pane_id).is_none() {
                return Err(AppError::new(format!("pane {pane_id} was not found")));
            }
            split.focused_pane = pane_id;
            let tab_id = split
                .find_pane(pane_id)
                .and_then(|pane| pane.active_tab.clone());
            if let Some(tab_id) = tab_id.as_ref() {
                window.active_tab = Some(tab_id.clone());
            }
            tab_id
        };
        if let Some(tab_id) = tab_id.as_deref() {
            if let Some(index) = self.tab_index(tab_id) {
                self.tabs[index].unread = 0;
            }
            if self.focused_window == Some(window_id) {
                self.active_tab_id = Some(tab_id.to_owned());
                self.active_session_id = self
                    .tab_index(tab_id)
                    .and_then(|index| self.tabs[index].session_id().map(str::to_owned));
                if self.is_sftp_tab(tab_id) {
                    self.restore_sftp_tab(tab_id);
                    if let Some(session_key) = self.active_session_id.clone() {
                        let _ = self.sync_sftp_lifecycle_for_session(&session_key, false);
                        if matches!(self.sftp_listing, SftpListingState::ConnectReal) {
                            self.sftp_listing_pending = true;
                        }
                    }
                } else {
                    self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
                    self.sftp_listing = SftpListingState::RefreshNeedsSession;
                    self.sftp_listing_pending = false;
                }
            }
            self.refresh_terminal_search_for_active_session();
        }
        self.set_status_kind(
            "pane-focused",
            format!("Focused pane {pane_id}."),
            pane_id.to_string(),
            String::new(),
        );
        Ok(self.projection())
    }

    /// WP4：关闭当前（或指定）pane；剩 1 片时拆掉分屏。
    pub fn close_split_pane(&mut self, pane_id: Option<i32>) -> AppResult<AppProjection> {
        let window_id = self
            .focused_window
            .or_else(|| self.main_window_id())
            .ok_or_else(|| AppError::new("no window is available"))?;
        self.park_active_sftp_tab();
        let closed_id = {
            let window = self
                .windows
                .get_mut(&window_id)
                .ok_or_else(|| AppError::new("window registration is missing"))?;
            let Some(mut split) = window.split.take() else {
                return Err(AppError::new("workspace is not split"));
            };
            let id = pane_id
                .and_then(|value| u64::try_from(value).ok())
                .unwrap_or(split.focused_pane);
            match split.close_pane(id) {
                Ok(CompactOutcome::Unsplit) => {
                    let tab = match &split.root {
                        SplitNode::Leaf { pane, .. } => pane.active_tab.clone(),
                        SplitNode::Split { .. } => split.focused_tab().map(str::to_owned),
                    };
                    window.active_tab = tab.or(window.active_tab.clone());
                    id
                }
                Ok(CompactOutcome::Keep) => {
                    let tab = split.focused_tab().map(str::to_owned);
                    window.active_tab = tab.or(window.active_tab.clone());
                    window.split = Some(split);
                    id
                }
                Err(message) => {
                    window.split = Some(split);
                    return Err(AppError::new(message));
                }
            }
        };
        if let Some(tab_id) = self
            .windows
            .get(&window_id)
            .and_then(|window| window.active_tab.clone())
        {
            if self.focused_window == Some(window_id) {
                self.active_tab_id = Some(tab_id.clone());
                self.active_session_id = self
                    .tab_index(&tab_id)
                    .and_then(|index| self.tabs[index].session_id().map(str::to_owned));
                self.refresh_active_view_mirrors();
            }
            if let Some(index) = self.tab_index(&tab_id) {
                self.tabs[index].unread = 0;
            }
            self.refresh_terminal_search_for_active_session();
        }
        self.set_status_kind(
            "pane-closed",
            format!("Closed pane {closed_id}."),
            closed_id.to_string(),
            String::new(),
        );
        Ok(self.projection())
    }

    /// WP4：调整某个 split 节点比例（`split_id`；立即写内存）。
    pub fn set_split_ratio(&mut self, split_id: i32, ratio: f32) -> AppResult<AppProjection> {
        let window_id = self
            .focused_window
            .or_else(|| self.main_window_id())
            .ok_or_else(|| AppError::new("no window is available"))?;
        let window = self
            .windows
            .get_mut(&window_id)
            .ok_or_else(|| AppError::new("window registration is missing"))?;
        let split = window
            .split
            .as_mut()
            .ok_or_else(|| AppError::new("workspace is not split"))?;
        let split_id = u64::try_from(split_id).unwrap_or(0);
        if !split.set_split_ratio(split_id, ratio) {
            // 兼容：旧 UI 只传 ratio 时 split_id=0 → 改第一个 splitter。
            if split_id == 0 {
                let (_, splitters) = split.layout();
                if let Some(first) = splitters.first() {
                    let id = first.split_id;
                    let _ = split.set_split_ratio(id, ratio);
                }
            } else {
                return Err(AppError::new(format!("split {split_id} was not found")));
            }
        }
        Ok(self.projection())
    }

    /// WP4：投影用分屏快照（无分屏时 active=false）。
    ///
    /// TAB-6：Quick Connect 覆盖层只叠在**聚焦叶**的终端内容区上；其它叶保持正常
    /// 终端（可点选聚焦，聚焦后覆盖层随焦点移动）。覆盖层不改变叶子/标签归属。
    pub(crate) fn workspace_split_projection(&self) -> WorkspaceSplitProjection {
        let window_id = self.projection_window_id().or(self.focused_window);
        let Some(window_id) = window_id else {
            return WorkspaceSplitProjection::default();
        };
        let Some(window) = self.windows.get(&window_id) else {
            return WorkspaceSplitProjection::default();
        };
        let Some(split) = &window.split else {
            return WorkspaceSplitProjection::default();
        };
        let (laid_leaves, laid_splitters) = split.layout();
        let leaves = laid_leaves
            .iter()
            .map(|leaf| {
                let tab_id = leaf.pane.active_tab.clone().unwrap_or_default();
                let focused = leaf.pane_id == split.focused_pane;
                let tab = self.tab_index(&tab_id).map(|index| &self.tabs[index]);
                let mode_text = tab
                    .map(|tab| tab.kind.mode_text().to_owned())
                    .unwrap_or_default();
                let (quick_connect_input_text, quick_connect_error_text) = tab
                    .map(|tab| {
                        (
                            tab.quick_connect_input.clone(),
                            tab.quick_connect_error.clone(),
                        )
                    })
                    .unwrap_or_default();
                let sftp = if tab.is_some_and(|tab| tab.is_sftp()) {
                    let state = if focused && self.active_tab_id.as_deref() == Some(tab_id.as_str())
                    {
                        Some(self.capture_live_sftp_state())
                    } else {
                        self.sftp_tab_states.get(&tab_id).cloned()
                    };
                    let session_connected = tab
                        .and_then(|tab| tab.session_id())
                        .and_then(|session_id| self.sessions.get(session_id))
                        .is_some_and(|runtime| runtime.state == SessionState::Connected);
                    state
                        .map(|state| {
                            super::sftp::SftpPaneProjection::from_state(&state, session_connected)
                        })
                        .unwrap_or_default()
                } else {
                    Default::default()
                };
                WorkspacePaneLeafProjection {
                    pane_id: i32::try_from(leaf.pane_id).unwrap_or(0),
                    x: leaf.rect.x,
                    y: leaf.rect.y,
                    w: leaf.rect.w,
                    h: leaf.rect.h,
                    empty: leaf.pane.active_tab.is_none(),
                    focused,
                    quick_connect: false,
                    tab_id: tab_id.clone(),
                    title_text: if tab_id.is_empty() {
                        String::new()
                    } else {
                        self.tab_display_title(&tab_id)
                    },
                    mode_text,
                    quick_connect_input_text,
                    quick_connect_error_text,
                    sftp,
                    tabs: self.pane_tab_data(&leaf.pane),
                }
            })
            .collect::<Vec<_>>();
        let splitters = laid_splitters
            .iter()
            .map(|s| WorkspaceSplitterProjection {
                split_id: i32::try_from(s.split_id).unwrap_or(0),
                vertical: matches!(s.orientation, SplitOrientation::Horizontal),
                x: s.rect.x,
                y: s.rect.y,
                w: s.rect.w,
                h: s.rect.h,
                parent_x: s.parent.x,
                parent_y: s.parent.y,
                parent_w: s.parent.w,
                parent_h: s.parent.h,
                ratio: s.ratio,
            })
            .collect();
        // 兼容字段：前两叶 + 根 split 比例/朝向。
        let pane0 = leaves.first();
        let pane1 = leaves.get(1);
        let root_orient = match &split.root {
            SplitNode::Split { orientation, .. } => orientation.as_str(),
            SplitNode::Leaf { .. } => "horizontal",
        };
        let root_ratio = match &split.root {
            SplitNode::Split { ratio, .. } => *ratio,
            SplitNode::Leaf { .. } => 0.5,
        };
        WorkspaceSplitProjection {
            active: true,
            orientation_text: root_orient.to_owned(),
            ratio: root_ratio,
            focused_pane: i32::try_from(split.focused_pane).unwrap_or(0),
            pane_count: i32::try_from(split.pane_count()).unwrap_or(0),
            pane0_quick_connect: pane0.map(|p| p.quick_connect).unwrap_or(false),
            pane0_tab_id: pane0.map(|p| p.tab_id.clone()).unwrap_or_default(),
            pane0_title_text: pane0.map(|p| p.title_text.clone()).unwrap_or_default(),
            pane0_empty: pane0.map(|p| p.empty).unwrap_or(true),
            pane0_tabs: pane0.map(|p| p.tabs.clone()).unwrap_or_default(),
            pane1_quick_connect: pane1.map(|p| p.quick_connect).unwrap_or(false),
            pane1_tab_id: pane1.map(|p| p.tab_id.clone()).unwrap_or_default(),
            pane1_title_text: pane1.map(|p| p.title_text.clone()).unwrap_or_default(),
            pane1_empty: pane1.map(|p| p.empty).unwrap_or(true),
            pane1_tabs: pane1.map(|p| p.tabs.clone()).unwrap_or_default(),
            can_split: split.pane_count() < WorkspaceSplit::MAX_LEAVES,
            can_close_pane: split.pane_count() > 1,
            leaves,
            splitters,
        }
    }

    /// 单个分屏叶子的标签条投影（`active` 相对该 pane 的 `active_tab`）。
    fn pane_tab_data(&self, pane: &super::SplitPane) -> Vec<TabData> {
        pane.tabs
            .iter()
            .map(|tab_id| {
                let active = pane.active_tab.as_deref() == Some(tab_id.as_str());
                self.tab_data_entry(tab_id, active)
            })
            .collect()
    }

    /// WP4：非聚焦 pane 的会话 key（次图渲染；多叶时取第一个非聚焦叶）。
    pub(crate) fn window_secondary_session(&self, window_id: u64) -> Option<&str> {
        let window = self.windows.get(&window_id)?;
        let split = window.split.as_ref()?;
        let secondary = split.secondary_pane_ids().into_iter().next()?;
        let tab_id = split.find_pane(secondary)?.active_tab.as_deref()?;
        let index = self.tab_index(tab_id)?;
        self.tabs[index].session_id()
    }

    /// 分屏叶上活动会话（按 pane id）。
    #[allow(dead_code)]
    pub(crate) fn window_pane_session(&self, window_id: u64, pane_id: u64) -> Option<&str> {
        let window = self.windows.get(&window_id)?;
        let split = window.split.as_ref()?;
        let tab_id = split.find_pane(pane_id)?.active_tab.as_deref()?;
        let index = self.tab_index(tab_id)?;
        self.tabs[index].session_id()
    }

    pub(crate) fn tab_data(&self) -> Vec<TabData> {
        self.tab_data_filtered(None)
    }

    /// N7：按窗口过滤的标签投影（`active_tab` 为空 = 用全局活动标签）。
    pub(crate) fn tab_data_filtered(&self, active_tab: Option<&str>) -> Vec<TabData> {
        self.tabs
            .iter()
            .map(|tab| {
                let active = active_tab
                    .map(|tab_id| tab_id == tab.tab_id)
                    .unwrap_or_else(|| self.active_tab_id.as_deref() == Some(tab.tab_id.as_str()));
                // N9：同步发送按键的源/目标角标（源不在 targets 内，两者互斥）。
                let sync_role_text = self.tab_sync_role_text(&tab.tab_id);
                let mode_text = tab.kind.mode_text().to_owned();
                let (default_title, _state_text, connected) = match tab
                    .session_id()
                    .and_then(|session_id| self.sessions.get(session_id))
                {
                    Some(runtime) => (
                        runtime.display_name.clone(),
                        runtime.state_label().to_owned(),
                        runtime.state == SessionState::Connected,
                    ),
                    None if tab.is_new_tab() => ("New Tab".to_owned(), String::new(), false),
                    None => (tab.tab_id.clone(), "failed".to_owned(), false),
                };
                let (connection_badge, state_text) = match tab
                    .session_id()
                    .and_then(|session_id| self.sessions.get(session_id))
                {
                    Some(runtime) => tab_connection_state(runtime.state),
                    None if tab.is_new_tab() => (String::new(), String::new()),
                    None => ("failed".to_owned(), String::new()),
                };
                let title = tab.custom_title.clone().unwrap_or(default_title);
                TabData {
                    id: tab.tab_id.clone(),
                    title,
                    state_text,
                    connection_badge,
                    mode_text,
                    connected,
                    active,
                    // N6：标签角标 REC（该会话正在写日志时）。
                    logging: tab
                        .session_id()
                        .is_some_and(|session_id| self.session_logging_active(session_id)),
                    sync_role_text,
                    unread: tab.unread,
                }
            })
            .collect()
    }

    /// N7：投影用标签条数据——只含"投影窗口"的标签，活动态按该窗口自己的活动标签。
    ///
    /// 无窗口登记（单元测试）时退化为全局标签表 + 全局活动标签。
    pub(crate) fn tab_data_for_projection(&self) -> Vec<TabData> {
        let Some(window_id) = self.projection_window_id() else {
            return self.tab_data();
        };
        let active_tab = self.window_active_tab(window_id).map(str::to_owned);
        self.tabs
            .iter()
            .filter(|tab| self.window_of_tab(&tab.tab_id) == Some(window_id))
            .map(|tab| {
                let active = active_tab.as_deref() == Some(tab.tab_id.as_str());
                self.tab_data_entry(&tab.tab_id, active)
            })
            .collect()
    }

    /// 单个标签的投影（`active` 由调用方决定）。
    fn tab_data_entry(&self, tab_id: &str, active: bool) -> TabData {
        let Some(index) = self.tab_index(tab_id) else {
            return TabData {
                id: tab_id.to_owned(),
                title: String::new(),
                state_text: String::new(),
                connection_badge: String::new(),
                mode_text: "terminal".to_owned(),
                connected: false,
                active,
                logging: false,
                sync_role_text: String::new(),
                unread: 0,
            };
        };
        let tab = &self.tabs[index];
        let sync_role_text = self.tab_sync_role_text(&tab.tab_id);
        let mode_text = tab.kind.mode_text().to_owned();
        let (default_title, _state_text, connected) = match tab
            .session_id()
            .and_then(|session_id| self.sessions.get(session_id))
        {
            Some(runtime) => (
                runtime.display_name.clone(),
                runtime.state_label().to_owned(),
                runtime.state == SessionState::Connected,
            ),
            None if tab.is_new_tab() => ("New Tab".to_owned(), String::new(), false),
            None => (tab.tab_id.clone(), "failed".to_owned(), false),
        };
        let (connection_badge, state_text) = match tab
            .session_id()
            .and_then(|session_id| self.sessions.get(session_id))
        {
            Some(runtime) => tab_connection_state(runtime.state),
            None if tab.is_new_tab() => (String::new(), String::new()),
            None => ("failed".to_owned(), String::new()),
        };
        let title = tab.custom_title.clone().unwrap_or(default_title);
        TabData {
            id: tab.tab_id.clone(),
            title,
            state_text,
            connection_badge,
            mode_text,
            connected,
            active,
            logging: tab
                .session_id()
                .is_some_and(|session_id| self.session_logging_active(session_id)),
            sync_role_text,
            unread: tab.unread,
        }
    }

    pub(crate) fn session_has_active_connection(&self, session_id: &str) -> bool {
        self.sessions.get(session_id).is_some_and(|runtime| {
            matches!(
                runtime.state,
                SessionState::Connected | SessionState::Connecting
            )
        })
    }

    pub(crate) fn has_disconnected_tabs(&self) -> bool {
        // 作用域与"关闭断开标签"一致；只统计有运行时会话的标签。
        self.menu_anchor_pane_tab_ids().iter().any(|tab_id| {
            self.tab_index(tab_id)
                .and_then(|index| self.tabs[index].session_id())
                .is_some_and(|session_id| !self.session_has_active_connection(session_id))
        })
    }

    /// N7：菜单/批量动作的锚定窗口——优先标签右键时的标签，其次聚焦窗口。
    ///
    /// 没有窗口登记（单元测试/单窗口路径）时返回 `None`，调用方退化为全局标签表。
    pub(crate) fn menu_anchor_window(&self) -> Option<u64> {
        self.tab_menu_tab_id
            .as_deref()
            .and_then(|tab_id| self.window_of_tab(tab_id))
            .or(self.focused_window)
            .or_else(|| self.main_window_id())
    }

    /// 菜单作用域内的标签 id（分屏时 = 当前 terminal 区域）。
    ///
    /// 标签右键菜单的"关闭其它/左侧/右侧/全部"只应影响被右键标签所在的那块
    /// terminal 区域（pane），同窗口的其它区域不受影响。未分屏时退化为窗口标签
    /// 集合（单终端区域 = 整个窗口）；无窗口登记（单窗口/测试）时 = 全部标签。
    pub(crate) fn menu_anchor_pane_tab_ids(&self) -> Vec<String> {
        let Some(window_id) = self.menu_anchor_window() else {
            return self.tabs.iter().map(|tab| tab.tab_id.clone()).collect();
        };
        let Some(window) = self.windows.get(&window_id) else {
            return Vec::new();
        };
        let Some(split) = window.split.as_ref() else {
            return window.tabs.clone();
        };
        // 优先取被右键标签所在的 pane；没有右键目标时用聚焦 pane。
        let pane_id = self
            .tab_menu_tab_id
            .as_deref()
            .and_then(|tab_id| split.find_pane_id_for_tab(tab_id))
            .unwrap_or(split.focused_pane);
        split
            .find_pane(pane_id)
            .map(|pane| pane.tabs.clone())
            .unwrap_or_default()
    }

    pub(crate) fn close_scope_targets(&self, scope: &str) -> AppResult<Vec<String>> {
        // 全部 scope 都在"当前 terminal 区域"的标签集合内计算（未分屏 = 窗口）。
        let pane_tabs = self.menu_anchor_pane_tab_ids();
        if scope == CLOSE_SCOPE_ALL {
            return Ok(pane_tabs);
        }
        if scope == CLOSE_SCOPE_DISCONNECTED {
            return Ok(self
                .tabs
                .iter()
                .filter(|tab| match tab.session_id() {
                    Some(session_id) => !self.session_has_active_connection(session_id),
                    // N2：快速连接页视为"可关闭"，但不属于"断开标签"批量语义。
                    None => false,
                })
                .filter(|tab| pane_tabs.iter().any(|id| id == &tab.tab_id))
                .map(|tab| tab.tab_id.clone())
                .collect());
        }
        let (prefix, reference) = [CLOSE_SCOPE_OTHERS, CLOSE_SCOPE_LEFT, CLOSE_SCOPE_RIGHT]
            .iter()
            .find_map(|prefix| {
                scope
                    .strip_prefix(prefix)
                    .map(|reference| (*prefix, reference))
            })
            .ok_or_else(|| AppError::new(format!("unknown tab close scope `{scope}`")))?;
        if self.tab_index(reference).is_none() {
            return Err(AppError::new(format!("tab `{reference}` was not found")));
        }
        let index = pane_tabs
            .iter()
            .position(|tab_id| tab_id == reference)
            .ok_or_else(|| AppError::new(format!("tab `{reference}` was not found")))?;
        let targets = match prefix {
            CLOSE_SCOPE_OTHERS => pane_tabs
                .iter()
                .enumerate()
                .filter(|(position, _)| *position != index)
                .map(|(_, tab_id)| tab_id.clone())
                .collect(),
            CLOSE_SCOPE_LEFT => pane_tabs[..index].to_vec(),
            CLOSE_SCOPE_RIGHT => pane_tabs[index + 1..].to_vec(),
            _ => unreachable!("matched one of the three prefixed scopes"),
        };
        Ok(targets)
    }

    /// 立即关闭标签（不询问）。返回被停止的活动日志文件路径（N6：关闭前强制
    /// 停止并 flush，供调用方在状态文案里提示文件位置）。
    pub(crate) fn close_tab_immediate(&mut self, tab_id: &str) -> Option<String> {
        let index = self.tab_index(tab_id)?;
        let session_id = self.tabs[index].session_id().map(str::to_owned);
        let was_sftp = self.tabs[index].is_sftp();
        self.drop_sftp_tab_state(tab_id);
        // N6：标签关闭/会话断开前先停止日志并 flush（设计 §3：关闭后不自动恢复）。
        let stopped_log_path = session_id
            .as_deref()
            .and_then(|session_id| self.stop_session_logging_for_close(session_id));
        let was_active = self.active_tab_id.as_deref() == Some(tab_id);
        if was_active && was_sftp {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.sftp_listing = SftpListingState::RefreshNeedsSession;
            self.sftp_listing_pending = false;
        }
        // N9：被关闭的标签若是同步源/目标，先抓住显示名（移除后就查不到了），
        // 再让收敛逻辑停/移除并写状态栏提示。
        let sync_closed_title = self
            .input_sync
            .involves(tab_id)
            .then(|| self.tab_display_title(tab_id));
        // N7：先取出窗口归属；`tabs` 移除后按窗口内邻居让位活动标签。
        let window_id = self.window_of_tab(tab_id);
        self.tabs.remove(index);
        if window_id.is_some() {
            if let Some(window) = window_id.and_then(|id| self.windows.get_mut(&id)) {
                if let Some(position) = window.tabs.iter().position(|id| id == tab_id) {
                    window.tabs.remove(position);
                    if window.active_tab.as_deref() == Some(tab_id) {
                        window.active_tab = window
                            .tabs
                            .get(position)
                            .or_else(|| {
                                position
                                    .checked_sub(1)
                                    .and_then(|previous| window.tabs.get(previous))
                            })
                            .cloned();
                    }
                }
                // WP4：关闭标签后清理分屏引用；空叶合并，剩 ≤1 片则取消分屏。
                if let Some(mut split) = window.split.take() {
                    split.remove_tab_from_all(tab_id);
                    match split.compact_empty_leaves() {
                        CompactOutcome::Unsplit => {
                            if let SplitNode::Leaf { pane, .. } = &split.root {
                                window.active_tab =
                                    pane.active_tab.clone().or(window.active_tab.clone());
                            }
                        }
                        CompactOutcome::Keep => {
                            window.active_tab = split
                                .focused_tab()
                                .map(str::to_owned)
                                .or(window.active_tab.clone());
                            window.split = Some(split);
                        }
                    }
                }
            }
            // 只在该窗口是聚焦窗口时同步全局活动标签；否则全局值属于另一个窗口。
            if window_id == self.focused_window {
                self.sync_active_from_focused_window();
            }
            // Closing the last tab behaves like clicking the window close button.
            if window_id.is_some_and(|id| {
                self.windows
                    .get(&id)
                    .is_some_and(|window| window.tabs.is_empty())
            }) {
                self.requested_window_close = window_id;
            }
        } else if was_active {
            let neighbor = self
                .tabs
                .get(index)
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|previous| self.tabs.get(previous))
                })
                .map(|tab| (tab.tab_id.clone(), tab.session_id().map(str::to_owned)));
            match neighbor {
                Some((next_tab_id, next_session_id)) => {
                    self.active_tab_id = Some(next_tab_id);
                    self.active_session_id = next_session_id;
                }
                None => {
                    self.active_tab_id = None;
                    self.active_session_id = None;
                }
            }
            self.refresh_active_view_mirrors();
        }
        if self.tab_menu_tab_id.as_deref() == Some(tab_id) {
            self.tab_menu_tab_id = None;
        }
        if sync_closed_title.is_some() {
            self.reconcile_input_sync_with(sync_closed_title);
        }
        // 防御：TAB-6 起所有标签都是终端标签；没有运行时会话时只做 UI 收敛。
        let Some(session_id) = session_id else {
            self.refresh_terminal_search_for_active_session();
            self.terminal_poll_cursor = if self.tabs.is_empty() {
                0
            } else {
                self.terminal_poll_cursor % self.tabs.len()
            };
            return stopped_log_path;
        };
        if let Some(core_tab_id) = self
            .sessions
            .get(&session_id)
            .map(|runtime| runtime.tab_id().clone())
        {
            // core 侧的标签登记同步回收（best effort；会话未登记时忽略）。
            let _ = self.dispatcher.dispatch(SessionCommand::CloseSession {
                tab_id: core_tab_id,
            });
        }
        // 显式断开 shell（ssh2 session 随 runtime drop 关闭；这里保留现有清理语义）。
        if let Some(mut runtime) = self.sessions.remove(&session_id) {
            let _ = runtime.disconnect_shell();
        }
        self.recent_session_ids.retain(|id| id != &session_id);
        if self.sftp_session.status_session_key() == session_id.as_str() {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.sftp_listing = SftpListingState::RefreshNeedsSession;
        }
        if self.sftp_ready_session_key().is_none() {
            self.remote_edit_session = None;
        }
        self.refresh_terminal_search_for_active_session();
        if let Some(active_session) = self.active_session_id.clone() {
            let _ = self.sync_sftp_lifecycle_for_session(&active_session, false);
        }
        self.terminal_poll_cursor = if self.tabs.is_empty() {
            0
        } else {
            self.terminal_poll_cursor % self.tabs.len()
        };
        stopped_log_path
    }

    pub(crate) fn attach_tab(&mut self, tab_id: &str, session_id: &str) {
        let profile_id = self.sessions.get(session_id).and_then(|runtime| {
            if let SessionSource::SavedSession { profile_id } = &runtime.source {
                Some(profile_id.clone())
            } else {
                None
            }
        });
        if let Some(profile_id) = profile_id {
            self.drop_inventory_placeholder(&profile_id);
        }
        self.attach_tab_entry(
            TabEntry::terminal(tab_id.to_owned(), session_id.to_owned()),
            Some(session_id),
        );
    }

    /// Converts an existing New Tab into a terminal tab bound to `session_id`.
    ///
    /// Quick Connect submit keeps the same tab id and position instead of
    /// opening a second terminal tab.
    pub(crate) fn bind_runtime_tab(&mut self, tab_id: &str, session_id: &str, sftp: bool) {
        if let Some(index) = self.tab_index(tab_id) {
            if self.tabs[index].is_new_tab() {
                self.tabs[index].kind = TabKind::Terminal {
                    session_id: session_id.to_owned(),
                };
                self.tabs[index].quick_connect_input.clear();
                self.tabs[index].quick_connect_error.clear();
                self.tabs[index].unread = 0;
                self.register_tab_in_focused_window(tab_id);
                self.active_tab_id = Some(tab_id.to_owned());
                self.active_session_id = Some(session_id.to_owned());
                self.assign_tab_to_focused_pane(tab_id);
                self.tab_menu_tab_id = None;
                return;
            }
            if self.tabs[index].session_id() == Some(session_id) {
                self.active_tab_id = Some(tab_id.to_owned());
                self.active_session_id = Some(session_id.to_owned());
                return;
            }
            // 已有的终端/SFTP 标签：旧会话回收后原地换绑（标签数量与位置不变）。
            self.rebind_runtime_tab(tab_id, session_id, sftp);
            return;
        }
        self.attach_tab(tab_id, session_id);
    }

    /// 把已死的终端/SFTP 标签原地换成新会话（同一 tab id、同一位置）。
    ///
    /// 旧会话先被彻底回收（core 登记、shell、SFTP 客户端、日志），标签再绑定
    /// 新会话；断开/未连接的标签因此可以"就地复活"，不会多出一个空标签。
    pub(crate) fn rebind_runtime_tab(&mut self, tab_id: &str, session_id: &str, sftp: bool) {
        let Some(index) = self.tab_index(tab_id) else {
            return;
        };
        let old_session_id = self.tabs[index].session_id().map(str::to_owned);
        if let Some(old_session_id) = old_session_id.as_deref() {
            if old_session_id == session_id {
                return;
            }
            self.release_tab_session(tab_id, old_session_id);
        }
        let entry = &mut self.tabs[index];
        entry.kind = if sftp {
            TabKind::Sftp {
                session_id: session_id.to_owned(),
            }
        } else {
            TabKind::Terminal {
                session_id: session_id.to_owned(),
            }
        };
        entry.unread = 0;
        entry.custom_title = None;
        entry.quick_connect_input.clear();
        entry.quick_connect_error.clear();
        self.register_tab_in_focused_window(tab_id);
        self.active_tab_id = Some(tab_id.to_owned());
        self.active_session_id = Some(session_id.to_owned());
        self.assign_tab_to_focused_pane(tab_id);
        self.tab_menu_tab_id = None;
        if sftp {
            self.ensure_sftp_tab_state(tab_id);
            self.restore_sftp_tab(tab_id);
        }
        self.refresh_active_view_mirrors();
    }

    /// 回收标签上的旧会话（标签本身保留）：core 登记、shell、SFTP 客户端、日志。
    fn release_tab_session(&mut self, tab_id: &str, session_id: &str) {
        self.drop_sftp_tab_state(tab_id);
        // N6：标签换绑前先停止日志并 flush（设计 §3：关闭后不自动恢复）。
        let _ = self.stop_session_logging_for_close(session_id);
        if let Some(core_tab_id) = self
            .sessions
            .get(session_id)
            .map(|runtime| runtime.tab_id().clone())
        {
            let _ = self.dispatcher.dispatch(SessionCommand::CloseSession {
                tab_id: core_tab_id,
            });
        }
        if let Some(mut runtime) = self.sessions.remove(session_id) {
            let _ = runtime.disconnect_shell();
        }
        self.recent_session_ids.retain(|id| id != session_id);
        if self.sftp_session.status_session_key() == session_id {
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.sftp_listing = SftpListingState::RefreshNeedsSession;
        }
        if self.sftp_ready_session_key().is_none() {
            self.remote_edit_session = None;
        }
        // N9：旧会话可能是同步发送的源/目标，换绑后要收敛同步状态。
        self.reconcile_input_sync();
    }

    /// 标签是否可以就地承载新会话（不新开标签）。
    ///
    /// New Tab（Quick Connect 页）与"已断开/未连接"的终端/SFTP 标签都可以复用；
    /// 正在连接的标签不参与（连接在途，替换会打断握手）。
    pub(crate) fn tab_reusable_for_new_session(&self, tab_id: &str) -> bool {
        let Some(index) = self.tab_index(tab_id) else {
            return false;
        };
        if self.tabs[index].is_new_tab() {
            return true;
        }
        let Some(session_id) = self.tabs[index].session_id() else {
            return false;
        };
        let Some(runtime) = self.sessions.get(session_id) else {
            // 会话已经不在了（只剩一个空壳标签）：也可以复用。
            return true;
        };
        matches!(
            runtime.state,
            SessionState::Idle | SessionState::Disconnected | SessionState::Failed
        )
    }

    /// 当前活动标签是否可以就地承载新会话（返回标签 id）。
    ///
    /// 新会话不新开标签：活动标签已经断开/未连接时直接占用它（位置不变）。
    pub(crate) fn reusable_active_tab_id(&self) -> Option<String> {
        let tab_id = self.active_tab_id.clone()?;
        self.tab_reusable_for_new_session(&tab_id).then_some(tab_id)
    }

    /// Binds a freshly created runtime to either its new tab or an existing
    /// New Tab that is being converted in place.
    pub(crate) fn attach_or_bind_runtime_tab(
        &mut self,
        reuse_tab_id: Option<&str>,
        tab_id: &str,
        session_id: &str,
        sftp: bool,
    ) {
        if let Some(reuse_tab_id) = reuse_tab_id {
            self.bind_runtime_tab(reuse_tab_id, session_id, sftp);
        } else if sftp {
            self.attach_sftp_tab(tab_id, session_id);
        } else {
            self.attach_tab(tab_id, session_id);
        }
    }

    /// Adds an SFTP tab for an already registered runtime session.
    pub(crate) fn attach_sftp_tab(&mut self, tab_id: &str, session_id: &str) {
        if let Some(profile_id) = self.sessions.get(session_id).and_then(|runtime| {
            if let SessionSource::SavedSession { profile_id } = &runtime.source {
                Some(profile_id.clone())
            } else {
                None
            }
        }) {
            self.drop_inventory_placeholder(&profile_id);
        }
        self.ensure_sftp_tab_state(tab_id);
        self.attach_tab_entry(
            TabEntry::sftp(tab_id.to_owned(), session_id.to_owned()),
            Some(session_id),
        );
        self.restore_sftp_tab(tab_id);
    }

    /// Adds a New Tab (Quick Connect) tab. New Tabs have no runtime session.
    pub(crate) fn attach_new_tab(&mut self, tab_id: &str) {
        self.attach_tab_entry(TabEntry::new_tab(tab_id.to_owned()), None);
    }

    /// `+` / Ctrl+O / startup: open a fresh New Tab page.
    pub fn open_new_tab(&mut self) -> AppProjection {
        let ordinal = self.allocate_runtime_ordinal();
        let tab_id = format!("tab-new-{ordinal}");
        self.attach_new_tab(&tab_id);
        self.set_status_kind(
            "tab-new",
            "Opened a new tab.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    fn attach_tab_entry(&mut self, entry: TabEntry, session_id: Option<&str>) {
        self.park_active_sftp_tab();
        let tab_id = entry.tab_id.clone();
        self.tabs.push(entry);
        // N7：新标签登记到当前交互窗口（键盘/菜单回调已把焦点切到该窗口）。
        self.register_tab_in_focused_window(&tab_id);
        self.active_tab_id = Some(tab_id.clone());
        self.active_session_id = session_id.map(str::to_owned);
        self.tab_menu_tab_id = None;
        // 分屏时新标签归入当前聚焦 pane 的标签条。
        self.assign_tab_to_focused_pane(&tab_id);
        self.refresh_active_view_mirrors();
    }

    pub(crate) fn drop_inventory_placeholder(&mut self, profile_id: &str) {
        let placeholder_key = self
            .sessions
            .iter()
            .find(|(_, runtime)| {
                matches!(
                    &runtime.source,
                    SessionSource::SavedSession { profile_id: current } if current == profile_id
                ) && runtime.state == SessionState::Idle
                    && runtime.shell_session.is_none()
            })
            .map(|(key, _)| key.clone());
        if let Some(key) = placeholder_key {
            self.sessions.remove(&key);
        }
    }

    pub(crate) fn tab_parts(&self) -> (bool, String, String) {
        match self
            .active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
        {
            Some(session) => (
                true,
                session.display_name.clone(),
                session.state_label().to_owned(),
            ),
            None => (false, String::new(), "idle".to_owned()),
        }
    }
}

// --- N7：窗口级标签归属 -------------------------------------------------------
//
// 设计：`docs/product/yshell-next-n7-multi-window.md`（D5–D8 已确认）。
// * 运行时只换 UI 归属：`tabs`/`sessions` 全量保留，`SessionRuntime`（shell/grid/日志）
//   在迁移前后是同一个实例（`move_tab_to_window` 不碰 sessions）。
// * `active_tab_id`/`active_session_id` 是"聚焦窗口"的活动标签镜像；
//   每个窗口自己的活动标签存在 `windows[window_id].active_tab`。
// * 未登记窗口时（单元测试）退化为单窗口语义：`register_tab_in_focused_window`
//   自动登记隐式窗口 0。

impl AppRuntime {
    /// N7：登记一个新窗口（`WindowManager::open_window` 调用）。
    pub(crate) fn register_window(&mut self, window_id: u64) {
        self.windows.entry(window_id).or_default();
    }

    /// N7：主窗口 = 现存最小编号窗口。
    pub(crate) fn main_window_id(&self) -> Option<u64> {
        self.windows.keys().next().copied()
    }

    /// N7：标签当前所属窗口（未登记 → `None`）。
    pub(crate) fn window_of_tab(&self, tab_id: &str) -> Option<u64> {
        self.windows
            .iter()
            .find(|(_, window)| window.tabs.iter().any(|id| id == tab_id))
            .map(|(id, _)| *id)
    }

    /// N7：窗口的标签 id（显示顺序）。
    pub(crate) fn window_tab_ids(&self, window_id: u64) -> Vec<String> {
        self.windows
            .get(&window_id)
            .map(|window| window.tabs.clone())
            .unwrap_or_default()
    }

    /// N7：窗口的活动标签 id。
    pub(crate) fn window_active_tab(&self, window_id: u64) -> Option<&str> {
        self.windows
            .get(&window_id)
            .and_then(|window| window.active_tab.as_deref())
    }

    /// N7：窗口的活动标签对应的运行时会话 key。
    pub(crate) fn window_active_session(&self, window_id: u64) -> Option<&str> {
        let tab_id = self.window_active_tab(window_id)?;
        let index = self.tab_index(tab_id)?;
        self.tabs[index].session_id()
    }

    /// N7：窗口内处于 connected/connecting 的标签数。
    pub(crate) fn active_connections_in_window(&self, window_id: u64) -> usize {
        self.window_tab_ids(window_id)
            .iter()
            .filter(|tab_id| {
                self.tab_index(tab_id)
                    .and_then(|index| self.tabs[index].session_id())
                    .is_some_and(|session_id| self.session_has_active_connection(session_id))
            })
            .count()
    }

    /// N7：把窗口标记为交互窗口，并把全局 `active_*` 同步到该窗口的活动标签。
    pub(crate) fn focus_window(&mut self, window_id: u64) {
        if self.focused_window == Some(window_id) {
            return;
        }
        if !self.windows.contains_key(&window_id) {
            return;
        }
        self.focused_window = Some(window_id);
        self.sync_active_from_focused_window();
        // 全局活动标签变化会影响状态栏/菜单启用态等投影字段。
        self.mark_ui_dirty();
    }

    /// N7：置位 UI 脏标记（窗口回调的所有变更都置位；定时器消费一次）。
    pub(crate) fn mark_ui_dirty(&mut self) {
        self.ui_dirty = true;
    }

    /// N7：取出并清除 UI 脏标记（应用级定时器调用）。
    pub(crate) fn take_ui_dirty(&mut self) -> bool {
        std::mem::take(&mut self.ui_dirty)
    }

    /// N7：重算全局 `active_tab_id`/`active_session_id`（聚焦窗口的活动标签）。
    ///
    /// 没有窗口归属时不动全局值（单窗口/测试路径自己维护）。
    pub(crate) fn sync_active_from_focused_window(&mut self) {
        let Some(window_id) = self.focused_window else {
            return;
        };
        if !self.windows.contains_key(&window_id) {
            return;
        }
        let active_tab = self
            .windows
            .get(&window_id)
            .and_then(|window| window.active_tab.clone());
        self.park_active_sftp_tab();
        self.active_tab_id = active_tab.clone();
        self.active_session_id = active_tab
            .as_deref()
            .and_then(|tab_id| self.tab_index(tab_id))
            .and_then(|index| self.tabs[index].session_id().map(str::to_owned));
        self.refresh_active_view_mirrors();
    }

    /// N7：把新标签登记到当前窗口（返回窗口 id）；无窗口时登记隐式窗口 0。
    ///
    /// 与 `attach_tab` 的区别：调用方已经处理了会话/状态栏，这里只维护 UI 归属。
    pub(crate) fn register_tab_in_focused_window(&mut self, tab_id: &str) -> u64 {
        let window_id = match self.focused_window.or_else(|| self.main_window_id()) {
            Some(window_id) => window_id,
            None => {
                self.windows.insert(0, WindowTabs::default());
                self.focused_window = Some(0);
                0
            }
        };
        if let Some(window) = self.windows.get_mut(&window_id) {
            if !window.tabs.iter().any(|id| id == tab_id) {
                window.tabs.push(tab_id.to_owned());
            }
            window.active_tab = Some(tab_id.to_owned());
        }
        self.active_tab_id = Some(tab_id.to_owned());
        window_id
    }

    /// N7：把已登记的标签设为所属窗口的活动标签。
    pub(crate) fn activate_tab_in_window(&mut self, tab_id: &str) {
        let window_id = self.window_of_tab(tab_id);
        if let Some(window_id) = window_id {
            if let Some(window) = self.windows.get_mut(&window_id) {
                window.active_tab = Some(tab_id.to_owned());
            }
            if self.focused_window == Some(window_id) {
                self.sync_active_from_focused_window();
            }
        } else {
            // 未登记（无窗口的单窗口/测试路径）：保持全局语义。
            self.active_tab_id = Some(tab_id.to_owned());
        }
    }

    /// N7：把标签移动到目标窗口（只换 UI 归属；会话/网格/日志句柄不重建）。
    ///
    /// 迁入后目标窗口把该标签设为活动标签；源窗口若失去活动标签则让位给同窗口
    /// 邻居（右邻居优先，其次左邻居）。跨窗口的同步发送目标会被收敛移除。
    pub(crate) fn move_tab_to_window(&mut self, tab_id: &str, window_id: u64) -> AppResult<()> {
        if !self.windows.contains_key(&window_id) {
            return Err(AppError::new(format!("window `{window_id}` was not found")));
        }
        if self.tab_index(tab_id).is_none() {
            return Err(AppError::new(format!("tab `{tab_id}` was not found")));
        }
        let source = self.window_of_tab(tab_id);
        if source == Some(window_id) {
            return Ok(());
        }
        if let Some(source) = source {
            if let Some(window) = self.windows.get_mut(&source) {
                if let Some(index) = window.tabs.iter().position(|id| id == tab_id) {
                    window.tabs.remove(index);
                    if window.active_tab.as_deref() == Some(tab_id) {
                        window.active_tab = window
                            .tabs
                            .get(index)
                            .or_else(|| index.checked_sub(1).and_then(|prev| window.tabs.get(prev)))
                            .cloned();
                    }
                }
            }
        }
        if let Some(window) = self.windows.get_mut(&window_id) {
            if !window.tabs.iter().any(|id| id == tab_id) {
                window.tabs.push(tab_id.to_owned());
            }
            window.active_tab = Some(tab_id.to_owned());
        }
        self.sync_active_from_focused_window();
        // N9：同步发送目标跨窗口后不再属于"当前窗口"，让收敛逻辑移除它们。
        self.reconcile_input_sync();
        Ok(())
    }

    /// N7：当前投影的目标窗口（`projection_for_window` 覆盖 > 聚焦窗口 > 主窗口）。
    pub(crate) fn projection_window_id(&self) -> Option<u64> {
        self.projection_window
            .get()
            .or(self.focused_window)
            .or_else(|| self.main_window_id())
    }

    /// N7：投影里的"活动标签"（目标窗口自己的活动标签；无窗口登记时用全局值）。
    pub(crate) fn projection_active_tab_id(&self) -> String {
        self.projection_window_id()
            .and_then(|window_id| self.window_active_tab(window_id).map(str::to_owned))
            .or_else(|| self.active_tab_id.clone())
            .unwrap_or_default()
    }

    /// N7：窗口是否正在等待"关闭窗口 = 断开连接"确认。
    pub(crate) fn window_close_pending(&self, window_id: u64) -> bool {
        self.pending_close_windows.contains(&window_id)
    }

    /// Drains "last tab closed → close the window" requests for the UI tick.
    pub(crate) fn take_requested_window_close(&mut self) -> Option<u64> {
        self.requested_window_close.take()
    }

    /// N7：标记/清除窗口关闭确认（每窗口同时最多一个）。
    pub(crate) fn set_window_close_pending(&mut self, window_id: u64, pending: bool) {
        if pending {
            self.pending_close_windows.insert(window_id);
        } else {
            self.pending_close_windows.remove(&window_id);
        }
        // 旧窗口关闭后清理残留标记。
        let live: BTreeSet<u64> = self.windows.keys().copied().collect();
        self.pending_close_windows.retain(|id| live.contains(id));
    }

    /// N7：是否只剩一个窗口（关闭 = 退出应用）。
    pub(crate) fn is_last_window(&self, window_id: u64) -> bool {
        self.windows.len() <= 1 && self.windows.contains_key(&window_id)
    }

    /// N7：关闭窗口的全部标签（先停日志再断开会话），并移除窗口登记。
    ///
    /// 返回 `(关闭的标签数, 被停止的日志文件路径)`。调用方负责状态栏文案与
    /// 最后一个窗口的退出逻辑。
    pub(crate) fn close_window_tabs(&mut self, window_id: u64) -> (usize, Option<String>) {
        let tab_ids = self.window_tab_ids(window_id);
        let mut closed = 0usize;
        let mut stopped_log_path = None;
        for tab_id in &tab_ids {
            if self.tab_index(tab_id).is_some() {
                if let Some(path) = self.close_tab_immediate(tab_id) {
                    stopped_log_path = Some(path);
                }
                closed += 1;
            }
        }
        self.windows.remove(&window_id);
        self.pending_close_windows.remove(&window_id);
        if self.requested_window_close == Some(window_id) {
            self.requested_window_close = None;
        }
        if self.focused_window == Some(window_id) {
            self.focused_window = self.windows.keys().next().copied();
            self.sync_active_from_focused_window();
        }
        (closed, stopped_log_path)
    }
}

/// N0/N2：标签承载的内容。
///
/// The workspace has three equal tab modes:
/// * `NewTab` — the Quick Connect start page (no runtime session);
/// * `Terminal` — an interactive terminal page;
/// * `Sftp` — the two-column local/remote SFTP page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TabKind {
    NewTab,
    Terminal { session_id: String },
    Sftp { session_id: String },
}

impl TabKind {
    pub(crate) const fn mode_text(&self) -> &'static str {
        match self {
            Self::NewTab => "new",
            Self::Terminal { .. } => "terminal",
            Self::Sftp { .. } => "sftp",
        }
    }
}

/// N0：一个标签的运行时条目；`tabs` 的顺序就是标签条的显示顺序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TabEntry {
    pub(crate) tab_id: String,
    pub(crate) kind: TabKind,
    /// WP4：后台标签有新输出时递增；激活（或分屏可见）时清零。
    pub(crate) unread: u32,
    /// WP4：标签显示名覆盖（仅内存，会话生命周期内有效；不落盘）。
    pub(crate) custom_title: Option<String>,
    /// New Tab mode: per-tab Quick Connect input text (multiple New Tabs are
    /// independent, including when split across panes).
    pub(crate) quick_connect_input: String,
    /// New Tab mode: inline parse error shown under the input.
    pub(crate) quick_connect_error: String,
}

impl TabEntry {
    pub(crate) fn new_tab(tab_id: String) -> Self {
        Self {
            tab_id,
            kind: TabKind::NewTab,
            unread: 0,
            custom_title: None,
            quick_connect_input: String::new(),
            quick_connect_error: String::new(),
        }
    }

    pub(crate) fn terminal(tab_id: String, session_id: String) -> Self {
        Self {
            tab_id,
            kind: TabKind::Terminal { session_id },
            unread: 0,
            custom_title: None,
            quick_connect_input: String::new(),
            quick_connect_error: String::new(),
        }
    }

    pub(crate) fn sftp(tab_id: String, session_id: String) -> Self {
        Self {
            tab_id,
            kind: TabKind::Sftp { session_id },
            unread: 0,
            custom_title: None,
            quick_connect_input: String::new(),
            quick_connect_error: String::new(),
        }
    }

    /// 标签对应的运行时会话 key（New Tab 没有会话）。
    pub(crate) fn session_id(&self) -> Option<&str> {
        match &self.kind {
            TabKind::NewTab => None,
            TabKind::Terminal { session_id } | TabKind::Sftp { session_id } => Some(session_id),
        }
    }

    pub(crate) fn is_new_tab(&self) -> bool {
        matches!(self.kind, TabKind::NewTab)
    }

    pub(crate) fn is_sftp(&self) -> bool {
        matches!(self.kind, TabKind::Sftp { .. })
    }
}

/// 标签上的连接状态：图标右下角的角标 + 是否还显示状态文字。
///
/// `connected`/`disconnected`/`failed` **只**用角标表达（绿勾 / 红叉）：标签上
/// 同时出现 "connected" 和一个绿勾既重复又难看。"改成符号" 就是替换掉文字。
fn tab_connection_state(state: SessionState) -> (String, String) {
    match state {
        SessionState::Connected => ("connected".to_owned(), "connected".to_owned()),
        SessionState::Disconnected => ("disconnected".to_owned(), "disconnected".to_owned()),
        SessionState::Failed => ("failed".to_owned(), "failed".to_owned()),
        SessionState::Connecting => (String::new(), "connecting".to_owned()),
        SessionState::Idle => (String::new(), "idle".to_owned()),
    }
}

/// N0：单个标签的只读投影（Slint 标签条模型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabData {
    /// 标签 id（`activate_tab`/`close_tab` 的入参）。
    pub id: String,
    pub title: String,
    /// 会话状态枚举 id（`idle`/`connecting`/...）。
    ///
    /// `connected`/`disconnected`/`failed` 不再以文字出现——它们由图标右下角的
    /// 角标表达（见 `connection_badge`）。
    pub state_text: String,
    /// 图标右下角的连接状态角标（`connected` / `disconnected` / `failed` /
    /// 空 = 不显示角标）。
    pub connection_badge: String,
    /// `new` / `terminal` / `sftp`（Slint 标签图标与菜单分流）。
    pub mode_text: String,
    /// 是否处于 `connected`（Slint 侧标签样式/菜单启用条件用）。
    pub connected: bool,
    pub active: bool,
    /// N6：该标签的会话是否正在写日志（标签角标 REC）。
    pub logging: bool,
    /// N9：同步发送按键的角色（空 = 无；`source` = 源；`target` = 接收目标）。
    pub sync_role_text: String,
    /// WP4：未读输出计数（角标；0 = 不显示）。
    pub unread: u32,
}

/// N0：等待用户确认的关闭请求（单个或批量），确认后按 `tab_ids` 顺序逐个关闭。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingCloseTabs {
    /// 关闭顺序（批量时已经是右→左；单关只有一个）。
    pub(crate) tab_ids: Vec<String>,
    /// 单标签关闭（确认文案显示会话名）还是批量关闭（显示数量/活动连接数）。
    pub(crate) single: bool,
    /// 目标集合里处于 connected/connecting 的标签数。
    pub(crate) active_connections: usize,
}

/// N0 轮询：一个会话在本次 tick 的结果（供活动/后台两条路径分别决定是否投影）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SessionPollOutcome {
    /// 新输出到达（chunks 或可见文本变化）。
    pub(crate) output_changed: bool,
    /// 会话状态变化（含 shell-closed 落到 disconnected）。
    pub(crate) state_changed: bool,
}

/// 单个分屏叶子投影（绝对归一化几何）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkspacePaneLeafProjection {
    pub pane_id: i32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub empty: bool,
    pub focused: bool,
    pub quick_connect: bool,
    pub tab_id: String,
    pub title_text: String,
    /// `new` / `terminal` / `sftp`.
    pub mode_text: String,
    /// Per-New-Tab Quick Connect input state (split panes are independent).
    pub quick_connect_input_text: String,
    pub quick_connect_error_text: String,
    /// Per-SFTP-tab pane data (default for terminal/New Tab leaves).
    pub sftp: super::sftp::SftpPaneProjection,
    pub tabs: Vec<TabData>,
}

/// 分屏把手投影。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkspaceSplitterProjection {
    pub split_id: i32,
    /// `true` = 竖条（左右分）；`false` = 横条（上下分）。
    pub vertical: bool,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub parent_x: f32,
    pub parent_y: f32,
    pub parent_w: f32,
    pub parent_h: f32,
    pub ratio: f32,
}

/// WP4：分屏投影（递归树摊平为叶子 + 把手）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkspaceSplitProjection {
    pub active: bool,
    pub orientation_text: String,
    pub ratio: f32,
    pub focused_pane: i32,
    pub pane_count: i32,
    pub pane0_tab_id: String,
    pub pane0_title_text: String,
    pub pane0_empty: bool,
    pub pane0_quick_connect: bool,
    pub pane0_tabs: Vec<TabData>,
    pub pane1_tab_id: String,
    pub pane1_title_text: String,
    pub pane1_empty: bool,
    pub pane1_quick_connect: bool,
    pub pane1_tabs: Vec<TabData>,
    pub can_split: bool,
    pub can_close_pane: bool,
    pub leaves: Vec<WorkspacePaneLeafProjection>,
    pub splitters: Vec<WorkspaceSplitterProjection>,
}

/// N0 轮询：单个 tick 最多轮询的会话数（活动标签优先，其余按游标轮转）。
pub(crate) const MAX_POLL_SESSIONS_PER_TICK: usize = 8;

/// N0 批量关闭 scope 的前缀（`close_tabs(scope)` 的编码，见设计 §4）。
pub(crate) const CLOSE_SCOPE_OTHERS: &str = "others:";

pub(crate) const CLOSE_SCOPE_LEFT: &str = "left:";

pub(crate) const CLOSE_SCOPE_RIGHT: &str = "right:";

pub(crate) const CLOSE_SCOPE_ALL: &str = "all";

pub(crate) const CLOSE_SCOPE_DISCONNECTED: &str = "disconnected";

impl AppRuntime {
    #[cfg(test)]
    pub fn poll_active_terminal_output(&mut self) -> AppResult<AppProjection> {
        if let Some(projection) = self.poll_all_terminal_outputs()? {
            return Ok(projection);
        }
        self.set_status_kind(
            "terminal-poll-empty",
            "Polled the active terminal, but no new output was available.".to_owned(),
            String::new(),
            String::new(),
        );
        Ok(self.projection())
    }
}

// --- N9：同步发送按键（Input Sync）-----------------------------------------
//
// 设计：`docs/product/yshell-next-n9-sync-input.md`（D23/D24 已确认）。
// * 源 = 一个终端标签；目标 = 当前窗口内的一批已连接终端标签（不持久化）。
// * 扇出发生在输入的唯一汇聚点（`runtime/keys.rs::send_active_terminal_bytes`），
//   目标只写输入，其回显不再回灌到源。
// * `Visible` 在分屏落地前等同 `All`（UI 侧有说明）；`Selected` = 用户通过标签
//   右键勾选/取消"接收键输入"手工挑选过的目标集合。
//
// 不变量：源不在 targets 内；源关闭/断开 → 停止；目标关闭/断开 → 自动移出
// （见 `reconcile_input_sync`，由轮询 tick / 关闭 / 断开路径调用）。

/// N9：同步发送按键的运行时会话态（内存态，不落盘）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct InputSyncState {
    /// 源标签 id（`None` = 未在同步）。
    pub(crate) source: Option<String>,
    /// 目标标签 id 集合（`BTreeSet`：扇出顺序稳定）。
    pub(crate) targets: BTreeSet<String>,
    /// 目标选择模式（All/Visible/Selected）。
    pub(crate) mode: InputSyncMode,
    /// 状态栏 chip 的一次性提示：kind（空 = 无提示）+ 参数。
    pub(crate) notice_kind: String,
    pub(crate) notice_param: String,
}

impl InputSyncState {
    pub(crate) fn is_active(&self) -> bool {
        self.source.is_some()
    }

    /// 标签是否参与当前同步（源或目标）。
    pub(crate) fn involves(&self, tab_id: &str) -> bool {
        self.source.as_deref() == Some(tab_id) || self.targets.contains(tab_id)
    }

    pub(crate) fn set_notice(&mut self, kind: &str, param: String) {
        self.notice_kind = kind.to_owned();
        self.notice_param = param;
    }

    pub(crate) fn clear_notice(&mut self) {
        self.notice_kind.clear();
        self.notice_param.clear();
    }
}

/// N9：目标选择模式（`Send to Current` 是"取消同步"，不是一种模式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum InputSyncMode {
    /// 当前窗口内所有已连接的终端标签（排除源）。
    #[default]
    All,
    /// 当前无分屏，暂等同 `All`；分屏落地后收紧为"可见终端"。
    Visible,
    /// 用户通过标签右键"接收键输入"手工挑选过的目标集合。
    Selected,
}

impl InputSyncMode {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Visible => "visible",
            Self::Selected => "selected",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            "all" => Some(Self::All),
            "visible" => Some(Self::Visible),
            "selected" => Some(Self::Selected),
            _ => None,
        }
    }
}

impl AppRuntime {
    /// N9：从活动终端开始同步发送按键（终端右键菜单 `Send Key Input to …`）。
    ///
    /// 目标集合 = 当前窗口内所有已连接、非源的终端标签；没有目标时只写状态栏提示，
    /// 不进入同步态（避免 `SYNC → 0 targets` 的幽灵状态）。
    pub fn start_input_sync_mode(&mut self, mode_id: &str) -> AppResult<AppProjection> {
        let mode = InputSyncMode::from_id(mode_id)
            .ok_or_else(|| AppError::new(format!("unknown input sync mode `{mode_id}`")))?;
        let source = self
            .active_tab_id
            .clone()
            .ok_or_else(|| AppError::new("no active terminal to send key input from"))?;
        if !self.input_sync_tab_connected(&source) {
            return Err(AppError::new("the active terminal is not connected"));
        }
        // N7：目标 = 源标签所在窗口（当前窗口）内所有已连接终端标签。
        let source_window = self.window_of_tab(&source);
        let targets: BTreeSet<String> = self
            .tabs
            .iter()
            .filter(|tab| tab.tab_id != source && self.input_sync_tab_connected(&tab.tab_id))
            .filter(|tab| {
                source_window.is_none() || self.window_of_tab(&tab.tab_id) == source_window
            })
            .map(|tab| tab.tab_id.clone())
            .collect();
        self.input_sync.clear_notice();
        if targets.is_empty() {
            self.set_status_kind(
                "input-sync-no-targets",
                "No other connected terminal tab can receive key input.".to_owned(),
                String::new(),
                String::new(),
            );
            return Ok(self.projection());
        }
        let target_count = targets.len();
        self.input_sync.source = Some(source);
        self.input_sync.targets = targets;
        self.input_sync.mode = mode;
        self.set_status_kind(
            "input-sync-started",
            format!("Sending key input to {target_count} target tab(s)."),
            target_count.to_string(),
            mode.id().to_owned(),
        );
        Ok(self.projection())
    }

    /// N9：停止同步（终端右键 `Stop Sending Key Input` / 状态栏 chip 一键停止）。
    pub fn stop_input_sync_command(&mut self) -> AppProjection {
        if self.input_sync.is_active() {
            let target_count = self.input_sync.targets.len();
            self.input_sync.source = None;
            self.input_sync.targets.clear();
            self.input_sync.mode = InputSyncMode::All;
            self.input_sync.clear_notice();
            self.set_status_kind(
                "input-sync-stopped",
                format!("Stopped sending key input to {target_count} tab(s)."),
                target_count.to_string(),
                String::new(),
            );
        } else {
            // 非同步态下的 chip 点击 = 清掉残留提示（错误态可见但可关闭）。
            self.input_sync.clear_notice();
        }
        self.projection()
    }

    /// N9：标签右键"接收键输入"勾选/取消（仅在同步进行时可用）。
    pub fn toggle_tab_receives_key_input(&mut self, tab_id: &str) -> AppResult<AppProjection> {
        if !self.input_sync.is_active() {
            return Err(AppError::new("key input sync is not active"));
        }
        if self.input_sync.source.as_deref() == Some(tab_id) {
            return Err(AppError::new(
                "the source tab cannot receive its own key input",
            ));
        }
        if !self.input_sync_tab_connected(tab_id) {
            return Err(AppError::new("the tab is not a connected terminal"));
        }
        let receiving = if self.input_sync.targets.remove(tab_id) {
            false
        } else {
            self.input_sync.targets.insert(tab_id.to_owned());
            true
        };
        self.input_sync.mode = InputSyncMode::Selected;
        self.input_sync.clear_notice();
        let title = self.tab_display_title(tab_id);
        self.set_status_kind(
            "input-sync-receive-toggled",
            if receiving {
                format!("`{title}` now receives key input.")
            } else {
                format!("`{title}` no longer receives key input.")
            },
            title,
            if receiving { "on" } else { "off" }.to_owned(),
        );
        Ok(self.projection())
    }

    /// N9：把源输入同步写入全部目标（在 `send_active_terminal_bytes` 成功写源之后调用）。
    ///
    /// 顺序 = `targets`（BTreeSet）顺序；单个目标失败只移出该目标并写提示，
    /// 不影响源与其它目标。目标只写输入——它的回显由轮询进它自己的网格，
    /// 不会经过这里回灌。
    pub(crate) fn broadcast_synced_input(&mut self, bytes: &[u8]) {
        if bytes.is_empty() || !self.input_sync.is_active() {
            return;
        }
        // 只有源标签持有活动终端时输入才广播：切到目标标签上打字 = 只写给该标签
        // （源语义锚定在标签，而不是"任意活动输入"）。
        if self.active_tab_id.as_deref() != self.input_sync.source.as_deref() {
            return;
        }
        let targets: Vec<String> = self.input_sync.targets.iter().cloned().collect();
        for tab_id in targets {
            let Some(session_key) = self
                .tab_index(&tab_id)
                .and_then(|index| self.tabs[index].session_id().map(str::to_owned))
            else {
                self.drop_input_sync_target(&tab_id, "target-removed");
                continue;
            };
            if !self.input_sync_tab_connected(&tab_id) {
                self.drop_input_sync_target(&tab_id, "target-removed");
                continue;
            }
            let session_id = self
                .sessions
                .get(&session_key)
                .map(|session| session.session_id().clone());
            let dispatched = session_id.is_some_and(|session_id| {
                self.dispatcher
                    .dispatch(SessionCommand::SendTerminalInput {
                        session_id,
                        bytes: bytes.to_vec(),
                    })
                    .is_ok()
            });
            let written = dispatched
                && self
                    .sessions
                    .get_mut(&session_key)
                    .is_some_and(|runtime| runtime.write_terminal_input(bytes).is_ok());
            if !written {
                self.drop_input_sync_target(&tab_id, "target-removed");
            }
        }
    }

    /// N9：控制键（Ctrl/Alt/Meta 组合或控制字符）广播时的一次性非阻塞提示。
    ///
    /// 返回 `true` 表示提示有更新（调用方据此重投影刷新状态栏 chip）。
    pub(crate) fn note_input_sync_control_broadcast(
        &mut self,
        text: &str,
        ctrl: bool,
        alt: bool,
        shift: bool,
        meta: bool,
        bytes: &[u8],
    ) -> bool {
        if !self.input_sync.is_active() || bytes.is_empty() {
            return false;
        }
        if !input_sync_is_control_key(text, ctrl, alt, shift, meta, bytes) {
            return false;
        }
        let label = input_sync_control_label(text, ctrl, alt, meta, bytes);
        self.input_sync.set_notice("control-broadcast", label);
        true
    }

    /// N9：收敛同步状态（源/目标关闭、断开、shell 掉线）。
    ///
    /// 返回 `true` 表示状态有变化（需要重投影）。只改运行时状态与 chip 提示，
    /// 不改 `status_text`（断开/关闭自己的状态文案优先保留）。
    pub(crate) fn reconcile_input_sync(&mut self) -> bool {
        self.reconcile_input_sync_with(None)
    }

    /// 同 [`Self::reconcile_input_sync`]，但可为"刚被关闭的标签"提供显示名（标签
    /// 移除后已查不到标题）。
    pub(crate) fn reconcile_input_sync_with(&mut self, closed_title: Option<String>) -> bool {
        if !self.input_sync.is_active() {
            return false;
        }
        let Some(source) = self.input_sync.source.clone() else {
            return false;
        };
        if self.tab_index(&source).is_none() {
            let param = closed_title.unwrap_or_default();
            self.stop_input_sync_with_notice("source-closed", param);
            return true;
        }
        if !self.input_sync_tab_connected(&source) {
            self.stop_input_sync_with_notice("source-disconnected", String::new());
            return true;
        }
        // 先处理"关闭/断开"的目标（关闭的标签已查不到标题，用 closed_title 兜底），
        // 再处理 N7 的"被移到别的窗口"（仍连接、但已不属于当前窗口）。
        let stale: Vec<String> = self
            .input_sync
            .targets
            .iter()
            .filter(|tab_id| !self.input_sync_tab_connected(tab_id))
            .cloned()
            .collect();
        let mut changed = false;
        for tab_id in stale {
            self.input_sync.targets.remove(&tab_id);
            let title = self.tab_display_title(&tab_id);
            let param = if title.is_empty() {
                closed_title.clone().unwrap_or_else(|| tab_id.clone())
            } else {
                title
            };
            self.input_sync.set_notice("target-removed", param);
            changed = true;
        }
        // N7：目标被移到别的窗口后不再是"当前窗口"的目标，同样移出（源仍在同一
        // 窗口时才保留）；没有目标时同步态自动收敛为停止。
        if let Some(source_window) = self.window_of_tab(&source) {
            let moved_tabs: Vec<String> = self
                .input_sync
                .targets
                .iter()
                .filter(|tab_id| {
                    self.tab_index(tab_id).is_some()
                        && self.window_of_tab(tab_id) != Some(source_window)
                })
                .cloned()
                .collect();
            let mut removed_moved = false;
            for tab_id in moved_tabs {
                self.drop_input_sync_target(&tab_id, "target-removed");
                removed_moved = true;
                changed = true;
            }
            if removed_moved && self.input_sync.targets.is_empty() {
                self.stop_input_sync_with_notice("source-moved", String::new());
            }
        }
        changed
    }

    /// N9：投影用——标签的同步角色（空/`source`/`target`）。
    pub(crate) fn tab_sync_role_text(&self, tab_id: &str) -> String {
        if !self.input_sync.is_active() {
            return String::new();
        }
        if self.input_sync.source.as_deref() == Some(tab_id) {
            return "source".to_owned();
        }
        if self.input_sync.targets.contains(tab_id) {
            return "target".to_owned();
        }
        String::new()
    }

    /// N9：同步源的显示名（状态栏 chip 的 accessible label / 状态文案用）。
    pub(crate) fn input_sync_source_name(&self) -> String {
        self.input_sync
            .source
            .as_deref()
            .map(|tab_id| self.tab_display_title(tab_id))
            .unwrap_or_default()
    }

    fn stop_input_sync_with_notice(&mut self, kind: &str, param: String) {
        self.input_sync.source = None;
        self.input_sync.targets.clear();
        self.input_sync.mode = InputSyncMode::All;
        self.input_sync.set_notice(kind, param);
    }

    fn drop_input_sync_target(&mut self, tab_id: &str, kind: &str) {
        self.input_sync.targets.remove(tab_id);
        let title = self.tab_display_title(tab_id);
        let param = if title.is_empty() {
            tab_id.to_owned()
        } else {
            title
        };
        self.input_sync.set_notice(kind, param);
    }

    /// N9：标签是否是可作为同步源/目标的"已连接终端标签"。
    pub(crate) fn input_sync_tab_connected(&self, tab_id: &str) -> bool {
        self.tab_index(tab_id)
            .and_then(|index| self.tabs[index].session_id())
            .and_then(|session_id| self.sessions.get(session_id))
            .is_some_and(|runtime| runtime.state == SessionState::Connected)
    }
}

/// N9：按键是否是"控制键广播"（Ctrl/Alt/Meta 组合或单字节控制字符）。
///
/// Enter/Tab/Backspace/Esc 属于常规编辑键，不触发提示；Ctrl+C 这类中断广播才行。
fn input_sync_is_control_key(
    text: &str,
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
    bytes: &[u8],
) -> bool {
    let _ = shift;
    if ctrl || alt || meta {
        return true;
    }
    if bytes.len() != 1 {
        // 组合键（IME 提交/粘贴）走各自的输入路径，这里只提示单字节控制字符。
        return !text.is_empty() && text.chars().all(|ch| ch.is_ascii_control());
    }
    !matches!(bytes[0], b'\t' | b'\n' | b'\r' | 0x1b | 0x7f)
}

/// N9：控制键的可读标签（"Ctrl+C" 等；作为 i18n 模板的 `{0}` 参数）。
fn input_sync_control_label(text: &str, ctrl: bool, alt: bool, meta: bool, bytes: &[u8]) -> String {
    let printable = text
        .chars()
        .find(|ch| !ch.is_ascii_control())
        .map(|ch| ch.to_ascii_uppercase().to_string());
    let prefix = if ctrl {
        "Ctrl"
    } else if alt {
        "Alt"
    } else if meta {
        "Meta"
    } else {
        "Ctrl"
    };
    if let Some(key) = printable {
        return format!("{prefix}+{key}");
    }
    if bytes.len() == 1 {
        let byte = bytes[0];
        if (0x01..=0x1a).contains(&byte) {
            return format!("Ctrl+{}", (b'A' + byte - 1) as char);
        }
        if let Some(key) = match byte {
            0x1c => Some('\\'),
            0x1d => Some(']'),
            0x1e => Some('^'),
            0x1f => Some('_'),
            _ => None,
        } {
            return format!("Ctrl+{key}");
        }
    }
    "Control key".to_owned()
}

#[cfg(test)]
mod window_ownership_tests {
    use crate::runtime::tests::{assert_tab_invariants, open_saved_tab, tab_runtime};
    use tempfile::tempdir;

    /// N7：没有登记窗口时（测试/CLI）标签自动归属隐式窗口 0，投影退化为全量标签。
    #[test]
    fn tabs_fall_back_to_an_implicit_window_without_registry() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = tab_runtime(temp.path(), 2);
        let first = open_saved_tab(&mut runtime, "saved-0");
        let second = open_saved_tab(&mut runtime, "saved-1");

        assert_eq!(runtime.windows.len(), 1);
        assert_eq!(runtime.focused_window, Some(0));
        assert_eq!(runtime.window_of_tab(&first), Some(0));
        assert_eq!(runtime.window_of_tab(&second), Some(0));
        let projection = runtime.projection();
        assert_eq!(projection.tab_count, 2);
        assert_eq!(projection.active_tab_id, second);
        assert_tab_invariants(&runtime);
    }

    /// N7：迁移标签只换 UI 归属——会话/标签登记不变，两个窗口的投影各看各的标签。
    #[test]
    fn moving_a_tab_to_another_window_filters_projections_only() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = tab_runtime(temp.path(), 2);
        let first = open_saved_tab(&mut runtime, "saved-0");
        let second = open_saved_tab(&mut runtime, "saved-1");
        let session_count = runtime.sessions.len();
        let session_key = runtime.tabs[1].session_id().expect("session").to_owned();

        runtime.register_window(1);
        runtime
            .move_tab_to_window(&second, 1)
            .expect("move tab to window 1");

        // 会话与标签登记没有被重建/移除。
        assert_eq!(runtime.sessions.len(), session_count);
        assert!(runtime.sessions.contains_key(&session_key));
        assert_eq!(runtime.tabs.len(), 2);
        // 归属发生变化：焦点仍在窗口 0，全局活动标签仍是窗口 0 的活动标签。
        assert_eq!(runtime.window_of_tab(&second), Some(1));
        assert_eq!(runtime.window_active_tab(1), Some(second.as_str()));
        assert_eq!(runtime.focused_window, Some(0));
        assert_eq!(runtime.active_tab_id.as_deref(), Some(first.as_str()));

        let window0 = runtime.projection_for_window(0);
        let window1 = runtime.projection_for_window(1);
        assert_eq!(window0.tab_count, 1);
        assert_eq!(window0.tabs[0].id, first);
        assert_eq!(window0.active_tab_id, first);
        assert_eq!(window1.tab_count, 1);
        assert_eq!(window1.tabs[0].id, second);
        assert_eq!(window1.active_tab_id, second);
        assert!(window1.tabs[0].active);
        assert!(!window0.tabs[0].active || window0.tabs[0].id == first);
        // 单窗口投影（无窗口覆盖）仍是聚焦窗口的全量语义。
        assert_eq!(runtime.projection().tabs.len(), 1);
        assert_tab_invariants(&runtime);

        // 移回主窗口：归属恢复，标签顺序追加在末尾。
        runtime
            .move_tab_to_window(&second, 0)
            .expect("move tab back");
        assert_eq!(runtime.window_of_tab(&second), Some(0));
        assert_eq!(runtime.window_active_tab(0), Some(second.as_str()));
        let merged = runtime.projection_for_window(0);
        assert_eq!(merged.tab_count, 2);
        assert_eq!(merged.active_tab_id, second);
    }

    /// N7：关闭窗口只断开它自己的标签；其它窗口/会话不受影响。
    #[test]
    fn closing_a_window_disconnects_only_its_tabs() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = tab_runtime(temp.path(), 2);
        let first = open_saved_tab(&mut runtime, "saved-0");
        let second = open_saved_tab(&mut runtime, "saved-1");
        let second_session = runtime.tabs[1].session_id().expect("session").to_owned();

        runtime.register_window(1);
        runtime
            .move_tab_to_window(&second, 1)
            .expect("move tab to window 1");
        assert_eq!(runtime.active_connections_in_window(1), 1);

        let (closed, stopped_log) = runtime.close_window_tabs(1);
        assert_eq!(closed, 1);
        assert!(stopped_log.is_none());
        assert!(!runtime.sessions.contains_key(&second_session));
        assert!(runtime
            .sessions
            .contains_key(runtime.tabs[0].session_id().expect("first session")));
        assert_eq!(runtime.window_of_tab(&first), Some(0));
        assert!(!runtime.windows.contains_key(&1));
        assert_eq!(runtime.focused_window, Some(0));
        assert_tab_invariants(&runtime);
    }

    /// N7：标签右键的迁移启用条件（主窗口内不能"移到主窗口"）。
    #[test]
    fn move_menu_flags_target_the_main_window() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = tab_runtime(temp.path(), 2);
        let first = open_saved_tab(&mut runtime, "saved-0");
        let second = open_saved_tab(&mut runtime, "saved-1");

        let in_main = runtime.prepare_tab_context_menu(&first);
        assert!(in_main.tab_menu_move_to_new_window_enabled);
        assert!(!in_main.tab_menu_move_to_main_window_enabled);

        runtime.register_window(7);
        runtime
            .move_tab_to_window(&second, 7)
            .expect("move tab to window 7");
        let moved = runtime.prepare_tab_context_menu(&second);
        assert!(moved.tab_menu_move_to_new_window_enabled);
        assert!(moved.tab_menu_move_to_main_window_enabled);
        assert_eq!(runtime.main_window_id(), Some(0));
        let _ = first;
    }

    /// N7/TAB-6：零标签窗口不再强制显示 Quick Connect（D1）；覆盖层是工作区状态，
    /// 关闭后空窗口保持空工作区。
    #[test]
    fn empty_window_projects_an_empty_workspace_when_the_overlay_is_closed() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = tab_runtime(temp.path(), 2);
        let _first = open_saved_tab(&mut runtime, "saved-0");
        let second = open_saved_tab(&mut runtime, "saved-1");
        // 打开已保存会话会收起启动时的 QC 覆盖层。
        assert!(!runtime.projection().quick_connect_visible);
        runtime.register_window(1);
        runtime
            .move_tab_to_window(&second, 1)
            .expect("move tab to window 1");
        runtime
            .move_tab_to_window(&second, 0)
            .expect("move tab back to window 0");

        let empty = runtime.projection_for_window(1);
        assert_eq!(empty.tab_count, 0);
        assert!(!empty.quick_connect_visible);
        assert!(!empty.has_active_session);

        let main = runtime.projection_for_window(0);
        assert_eq!(main.tab_count, 2);
        assert!(!main.quick_connect_visible);

        // 显式打开覆盖层：零标签窗口也会投影 QC（工作区级状态）。
        runtime.open_quick_connect();
        assert!(runtime.projection_for_window(1).quick_connect_visible);
        runtime.dismiss_quick_connect();
        assert!(!runtime.projection_for_window(1).quick_connect_visible);
    }

    /// N7：窗口关闭确认标记按窗口隔离，并在窗口移除后清理。
    #[test]
    fn window_close_pending_is_per_window() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = tab_runtime(temp.path(), 1);
        let tab = open_saved_tab(&mut runtime, "saved-0");
        runtime.register_window(1);
        runtime
            .move_tab_to_window(&tab, 1)
            .expect("move tab to window 1");

        runtime.set_window_close_pending(1, true);
        assert!(runtime.window_close_pending(1));
        assert!(!runtime.window_close_pending(0));
        assert!(
            runtime
                .projection_for_window(1)
                .close_window_confirm_visible
        );
        assert!(
            !runtime
                .projection_for_window(0)
                .close_window_confirm_visible
        );
        assert_eq!(
            runtime
                .projection_for_window(1)
                .close_window_confirm_active_count,
            1
        );

        runtime.set_window_close_pending(1, false);
        assert!(!runtime.window_close_pending(1));
    }
}
