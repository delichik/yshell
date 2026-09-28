//! Menu flags: tab context-menu preparation and close-scope menu state.

use std::path::Path;

use super::*;

impl AppRuntime {
    pub fn prepare_tab_context_menu(&mut self, tab_id: &str) -> AppProjection {
        self.tab_menu_tab_id = if self.tab_index(tab_id).is_some() {
            Some(tab_id.to_owned())
        } else {
            None
        };
        self.projection()
    }

    pub(crate) fn tab_menu_flags(&self) -> (bool, bool, bool, bool, bool) {
        // N7：关闭 scope 在右键标签所属窗口内计算（无窗口登记 = 全部标签）。
        let window_tabs = self.menu_anchor_window_tab_ids();
        let index = self
            .tab_menu_tab_id
            .as_deref()
            .and_then(|tab_id| window_tabs.iter().position(|id| id == tab_id));
        let count = window_tabs.len();
        (
            count > 1,
            index.is_some_and(|index| index > 0),
            index.is_some_and(|index| index + 1 < count),
            count > 0,
            self.has_disconnected_tabs(),
        )
    }

    /// N7：标签右键"移动到新窗口 / 移动到主窗口"的启用态。
    ///
    /// 返回 `(move_to_new_window, move_to_main_window)`：已经有主窗口之外的窗口时
    /// 才能"移到主窗口"；没有窗口登记（单窗口/测试）时两项都不可用。
    pub(crate) fn tab_menu_move_flags(&self) -> (bool, bool) {
        let Some(tab_id) = self.tab_menu_tab_id.as_deref() else {
            return (false, false);
        };
        if self.tab_index(tab_id).is_none() {
            return (false, false);
        }
        let Some(current) = self.window_of_tab(tab_id) else {
            return (false, false);
        };
        let Some(main_window) = self.main_window_id() else {
            return (false, false);
        };
        (true, current != main_window)
    }

    /// N6：终端日志菜单/状态栏入口的启用旗标（针对活动会话）。
    ///
    /// 返回 `(start, stop, open_file, open_folder)`：
    /// * `start`：有活动会话（弹窗会按当前状态显示开启表单或停止提示）。
    /// * `stop`：活动会话正在写日志（手动或自动）。
    /// * `open_file` / `open_folder`：最近一次日志文件存在 / 其目录存在。
    ///
    /// 在生成投影时调用（`projection()` → `terminal_logging_menu_flags`）。
    pub(crate) fn terminal_logging_menu_flags(&self) -> (bool, bool, bool, bool) {
        let session_key = self.active_session_id.as_deref();
        let logging_active = session_key.is_some_and(|key| self.session_logging_active(key));
        let path_text = session_key
            .map(|key| self.session_logging_path_text(key))
            .unwrap_or_default();
        let path = Path::new(&path_text);
        let open_file = !path_text.is_empty() && path.is_file();
        let open_folder =
            !path_text.is_empty() && path.parent().is_some_and(|directory| directory.is_dir());
        (
            session_key.is_some(),
            logging_active,
            open_file,
            open_folder,
        )
    }

    /// N9：终端右键"同步发送按键"菜单的启用旗标（针对活动终端）。
    ///
    /// 返回 `(send_all, send_visible, stop)`：前两项要求活动标签是已连接终端；
    /// `stop` 只在同步进行中可用（`Send to Current` 语义 = 取消同步）。
    pub(crate) fn input_sync_menu_flags(&self) -> (bool, bool, bool) {
        let source_ready = self
            .active_tab_id
            .as_deref()
            .is_some_and(|tab_id| self.input_sync_tab_connected(tab_id));
        (source_ready, source_ready, self.input_sync.is_active())
    }

    /// N9：标签右键"接收键输入"的 `(enabled, checked, disabled_reason)`。
    ///
    /// `disabled_reason` 取值：`""`（可用）/ `no-sync` / `source` / `not-connected` /
    /// `no-session`，由 Slint 侧 `TextFormats` 渲染成提示句。
    pub(crate) fn tab_menu_receive_key_input_flags(&self) -> (bool, bool, String) {
        let Some(tab_id) = self.tab_menu_tab_id.as_deref() else {
            return (false, false, "no-session".to_owned());
        };
        let Some(index) = self.tab_index(tab_id) else {
            return (false, false, "no-session".to_owned());
        };
        if !self.input_sync.is_active() {
            return (false, false, "no-sync".to_owned());
        }
        if self.input_sync.source.as_deref() == Some(tab_id) {
            return (false, false, "source".to_owned());
        }
        if self.tabs[index].session_id().is_none() {
            return (false, false, "no-session".to_owned());
        }
        if !self.input_sync_tab_connected(tab_id) {
            return (false, false, "not-connected".to_owned());
        }
        (
            true,
            self.input_sync.targets.contains(tab_id),
            String::new(),
        )
    }
}
