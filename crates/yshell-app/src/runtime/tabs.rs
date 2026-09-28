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

        if active_changed || background_state_changed {
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
            self.active_tab_id = Some(tab_id.to_owned());
            self.active_session_id = None;
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
        self.active_tab_id = Some(tab_id.to_owned());
        self.active_session_id = Some(session_id.clone());
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
    pub fn disconnect_tab_session(&mut self, tab_id: &str) -> AppResult<AppProjection> {
        let session_key = self.tab_session_key(tab_id)?;
        self.disconnect_session_by_key(&session_key)
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
        self.tabs
            .iter()
            .map(|tab| {
                let active = self.active_tab_id.as_deref() == Some(tab.tab_id.as_str());
                if tab.kind.is_quick_connect() {
                    return TabData {
                        id: tab.tab_id.clone(),
                        title: String::new(),
                        state_text: String::new(),
                        connected: false,
                        active,
                        kind_text: "quick-connect".to_owned(),
                        logging: false,
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
                }
            })
            .collect()
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
        self.tabs.iter().any(|tab| {
            tab.session_id()
                .is_some_and(|session_id| !self.session_has_active_connection(session_id))
        })
    }

    pub(crate) fn close_scope_targets(&self, scope: &str) -> AppResult<Vec<String>> {
        if scope == CLOSE_SCOPE_ALL {
            return Ok(self.tabs.iter().map(|tab| tab.tab_id.clone()).collect());
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
        let index = self
            .tab_index(reference)
            .ok_or_else(|| AppError::new(format!("tab `{reference}` was not found")))?;
        let targets = match prefix {
            CLOSE_SCOPE_OTHERS => self
                .tabs
                .iter()
                .enumerate()
                .filter(|(position, _)| *position != index)
                .map(|(_, tab)| tab.tab_id.clone())
                .collect(),
            CLOSE_SCOPE_LEFT => self.tabs[..index]
                .iter()
                .map(|tab| tab.tab_id.clone())
                .collect(),
            CLOSE_SCOPE_RIGHT => self.tabs[index + 1..]
                .iter()
                .map(|tab| tab.tab_id.clone())
                .collect(),
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
        self.tabs.remove(index);
        if was_active {
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
