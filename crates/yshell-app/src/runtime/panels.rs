//! Panel visibility/layout state and the settings terminal (scrollback) form.
//!
//! N3 Phase 1 adds the docked-panel layout model ([`PanelLayoutModel`]), the
//! breakpoint helpers used to replace the `window.width`-driven Slint bindings
//! with Rust-computed values, and the `AppRuntime` read/write path for C0's
//! `UiProfile.layout`. Unit tests live at the bottom of this file.

#![allow(dead_code)] // N3 Phase 1：布局模型/断点纯函数在 Phase 2 投影接线前仅由单测消费。

use crate::{
    error::AppError, error::AppResult, session_runtime::SessionRuntime,
    session_runtime::SessionSource,
};
use std::collections::BTreeSet;
use yshell_config::{LayoutProfile, PanelId, PanelSide, PanelSlot, TerminalProfile};
use yshell_terminal::{DEFAULT_SCROLLBACK_LINES, DEFAULT_SCROLLBACK_MAX_CELLS};

use super::*;

impl AppRuntime {
    pub fn update_settings_scrollback_lines(&mut self, value: &str) -> AppProjection {
        self.settings_scrollback_lines_text = value.to_owned();
        self.projection()
    }

    pub fn update_settings_scrollback_max_cells(&mut self, value: &str) -> AppProjection {
        self.settings_scrollback_max_cells_text = value.to_owned();
        self.projection()
    }

    pub fn reset_settings_terminal_defaults(&mut self) -> AppProjection {
        self.settings_scrollback_lines_text = DEFAULT_SCROLLBACK_LINES.to_string();
        self.settings_scrollback_max_cells_text = DEFAULT_SCROLLBACK_MAX_CELLS.to_string();
        self.settings_terminal_status = SettingsTerminalStatus::DefaultsLoaded;
        self.projection()
    }

    pub fn save_settings_terminal(&mut self) -> AppResult<AppProjection> {
        let scrollback_lines = match parse_settings_usize(
            SettingsTerminalField::ScrollbackLines,
            &self.settings_scrollback_lines_text,
            SETTINGS_SCROLLBACK_LINES_MIN,
            SETTINGS_SCROLLBACK_LINES_MAX,
        ) {
            Ok(value) => value,
            Err(status) => {
                self.settings_terminal_status = status;
                return Ok(self.projection());
            }
        };
        let scrollback_max_cells = match parse_settings_usize(
            SettingsTerminalField::ScrollbackMaxCells,
            &self.settings_scrollback_max_cells_text,
            SETTINGS_SCROLLBACK_MAX_CELLS_MIN,
            SETTINGS_SCROLLBACK_MAX_CELLS_MAX,
        ) {
            Ok(value) => value,
            Err(status) => {
                self.settings_terminal_status = status;
                return Ok(self.projection());
            }
        };

        let terminal = TerminalProfile {
            scrollback_lines,
            scrollback_max_cells,
            ..TerminalProfile::default()
        };
        self.config_document.terminal = terminal.clone();
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        for runtime in self.sessions.values_mut() {
            runtime.configure_terminal_limits(&terminal);
        }
        self.settings_scrollback_lines_text = scrollback_lines.to_string();
        self.settings_scrollback_max_cells_text = scrollback_max_cells.to_string();
        self.settings_terminal_status = SettingsTerminalStatus::Saved {
            lines: scrollback_lines,
            max_cells: scrollback_max_cells,
        };
        self.status_text = self.settings_terminal_status.legacy_text();
        Ok(self.projection())
    }

    pub fn toggle_sftp(&mut self) -> AppProjection {
        self.sftp_visible = !self.sftp_visible;
        self.status_text = if self.sftp_visible {
            "SFTP panel shown".to_owned()
        } else {
            "SFTP panel hidden".to_owned()
        };
        self.projection()
    }

    pub fn toggle_tunnels(&mut self) -> AppProjection {
        self.tunnels_visible = !self.tunnels_visible;
        self.status_text = if self.tunnels_visible {
            "Tunnels panel shown".to_owned()
        } else {
            "Tunnels panel hidden".to_owned()
        };
        self.projection()
    }

    pub fn toggle_commands(&mut self) -> AppProjection {
        self.commands_visible = !self.commands_visible;
        self.status_text = if self.commands_visible {
            "Quick Commands panel shown".to_owned()
        } else {
            "Quick Commands panel hidden".to_owned()
        };
        self.projection()
    }

    pub(crate) fn tunnels_summary_parts(&self) -> (&'static str, String) {
        match self
            .active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
        {
            Some(session) => {
                if session.ssh_config.tunnels.is_empty() {
                    ("none", String::new())
                } else {
                    let rows = session
                        .ssh_config
                        .tunnels
                        .iter()
                        .take(4)
                        .map(tunnel_config_summary)
                        .collect::<Vec<_>>()
                        .join("\n");
                    ("rows", rows)
                }
            }
            None => ("no-active", String::new()),
        }
    }

    pub(crate) fn resolved_terminal_profile_for_runtime(
        &self,
        runtime: &SessionRuntime,
    ) -> TerminalProfile {
        match &runtime.source {
            SessionSource::SavedSession { profile_id } => self
                .config_document
                .resolve_session(profile_id)
                .map(|resolved| resolved.terminal)
                .unwrap_or_else(|| self.config_document.terminal.clone()),
            SessionSource::QuickConnect | SessionSource::Draft => {
                self.config_document.terminal.clone()
            }
        }
    }

    pub(crate) fn configure_runtime_terminal_limits(&self, runtime: &mut SessionRuntime) {
        let terminal = self.resolved_terminal_profile_for_runtime(runtime);
        runtime.configure_terminal_limits(&terminal);
    }
}

/// Settings-dialog field ids for terminal validation messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsTerminalField {
    ScrollbackLines,
    ScrollbackMaxCells,
}

impl SettingsTerminalField {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::ScrollbackLines => "scrollback-lines",
            Self::ScrollbackMaxCells => "scrollback-max-cells",
        }
    }

    pub(crate) const fn legacy_label(self) -> &'static str {
        match self {
            Self::ScrollbackLines => "Scrollback lines",
            Self::ScrollbackMaxCells => "Scrollback memory cap",
        }
    }
}

/// Value-only status shown under the terminal settings form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsTerminalStatus {
    Hint,
    DefaultsLoaded,
    Empty {
        field: SettingsTerminalField,
    },
    NotNumber {
        field: SettingsTerminalField,
    },
    OutOfRange {
        field: SettingsTerminalField,
        minimum: usize,
        maximum: usize,
    },
    Saved {
        lines: usize,
        max_cells: usize,
    },
}

impl SettingsTerminalStatus {
    pub(crate) const fn kind_id(&self) -> &'static str {
        match self {
            Self::Hint => "hint",
            Self::DefaultsLoaded => "defaults-loaded",
            Self::Empty { .. } => "empty",
            Self::NotNumber { .. } => "not-number",
            Self::OutOfRange { .. } => "out-of-range",
            Self::Saved { .. } => "saved",
        }
    }

    pub(crate) fn field_id(&self) -> &'static str {
        match self {
            Self::Empty { field } | Self::NotNumber { field } | Self::OutOfRange { field, .. } => {
                field.id()
            }
            Self::Hint | Self::DefaultsLoaded | Self::Saved { .. } => "",
        }
    }

    pub(crate) fn value_text(&self) -> String {
        match self {
            Self::OutOfRange { minimum, .. } => minimum.to_string(),
            Self::Saved { lines, .. } => lines.to_string(),
            Self::Hint | Self::DefaultsLoaded | Self::Empty { .. } | Self::NotNumber { .. } => {
                String::new()
            }
        }
    }

    pub(crate) fn limit_text(&self) -> String {
        match self {
            Self::OutOfRange { maximum, .. } => maximum.to_string(),
            Self::Saved { max_cells, .. } => max_cells.to_string(),
            Self::Hint | Self::DefaultsLoaded | Self::Empty { .. } | Self::NotNumber { .. } => {
                String::new()
            }
        }
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    pub(crate) fn legacy_text(&self) -> String {
        match self {
            Self::Hint => {
                "Save terminal settings to apply the scrollback limits to existing sessions."
                    .to_owned()
            }
            Self::DefaultsLoaded => "Defaults loaded. Click Save to apply.".to_owned(),
            Self::Empty { field } => format!("{} must not be empty.", field.legacy_label()),
            Self::NotNumber { field } => {
                format!("{} must be a whole number.", field.legacy_label())
            }
            Self::OutOfRange {
                field,
                minimum,
                maximum,
            } => format!(
                "{} must be between {minimum} and {maximum}.",
                field.legacy_label()
            ),
            Self::Saved { lines, max_cells } => {
                format!("Saved terminal settings: scrollback={lines} lines, cap={max_cells} cells.")
            }
        }
    }
}

pub(crate) const SETTINGS_SCROLLBACK_LINES_MIN: usize = 100;

pub(crate) const SETTINGS_SCROLLBACK_LINES_MAX: usize = 1_000_000;

pub(crate) const SETTINGS_SCROLLBACK_MAX_CELLS_MIN: usize = 100_000;

pub(crate) const SETTINGS_SCROLLBACK_MAX_CELLS_MAX: usize = 100_000_000;

/// Validates a terminal settings text field against an inclusive range.
pub(crate) fn parse_settings_usize(
    field: SettingsTerminalField,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<usize, SettingsTerminalStatus> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SettingsTerminalStatus::Empty { field });
    }
    let parsed = trimmed
        .parse::<usize>()
        .map_err(|_| SettingsTerminalStatus::NotNumber { field })?;
    if parsed < minimum || parsed > maximum {
        return Err(SettingsTerminalStatus::OutOfRange {
            field,
            minimum,
            maximum,
        });
    }
    Ok(parsed)
}

// --- N3 Phase 1：面板布局模型与断点纯函数 -----------------------------------
//
// 设计：docs/product/yshell-next-n3-panels.md §1/§2。本段做三件事：
// 1. 把 C0 的 `UiProfile.layout` 读成可操作的 [`PanelLayoutModel`]，变更加
//    清洗后写回配置（全局一套，所有窗口共享）；
// 2. 把窗口宽度断点判定收敛为纯函数（`nav_rail_active`/`dock_toggle_visible`/
//    `dock_auto_collapsed` 等）：Phase 2 由 Rust 投影下发，消除 `main_window`
//    里 `window.width` ↔ 布局属性往返绑定造成的 binding-loop 告警；
// 3. `AppRuntime` 的读写入口（Phase 2 由 bootstrap 回调消费）。

/// 全宽布局断点：≥1280px 两栏按记忆宽度显示。
pub(crate) const BREAKPOINT_FULL_LAYOUT: f32 = 1280.0;
/// 窄窗自动折叠断点（§4.2：<1120px）。
pub(crate) const BREAKPOINT_DOCK_COLLAPSE: f32 = 1120.0;
/// 左栏图标条断点（§4.2：<960px）。
pub(crate) const BREAKPOINT_NAV_RAIL: f32 = 960.0;
/// 图标条宽度（`Theme.nav-rail-width` 的 Rust 侧镜像）。
pub(crate) const NAV_RAIL_WIDTH: f32 = 48.0;
/// 左栏宽度范围（§4.3：左 200–420）。
pub(crate) const LEFT_WIDTH_MIN: u32 = 200;
pub(crate) const LEFT_WIDTH_MAX: u32 = 420;
/// 右栏宽度范围（§4.3：右 280–560）。
pub(crate) const RIGHT_WIDTH_MIN: u32 = 280;
pub(crate) const RIGHT_WIDTH_MAX: u32 = 560;
/// 默认栏宽（与 C0 `LayoutProfile` 的默认值一致）。
pub(crate) const DEFAULT_LEFT_WIDTH: u32 = 240;
pub(crate) const DEFAULT_RIGHT_WIDTH: u32 = 340;
/// 分栏边界比例范围与相邻边界最小间距（与 UI 的每面板最小高度配合）。
pub(crate) const SPLIT_RATIO_MIN: f32 = 0.05;
pub(crate) const SPLIT_RATIO_MAX: f32 = 0.95;
pub(crate) const SPLIT_RATIO_GAP_MIN: f32 = 0.05;

/// 面板的落位：左栏栈 / 右栏栈 / 隐藏（两侧都没有）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelPlacement {
    Left,
    Right,
    Hidden,
}

impl PanelPlacement {
    /// 对应到 [`PanelSide`]（`Hidden` 为 `None`）。
    pub(crate) const fn side(self) -> Option<PanelSide> {
        match self {
            Self::Left => Some(PanelSide::Left),
            Self::Right => Some(PanelSide::Right),
            Self::Hidden => None,
        }
    }

    /// 稳定的投影 id（供 Slint 侧枚举/字符串使用）。
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Hidden => "hidden",
        }
    }
}

impl From<PanelSide> for PanelPlacement {
    fn from(side: PanelSide) -> Self {
        match side {
            PanelSide::Left => Self::Left,
            PanelSide::Right => Self::Right,
        }
    }
}

/// 未配置时的默认落位：Sessions 在左，其余面板在右。
pub(crate) const fn default_side(panel: PanelId) -> PanelSide {
    match panel {
        PanelId::Sessions => PanelSide::Left,
        PanelId::Sftp | PanelId::Tunnels | PanelId::QuickCommands | PanelId::Transfers => {
            PanelSide::Right
        }
    }
}

// ---------------------------------------------------------------- 断点纯函数
// 以下函数是 `ui/main_window.slint`（W5-④）同名属性的逐条镜像；Phase 2 把这些
// 值作为普通属性下发后，Slint 侧不再从 `window.width` 反推布局。

/// <960px 且用户未手动展开时，左栏折叠为 48px 图标条。
pub(crate) fn nav_rail_active(window_width: f32, nav_user_expanded: bool) -> bool {
    window_width < BREAKPOINT_NAV_RAIL && !nav_user_expanded
}

/// <960px：提供"图标条 ↔ 完整左栏"入口。
pub(crate) fn nav_rail_available(window_width: f32) -> bool {
    window_width < BREAKPOINT_NAV_RAIL
}

/// 全宽左导航列是否可见（图标条模式下由图标条取代）。
pub(crate) fn nav_column_visible(
    session_manager_visible: bool,
    window_width: f32,
    nav_user_expanded: bool,
) -> bool {
    session_manager_visible && !nav_rail_active(window_width, nav_user_expanded)
}

/// 左栏图标条是否可见。
pub(crate) fn nav_rail_visible(
    session_manager_visible: bool,
    window_width: f32,
    nav_user_expanded: bool,
) -> bool {
    session_manager_visible && nav_rail_active(window_width, nav_user_expanded)
}

/// <1120px：命令栏显示栏展开/收起按钮。
pub(crate) fn dock_toggle_visible(window_width: f32) -> bool {
    window_width < BREAKPOINT_DOCK_COLLAPSE
}

/// 栏因窗口过窄而自动折叠（用户手动展开可覆盖，仅内存态）。
pub(crate) fn dock_auto_collapsed(window_width: f32, dock_user_expanded: bool) -> bool {
    window_width < BREAKPOINT_DOCK_COLLAPSE && !dock_user_expanded
}

/// 1120–1280px 档的线性收缩系数（≤1120 为 0，≥1280 为 1）。
pub(crate) fn layout_squeeze_ratio(window_width: f32) -> f32 {
    ((window_width - BREAKPOINT_DOCK_COLLAPSE)
        / (BREAKPOINT_FULL_LAYOUT - BREAKPOINT_DOCK_COLLAPSE))
        .clamp(0.0, 1.0)
}

/// 拖拽换算系数：收缩档内指针 1px 对应目标宽度 `1/ratio` px，保证把手跟手。
pub(crate) fn layout_drag_scale(window_width: f32) -> f32 {
    if window_width >= BREAKPOINT_FULL_LAYOUT || window_width < BREAKPOINT_DOCK_COLLAPSE {
        1.0
    } else {
        layout_squeeze_ratio(window_width).max(0.0001)
    }
}

/// 左栏最终宽度：图标条 48px；否则全宽/收缩档换算（与 Slint 侧公式一致）。
pub(crate) fn nav_effective_width(
    window_width: f32,
    nav_panel_width: f32,
    nav_user_expanded: bool,
) -> f32 {
    if nav_rail_active(window_width, nav_user_expanded) {
        return NAV_RAIL_WIDTH;
    }
    squeezed_side_width(window_width, nav_panel_width, LEFT_WIDTH_MIN as f32)
}

/// 右栏最终宽度：0 = 自动折叠；否则全宽/收缩档换算（与 Slint 侧公式一致）。
pub(crate) fn dock_effective_width(
    window_width: f32,
    dock_panel_width: f32,
    dock_user_expanded: bool,
) -> f32 {
    if dock_auto_collapsed(window_width, dock_user_expanded) {
        return 0.0;
    }
    squeezed_side_width(window_width, dock_panel_width, RIGHT_WIDTH_MIN as f32)
}

fn squeezed_side_width(window_width: f32, panel_width: f32, width_min: f32) -> f32 {
    if window_width >= BREAKPOINT_FULL_LAYOUT || window_width < BREAKPOINT_DOCK_COLLAPSE {
        panel_width
    } else {
        width_min + (panel_width - width_min) * layout_squeeze_ratio(window_width)
    }
}

/// 窄窗下自动折叠的一侧：<1120px 生效，默认折叠右栏，否则用持久化的记忆值。
pub(crate) fn auto_collapsed_side(
    window_width: f32,
    persisted: Option<PanelSide>,
) -> Option<PanelSide> {
    if window_width < BREAKPOINT_DOCK_COLLAPSE {
        Some(persisted.unwrap_or(PanelSide::Right))
    } else {
        None
    }
}

/// 栏宽边界（左 200–420 / 右 280–560）。
pub(crate) const fn side_width_bounds(side: PanelSide) -> (u32, u32) {
    match side {
        PanelSide::Left => (LEFT_WIDTH_MIN, LEFT_WIDTH_MAX),
        PanelSide::Right => (RIGHT_WIDTH_MIN, RIGHT_WIDTH_MAX),
    }
}

/// 把拖拽/配置里的栏宽夹取到对应侧的范围（非有限值回退默认宽度）。
pub(crate) fn clamp_side_width(side: PanelSide, width: f32) -> u32 {
    let (minimum, maximum) = side_width_bounds(side);
    let default = match side {
        PanelSide::Left => DEFAULT_LEFT_WIDTH,
        PanelSide::Right => DEFAULT_RIGHT_WIDTH,
    };
    let rounded = if width.is_finite() {
        width.round()
    } else {
        default as f32
    };
    rounded.clamp(minimum as f32, maximum as f32) as u32
}

/// 均分边界：`count` 个边界把栈分成 `count + 1` 份。
fn even_fractions(count: usize) -> Vec<f32> {
    (1..=count)
        .map(|index| index as f32 / (count + 1) as f32)
        .collect()
}

/// 解析存储的分栏比例：长度/取值不可用时返回 `None`（调用方决定回退均分或清空）。
fn resolve_fractions(stored: &[f32], count: usize) -> Option<Vec<f32>> {
    if count == 0 {
        return Some(Vec::new());
    }
    if stored.len() != count {
        return None;
    }
    let mut resolved = Vec::with_capacity(count);
    let mut previous = 0.0_f32;
    for (index, raw) in stored.iter().enumerate() {
        if !raw.is_finite() {
            return None;
        }
        let tail = (count - index - 1) as f32;
        let lower = if index == 0 {
            SPLIT_RATIO_MIN
        } else {
            previous + SPLIT_RATIO_GAP_MIN
        };
        let upper = SPLIT_RATIO_MAX - tail * SPLIT_RATIO_GAP_MIN;
        if lower > upper {
            return None;
        }
        let value = raw.clamp(lower, upper);
        resolved.push(value);
        previous = value;
    }
    Some(resolved)
}

/// 面板布局模型：C0 `UiProfile.layout` 的内存镜像 + 变更操作。
///
/// 不变式（构造时清洗）：
/// * 同一栈内不会出现重复面板，两侧不会同时拥有同一面板（保留先出现的那个）；
/// * 比例列表为空 = 均分；非空时长度 = 栈长 - 1，取值递增且留最小间距；
/// * 栏宽按侧夹取到 200–420 / 280–560。
///
/// 栈内改序保留比例（分栏几何不变、面板互换）；增删成员清空该侧比例（回到均分）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PanelLayoutModel {
    layout: LayoutProfile,
}

impl PanelLayoutModel {
    /// 从配置读取并清洗（不修改入参）。
    pub(crate) fn from_layout(layout: &LayoutProfile) -> Self {
        Self {
            layout: sanitize_layout(layout),
        }
    }

    /// 清洗后的布局（只读）。
    pub(crate) fn layout(&self) -> &LayoutProfile {
        &self.layout
    }

    /// 取出布局（写回配置用）。
    pub(crate) fn into_layout(self) -> LayoutProfile {
        self.layout
    }

    /// 面板当前落位。
    pub(crate) fn placement(&self, panel: PanelId) -> PanelPlacement {
        if self.layout.left.iter().any(|slot| slot.panel == panel) {
            PanelPlacement::Left
        } else if self.layout.right.iter().any(|slot| slot.panel == panel) {
            PanelPlacement::Right
        } else {
            PanelPlacement::Hidden
        }
    }

    /// 折叠状态（隐藏面板视为未折叠）。
    pub(crate) fn collapsed(&self, panel: PanelId) -> bool {
        self.stack(PanelSide::Left)
            .iter()
            .chain(self.stack(PanelSide::Right))
            .find(|slot| slot.panel == panel)
            .is_some_and(|slot| slot.collapsed)
    }

    /// 一侧的面板栈（顺序 = 显示顺序）。
    pub(crate) fn stack(&self, side: PanelSide) -> &[PanelSlot] {
        match side {
            PanelSide::Left => &self.layout.left,
            PanelSide::Right => &self.layout.right,
        }
    }

    /// 一侧的面板 id 序列（投影/测试用）。
    pub(crate) fn panels(&self, side: PanelSide) -> Vec<PanelId> {
        self.stack(side).iter().map(|slot| slot.panel).collect()
    }

    /// 一侧记忆的栏宽。
    pub(crate) fn width(&self, side: PanelSide) -> u32 {
        match side {
            PanelSide::Left => self.layout.left_width,
            PanelSide::Right => self.layout.right_width,
        }
    }

    /// 设置栏宽（按侧夹取）。
    pub(crate) fn set_width(&mut self, side: PanelSide, width: f32) {
        let clamped = clamp_side_width(side, width);
        match side {
            PanelSide::Left => self.layout.left_width = clamped,
            PanelSide::Right => self.layout.right_width = clamped,
        }
    }

    /// 设置折叠状态；面板不存在或状态未变时返回 `false`。
    pub(crate) fn set_collapsed(&mut self, panel: PanelId, collapsed: bool) -> bool {
        for slots in [&mut self.layout.left, &mut self.layout.right] {
            if let Some(slot) = slots.iter_mut().find(|slot| slot.panel == panel) {
                if slot.collapsed == collapsed {
                    return false;
                }
                slot.collapsed = collapsed;
                return true;
            }
        }
        false
    }

    /// 折叠 ↔ 展开，返回切换后的状态（隐藏面板返回 `false` 且不变化）。
    pub(crate) fn toggle_collapsed(&mut self, panel: PanelId) -> bool {
        if self.placement(panel) == PanelPlacement::Hidden {
            return false;
        }
        let collapsed = !self.collapsed(panel);
        self.set_collapsed(panel, collapsed);
        collapsed
    }

    /// 隐藏面板（从两侧栈移除）；返回是否发生变化。
    pub(crate) fn hide(&mut self, panel: PanelId) -> bool {
        let before = self.layout.clone();
        let left_len = self.layout.left.len();
        let right_len = self.layout.right.len();
        self.layout.left.retain(|slot| slot.panel != panel);
        self.layout.right.retain(|slot| slot.panel != panel);
        self.clear_ratios_if_resized(PanelSide::Left, left_len);
        self.clear_ratios_if_resized(PanelSide::Right, right_len);
        self.layout != before
    }

    /// 把面板放到指定一侧的指定位置（`None` = 追加到栈尾）。
    ///
    /// 隐藏面板由本操作显示；同侧同位置重复调用是无副作用的 no-op。
    pub(crate) fn move_to(
        &mut self,
        panel: PanelId,
        side: PanelSide,
        index: Option<usize>,
    ) -> bool {
        let before = self.layout.clone();
        let collapsed = self.collapsed(panel);
        let left_len = self.layout.left.len();
        let right_len = self.layout.right.len();
        let mut left = std::mem::take(&mut self.layout.left);
        let mut right = std::mem::take(&mut self.layout.right);
        left.retain(|slot| slot.panel != panel);
        right.retain(|slot| slot.panel != panel);
        let target = match side {
            PanelSide::Left => &mut left,
            PanelSide::Right => &mut right,
        };
        let position = index.map_or(target.len(), |value| value.min(target.len()));
        target.insert(position, PanelSlot { panel, collapsed });
        self.layout.left = left;
        self.layout.right = right;
        self.clear_ratios_if_resized(PanelSide::Left, left_len);
        self.clear_ratios_if_resized(PanelSide::Right, right_len);
        self.layout != before
    }

    /// 一侧的分栏边界数量（= 栈长 - 1）。
    pub(crate) fn boundary_count(&self, side: PanelSide) -> usize {
        self.stack(side).len().saturating_sub(1)
    }

    /// 一侧的边界比例（始终可用：空/非法时回退均分）。
    pub(crate) fn boundary_fractions(&self, side: PanelSide) -> Vec<f32> {
        let count = self.boundary_count(side);
        let stored = match side {
            PanelSide::Left => &self.layout.left_ratios,
            PanelSide::Right => &self.layout.right_ratios,
        };
        resolve_fractions(stored, count).unwrap_or_else(|| even_fractions(count))
    }

    /// 拖动某个分栏边界（0-based，从上往下）；返回是否发生变化。
    ///
    /// 比例被夹取在相邻边界之间（含最小间距）；首次拖动会把均分解析为显式比例。
    pub(crate) fn set_boundary_fraction(
        &mut self,
        side: PanelSide,
        boundary: usize,
        fraction: f32,
    ) -> bool {
        let count = self.boundary_count(side);
        if boundary >= count {
            return false;
        }
        let mut fractions = self.boundary_fractions(side);
        let lower = if boundary == 0 {
            SPLIT_RATIO_MIN
        } else {
            fractions[boundary - 1] + SPLIT_RATIO_GAP_MIN
        };
        let upper = if boundary + 1 == count {
            SPLIT_RATIO_MAX
        } else {
            fractions[boundary + 1] - SPLIT_RATIO_GAP_MIN
        };
        let value = if fraction.is_finite() {
            fraction.clamp(lower, upper.max(lower))
        } else {
            fractions[boundary]
        };
        let changed = (fractions[boundary] - value).abs() > f32::EPSILON;
        fractions[boundary] = value;
        match side {
            PanelSide::Left => self.layout.left_ratios = fractions,
            PanelSide::Right => self.layout.right_ratios = fractions,
        }
        changed
    }

    /// 窄窗自动折叠的持久化记忆侧。
    pub(crate) fn auto_collapsed_side(&self) -> Option<PanelSide> {
        self.layout.narrow_collapsed_side
    }

    /// 记忆窄窗自动折叠的一侧（`None` = 恢复默认：右栏）。
    pub(crate) fn set_auto_collapsed_side(&mut self, side: Option<PanelSide>) -> bool {
        if self.layout.narrow_collapsed_side == side {
            return false;
        }
        self.layout.narrow_collapsed_side = side;
        true
    }

    fn clear_ratios_if_resized(&mut self, side: PanelSide, previous_len: usize) {
        let resized = self.stack(side).len() != previous_len;
        if resized {
            match side {
                PanelSide::Left => self.layout.left_ratios.clear(),
                PanelSide::Right => self.layout.right_ratios.clear(),
            }
        }
    }
}

/// 读取清洗：去重（保留先出现者）、夹取栏宽、丢弃与栈长不符或不可用的比例。
fn sanitize_layout(layout: &LayoutProfile) -> LayoutProfile {
    let mut seen = BTreeSet::new();
    let mut left = Vec::new();
    let mut right = Vec::new();
    for slot in &layout.left {
        if seen.insert(slot.panel) {
            left.push(*slot);
        }
    }
    for slot in &layout.right {
        if seen.insert(slot.panel) {
            right.push(*slot);
        }
    }
    let left_count = left.len().saturating_sub(1);
    let right_count = right.len().saturating_sub(1);
    LayoutProfile {
        left_ratios: resolve_fractions(&layout.left_ratios, left_count).unwrap_or_default(),
        right_ratios: resolve_fractions(&layout.right_ratios, right_count).unwrap_or_default(),
        left_width: clamp_side_width(PanelSide::Left, layout.left_width as f32),
        right_width: clamp_side_width(PanelSide::Right, layout.right_width as f32),
        left,
        right,
        narrow_collapsed_side: layout.narrow_collapsed_side,
    }
}

// --------------------------------------------------- AppRuntime 读写入口
// Phase 2 由 bootstrap 的回调调用；配置写入失败向上抛 AppError。

impl AppRuntime {
    /// 读取全局面板布局（`[ui.layout]`）。
    pub fn panel_layout(&self) -> PanelLayoutModel {
        PanelLayoutModel::from_layout(&self.config_document.ui.layout)
    }

    /// 布局有变化时落盘并返回新投影（无变化不写盘）。
    fn persist_panel_layout(&mut self, model: PanelLayoutModel) -> AppResult<AppProjection> {
        let layout = model.into_layout();
        if layout != self.config_document.ui.layout {
            self.config_document.ui.layout = layout;
            self.config_store
                .save(&self.config_document)
                .map_err(AppError::from_error)?;
        }
        Ok(self.projection())
    }

    /// 面板菜单/拖拽：移动到指定一侧的指定位置（`None` = 栈尾）。
    pub fn move_panel(
        &mut self,
        panel: PanelId,
        side: PanelSide,
        index: Option<usize>,
    ) -> AppResult<AppProjection> {
        let mut model = self.panel_layout();
        model.move_to(panel, side, index);
        self.persist_panel_layout(model)
    }

    /// 面板菜单 Hide：从两侧栈移除。
    pub fn hide_panel(&mut self, panel: PanelId) -> AppResult<AppProjection> {
        let mut model = self.panel_layout();
        model.hide(panel);
        self.persist_panel_layout(model)
    }

    /// 面板菜单 Collapse/Expand。
    pub fn toggle_panel_collapsed(&mut self, panel: PanelId) -> AppResult<AppProjection> {
        let mut model = self.panel_layout();
        model.toggle_collapsed(panel);
        self.persist_panel_layout(model)
    }

    /// 栏宽拖拽结束：按侧夹取后落盘。
    pub fn set_panel_side_width(
        &mut self,
        side: PanelSide,
        width: f32,
    ) -> AppResult<AppProjection> {
        let mut model = self.panel_layout();
        model.set_width(side, width);
        self.persist_panel_layout(model)
    }

    /// 分栏拖拽：设置某侧的某个边界比例。
    pub fn set_panel_split_fraction(
        &mut self,
        side: PanelSide,
        boundary: usize,
        fraction: f32,
    ) -> AppResult<AppProjection> {
        let mut model = self.panel_layout();
        model.set_boundary_fraction(side, boundary, fraction);
        self.persist_panel_layout(model)
    }

    /// 记忆窄窗自动折叠的一侧（`narrow_collapsed_side`）。
    pub fn set_panel_auto_collapsed_side(
        &mut self,
        side: Option<PanelSide>,
    ) -> AppResult<AppProjection> {
        let mut model = self.panel_layout();
        model.set_auto_collapsed_side(side);
        self.persist_panel_layout(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::AppRuntime;
    use yshell_config::{ConfigDocument, LayoutProfile, PanelId, PanelSide, PanelSlot, UiProfile};

    fn slot(panel: PanelId, collapsed: bool) -> PanelSlot {
        PanelSlot { panel, collapsed }
    }

    fn sample_layout() -> LayoutProfile {
        LayoutProfile {
            left: vec![
                slot(PanelId::Sessions, false),
                slot(PanelId::Tunnels, false),
            ],
            right: vec![
                slot(PanelId::Sftp, false),
                slot(PanelId::QuickCommands, true),
            ],
            left_ratios: vec![0.4],
            right_ratios: Vec::new(),
            left_width: 260,
            right_width: 380,
            narrow_collapsed_side: Some(PanelSide::Right),
        }
    }

    #[test]
    fn breakpoint_boundaries_match_the_slint_formulas() {
        // 960 / 1120 / 1280 三个断点的两侧取值。
        assert!(nav_rail_active(959.0, false));
        assert!(!nav_rail_active(960.0, false));
        assert!(!nav_rail_active(400.0, true));
        assert!(nav_rail_available(959.0));
        assert!(!nav_rail_available(960.0));

        assert!(dock_toggle_visible(1119.0));
        assert!(!dock_toggle_visible(1120.0));
        assert!(dock_auto_collapsed(1119.0, false));
        assert!(!dock_auto_collapsed(1119.0, true));
        assert!(!dock_auto_collapsed(1120.0, false));

        assert_eq!(layout_squeeze_ratio(1119.0), 0.0);
        assert_eq!(layout_squeeze_ratio(1120.0), 0.0);
        assert_eq!(layout_squeeze_ratio(1200.0), 0.5);
        assert_eq!(layout_squeeze_ratio(1280.0), 1.0);
        assert_eq!(layout_squeeze_ratio(2000.0), 1.0);
        assert_eq!(layout_drag_scale(1000.0), 1.0);
        assert!(layout_drag_scale(1200.0) > 0.0);

        assert_eq!(nav_effective_width(940.0, 300.0, false), NAV_RAIL_WIDTH);
        assert_eq!(nav_effective_width(940.0, 300.0, true), 300.0);
        assert_eq!(nav_effective_width(1200.0, 300.0, false), 250.0);
        assert_eq!(nav_effective_width(1300.0, 300.0, false), 300.0);

        assert_eq!(dock_effective_width(1100.0, 340.0, false), 0.0);
        assert_eq!(dock_effective_width(1100.0, 340.0, true), 340.0);
        assert_eq!(dock_effective_width(1140.0, 340.0, false), 287.5);
        assert_eq!(dock_effective_width(1400.0, 340.0, false), 340.0);

        assert!(!nav_column_visible(true, 940.0, false));
        assert!(nav_column_visible(true, 940.0, true));
        assert!(nav_rail_visible(true, 940.0, false));
        assert!(!nav_rail_visible(false, 940.0, false));
    }

    #[test]
    fn side_widths_clamp_to_their_ranges() {
        assert_eq!(clamp_side_width(PanelSide::Left, 199.4), LEFT_WIDTH_MIN);
        assert_eq!(clamp_side_width(PanelSide::Left, 420.6), LEFT_WIDTH_MAX);
        assert_eq!(clamp_side_width(PanelSide::Right, 10.0), RIGHT_WIDTH_MIN);
        assert_eq!(clamp_side_width(PanelSide::Right, 700.0), RIGHT_WIDTH_MAX);
        assert_eq!(
            clamp_side_width(PanelSide::Left, f32::NAN),
            DEFAULT_LEFT_WIDTH
        );
        assert_eq!(
            clamp_side_width(PanelSide::Right, f32::INFINITY),
            DEFAULT_RIGHT_WIDTH
        );
        assert_eq!(side_width_bounds(PanelSide::Left), (200, 420));
        assert_eq!(side_width_bounds(PanelSide::Right), (280, 560));
    }

    #[test]
    fn layout_reading_dedupes_and_sanitizes() {
        let dirty = LayoutProfile {
            left: vec![slot(PanelId::Sftp, true), slot(PanelId::Sftp, false)],
            right: vec![
                slot(PanelId::Tunnels, false),
                slot(PanelId::Transfers, false),
                slot(PanelId::Tunnels, true),
            ],
            left_ratios: vec![0.9, 0.1],
            right_ratios: vec![0.99],
            left_width: 999,
            right_width: 10,
            narrow_collapsed_side: Some(PanelSide::Left),
        };
        let model = PanelLayoutModel::from_layout(&dirty);
        assert_eq!(model.panels(PanelSide::Left), vec![PanelId::Sftp]);
        assert_eq!(
            model.panels(PanelSide::Right),
            vec![PanelId::Tunnels, PanelId::Transfers]
        );
        // 左栈只剩 1 个面板（0 个边界）→ 比例被清空；右栈 1 个边界 → 夹取到上限。
        assert!(model.layout().left_ratios.is_empty());
        assert_eq!(model.layout().right_ratios, vec![SPLIT_RATIO_MAX]);
        assert_eq!(model.width(PanelSide::Left), LEFT_WIDTH_MAX);
        assert_eq!(model.width(PanelSide::Right), RIGHT_WIDTH_MIN);
        assert_eq!(model.auto_collapsed_side(), Some(PanelSide::Left));
        assert!(model.collapsed(PanelId::Sftp));
        assert!(!model.collapsed(PanelId::Tunnels));
        assert_eq!(model.placement(PanelId::Sftp), PanelPlacement::Left);
        assert_eq!(model.placement(PanelId::Tunnels), PanelPlacement::Right);
    }

    #[test]
    fn move_between_sides_clears_ratios_and_keeps_collapse() {
        let mut model = PanelLayoutModel::from_layout(&sample_layout());
        assert!(model.set_collapsed(PanelId::Sessions, true));
        assert!(model.move_to(PanelId::Sessions, PanelSide::Right, Some(0)));
        assert_eq!(
            model.panels(PanelSide::Right),
            vec![PanelId::Sessions, PanelId::Sftp, PanelId::QuickCommands]
        );
        assert_eq!(model.panels(PanelSide::Left), vec![PanelId::Tunnels]);
        assert!(model.collapsed(PanelId::Sessions));
        assert!(model.layout().right_ratios.is_empty());
        assert!(model.boundary_fractions(PanelSide::Left).is_empty());

        // 移回左栏栈尾并保留折叠态。
        assert!(model.move_to(PanelId::Sessions, PanelSide::Left, None));
        assert_eq!(model.placement(PanelId::Sessions), PanelPlacement::Left);
        assert!(model.collapsed(PanelId::Sessions));
        assert_eq!(
            model.panels(PanelSide::Left),
            vec![PanelId::Tunnels, PanelId::Sessions]
        );
    }

    #[test]
    fn reorder_within_side_keeps_split_ratios() {
        let mut model = PanelLayoutModel::from_layout(&sample_layout());
        let before = model.boundary_fractions(PanelSide::Left);
        assert!(model.move_to(PanelId::Sessions, PanelSide::Left, Some(1)));
        assert_eq!(
            model.panels(PanelSide::Left),
            vec![PanelId::Tunnels, PanelId::Sessions]
        );
        assert_eq!(model.layout().left_ratios, vec![0.4]);
        assert_eq!(model.boundary_fractions(PanelSide::Left), before);
        // 同位置重复移动：无变化。
        assert!(!model.move_to(PanelId::Sessions, PanelSide::Left, None));
    }

    #[test]
    fn hide_and_show_roundtrip() {
        let mut model = PanelLayoutModel::from_layout(&sample_layout());
        assert!(model.hide(PanelId::Sftp));
        assert_eq!(model.placement(PanelId::Sftp), PanelPlacement::Hidden);
        assert!(!model.collapsed(PanelId::Sftp));
        assert!(!model.hide(PanelId::Sftp));
        assert!(model.layout().right_ratios.is_empty());
        // 显式显示：追加到指定栈尾。
        assert!(model.move_to(PanelId::Sftp, PanelSide::Left, None));
        assert_eq!(
            model.panels(PanelSide::Left),
            vec![PanelId::Sessions, PanelId::Tunnels, PanelId::Sftp]
        );
        assert_eq!(model.placement(PanelId::Sftp), PanelPlacement::Left);
    }

    #[test]
    fn boundary_fractions_clamp_and_stay_monotonic() {
        let layout = LayoutProfile {
            left: vec![
                slot(PanelId::Sessions, false),
                slot(PanelId::Sftp, false),
                slot(PanelId::Tunnels, false),
            ],
            right: vec![slot(PanelId::Transfers, false)],
            left_ratios: vec![0.0, 1.0],
            ..LayoutProfile::default()
        };
        let mut model = PanelLayoutModel::from_layout(&layout);
        assert_eq!(model.boundary_count(PanelSide::Left), 2);
        assert_eq!(
            model.boundary_fractions(PanelSide::Left),
            vec![SPLIT_RATIO_MIN, SPLIT_RATIO_MAX]
        );
        // 向下拖第一个边界：被第二个边界（含间距）压住。
        assert!(model.set_boundary_fraction(PanelSide::Left, 0, 0.8));
        assert_eq!(model.layout().left_ratios, vec![0.8, SPLIT_RATIO_MAX]);
        assert!(model.set_boundary_fraction(PanelSide::Left, 1, 0.5));
        assert_eq!(
            model.layout().left_ratios,
            vec![0.8, 0.8 + SPLIT_RATIO_GAP_MIN]
        );
        assert!(!model.set_boundary_fraction(PanelSide::Left, 2, 0.5));
        // 单面板（0 个边界）无比例可设。
        assert_eq!(model.boundary_count(PanelSide::Right), 0);
        assert!(!model.set_boundary_fraction(PanelSide::Right, 0, -1.0));
        assert!(model.boundary_fractions(PanelSide::Right).is_empty());
        assert!(model.layout().right_ratios.is_empty());
    }

    #[test]
    fn boundary_fractions_fall_back_to_even() {
        let layout = LayoutProfile {
            left: vec![
                slot(PanelId::Sessions, false),
                slot(PanelId::Sftp, false),
                slot(PanelId::Tunnels, false),
            ],
            left_ratios: vec![0.4],
            ..LayoutProfile::default()
        };
        let model = PanelLayoutModel::from_layout(&layout);
        let even = model.boundary_fractions(PanelSide::Left);
        assert_eq!(even.len(), 2);
        assert!((even[0] - 1.0 / 3.0).abs() < f32::EPSILON);
        assert!((even[1] - 2.0 / 3.0).abs() < f32::EPSILON);
        assert!(model.layout().left_ratios.is_empty());

        let nan = LayoutProfile {
            left: vec![
                slot(PanelId::Sessions, false),
                slot(PanelId::Sftp, false),
                slot(PanelId::Tunnels, false),
            ],
            left_ratios: vec![f32::NAN, 0.6],
            ..LayoutProfile::default()
        };
        let model = PanelLayoutModel::from_layout(&nan);
        assert_eq!(model.boundary_fractions(PanelSide::Left), even);
        assert!(model.layout().left_ratios.is_empty());
    }

    #[test]
    fn widths_and_collapse_toggle_roundtrip() {
        let mut model = PanelLayoutModel::from_layout(&sample_layout());
        model.set_width(PanelSide::Right, 1000.0);
        assert_eq!(model.width(PanelSide::Right), RIGHT_WIDTH_MAX);
        model.set_width(PanelSide::Left, 250.0);
        assert_eq!(model.width(PanelSide::Left), 250);
        assert!(model.set_collapsed(PanelId::Sftp, true));
        assert!(!model.set_collapsed(PanelId::Sftp, true));
        assert!(!model.toggle_collapsed(PanelId::Sftp));
        assert!(!model.collapsed(PanelId::Sftp));
        assert!(model.toggle_collapsed(PanelId::Sftp));
        assert!(model.collapsed(PanelId::Sftp));
        // 隐藏面板不可折叠。
        model.hide(PanelId::QuickCommands);
        assert!(!model.toggle_collapsed(PanelId::QuickCommands));
        assert!(!model.collapsed(PanelId::QuickCommands));
    }

    #[test]
    fn every_panel_can_move_to_every_side() {
        let panels = [
            PanelId::Sessions,
            PanelId::Sftp,
            PanelId::Tunnels,
            PanelId::QuickCommands,
            PanelId::Transfers,
        ];
        for panel in panels {
            for side in [PanelSide::Left, PanelSide::Right] {
                let mut model = PanelLayoutModel::from_layout(&LayoutProfile::default());
                model.move_to(panel, side, None);
                assert_eq!(model.placement(panel), PanelPlacement::from(side));
                assert!(model.panels(side).contains(&panel));
            }
        }
        assert_eq!(default_side(PanelId::Sessions), PanelSide::Left);
        for panel in [
            PanelId::Sftp,
            PanelId::Tunnels,
            PanelId::QuickCommands,
            PanelId::Transfers,
        ] {
            assert_eq!(default_side(panel), PanelSide::Right);
        }
    }

    #[test]
    fn narrow_window_auto_collapse_side_uses_memory_then_default() {
        assert_eq!(auto_collapsed_side(1200.0, None), None);
        assert_eq!(auto_collapsed_side(1000.0, None), Some(PanelSide::Right));
        assert_eq!(
            auto_collapsed_side(1000.0, Some(PanelSide::Left)),
            Some(PanelSide::Left)
        );
        assert_eq!(PanelPlacement::Hidden.side(), None);
        assert_eq!(PanelPlacement::Left.side(), Some(PanelSide::Left));
        assert_eq!(PanelPlacement::Right.id(), "right");
        assert_eq!(PanelPlacement::Hidden.id(), "hidden");
    }

    #[test]
    fn layout_round_trips_through_toml() {
        let document = ConfigDocument {
            ui: UiProfile {
                layout: sample_layout(),
                ..UiProfile::default()
            },
            ..ConfigDocument::default()
        };
        let toml = document.to_toml_string().expect("serialize");
        assert!(toml.contains("narrow_collapsed_side = \"right\""));
        let reparsed = ConfigDocument::from_toml_str(&toml).expect("parse");
        assert_eq!(reparsed.ui.layout, document.ui.layout);
        assert_eq!(
            PanelLayoutModel::from_layout(&reparsed.ui.layout),
            PanelLayoutModel::from_layout(&document.ui.layout)
        );
    }

    #[test]
    fn panel_layout_mutators_persist_to_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut runtime =
            AppRuntime::new_with_keychain(dir.path().to_path_buf(), None).expect("runtime");
        runtime
            .move_panel(PanelId::Sftp, PanelSide::Left, Some(1))
            .expect("move");
        runtime
            .toggle_panel_collapsed(PanelId::Sftp)
            .expect("collapse");
        runtime
            .set_panel_side_width(PanelSide::Left, 260.0)
            .expect("width");
        runtime
            .set_panel_split_fraction(PanelSide::Left, 0, 0.35)
            .expect("fraction");
        runtime
            .set_panel_auto_collapsed_side(Some(PanelSide::Left))
            .expect("pin");

        let reloaded =
            AppRuntime::new_with_keychain(dir.path().to_path_buf(), None).expect("reload");
        let model = reloaded.panel_layout();
        assert_eq!(model.placement(PanelId::Sftp), PanelPlacement::Left);
        assert!(model.collapsed(PanelId::Sftp));
        assert_eq!(model.width(PanelSide::Left), 260);
        assert_eq!(model.layout().left_ratios, vec![0.35]);
        assert_eq!(model.layout().narrow_collapsed_side, Some(PanelSide::Left));
        assert_eq!(
            model.panels(PanelSide::Left),
            vec![PanelId::Sessions, PanelId::Sftp]
        );
    }
}
