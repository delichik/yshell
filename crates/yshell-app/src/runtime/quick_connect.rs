//! N2 Quick Connect：工作区覆盖层状态、输入态、历史/快速链接持久化与行投影。
//!
//! 设计：`docs/product/yshell-next-n2-quick-connect.md` §1/§2/§4。
//!
//! TAB-6 起 Quick Connect 是**工作区级覆盖层**，不再占用标签页：
//! * 开关只由 [`AppRuntime::quick_connect_overlay_open`] 驱动（打开/关闭/切换不改变
//!   标签数量）；零标签时仍可打开覆盖层，关闭后保持空工作区（D1，不强制重开）。
//! * 页面（`ui/pages/quick_connect.slint`）只渲染；本模块投影行数据、错误文案与摘要。
//! * 历史/快速链接即时写回 `config.toml`；写失败只提示状态栏，不阻断连接（§1.5）。
//! * 连接成功钩子由 `connection.rs` 在 shell 真正连上后调用
//!   （`record_connect_success`），覆盖直连、密码重试与主机密钥信任三条路径。

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use yshell_config::{parse_quick_connect, QuickConnectEntry, QuickConnectTarget};
use yshell_core::SessionState;

use crate::session_runtime::{SessionRuntime, SessionSource};

use super::*;

/// "最近连接"的显示上限（历史本身由 `quick_connect.limit` 限制，默认 20）。
const QUICK_CONNECT_ROW_LIMIT: usize = 20;

/// 已保存会话最近使用的记录条数上限（仅内存态）。
const RECENT_SAVED_USAGE_LIMIT: usize = 8;

/// "最近连接"行（与 Slint `QuickConnectRow` 字段一一对应，纯展示投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickConnectRowData {
    /// `"recent"`（QC 历史）/ `"saved"`（已保存会话最近使用）。
    pub kind: String,
    pub title: String,
    pub target: String,
    pub last_used_text: String,
}

/// "快速链接"行（与 Slint `QuickLinkRow` 字段一一对应）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickLinkRowData {
    pub id: String,
    pub label: String,
    pub target: String,
}

impl AppRuntime {
    // ------------------------------------------- 新标签页（原 Quick Connect）
    /// `+` 按钮：新建一个 New Tab 页（快速连接作为标签内容，不再使用本地终端）。
    ///
    /// `Ctrl+N`（`start_new_saved_session_editor`）不经过这里，始终打开编辑器；
    /// `Ctrl+O` 也创建/聚焦 New Tab。
    pub fn handle_new_tab_default(&mut self) -> AppProjection {
        self.open_new_tab()
    }

    /// 打开 Quick Connect 页：聚焦当前 New Tab，没有则新建一个。
    pub fn open_quick_connect(&mut self) -> AppProjection {
        if self.quick_connect_visible() {
            self.set_status_kind(
                "quick-connect-activated",
                "Activated the Quick Connect page.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        }
        self.open_new_tab()
    }

    /// 关闭当前 New Tab（等同于关闭标签）。
    pub fn dismiss_quick_connect(&mut self) -> AppProjection {
        if self.quick_connect_visible() {
            self.close_quick_connect()
        } else {
            self.projection()
        }
    }

    /// 关闭当前 New Tab（无论当前模式，用于重置/测试路径）。
    pub fn close_quick_connect(&mut self) -> AppProjection {
        let Some(tab_id) = self.active_tab_id.clone().filter(|tab_id| {
            self.tab_index(tab_id)
                .is_some_and(|index| self.tabs[index].is_new_tab())
        }) else {
            return self.projection();
        };
        self.close_tab_immediate(&tab_id);
        self.set_status_kind(
            "quick-connect-closed",
            "Closed the Quick Connect page.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    /// Ctrl+O：当前是 New Tab 时关闭，否则新建/聚焦 New Tab。
    pub fn toggle_quick_connect(&mut self) -> AppProjection {
        if self.quick_connect_visible() {
            self.close_quick_connect()
        } else {
            self.open_quick_connect()
        }
    }

    /// Whether the focused tab is a New Tab (Quick Connect) page.
    pub(crate) fn quick_connect_visible(&self) -> bool {
        self.active_tab_id
            .as_deref()
            .and_then(|tab_id| self.tab_index(tab_id))
            .is_some_and(|index| self.tabs[index].is_new_tab())
    }

    // -------------------------------------------------------- 输入与提交（§2）
    /// 输入框逐键回传：写入当前 New Tab 的独立输入状态。
    pub fn update_quick_connect_input(&mut self, value: &str) -> AppProjection {
        if let Some(index) = self
            .active_tab_id
            .as_deref()
            .and_then(|tab_id| self.tab_index(tab_id))
            .filter(|index| self.tabs[*index].is_new_tab())
        {
            self.tabs[index].quick_connect_input = value.to_owned();
            self.tabs[index].quick_connect_error.clear();
        }
        self.projection()
    }

    /// Enter / "连接"：解析并直连。
    ///
    /// * 解析失败：只更新当前 New Tab 的内联错误，不改标签模式；
    /// * 解析成功：当前 New Tab 原地变成 Terminal，沿用同一 tab id；
    /// * 需要密码/主机密钥确认时，弹窗流程结束后仍回到这个 tab。
    pub fn submit_quick_connect(&mut self, input: &str) -> AppResult<AppProjection> {
        let mut active_new_tab = self
            .active_tab_id
            .as_deref()
            .and_then(|tab_id| self.tab_index(tab_id))
            .filter(|index| self.tabs[*index].is_new_tab());
        // Robustness for non-UI callers: submitting a Quick Connect target
        // always has a New Tab to live in.
        if active_new_tab.is_none() {
            self.open_new_tab();
            active_new_tab = self
                .active_tab_id
                .as_deref()
                .and_then(|tab_id| self.tab_index(tab_id))
                .filter(|index| self.tabs[*index].is_new_tab());
        }
        let target = match parse_quick_connect(input) {
            Ok(target) => target,
            Err(error) => {
                let detail = error.to_string();
                if let Some(index) = active_new_tab {
                    self.tabs[index].quick_connect_input = input.to_owned();
                    self.tabs[index].quick_connect_error = detail.clone();
                }
                self.set_status_kind(
                    "quick-connect-error",
                    format!("Quick Connect error: {detail}"),
                    detail,
                    String::new(),
                );
                return Ok(self.projection());
            }
        };
        let canonical = target.canonical();
        // D10："已连接目标"只由成功钩子写入；新的尝试先清掉上一次的标记，
        // 失败时页面不会再显示"Connected to …"（B 轮3 #15）。
        self.quick_connect_last_target.clear();
        let Some(index) = active_new_tab else {
            return Ok(self.projection());
        };
        self.tabs[index].quick_connect_input = input.to_owned();
        self.tabs[index].quick_connect_error.clear();
        let tab_id = self.tabs[index].tab_id.clone();
        let mut runtime =
            SessionRuntime::from_quick_connect(target, self.allocate_runtime_ordinal());
        runtime.retarget_tab_id(&tab_id);
        self.pending_quick_connect_targets
            .insert(runtime.session_id().as_str().to_owned(), canonical);
        self.activate_runtime_session_bound(runtime, Some(tab_id))?;
        if let Some(session_key) = self.active_session_id.clone() {
            // 测试壳/已就绪会话会同步连接成功：立即补一次成功钩子（异步路径由
            // `record_connect_success` 在 shell 连上时触发）。
            let connected = self
                .sessions
                .get(&session_key)
                .is_some_and(|runtime| runtime.state == SessionState::Connected);
            if connected {
                self.record_connect_success(&session_key);
            }
        }
        Ok(self.projection())
    }

    /// 连接成功钩子（由 `connection.rs` 在 shell 连上后调用）。
    ///
    /// QC 直连：落 Quick Connect 历史 + 记录"最近一次成功目标"并清空输入；
    /// 已保存会话：记录该 profile 的最近使用与历史（行投影据此合并去重）。
    pub(crate) fn record_connect_success(&mut self, session_id: &str) {
        // `SessionSource` 不实现 Clone，先取出需要的最小信息。
        let saved_profile_id = match self.sessions.get(session_id).map(|runtime| &runtime.source) {
            Some(SessionSource::QuickConnect) => None,
            Some(SessionSource::SavedSession { profile_id }) => Some(profile_id.clone()),
            // 草稿/本地终端不计入 Quick Connect 历史。
            Some(SessionSource::Draft | SessionSource::Local) | None => return,
        };
        let Some(profile_id) = saved_profile_id else {
            let Some(target) = self.pending_quick_connect_targets.remove(session_id) else {
                return;
            };
            self.quick_connect_last_target = target.clone();
            self.record_quick_connect_usage(&target);
            return;
        };
        let Some(profile) = self.config_document.find_session(&profile_id).cloned() else {
            return;
        };
        let canonical =
            canonical_target_text(profile.username.as_deref(), &profile.host, profile.port);
        self.mark_saved_session_used(&profile_id);
        self.record_quick_connect_usage(&canonical);
        self.quick_connect_last_target = canonical;
    }

    /// 把目标写入 `quick_connect.history`（去重、前插、按 `limit` 截断、即时写回）。
    fn record_quick_connect_usage(&mut self, canonical: &str) {
        if !self.config_document.quick_connect.enabled {
            return;
        }
        let now = now_epoch_seconds();
        let history = &mut self.config_document.quick_connect.history;
        if let Some(entry) = history.iter_mut().find(|entry| entry.target == canonical) {
            entry.last_used_at = now;
            entry.use_count = entry.use_count.saturating_add(1);
            let updated = entry.clone();
            history.retain(|entry| entry.target != canonical);
            history.insert(0, updated);
        } else {
            history.insert(
                0,
                QuickConnectEntry {
                    target: canonical.to_owned(),
                    last_used_at: now,
                    use_count: 1,
                },
            );
        }
        let limit = self.config_document.quick_connect.limit.max(1);
        history.truncate(limit);
        // D37：`quick-connect-history` 现被摘要模板占用（`TextFormats.status-message`），
        // 持久化失败改用独立 kind，避免被套进 "{0} recorded target(s)…" 模板。
        self.persist_config_with_status();
    }

    fn mark_saved_session_used(&mut self, profile_id: &str) {
        let now = now_epoch_seconds();
        self.recent_saved_usage
            .retain(|(existing, _)| existing != profile_id);
        self.recent_saved_usage
            .insert(0, (profile_id.to_owned(), now));
        self.recent_saved_usage.truncate(RECENT_SAVED_USAGE_LIMIT);
    }

    // ---------------------------------------------------- 快速链接 / 历史（§1.3/§1.4）
    /// `pin-target`：把目标固定为快速链接（已存在时不重复添加）。
    pub fn quick_connect_pin(&mut self, target: &str) -> AppResult<AppProjection> {
        let canonical = canonicalize_target(target);
        if self
            .config_document
            .quick_links
            .iter()
            .any(|link| link.target == canonical)
        {
            self.set_status_kind(
                "quick-link-already",
                format!("`{canonical}` is already a quick link."),
                canonical.clone(),
                String::new(),
            );
            return Ok(self.projection());
        }
        // D13：快速链接只存目标；标签优先用同目标的已保存会话名。
        let label = self
            .saved_session_label_for_target(&canonical)
            .unwrap_or_else(|| canonical.clone());
        let id = self.next_quick_link_id();
        let sort_order = self
            .config_document
            .quick_links
            .iter()
            .map(|link| link.sort_order)
            .max()
            .map_or(0, |max| max.saturating_add(1));
        self.config_document.quick_links.push(QuickLink {
            id,
            label,
            target: canonical.clone(),
            sort_order,
        });
        self.persist_config_with_status();
        self.set_status_kind(
            "quick-link-pinned",
            format!("Pinned `{canonical}` as a quick link."),
            canonical,
            String::new(),
        );
        Ok(self.projection())
    }

    /// `remove-link`：按 id 移除快速链接。
    pub fn quick_link_remove(&mut self, id: &str) -> AppResult<AppProjection> {
        let before = self.config_document.quick_links.len();
        self.config_document
            .quick_links
            .retain(|link| link.id != id);
        if self.config_document.quick_links.len() == before {
            self.set_status_kind(
                "quick-link-missing",
                format!("Quick link `{id}` was not found."),
                id.to_owned(),
                String::new(),
            );
            return Ok(self.projection());
        }
        self.persist_config_with_status();
        self.set_status_kind(
            "quick-link-removed",
            "Removed the quick link.".to_owned(),
            String::new(),
            String::new(),
        );
        Ok(self.projection())
    }

    /// `clear-history`（D12：可清空）。
    pub fn quick_connect_history_clear(&mut self) -> AppResult<AppProjection> {
        let cleared = self.config_document.quick_connect.history.len();
        self.config_document.quick_connect.history.clear();
        self.persist_config_with_status();
        self.set_status_kind(
            "quick-connect-history-cleared",
            format!("Cleared {cleared} Quick Connect history entrie(s)."),
            cleared.to_string(),
            String::new(),
        );
        Ok(self.projection())
    }

    /// 复制成功后的状态栏文案（剪贴板写入由 bootstrap 完成）。
    pub fn quick_connect_copied_status(&mut self, target: &str) -> AppProjection {
        self.set_status_kind(
            "quick-connect-copied",
            format!("Copied `{target}` to the clipboard."),
            target.to_owned(),
            String::new(),
        );
        self.projection()
    }

    /// `save-as-session`（§1.6）：打开 Session Editor 并预填目标。
    pub fn start_quick_connect_session_editor(&mut self, target: &str) -> AppResult<AppProjection> {
        let parsed = parse_quick_connect(target).map_err(AppError::from_error)?;
        self.editor = SessionEditorDraft::default();
        self.editor.host = parsed.host.clone();
        self.editor.port_text = parsed.port.to_string();
        self.editor.username = parsed.username.clone().unwrap_or_default();
        self.new_folder_name.clear();
        self.editor_discard_dialog_visible = false;
        self.editor_modal_visible = true;
        self.editor_section = EditorSection::General;
        self.editor_auth_test_status = EditorAuthTestStatus::Hint;
        self.editor_set_theme_baseline(None);
        self.editor_baseline = Some(self.editor.clone());
        let canonical = parsed.canonical();
        self.set_status_kind(
            "session-editor-qc-target",
            format!("Session editor opened with Quick Connect target `{canonical}`."),
            canonical,
            String::new(),
        );
        Ok(self.projection())
    }

    // ------------------------------------------------------------ 投影辅助（§4）
    /// "最近连接"行：已保存会话最近使用 + QC 历史，按目标去重、时间倒序。
    pub(crate) fn quick_connect_row_data(&self) -> Vec<QuickConnectRowData> {
        let now = now_epoch_seconds();
        // (时间戳, 种类优先级, 行)：同目标同时存在 saved/recent 时优先显示 saved（带会话名）。
        let mut rows: Vec<(i64, u8, QuickConnectRowData)> = Vec::new();
        for (profile_id, last_used_at) in &self.recent_saved_usage {
            let Some(profile) = self.config_document.find_session(profile_id) else {
                continue;
            };
            rows.push((
                *last_used_at,
                1,
                QuickConnectRowData {
                    kind: "saved".to_owned(),
                    title: profile.name.clone(),
                    target: canonical_target_text(
                        profile.username.as_deref(),
                        &profile.host,
                        profile.port,
                    ),
                    last_used_text: format_relative_time(*last_used_at, now),
                },
            ));
        }
        for entry in &self.config_document.quick_connect.history {
            rows.push((
                entry.last_used_at,
                0,
                QuickConnectRowData {
                    kind: "recent".to_owned(),
                    title: entry.target.clone(),
                    target: entry.target.clone(),
                    last_used_text: format_relative_time(entry.last_used_at, now),
                },
            ));
        }
        rows.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        for (_, _, row) in rows {
            if !seen.insert(row.target.clone()) {
                continue;
            }
            result.push(row);
            if result.len() >= QUICK_CONNECT_ROW_LIMIT {
                break;
            }
        }
        result
    }

    /// "快速链接"行：按 `sort_order` 排序。
    pub(crate) fn quick_link_row_data(&self) -> Vec<QuickLinkRowData> {
        let mut links = self.config_document.quick_links.clone();
        links.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then_with(|| left.id.cmp(&right.id))
        });
        links
            .into_iter()
            .map(|link| QuickLinkRowData {
                id: link.id,
                label: link.label,
                target: link.target,
            })
            .collect()
    }

    /// 输入框下方的摘要（历史开关/条数/上限；回退文案，正式渲染走 `parts`）。
    pub(crate) fn quick_connect_summary_text(&self) -> String {
        let profile = &self.config_document.quick_connect;
        if !profile.enabled {
            return "Connection history is disabled.".to_owned();
        }
        format!(
            "{} recorded target(s); keep up to {}.",
            profile.history.len(),
            profile.limit
        )
    }

    /// D37：QC 摘要的结构化 kind/参数（页面经 `TextFormats.status-message` 渲染）。
    ///
    /// 启用态：`("quick-connect-history", 条数, 上限)`；历史关闭态没有模板，
    /// 返回空 kind 让 UI 回退 `quick_connect_summary_text()`（页面本身已有
    /// `@tr("Connection history is disabled.")` 空态提示）。
    pub(crate) fn quick_connect_summary_parts(&self) -> (String, String, String) {
        let profile = &self.config_document.quick_connect;
        if !profile.enabled {
            return (String::new(), String::new(), String::new());
        }
        (
            "quick-connect-history".to_owned(),
            profile.history.len().to_string(),
            profile.limit.to_string(),
        )
    }

    fn saved_session_label_for_target(&self, canonical: &str) -> Option<String> {
        self.saved_session_profiles()
            .into_iter()
            .find(|profile| {
                canonical_target_text(profile.username.as_deref(), &profile.host, profile.port)
                    == canonical
            })
            .map(|profile| profile.name)
    }

    fn next_quick_link_id(&self) -> String {
        for index in 1..=usize::MAX {
            let candidate = format!("quick-link-{index}");
            if !self
                .config_document
                .quick_links
                .iter()
                .any(|link| link.id == candidate)
            {
                return candidate;
            }
        }
        "quick-link".to_owned()
    }

    /// 写回配置；失败只更新状态栏（§1.5：保存失败不阻断连接）。
    fn persist_config_with_status(&mut self) {
        if let Err(error) = self.config_store.save(&self.config_document) {
            let detail = error.to_string();
            self.set_status_kind(
                "quick-connect-persist-failed",
                format!("Failed to persist Quick Connect state: {detail}"),
                detail,
                String::new(),
            );
        }
    }
}

/// `[user@]host:port` 规范化（IPv6 加方括号，端口必带）。
fn canonical_target_text(username: Option<&str>, host: &str, port: u16) -> String {
    QuickConnectTarget {
        username: username.map(str::to_owned),
        host: host.to_owned(),
        port,
    }
    .canonical()
}

/// 尽力把用户/行数据里的目标规范化为 canonical 形式（解析失败时原样去空白）。
fn canonicalize_target(target: &str) -> String {
    parse_quick_connect(target)
        .map(|parsed| parsed.canonical())
        .unwrap_or_else(|_| target.trim().to_owned())
}

fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// 最近使用时间的展示文案（相对时间；仅 Rust 侧拼装，不参与翻译）。
fn format_relative_time(timestamp: i64, now: i64) -> String {
    if timestamp <= 0 {
        return String::new();
    }
    let delta = now.saturating_sub(timestamp).max(0);
    match delta {
        0..=59 => "just now".to_owned(),
        60..=3_599 => {
            let minutes = delta / 60;
            if minutes == 1 {
                "1 minute ago".to_owned()
            } else {
                format!("{minutes} minutes ago")
            }
        }
        3_600..=86_399 => {
            let hours = delta / 3_600;
            if hours == 1 {
                "1 hour ago".to_owned()
            } else {
                format!("{hours} hours ago")
            }
        }
        86_400..=172_799 => "Yesterday".to_owned(),
        _ => {
            let days = delta / 86_400;
            format!("{days} days ago")
        }
    }
}
