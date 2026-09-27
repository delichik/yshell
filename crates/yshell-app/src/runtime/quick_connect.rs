//! N2 Quick Connect：标签页、输入态、历史/快速链接持久化与行投影。
//!
//! 设计：`docs/product/yshell-next-n2-quick-connect.md` §1/§2/§4。
//!
//! 职责划分：
//! * 页面（`ui/pages/quick_connect.slint`）只渲染；本模块投影行数据、错误文案与摘要。
//! * 历史/快速链接即时写回 `config.toml`；写失败只提示状态栏，不阻断连接（§1.5）。
//! * 连接成功钩子由 `connection.rs` 在 shell 真正连上后调用
//!   （`record_connect_success`），覆盖直连、密码重试与主机密钥信任三条路径。

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use yshell_config::{parse_quick_connect, NewTabMode, QuickConnectEntry, QuickConnectTarget};
use yshell_core::SessionState;

use crate::session_runtime::SessionSource;

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
    // ------------------------------------------------------------ 标签页（§1.2）
    /// `+` 按钮：按 `ui.new_tab_mode` 打开 Quick Connect 页或 Session Editor。
    ///
    /// `Ctrl+N`（`start_new_saved_session_editor`）不经过这里，始终打开编辑器。
    pub fn handle_new_tab_default(&mut self) -> AppProjection {
        match self.config_document.ui.new_tab_mode {
            NewTabMode::QuickConnect => self.open_quick_connect_tab(),
            NewTabMode::SessionEditor => self.start_new_saved_session_editor(),
        }
    }

    /// 打开或激活 Quick Connect 页标签（D11：`+` 默认、Ctrl+O 同路径）。
    ///
    /// 启动/无标签时由 `quick_connect_visible()` 直接显示页面（不新建标签），
    /// 见设计 §1.2"无标签则不新建"。
    pub fn open_quick_connect_tab(&mut self) -> AppProjection {
        let created = self.activate_quick_connect_tab_inner();
        self.status_text = if created {
            "Opened the Quick Connect page.".to_owned()
        } else {
            "Activated the Quick Connect page.".to_owned()
        };
        self.projection()
    }

    /// 打开/激活 QC 标签但不改状态栏（关闭标签后的回退要保持"Closed tab …"文案）。
    /// 返回是否新建了标签。
    pub(crate) fn activate_quick_connect_tab_inner(&mut self) -> bool {
        let created = if let Some(tab_id) = self.quick_connect_tab_id() {
            self.active_tab_id = Some(tab_id);
            false
        } else {
            let tab_id = format!("tab-quick-connect-{}", self.allocate_runtime_ordinal());
            self.tabs.push(TabEntry::quick_connect(tab_id.clone()));
            self.active_tab_id = Some(tab_id);
            true
        };
        self.active_session_id = None;
        self.refresh_terminal_search_for_active_session();
        created
    }

    /// 现有 Quick Connect 标签的 id（同一时刻最多一个）。
    pub(crate) fn quick_connect_tab_id(&self) -> Option<String> {
        self.tabs
            .iter()
            .find(|tab| tab.kind.is_quick_connect())
            .map(|tab| tab.tab_id.clone())
    }

    /// N2：内容区是否显示 Quick Connect 页。
    ///
    /// 两种情况：活动标签就是 QC 页；或者**根本没有标签**（启动落点、关掉最后一个
    /// 标签的回落）——此时显示 QC 页但不新建标签（设计 §1.2"无标签则不新建"）。
    pub(crate) fn quick_connect_visible(&self) -> bool {
        if self.tabs.is_empty() {
            return true;
        }
        self.active_tab_id
            .as_deref()
            .is_some_and(|tab_id| self.tab_kind_is_quick_connect(tab_id))
    }

    // -------------------------------------------------------- 输入与提交（§2）
    /// 输入框逐键回传：保持运行时副本与页面一致（投影回写时不会吃掉用户输入）。
    pub fn update_quick_connect_input(&mut self, value: &str) -> AppProjection {
        self.quick_connect_input = value.to_owned();
        self.quick_connect_error_text.clear();
        self.projection()
    }

    /// Enter / "连接"：解析并直连。
    ///
    /// * 解析失败：只更新内联错误文案，不建标签、不记录历史；
    /// * 解析成功：走既有 `handle_quick_connect`（与 CLI 同一条连接路径），
    ///   连接成功由 `record_connect_success` 落历史（含主机密钥/密码重试后成功）。
    pub fn submit_quick_connect(&mut self, input: &str) -> AppResult<AppProjection> {
        let target = match parse_quick_connect(input) {
            Ok(target) => target,
            Err(error) => {
                self.quick_connect_input = input.to_owned();
                self.quick_connect_error_text = error.to_string();
                self.status_text = format!("Quick Connect error: {error}");
                return Ok(self.projection());
            }
        };
        let canonical = target.canonical();
        self.quick_connect_input = input.to_owned();
        self.quick_connect_error_text.clear();
        self.handle_quick_connect(input)?;
        if let Some(session_key) = self.active_session_id.clone() {
            self.pending_quick_connect_targets
                .insert(session_key.clone(), canonical);
            // fake/已就绪后端会同步连接成功：立即补一次成功钩子（异步路径由
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
            // 草稿会话不计入 Quick Connect 历史。
            Some(SessionSource::Draft) | None => return,
        };
        let Some(profile_id) = saved_profile_id else {
            let Some(target) = self.pending_quick_connect_targets.remove(session_id) else {
                return;
            };
            self.quick_connect_last_target = target.clone();
            self.quick_connect_input.clear();
            self.quick_connect_error_text.clear();
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
        self.persist_config_with_status("quick-connect-history");
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
            self.status_text = format!("`{canonical}` is already a quick link.");
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
        self.persist_config_with_status("quick-connect-pin");
        self.status_text = format!("Pinned `{canonical}` as a quick link.");
        Ok(self.projection())
    }

    /// `remove-link`：按 id 移除快速链接。
    pub fn quick_link_remove(&mut self, id: &str) -> AppResult<AppProjection> {
        let before = self.config_document.quick_links.len();
        self.config_document
            .quick_links
            .retain(|link| link.id != id);
        if self.config_document.quick_links.len() == before {
            self.status_text = format!("Quick link `{id}` was not found.");
            return Ok(self.projection());
        }
        self.persist_config_with_status("quick-connect-unpin");
        self.status_text = "Removed the quick link.".to_owned();
        Ok(self.projection())
    }

    /// `clear-history`（D12：可清空）。
    pub fn quick_connect_history_clear(&mut self) -> AppResult<AppProjection> {
        let cleared = self.config_document.quick_connect.history.len();
        self.config_document.quick_connect.history.clear();
        self.persist_config_with_status("quick-connect-clear");
        self.status_text = format!("Cleared {cleared} Quick Connect history entrie(s).");
        Ok(self.projection())
    }

    /// 复制成功后的状态栏文案（剪贴板写入由 bootstrap 完成）。
    pub fn quick_connect_copied_status(&mut self, target: &str) -> AppProjection {
        self.status_text = format!("Copied `{target}` to the clipboard.");
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
        self.editor_modal_visible = true;
        self.editor_section = EditorSection::General;
        self.editor_auth_test_status = EditorAuthTestStatus::Hint;
        self.status_text = format!(
            "Session editor opened with Quick Connect target `{}`.",
            parsed.canonical()
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

    /// 输入框下方的摘要（历史开关/条数/上限）。
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
    fn persist_config_with_status(&mut self, status_kind: &str) {
        if let Err(error) = self.config_store.save(&self.config_document) {
            self.set_status_kind(
                status_kind,
                format!("Failed to persist Quick Connect state: {error}"),
                String::new(),
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
