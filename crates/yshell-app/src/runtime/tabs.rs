//! Terminal tabs: entries, activation, close requests (single and batch) and the
//! session polling cursor.

use crate::{error::AppError, error::AppResult, session_runtime::SessionSource};
use yshell_core::{CommandDispatcher, SessionCommand, SessionState};

use super::*;

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
                    // 后台标签只更新 grid/parser；状态变化才需要刷新标签条投影。
                    // 后台会话的瞬时错误不打断整个 tick（活动会话的错误照旧上报）。
                    if let Ok(outcome) = self.poll_session_output(&session_key) {
                        background_state_changed |= outcome.state_changed;
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
            let shell_closed =
                previous_state == yshell_core::SessionState::Connected && !shell_connected;
            // shell 已经掉线时 poll 返回错误是预期路径：按 shell-closed 处理而不是让
            // 整个 tick 失败；仍处于连接状态时的瞬时 I/O 错误继续上报。
            let chunks = match poll_result {
                Ok(chunks) => chunks,
                Err(_) if shell_closed => Vec::new(),
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
            if is_active {
                self.status_text =
                    "The active live shell closed while the terminal was refreshing.".to_owned();
            }
            let sftp_status = self.sync_sftp_lifecycle_for_session(session_key, false);
            if is_active {
                self.status_text = format!("{} {}", self.status_text, sftp_status);
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
        // N2：快速连接页没有运行时会话，只切换活动标签。
        if self.tabs[index].kind.is_quick_connect() {
            let window_id = self.window_of_tab(tab_id);
            self.activate_tab_in_window(tab_id);
            if window_id.is_none_or(|id| self.focused_window == Some(id)) {
                self.active_tab_id = Some(tab_id.to_owned());
                self.active_session_id = None;
            }
            self.tabs[index].unread = 0;
            self.refresh_terminal_search_for_active_session();
            // D16：无会话标签不得残留上一个会话的 SFTP 目录（B 轮3 #19）。
            self.reset_sftp_view_state();
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.status_text = "Activated the Quick Connect page.".to_owned();
            return Ok(self.projection());
        }
        let Some(session_id) = self.tabs[index].session_id().map(str::to_owned) else {
            return Err(AppError::new(format!(
                "tab `{tab_id}` has no runtime session"
            )));
        };
        if !self.sessions.contains_key(&session_id) {
            return Err(AppError::new(format!(
                "runtime session for tab `{tab_id}` is missing"
            )));
        }
        let window_id = self.window_of_tab(tab_id);
        self.activate_tab_in_window(tab_id);
        if window_id.is_none_or(|id| self.focused_window == Some(id)) {
            self.active_tab_id = Some(tab_id.to_owned());
            self.active_session_id = Some(session_id.clone());
        }
        self.tabs[index].unread = 0;
        self.refresh_terminal_search_for_active_session();
        let title = self.tab_display_title(tab_id);
        // D16：切换会话时清掉上一个会话的目录并重新加载当前会话的 SFTP 视图。
        let sftp_session_changed = self.sftp_session.status_session_key() != session_id.as_str();
        if sftp_session_changed {
            self.reset_sftp_view_state();
        }
        let sftp_status = self.sync_sftp_lifecycle_for_session(&session_id, sftp_session_changed);
        self.status_text = format!("Activated tab `{title}`. {sftp_status}");
        Ok(self.projection())
    }

    pub fn request_close_tab(&mut self, tab_id: &str) -> AppResult<AppProjection> {
        let Some(index) = self.tab_index(tab_id) else {
            return Err(AppError::new(format!("tab `{tab_id}` was not found")));
        };
        if let Some(session_id) = self.tabs[index].session_id().map(str::to_owned) {
            if self.session_has_active_connection(&session_id) {
                let title = self.tab_display_title(tab_id);
                self.pending_close_tabs = Some(PendingCloseTabs {
                    tab_ids: vec![tab_id.to_owned()],
                    single: true,
                    active_connections: 1,
                });
                self.status_text =
                    format!("Confirm closing tab `{title}`: the session is still active.");
                return Ok(self.projection());
            }
        }
        let title = self.tab_display_title(tab_id);
        let stopped_log_path = self.close_tab_immediate(tab_id);
        self.status_text = match stopped_log_path {
            Some(path) => {
                format!("Closed tab `{title}`. Stopped logging; log file: `{path}`.")
            }
            None => format!("Closed tab `{title}`."),
        };
        // N2：没有标签时内容区自动回到 QC 页（`quick_connect_visible`），无需新建标签。
        Ok(self.projection())
    }

    pub fn request_close_tabs(&mut self, scope: &str) -> AppResult<AppProjection> {
        let targets = self.close_scope_targets(scope)?;
        if targets.is_empty() {
            self.status_text = "No tabs matched the close request.".to_owned();
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
        if active_connections > 0 {
            self.pending_close_tabs = Some(PendingCloseTabs {
                tab_ids: close_order,
                single: false,
                active_connections,
            });
            self.status_text = format!(
                "Confirm closing {target_count} tab(s): {active_connections} still active connection(s)."
            );
            return Ok(self.projection());
        }
        let mut stopped_log_path = None;
        for tab_id in &close_order {
            if let Some(path) = self.close_tab_immediate(tab_id) {
                stopped_log_path = Some(path);
            }
        }
        self.status_text = match stopped_log_path {
            Some(path) => format!(
                "Closed {target_count} tab(s) without active connections. Stopped logging; log file: `{path}`."
            ),
            None => format!("Closed {target_count} tab(s) without active connections."),
        };
        // N2：没有标签时内容区自动回到 QC 页（`quick_connect_visible`）。
        Ok(self.projection())
    }

    pub fn confirm_close_tabs(&mut self) -> AppResult<AppProjection> {
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
        self.status_text = match stopped_log_path {
            Some(path) => format!("{base_status} Stopped logging; log file: `{path}`."),
            None => base_status,
        };
        Ok(self.projection())
    }

    pub fn cancel_close_tabs(&mut self) -> AppProjection {
        self.pending_close_tabs = None;
        self.status_text = "Canceled the tab close confirmation.".to_owned();
        self.projection()
    }

    pub(crate) fn tab_index(&self, tab_id: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.tab_id == tab_id)
    }

    /// N2：标签是否是快速连接页（不存在时返回 false）。
    pub(crate) fn tab_kind_is_quick_connect(&self, tab_id: &str) -> bool {
        self.tab_index(tab_id)
            .is_some_and(|index| self.tabs[index].kind.is_quick_connect())
    }

    /// D18：标签右键菜单的连接状态（针对 `tab_menu_tab_id`）。
    ///
    /// 返回 `(reconnect_enabled, disconnect_enabled)`；没有会话（QC 页）时都为
    /// `false`。WS-A 的标签菜单用这两个旗标启用 Reconnect/Disconnect。
    pub(crate) fn tab_menu_connection_flags(&self) -> (bool, bool) {
        let session_key = self
            .tab_menu_tab_id
            .as_deref()
            .and_then(|tab_id| self.tab_index(tab_id))
            .and_then(|index| self.tabs[index].session_id());
        match session_key.and_then(|key| self.sessions.get(key)) {
            Some(runtime) => (
                matches!(
                    runtime.state,
                    SessionState::Idle | SessionState::Disconnected | SessionState::Failed
                ),
                matches!(
                    runtime.state,
                    SessionState::Connected | SessionState::Connecting
                ),
            ),
            None => (false, false),
        }
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

    /// 标签对应的会话 key（QC 页/未知标签报错）。
    fn tab_session_key(&self, tab_id: &str) -> AppResult<String> {
        self.tab_index(tab_id)
            .and_then(|index| self.tabs[index].session_id().map(str::to_owned))
            .ok_or_else(|| AppError::new(format!("tab `{tab_id}` does not have a runtime session")))
    }

    pub(crate) fn tab_display_title(&self, tab_id: &str) -> String {
        let Some(index) = self.tab_index(tab_id) else {
            return String::new();
        };
        if self.tabs[index].kind.is_quick_connect() {
            // 标签条上的 @tr 由 Slint 侧渲染（`kind_text`），这里只给状态栏文案。
            return "Quick Connect".to_owned();
        }
        self.tabs[index]
            .session_id()
            .and_then(|session_id| self.sessions.get(session_id))
            .map(|runtime| runtime.display_name.clone())
            .unwrap_or_default()
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
                if tab.kind.is_quick_connect() {
                    return TabData {
                        id: tab.tab_id.clone(),
                        title: String::new(),
                        state_text: String::new(),
                        connected: false,
                        active,
                        kind_text: "quick-connect".to_owned(),
                        logging: false,
                        sync_role_text,
                    };
                }
                let (title, state_text, connected) = match tab
                    .session_id()
                    .and_then(|session_id| self.sessions.get(session_id))
                {
                    Some(runtime) => (
                        runtime.display_name.clone(),
                        runtime.state_label().to_owned(),
                        runtime.state == SessionState::Connected,
                    ),
                    None => (tab.tab_id.clone(), "failed".to_owned(), false),
                };
                TabData {
                    id: tab.tab_id.clone(),
                    title,
                    state_text,
                    connected,
                    active,
                    kind_text: String::new(),
                    // N6：标签角标 REC（该会话正在写日志时）。
                    logging: tab
                        .session_id()
                        .is_some_and(|session_id| self.session_logging_active(session_id)),
                    sync_role_text,
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
                connected: false,
                active,
                kind_text: String::new(),
                logging: false,
                sync_role_text: String::new(),
            };
        };
        let tab = &self.tabs[index];
        let sync_role_text = self.tab_sync_role_text(&tab.tab_id);
        if tab.kind.is_quick_connect() {
            return TabData {
                id: tab.tab_id.clone(),
                title: String::new(),
                state_text: String::new(),
                connected: false,
                active,
                kind_text: "quick-connect".to_owned(),
                logging: false,
                sync_role_text,
            };
        }
        let (title, state_text, connected) = match tab
            .session_id()
            .and_then(|session_id| self.sessions.get(session_id))
        {
            Some(runtime) => (
                runtime.display_name.clone(),
                runtime.state_label().to_owned(),
                runtime.state == SessionState::Connected,
            ),
            None => (tab.tab_id.clone(), "failed".to_owned(), false),
        };
        TabData {
            id: tab.tab_id.clone(),
            title,
            state_text,
            connected,
            active,
            kind_text: String::new(),
            logging: tab
                .session_id()
                .is_some_and(|session_id| self.session_logging_active(session_id)),
            sync_role_text,
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
        // N2：快速连接页没有会话，不计入"断开标签"。
        self.menu_anchor_window_tab_ids().iter().any(|tab_id| {
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

    /// N7：锚定窗口的标签 id；无窗口登记时 = 全部标签（保持单窗口语义）。
    pub(crate) fn menu_anchor_window_tab_ids(&self) -> Vec<String> {
        match self.menu_anchor_window() {
            Some(window_id) => self.window_tab_ids(window_id),
            None => self.tabs.iter().map(|tab| tab.tab_id.clone()).collect(),
        }
    }

    pub(crate) fn close_scope_targets(&self, scope: &str) -> AppResult<Vec<String>> {
        // N7：全部 scope 都在"锚定窗口"的标签集合内计算（无窗口登记 = 全部标签）。
        let window_tabs = self.menu_anchor_window_tab_ids();
        if scope == CLOSE_SCOPE_ALL {
            return Ok(window_tabs);
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
                .filter(|tab| window_tabs.iter().any(|id| id == &tab.tab_id))
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
        let index = window_tabs
            .iter()
            .position(|tab_id| tab_id == reference)
            .ok_or_else(|| AppError::new(format!("tab `{reference}` was not found")))?;
        let targets = match prefix {
            CLOSE_SCOPE_OTHERS => window_tabs
                .iter()
                .enumerate()
                .filter(|(position, _)| *position != index)
                .map(|(_, tab_id)| tab_id.clone())
                .collect(),
            CLOSE_SCOPE_LEFT => window_tabs[..index].to_vec(),
            CLOSE_SCOPE_RIGHT => window_tabs[index + 1..].to_vec(),
            _ => unreachable!("matched one of the three prefixed scopes"),
        };
        Ok(targets)
    }

    /// 立即关闭标签（不询问）。返回被停止的活动日志文件路径（N6：关闭前强制
    /// 停止并 flush，供调用方在状态文案里提示文件位置）。
    pub(crate) fn close_tab_immediate(&mut self, tab_id: &str) -> Option<String> {
        let index = self.tab_index(tab_id)?;
        let session_id = self.tabs[index].session_id().map(str::to_owned);
        // N6：标签关闭/会话断开前先停止日志并 flush（设计 §3：关闭后不自动恢复）。
        let stopped_log_path = session_id
            .as_deref()
            .and_then(|session_id| self.stop_session_logging_for_close(session_id));
        let was_active = self.active_tab_id.as_deref() == Some(tab_id);
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
            }
            // 只在该窗口是聚焦窗口时同步全局活动标签；否则全局值属于另一个窗口。
            if window_id == self.focused_window {
                self.sync_active_from_focused_window();
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
        }
        if self.tab_menu_tab_id.as_deref() == Some(tab_id) {
            self.tab_menu_tab_id = None;
        }
        if sync_closed_title.is_some() {
            self.reconcile_input_sync_with(sync_closed_title);
        }
        // N2：快速连接页没有运行时会话，无需回收会话/核心登记。
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
        self.tabs
            .push(TabEntry::terminal(tab_id.to_owned(), session_id.to_owned()));
        // N7：新标签登记到当前交互窗口（键盘/菜单/QC 回调已把焦点切到该窗口）。
        self.register_tab_in_focused_window(tab_id);
        self.active_tab_id = Some(tab_id.to_owned());
        self.active_session_id = Some(session_id.to_owned());
        self.tab_menu_tab_id = None;
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
        self.active_tab_id = active_tab.clone();
        self.active_session_id = active_tab
            .as_deref()
            .and_then(|tab_id| self.tab_index(tab_id))
            .and_then(|index| self.tabs[index].session_id().map(str::to_owned));
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
        if self.focused_window == Some(window_id) {
            self.focused_window = self.windows.keys().next().copied();
            self.sync_active_from_focused_window();
        }
        (closed, stopped_log_path)
    }
}

/// N0/N2：标签承载的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TabKind {
    Terminal {
        session_id: String,
    },
    /// N2：快速连接页（无运行时会话，内容区由宿主的页面实例渲染）。
    QuickConnect,
}

/// N0：一个标签的运行时条目；`tabs` 的顺序就是标签条的显示顺序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TabEntry {
    pub(crate) tab_id: String,
    pub(crate) kind: TabKind,
    /// V1 恒 0：未读角标延后（N0 设计 §3/§6）。
    #[allow(dead_code)]
    pub(crate) unread: u32,
}

impl TabEntry {
    pub(crate) fn terminal(tab_id: String, session_id: String) -> Self {
        Self {
            tab_id,
            kind: TabKind::Terminal { session_id },
            unread: 0,
        }
    }

    pub(crate) fn quick_connect(tab_id: String) -> Self {
        Self {
            tab_id,
            kind: TabKind::QuickConnect,
            unread: 0,
        }
    }

    /// 终端标签的运行时会话 key；快速连接页返回 `None`。
    pub(crate) fn session_id(&self) -> Option<&str> {
        match &self.kind {
            TabKind::Terminal { session_id } => Some(session_id),
            TabKind::QuickConnect => None,
        }
    }
}

impl TabKind {
    pub(crate) fn is_quick_connect(&self) -> bool {
        matches!(self, Self::QuickConnect)
    }
}

/// N0：单个标签的只读投影（Slint 标签条模型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabData {
    /// 标签 id（`activate_tab`/`close_tab` 的入参）。
    pub id: String,
    pub title: String,
    /// 会话状态枚举 id（`idle`/`connecting`/...）。
    pub state_text: String,
    /// 是否处于 `connected`（Slint 侧标签样式/菜单启用条件用）。
    pub connected: bool,
    pub active: bool,
    /// N2：标签种类 id（空 = 终端；`quick-connect` = 快速连接页，标题由 Slint 侧 @tr）。
    pub kind_text: String,
    /// N6：该标签的会话是否正在写日志（标签角标 REC）。
    pub logging: bool,
    /// N9：同步发送按键的角色（空 = 无；`source` = 源；`target` = 接收目标）。
    pub sync_role_text: String,
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
        self.status_text =
            "Polled the active terminal, but no new output was available.".to_owned();
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

    /// N7：空窗口投影 Quick Connect 页（而不是没有会话的终端视图）。
    #[test]
    fn empty_window_projects_the_quick_connect_page() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = tab_runtime(temp.path(), 2);
        let _first = open_saved_tab(&mut runtime, "saved-0");
        let second = open_saved_tab(&mut runtime, "saved-1");
        runtime.register_window(1);
        runtime
            .move_tab_to_window(&second, 1)
            .expect("move tab to window 1");
        runtime
            .move_tab_to_window(&second, 0)
            .expect("move tab back to window 0");

        let empty = runtime.projection_for_window(1);
        assert_eq!(empty.tab_count, 0);
        assert!(empty.quick_connect_visible);
        assert!(!empty.has_active_session);

        let main = runtime.projection_for_window(0);
        assert_eq!(main.tab_count, 2);
        assert!(!main.quick_connect_visible);
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
