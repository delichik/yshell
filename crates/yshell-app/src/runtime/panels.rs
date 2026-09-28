//! Panel visibility/layout state and the settings terminal (scrollback) form.
//!
//! N3 Phase 1 adds the docked-panel layout model ([`PanelLayoutModel`]), the
//! breakpoint helpers used to replace the `window.width`-driven Slint bindings
//! with Rust-computed values, and the `AppRuntime` read/write path for C0's
//! `UiProfile.layout`. Unit tests live at the bottom of this file.

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
        self.toggle_panel_or_status(PanelId::Sftp)
    }

    pub fn toggle_tunnels(&mut self) -> AppProjection {
        self.toggle_panel_or_status(PanelId::Tunnels)
    }

    pub fn toggle_commands(&mut self) -> AppProjection {
        self.toggle_panel_or_status(PanelId::QuickCommands)
    }

    /// 旧版布尔开关的兼容入口（失败时把错误写进状态栏文本）。
    fn toggle_panel_or_status(&mut self, panel: PanelId) -> AppProjection {
        match self.toggle_panel_visible(panel) {
            Ok(projection) => projection,
            Err(error) => {
                self.status_text = format!("Panel layout error: {error}");
                self.projection()
            }
        }
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
// 这些判定全部由 Rust 计算后作为普通属性下发；Slint 侧不再从 `window.width`
// 反推布局（Phase 2 的 `compute_panel_layout` 是唯一消费者）。

/// <960px 且用户未手动展开时，左栏折叠为 48px 图标条。
pub(crate) fn nav_rail_active(window_width: f32, nav_user_expanded: bool) -> bool {
    window_width < BREAKPOINT_NAV_RAIL && !nav_user_expanded
}

/// <960px：左栏处于图标条断点区间。
pub(crate) fn nav_rail_available(window_width: f32) -> bool {
    window_width < BREAKPOINT_NAV_RAIL
}

/// <1120px：命令栏显示栏展开/收起按钮。
pub(crate) fn dock_toggle_visible(window_width: f32) -> bool {
    window_width < BREAKPOINT_DOCK_COLLAPSE
}

/// 1120–1280px 档的线性收缩系数（≤1120 为 0，≥1280 为 1）。
pub(crate) fn layout_squeeze_ratio(window_width: f32) -> f32 {
    ((window_width - BREAKPOINT_DOCK_COLLAPSE)
        / (BREAKPOINT_FULL_LAYOUT - BREAKPOINT_DOCK_COLLAPSE))
        .clamp(0.0, 1.0)
}

/// 拖拽换算系数：收缩档内指针 1px 对应目标宽度 `1/ratio` px，保证把手跟手。
/// 下限取 0.001（而不是 Slint 侧的 0.0001）：投影按千分比取整下发，0 会让
/// Slint 侧出现除以 0。
pub(crate) fn layout_drag_scale(window_width: f32) -> f32 {
    if window_width >= BREAKPOINT_FULL_LAYOUT || window_width < BREAKPOINT_DOCK_COLLAPSE {
        1.0
    } else {
        layout_squeeze_ratio(window_width).max(0.001)
    }
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

    /// 整体写入一侧的边界比例（拖拽用；清洗后落盘）。
    pub(crate) fn set_boundary_fractions(&mut self, side: PanelSide, fractions: &[f32]) -> bool {
        let count = self.boundary_count(side);
        if count == 0 || fractions.len() != count {
            return false;
        }
        let values = resolve_fractions(fractions, count).unwrap_or_else(|| even_fractions(count));
        let changed = values != self.boundary_fractions(side);
        match side {
            PanelSide::Left => self.layout.left_ratios = values,
            PanelSide::Right => self.layout.right_ratios = values,
        }
        changed
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
        let layout = model.layout().clone();
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

    /// 面板菜单 Collapse/Expand（显式状态，不依赖菜单里读到的旧值）。
    pub fn set_panel_collapsed(
        &mut self,
        panel: PanelId,
        collapsed: bool,
    ) -> AppResult<AppProjection> {
        let mut model = self.panel_layout();
        model.set_collapsed(panel, collapsed);
        self.persist_panel_layout(model)
    }
}

// --- N3 Phase 2：内容区 px 布局（Rust 计算 → 投影下发）------------------------
//
// 面板层在 Slint 侧绝对定位：Rust 拿到内容区尺寸、面板栈顺序、折叠态与比例后直接
// 算出每个面板框的 x/y/宽/高与断点标志。Slint 不再从 `window.width` 反推布局，
// `nav_effective_width`/`dock_effective_width` 一类的绑定环随之消失。

/// 折叠面板高度（设计 28–32px）。
pub(crate) const PANEL_COLLAPSED_HEIGHT: f32 = 32.0;
/// 同栏相邻面板之间的分栏把手高度。
pub(crate) const PANEL_HANDLE_HEIGHT: f32 = 4.0;
/// 展开面板最小高度（§4.3：每面板最小高度如 120px）。
pub(crate) const PANEL_MIN_HEIGHT: f32 = 120.0;
/// 右栏上下留白（与 W5-④ 的卡片留白一致）。
pub(crate) const DOCK_TOP_PADDING: f32 = 12.0;
pub(crate) const DOCK_BOTTOM_PADDING: f32 = 12.0;
/// 内容区尺寸兜底（Slint 首次布局回调前；1440×900 减去菜单栏/命令栏/状态栏）。
pub(crate) const DEFAULT_PANEL_AREA: (f32, f32) = (1440.0, 794.0);

/// 窄窗策略的另一侧。
const fn other_side(side: PanelSide) -> PanelSide {
    match side {
        PanelSide::Left => PanelSide::Right,
        PanelSide::Right => PanelSide::Left,
    }
}

/// 非「填充」面板在默认（无显式比例）布局下的内容高度；`None` = 吸收剩余高度。
/// 与 N1 之前 WinCard 的内容高度对齐（Tunnels/Quick Commands ≈ 89px），保证
/// 1440×900 默认布局下 SFTP 面板与其内部行位置与旧版一致（e2e 像素基线）。
fn auto_panel_height(panel: PanelId) -> Option<f32> {
    match panel {
        PanelId::Sftp | PanelId::Sessions => None,
        PanelId::Tunnels | PanelId::QuickCommands => Some(89.0),
        PanelId::Transfers => Some(120.0),
    }
}

/// 单个面板框的 px 几何（相对内容区左上角）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PanelFrameView {
    pub panel: PanelId,
    pub placement: PanelPlacement,
    pub collapsed: bool,
    pub visible: bool,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// 一个分栏手柄的 px 几何（相对内容区左上角）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PanelSplitHandleView {
    pub side: PanelSide,
    pub boundary: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// 拖拽面板时的插入位置指示线。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PanelDragIndicatorView {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// 拖拽中的面板（内存态）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PanelDragState {
    pub panel: PanelId,
    pub target: Option<(PanelSide, usize)>,
}

/// 内容区布局视图（投影给 Slint 的纯数据）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PanelLayoutView {
    pub frames: Vec<PanelFrameView>,
    /// 模型记忆的栏宽（拖拽预览时也用于把手换算）。
    pub left_width: f32,
    pub right_width: f32,
    /// 最终栏宽（含图标条/自动折叠/收缩档换算）。
    pub left_effective_width: f32,
    pub right_effective_width: f32,
    pub rail_active: bool,
    pub rail_available: bool,
    pub dock_toggle_visible: bool,
    pub right_collapsed: bool,
    pub drag_scale: f32,
    pub split_handles: Vec<PanelSplitHandleView>,
    pub drag_indicator: Option<PanelDragIndicatorView>,
}

impl PanelLayoutView {
    pub(crate) fn frame(&self, panel: PanelId) -> Option<&PanelFrameView> {
        self.frames.iter().find(|frame| frame.panel == panel)
    }
}

/// 计算内容区布局（纯函数；`user_expanded_side` 是窄窗手动展开的内存覆盖）。
pub(crate) fn compute_panel_layout(
    layout: &LayoutProfile,
    area_width: f32,
    area_height: f32,
    user_expanded_side: Option<PanelSide>,
) -> PanelLayoutView {
    let area_width = if area_width.is_finite() {
        area_width.max(0.0)
    } else {
        DEFAULT_PANEL_AREA.0
    };
    let area_height = if area_height.is_finite() {
        area_height.max(0.0)
    } else {
        DEFAULT_PANEL_AREA.1
    };

    let sessions_in_left = layout
        .left
        .iter()
        .any(|slot| slot.panel == PanelId::Sessions);
    let left_nonempty = !layout.left.is_empty();
    let right_nonempty = !layout.right.is_empty();

    let collapsed_side = auto_collapsed_side(area_width, layout.narrow_collapsed_side)
        .filter(|side| user_expanded_side != Some(*side));
    let rail_available = nav_rail_available(area_width);
    let left_user_expanded = user_expanded_side == Some(PanelSide::Left);
    // 图标条：<960px（`nav_rail_active`），或窄窗策略把左栏定为自动折叠侧。
    let rail_active = left_nonempty
        && sessions_in_left
        && (nav_rail_active(area_width, left_user_expanded)
            || (collapsed_side == Some(PanelSide::Left) && !left_user_expanded));
    let left_auto_collapsed =
        left_nonempty && (rail_available || collapsed_side == Some(PanelSide::Left));

    let left_effective_width = if !left_nonempty {
        0.0
    } else if left_auto_collapsed && !left_user_expanded {
        if rail_active {
            NAV_RAIL_WIDTH
        } else {
            0.0
        }
    } else {
        squeezed_side_width(area_width, layout.left_width as f32, LEFT_WIDTH_MIN as f32)
    };

    let right_collapsed = right_nonempty && collapsed_side == Some(PanelSide::Right);
    let right_effective_width = if right_nonempty && !right_collapsed {
        squeezed_side_width(
            area_width,
            layout.right_width as f32,
            RIGHT_WIDTH_MIN as f32,
        )
    } else {
        0.0
    };

    let mut frames = Vec::with_capacity(layout.left.len() + layout.right.len());
    append_stack_frames(
        &mut frames,
        layout,
        PanelSide::Left,
        area_width,
        area_height,
        left_effective_width,
        left_effective_width > 0.0 && !rail_active,
    );
    append_stack_frames(
        &mut frames,
        layout,
        PanelSide::Right,
        area_width,
        area_height,
        right_effective_width,
        right_effective_width > 0.0,
    );

    let split_handles = split_handle_views(&frames);
    PanelLayoutView {
        frames,
        left_width: layout.left_width as f32,
        right_width: layout.right_width as f32,
        left_effective_width,
        right_effective_width,
        rail_active,
        rail_available,
        dock_toggle_visible: dock_toggle_visible(area_width),
        right_collapsed,
        drag_scale: layout_drag_scale(area_width),
        split_handles,
        drag_indicator: None,
    }
}

/// 相邻面板之间生成 4px 分栏手柄（位置由面板框决定）。
fn split_handle_views(frames: &[PanelFrameView]) -> Vec<PanelSplitHandleView> {
    let mut handles = Vec::new();
    for side in [PanelSide::Left, PanelSide::Right] {
        let side_frames: Vec<&PanelFrameView> = frames
            .iter()
            .filter(|frame| frame.placement == PanelPlacement::from(side))
            .collect();
        for (boundary, pair) in side_frames.windows(2).enumerate() {
            let top = pair[0];
            if !top.visible {
                continue;
            }
            handles.push(PanelSplitHandleView {
                side,
                boundary,
                x: top.x,
                y: top.y + top.height,
                width: top.width,
                height: PANEL_HANDLE_HEIGHT,
            });
        }
    }
    handles
}

fn append_stack_frames(
    frames: &mut Vec<PanelFrameView>,
    layout: &LayoutProfile,
    side: PanelSide,
    area_width: f32,
    area_height: f32,
    effective_width: f32,
    column_visible: bool,
) {
    let (slots, ratios, top_padding, bottom_padding) = match side {
        PanelSide::Left => (&layout.left, &layout.left_ratios, 0.0, 0.0),
        PanelSide::Right => (
            &layout.right,
            &layout.right_ratios,
            DOCK_TOP_PADDING,
            DOCK_BOTTOM_PADDING,
        ),
    };
    if slots.is_empty() {
        return;
    }
    let heights = stack_heights(slots, ratios, area_height, top_padding, bottom_padding);
    let x = match side {
        PanelSide::Left => 0.0,
        PanelSide::Right => (area_width - effective_width).max(0.0),
    };
    let mut y = top_padding;
    for (slot, height) in slots.iter().zip(heights) {
        frames.push(PanelFrameView {
            panel: slot.panel,
            placement: PanelPlacement::from(side),
            collapsed: slot.collapsed,
            visible: column_visible,
            x,
            y,
            width: effective_width.max(0.0),
            height,
        });
        y += height + PANEL_HANDLE_HEIGHT;
    }
}

/// 一侧栈内的高度分配：折叠面板钉 32px；显式比例按权重分摊；否则填充面板吸收剩余。
fn stack_heights(
    slots: &[PanelSlot],
    ratios: &[f32],
    area_height: f32,
    top_padding: f32,
    bottom_padding: f32,
) -> Vec<f32> {
    let count = slots.len();
    let handle_total = PANEL_HANDLE_HEIGHT * count.saturating_sub(1) as f32;
    let available = (area_height - top_padding - bottom_padding - handle_total).max(0.0);
    let collapsed_total =
        slots.iter().filter(|slot| slot.collapsed).count() as f32 * PANEL_COLLAPSED_HEIGHT;
    let flex = (available - collapsed_total).max(0.0);

    let expanded: Vec<usize> = slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| !slot.collapsed)
        .map(|(index, _)| index)
        .collect();
    let mut heights = vec![0.0_f32; count];
    for (index, slot) in slots.iter().enumerate() {
        if slot.collapsed {
            heights[index] = PANEL_COLLAPSED_HEIGHT;
        }
    }
    if expanded.is_empty() {
        return heights;
    }

    if let Some(fractions) = resolve_fractions(ratios, count.saturating_sub(1)) {
        let weights: Vec<f32> = slots
            .iter()
            .enumerate()
            .map(|(index, _)| {
                let lower = if index == 0 {
                    0.0
                } else {
                    fractions[index - 1]
                };
                let upper = if index + 1 == count {
                    1.0
                } else {
                    fractions[index]
                };
                (upper - lower).max(0.0)
            })
            .collect();
        let total: f32 = expanded.iter().map(|&index| weights[index]).sum();
        for &index in &expanded {
            heights[index] = if total > 0.0 {
                flex * weights[index] / total
            } else {
                flex / expanded.len() as f32
            };
        }
    } else {
        let fills: Vec<usize> = expanded
            .iter()
            .copied()
            .filter(|&index| auto_panel_height(slots[index].panel).is_none())
            .collect();
        if fills.is_empty() {
            // 没有「填充」面板（例如 SFTP 被折叠/移走）：展开面板等分剩余高度。
            let share = flex / expanded.len() as f32;
            for &index in &expanded {
                heights[index] = share;
            }
        } else {
            let fixed: f32 = expanded
                .iter()
                .filter_map(|&index| auto_panel_height(slots[index].panel))
                .sum();
            let share = ((flex - fixed).max(0.0)) / fills.len() as f32;
            for &index in &expanded {
                heights[index] = auto_panel_height(slots[index].panel).unwrap_or(share);
            }
            // 默认布局下内容尺寸面板（Tunnels/Quick Commands）不套 120px 下限，
            // 保持与旧 WinCard 相同的内容高度；「填充」面板仍受最小高度约束。
            enforce_min_heights(&mut heights, &fills);
            return heights;
        }
    }
    enforce_min_heights(&mut heights, &expanded);
    heights
}

/// 把低于最小高度的展开面板抬到 120px，并从有富余的面板按比例扣减。
fn enforce_min_heights(heights: &mut [f32], expanded: &[usize]) {
    let deficit: f32 = expanded
        .iter()
        .map(|&index| (PANEL_MIN_HEIGHT - heights[index]).max(0.0))
        .sum();
    if deficit <= 0.0 {
        return;
    }
    let slack_total: f32 = expanded
        .iter()
        .map(|&index| (heights[index] - PANEL_MIN_HEIGHT).max(0.0))
        .sum();
    for &index in expanded {
        if heights[index] < PANEL_MIN_HEIGHT {
            heights[index] = PANEL_MIN_HEIGHT;
        } else if slack_total > 0.0 {
            let slack = heights[index] - PANEL_MIN_HEIGHT;
            heights[index] -= (deficit * slack / slack_total).min(slack);
        }
    }
}

// C0 默认布局不含 Transfers（N1 起转移队列并入 SFTP 面板的队列抽屉；N3 起
// Transfers 是可停靠的独立面板，默认隐藏、经 Panels/View 菜单按需显示）。
// 因此「不在左右栈中」= 隐藏，不做启动期迁移：否则"用户隐藏 Transfers"会与
// 旧默认布局不可区分，导致重启后被自动塞回右栏。

impl AppRuntime {
    /// Slint 内容区尺寸回调（内存态，不落盘）。
    pub fn set_panel_area_size(&mut self, width: f32, height: f32) -> AppProjection {
        self.panel_area_width = if width.is_finite() {
            width.max(0.0)
        } else {
            DEFAULT_PANEL_AREA.0
        };
        self.panel_area_height = if height.is_finite() {
            height.max(0.0)
        } else {
            DEFAULT_PANEL_AREA.1
        };
        self.projection()
    }

    /// 当前内容区布局视图（投影用）。
    pub(crate) fn panel_layout_view(&self) -> PanelLayoutView {
        let mut view = compute_panel_layout(
            &self.config_document.ui.layout,
            self.panel_area_width,
            self.panel_area_height,
            self.panel_user_expanded_side,
        );
        view.drag_indicator = self.drag_indicator_view(&view);
        view
    }

    /// 面板是否可见（在左/右任一栈中）。
    pub(crate) fn panel_visible(&self, panel: PanelId) -> bool {
        self.panel_layout().placement(panel) != PanelPlacement::Hidden
    }

    /// View/Panels 菜单：隐藏的面板按默认侧恢复显示，已显示的隐藏。
    pub fn toggle_panel_visible(&mut self, panel: PanelId) -> AppResult<AppProjection> {
        let mut model = self.panel_layout();
        if model.placement(panel) == PanelPlacement::Hidden {
            model.move_to(panel, default_side(panel), None);
        } else {
            model.hide(panel);
        }
        self.persist_panel_layout(model)
    }

    /// 栏宽拖拽预览：只改内存布局，不落盘。
    pub fn preview_panel_side_width(&mut self, side: PanelSide, width: f32) -> AppProjection {
        let mut model = self.panel_layout();
        model.set_width(side, width);
        self.config_document.ui.layout = model.layout().clone();
        self.projection()
    }

    /// 拖拽结束：把当前内存布局落盘。
    pub fn commit_panel_layout(&mut self) -> AppResult<AppProjection> {
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        Ok(self.projection())
    }

    /// 分栏拖拽（px）：把指针 y（相对内容区顶边）换算成相邻两面板的新高度，
    /// 再还原为整侧的边界比例。只改内存，拖拽结束由 `commit_panel_layout` 落盘。
    pub fn preview_panel_split_pixels(
        &mut self,
        side: PanelSide,
        boundary: usize,
        pointer_y: f32,
    ) -> AppProjection {
        let view = self.panel_layout_view();
        let frames: Vec<PanelFrameView> = view
            .frames
            .iter()
            .filter(|frame| frame.placement == PanelPlacement::from(side))
            .copied()
            .collect();
        if boundary + 1 >= frames.len() {
            return self.projection();
        }
        let top = frames[boundary];
        let bottom = frames[boundary + 1];
        if !top.visible || !bottom.visible {
            return self.projection();
        }
        let delta = pointer_y - bottom.y;
        let min_top = if top.collapsed {
            PANEL_COLLAPSED_HEIGHT
        } else {
            PANEL_MIN_HEIGHT
        };
        let min_bottom = if bottom.collapsed {
            PANEL_COLLAPSED_HEIGHT
        } else {
            PANEL_MIN_HEIGHT
        };
        let mut heights: Vec<f32> = frames.iter().map(|frame| frame.height).collect();
        match (top.collapsed, bottom.collapsed) {
            (false, false) => {
                let total = heights[boundary] + heights[boundary + 1];
                let new_top =
                    (heights[boundary] + delta).clamp(min_top, (total - min_bottom).max(min_top));
                heights[boundary] = new_top;
                heights[boundary + 1] = (total - new_top).max(min_bottom);
            }
            (false, true) => {
                heights[boundary] = (heights[boundary] + delta).max(min_top);
            }
            (true, false) => {
                heights[boundary + 1] = (heights[boundary + 1] - delta).max(min_bottom);
            }
            (true, true) => return self.projection(),
        }
        let count = frames.len();
        let total: f32 = heights.iter().sum();
        if count < 2 || total <= 0.0 {
            return self.projection();
        }
        let mut cumulative = 0.0_f32;
        let mut fractions = Vec::with_capacity(count - 1);
        for height in heights.iter().take(count - 1) {
            cumulative += height;
            fractions.push((cumulative / total).clamp(SPLIT_RATIO_MIN, SPLIT_RATIO_MAX));
        }
        let mut model = self.panel_layout();
        model.set_boundary_fractions(side, &fractions);
        self.config_document.ui.layout = model.layout().clone();
        self.projection()
    }

    /// 面板头部开始拖拽（换边/换序）：记录拖拽面板。
    pub fn panel_drag_start(&mut self, panel: PanelId, _x: f32, _y: f32) -> AppProjection {
        self.panel_drag = Some(PanelDragState {
            panel,
            target: None,
        });
        self.projection()
    }

    /// 拖拽中：计算落点（用于插入指示线）。
    pub fn panel_drag_move(&mut self, panel: PanelId, x: f32, y: f32) -> AppProjection {
        let view = self.panel_layout_view();
        let target = self.panel_drop_target(&view, x, y);
        self.panel_drag = Some(PanelDragState { panel, target });
        self.projection()
    }

    /// 拖拽落下：合法落点则移动面板（落盘），否则保持原样。
    pub fn panel_drag_drop(&mut self, panel: PanelId, x: f32, y: f32) -> AppResult<AppProjection> {
        let view = self.panel_layout_view();
        let target = self.panel_drop_target(&view, x, y);
        self.panel_drag = None;
        match target {
            Some((side, index)) => self.move_panel(panel, side, Some(index)),
            None => Ok(self.projection()),
        }
    }

    /// 拖拽取消（指针取消事件）。
    pub fn panel_drag_cancel(&mut self) -> AppProjection {
        self.panel_drag = None;
        self.projection()
    }

    /// 窄窗图标条/菜单：显式展开一侧（内存覆盖 + 记忆另一侧折叠）。
    pub fn expand_panel_side(&mut self, side: PanelSide) -> AppResult<AppProjection> {
        self.panel_user_expanded_side = Some(side);
        let mut model = self.panel_layout();
        model.set_auto_collapsed_side(Some(other_side(side)));
        self.persist_panel_layout(model)
    }

    /// 拖拽落点：x 决定栏，y 决定插入序号（相对内容区坐标）。
    fn panel_drop_target(
        &self,
        view: &PanelLayoutView,
        x: f32,
        y: f32,
    ) -> Option<(PanelSide, usize)> {
        let side = if view.left_effective_width > 0.0 && x < view.left_effective_width {
            PanelSide::Left
        } else if view.right_effective_width > 0.0
            && x > (self.panel_area_width - view.right_effective_width).max(0.0)
        {
            PanelSide::Right
        } else {
            return None;
        };
        let frames: Vec<&PanelFrameView> = view
            .frames
            .iter()
            .filter(|frame| frame.placement == PanelPlacement::from(side))
            .collect();
        if frames.is_empty() {
            return None;
        }
        let mut index = frames.len();
        for (position, frame) in frames.iter().enumerate() {
            if y < frame.y + frame.height / 2.0 {
                index = position;
                break;
            }
        }
        Some((side, index))
    }

    fn drag_indicator_view(&self, view: &PanelLayoutView) -> Option<PanelDragIndicatorView> {
        let (side, index) = self.panel_drag.as_ref()?.target?;
        let frames: Vec<&PanelFrameView> = view
            .frames
            .iter()
            .filter(|frame| frame.placement == PanelPlacement::from(side))
            .collect();
        let first = frames.first()?;
        let last = frames.last()?;
        let y = if index >= frames.len() {
            (last.y + last.height + 2.0).max(0.0)
        } else {
            (frames[index].y - 2.0).max(0.0)
        };
        Some(PanelDragIndicatorView {
            x: first.x,
            y,
            width: first.width,
            height: 2.0,
        })
    }

    /// 窄窗栏展开/收起（命令栏 chevron / 图标条菜单）：
    /// 展开被自动折叠的一侧 → 记忆"另一侧折叠"；收起 → 记忆本侧折叠。
    pub fn toggle_panel_side_expanded(&mut self, side: PanelSide) -> AppResult<AppProjection> {
        let view = self.panel_layout_view();
        let collapsed = match side {
            PanelSide::Left => view.left_effective_width < LEFT_WIDTH_MIN as f32,
            PanelSide::Right => view.right_collapsed,
        };
        let expanding = collapsed && self.panel_user_expanded_side != Some(side);
        self.panel_user_expanded_side = if expanding { Some(side) } else { None };
        let mut model = self.panel_layout();
        if expanding {
            model.set_auto_collapsed_side(Some(other_side(side)));
        } else {
            model.set_auto_collapsed_side(Some(side));
        }
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

    fn panels_of(model: &PanelLayoutModel, side: PanelSide) -> Vec<PanelId> {
        model.stack(side).iter().map(|slot| slot.panel).collect()
    }

    fn width_of(model: &PanelLayoutModel, side: PanelSide) -> u32 {
        match side {
            PanelSide::Left => model.layout().left_width,
            PanelSide::Right => model.layout().right_width,
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

        assert_eq!(layout_squeeze_ratio(1119.0), 0.0);
        assert_eq!(layout_squeeze_ratio(1120.0), 0.0);
        assert_eq!(layout_squeeze_ratio(1200.0), 0.5);
        assert_eq!(layout_squeeze_ratio(1280.0), 1.0);
        assert_eq!(layout_squeeze_ratio(2000.0), 1.0);
        assert_eq!(layout_drag_scale(1000.0), 1.0);
        assert!(layout_drag_scale(1200.0) > 0.0);

        // 断点语义经由唯一消费者（布局视图）验证：
        // 940px → 图标条 48；960px 起恢复完整左栏。
        let rail = compute_panel_layout(&LayoutProfile::default(), 940.0, 600.0, None);
        assert!(rail.rail_active);
        assert_eq!(rail.left_effective_width, NAV_RAIL_WIDTH);
        assert!(rail.rail_available);
        let full = compute_panel_layout(&LayoutProfile::default(), 960.0, 600.0, None);
        assert!(!full.rail_active);
        assert!(!full.rail_available);
        assert_eq!(full.left_effective_width, DEFAULT_LEFT_WIDTH as f32);
        // 用户在 940px 手动展开左栏 → 完整左栏。
        let expanded = compute_panel_layout(
            &LayoutProfile::default(),
            940.0,
            600.0,
            Some(PanelSide::Left),
        );
        assert!(!expanded.rail_active);
        assert_eq!(expanded.left_effective_width, DEFAULT_LEFT_WIDTH as f32);

        // 1120px 是右栏自动折叠边界（默认折叠侧 Right）；恰好 1120 时进入收缩档下限 280。
        let collapsed = compute_panel_layout(&LayoutProfile::default(), 1119.0, 600.0, None);
        assert!(collapsed.right_collapsed);
        assert_eq!(collapsed.right_effective_width, 0.0);
        let visible = compute_panel_layout(&LayoutProfile::default(), 1120.0, 600.0, None);
        assert!(!visible.right_collapsed);
        assert_eq!(visible.right_effective_width, RIGHT_WIDTH_MIN as f32);

        // 1120–1280 收缩档：左 300 → 250，右 340 → 287.5。
        let squeeze = LayoutProfile {
            left_width: 300,
            right_width: 340,
            narrow_collapsed_side: None,
            ..LayoutProfile::default()
        };
        let mid = compute_panel_layout(&squeeze, 1200.0, 600.0, None);
        assert_eq!(mid.left_effective_width, 250.0);
        assert_eq!(mid.right_effective_width, 310.0);
        let wide = compute_panel_layout(&squeeze, 1300.0, 600.0, None);
        assert_eq!(wide.left_effective_width, 300.0);
        assert_eq!(wide.right_effective_width, 340.0);
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
        assert_eq!(panels_of(&model, PanelSide::Left), vec![PanelId::Sftp]);
        assert_eq!(
            panels_of(&model, PanelSide::Right),
            vec![PanelId::Tunnels, PanelId::Transfers]
        );
        // 左栈只剩 1 个面板（0 个边界）→ 比例被清空；右栈 1 个边界 → 夹取到上限。
        assert!(model.layout().left_ratios.is_empty());
        assert_eq!(model.layout().right_ratios, vec![SPLIT_RATIO_MAX]);
        assert_eq!(width_of(&model, PanelSide::Left), LEFT_WIDTH_MAX);
        assert_eq!(width_of(&model, PanelSide::Right), RIGHT_WIDTH_MIN);
        assert_eq!(model.layout().narrow_collapsed_side, Some(PanelSide::Left));
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
            panels_of(&model, PanelSide::Right),
            vec![PanelId::Sessions, PanelId::Sftp, PanelId::QuickCommands]
        );
        assert_eq!(panels_of(&model, PanelSide::Left), vec![PanelId::Tunnels]);
        assert!(model.collapsed(PanelId::Sessions));
        assert!(model.layout().right_ratios.is_empty());
        assert!(model.boundary_fractions(PanelSide::Left).is_empty());

        // 移回左栏栈尾并保留折叠态。
        assert!(model.move_to(PanelId::Sessions, PanelSide::Left, None));
        assert_eq!(model.placement(PanelId::Sessions), PanelPlacement::Left);
        assert!(model.collapsed(PanelId::Sessions));
        assert_eq!(
            panels_of(&model, PanelSide::Left),
            vec![PanelId::Tunnels, PanelId::Sessions]
        );
    }

    #[test]
    fn reorder_within_side_keeps_split_ratios() {
        let mut model = PanelLayoutModel::from_layout(&sample_layout());
        let before = model.boundary_fractions(PanelSide::Left);
        assert!(model.move_to(PanelId::Sessions, PanelSide::Left, Some(1)));
        assert_eq!(
            panels_of(&model, PanelSide::Left),
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
            panels_of(&model, PanelSide::Left),
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
        // 向下拖第一个边界：被第二个边界（含间距）压住（整侧写入，逐值夹取）。
        assert!(model.set_boundary_fractions(PanelSide::Left, &[0.8, SPLIT_RATIO_MAX]));
        assert_eq!(model.layout().left_ratios, vec![0.8, SPLIT_RATIO_MAX]);
        assert!(model.set_boundary_fractions(PanelSide::Left, &[0.8, 0.8 + SPLIT_RATIO_GAP_MIN]));
        assert_eq!(
            model.layout().left_ratios,
            vec![0.8, 0.8 + SPLIT_RATIO_GAP_MIN]
        );
        // 长度不符（越界边界）拒绝写入。
        assert!(!model.set_boundary_fractions(PanelSide::Left, &[0.1, 0.2, 0.3]));
        // 单面板（0 个边界）无比例可设。
        assert_eq!(model.boundary_count(PanelSide::Right), 0);
        assert!(!model.set_boundary_fractions(PanelSide::Right, &[-1.0]));
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
        assert_eq!(width_of(&model, PanelSide::Right), RIGHT_WIDTH_MAX);
        model.set_width(PanelSide::Left, 250.0);
        assert_eq!(width_of(&model, PanelSide::Left), 250);
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
                assert!(panels_of(&model, side).contains(&panel));
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
        runtime.set_panel_area_size(1440.0, 794.0);
        // 栏宽拖拽预览 + 分栏拖拽预览，结束时一起落盘（Phase 2 的拖拽路径）。
        runtime.preview_panel_side_width(PanelSide::Left, 260.0);
        // 左栈 = [Sessions, Sftp(折叠 32px)]；把边界拖到 Sessions 高 ≈ 400px 处。
        runtime.preview_panel_split_pixels(PanelSide::Left, 0, 400.0);
        runtime.commit_panel_layout().expect("commit");
        runtime
            .expand_panel_side(PanelSide::Left)
            .expect("pin left expanded");

        let reloaded =
            AppRuntime::new_with_keychain(dir.path().to_path_buf(), None).expect("reload");
        let model = reloaded.panel_layout();
        assert_eq!(model.placement(PanelId::Sftp), PanelPlacement::Left);
        assert!(model.collapsed(PanelId::Sftp));
        assert_eq!(width_of(&model, PanelSide::Left), 260);
        // 边界比例被夹取在 [MIN, MAX] 且显式落盘。
        let fractions = model.boundary_fractions(PanelSide::Left);
        assert_eq!(fractions.len(), 1);
        assert!((SPLIT_RATIO_MIN..=SPLIT_RATIO_MAX).contains(&fractions[0]));
        assert!(!model.layout().left_ratios.is_empty());
        assert_eq!(model.layout().narrow_collapsed_side, Some(PanelSide::Right));
        assert_eq!(
            panels_of(&model, PanelSide::Left),
            vec![PanelId::Sessions, PanelId::Sftp]
        );
    }

    // --- N3 Phase 2：px 布局引擎 ---------------------------------------------

    fn approx(left: f32, right: f32) -> bool {
        (left - right).abs() < 0.01
    }

    #[test]
    fn layout_view_keeps_legacy_geometry_at_1440x900() {
        let layout = LayoutProfile::default();
        let view = compute_panel_layout(&layout, 1440.0, 794.0, None);
        assert_eq!(view.left_width, 240.0);
        assert_eq!(view.right_width, 340.0);
        assert_eq!(view.left_effective_width, 240.0);
        assert_eq!(view.right_effective_width, 340.0);
        assert!(!view.rail_active);
        assert!(!view.dock_toggle_visible);
        assert!(!view.right_collapsed);

        let sessions = *view.frame(PanelId::Sessions).expect("sessions");
        assert_eq!(sessions.placement, PanelPlacement::Left);
        assert!(sessions.visible);
        assert_eq!(
            (sessions.x, sessions.y, sessions.width, sessions.height),
            (0.0, 0.0, 240.0, 794.0)
        );

        let sftp = *view.frame(PanelId::Sftp).expect("sftp");
        assert_eq!((sftp.x, sftp.y, sftp.width), (1100.0, 12.0, 340.0));
        // 12 上留白 + 2×4 把手 + 89/89 内容高度 → SFTP 吸收剩余 584px（与旧卡片一致）。
        assert!(approx(sftp.height, 584.0), "sftp height = {}", sftp.height);
        let tunnels = *view.frame(PanelId::Tunnels).expect("tunnels");
        assert!(approx(tunnels.y, 600.0), "tunnels y = {}", tunnels.y);
        assert!(
            approx(tunnels.height, 89.0),
            "tunnels height = {}",
            tunnels.height
        );
        let commands = *view.frame(PanelId::QuickCommands).expect("commands");
        assert!(approx(commands.y, 693.0), "commands y = {}", commands.y);
        assert!(approx(commands.y + commands.height + 12.0, 794.0));
        // C0 默认不含 Transfers：默认隐藏，等 Panels 菜单恢复。
        assert!(view.frame(PanelId::Transfers).is_none());
    }

    #[test]
    fn narrow_window_collapses_side_and_rails_left() {
        let layout = LayoutProfile::default();
        // 1120 以下：默认折叠右栏。
        let view = compute_panel_layout(&layout, 1000.0, 600.0, None);
        assert!(view.dock_toggle_visible);
        assert!(view.right_collapsed);
        assert_eq!(view.right_effective_width, 0.0);
        assert!(!view.frame(PanelId::Sftp).expect("sftp").visible);
        assert!(view.frame(PanelId::Sessions).expect("sessions").visible);
        assert_eq!(view.left_effective_width, 240.0);

        // 960 以下：左栏收成 48px 图标条。
        let view = compute_panel_layout(&layout, 900.0, 600.0, None);
        assert!(view.rail_available);
        assert!(view.rail_active);
        assert_eq!(view.left_effective_width, NAV_RAIL_WIDTH);
        assert!(!view.frame(PanelId::Sessions).expect("sessions").visible);

        // 用户在 900px 手动展开左栏：图标条让位给完整左栏。
        let view = compute_panel_layout(&layout, 900.0, 600.0, Some(PanelSide::Left));
        assert!(!view.rail_active);
        assert_eq!(view.left_effective_width, 240.0);
        assert!(view.frame(PanelId::Sessions).expect("sessions").visible);

        // 用户在 1000px 手动展开右栏：右栏恢复。
        let view = compute_panel_layout(&layout, 1000.0, 600.0, Some(PanelSide::Right));
        assert!(!view.right_collapsed);
        assert_eq!(view.right_effective_width, 340.0);
        assert!(view.frame(PanelId::Sftp).expect("sftp").visible);
    }

    #[test]
    fn explicit_ratios_drive_stack_heights() {
        let layout = LayoutProfile {
            left: vec![slot(PanelId::Sessions, false)],
            right: vec![slot(PanelId::Sftp, false), slot(PanelId::Tunnels, false)],
            right_ratios: vec![0.5],
            ..LayoutProfile::default()
        };
        let view = compute_panel_layout(&layout, 1200.0, 500.0, None);
        let sftp = *view.frame(PanelId::Sftp).expect("sftp");
        let tunnels = *view.frame(PanelId::Tunnels).expect("tunnels");
        // available = 500 − 24 留白 − 4 把手 = 472，对半 = 236。
        assert!(approx(sftp.height, 236.0), "sftp = {}", sftp.height);
        assert!(
            approx(tunnels.height, 236.0),
            "tunnels = {}",
            tunnels.height
        );
        assert!(approx(tunnels.y, 12.0 + 236.0 + 4.0));
    }

    #[test]
    fn min_height_is_enforced_by_rebalancing() {
        let layout = LayoutProfile {
            left: vec![slot(PanelId::Sessions, false)],
            right: vec![slot(PanelId::Sftp, false), slot(PanelId::Tunnels, false)],
            right_ratios: vec![0.05],
            ..LayoutProfile::default()
        };
        let view = compute_panel_layout(&layout, 1200.0, 500.0, None);
        let sftp = *view.frame(PanelId::Sftp).expect("sftp");
        let tunnels = *view.frame(PanelId::Tunnels).expect("tunnels");
        assert!(approx(sftp.height, PANEL_MIN_HEIGHT));
        assert!(approx(tunnels.height, 472.0 - PANEL_MIN_HEIGHT));
    }

    #[test]
    fn collapsed_panel_pins_to_32px() {
        let layout = LayoutProfile {
            left: vec![slot(PanelId::Sessions, false)],
            right: vec![slot(PanelId::Sftp, true), slot(PanelId::Tunnels, false)],
            ..LayoutProfile::default()
        };
        let view = compute_panel_layout(&layout, 1200.0, 500.0, None);
        let sftp = *view.frame(PanelId::Sftp).expect("sftp");
        let tunnels = *view.frame(PanelId::Tunnels).expect("tunnels");
        assert!(sftp.collapsed);
        assert!(approx(sftp.height, PANEL_COLLAPSED_HEIGHT));
        assert!(approx(tunnels.y, 12.0 + PANEL_COLLAPSED_HEIGHT + 4.0));
        // 唯一展开的面板吸收全部剩余高度（没有填充面板时等分）。
        assert!(approx(
            tunnels.height,
            500.0 - 12.0 - 12.0 - 4.0 - PANEL_COLLAPSED_HEIGHT
        ));
    }

    #[test]
    fn runtime_view_and_panel_visibility() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut runtime =
            AppRuntime::new_with_keychain(dir.path().to_path_buf(), None).expect("runtime");
        runtime.set_panel_area_size(1440.0, 794.0);
        let view = runtime.panel_layout_view();
        // C0 默认布局：Transfers 不在栈中（默认隐藏），其余四个面板全部可见。
        assert_eq!(view.frames.len(), 4);
        assert!(view.frames.iter().all(|frame| frame.visible));
        assert!(!runtime.panel_visible(PanelId::Transfers));
        let projection = runtime.projection();
        assert!(projection.sessions_visible);
        assert!(projection.sftp_visible);
        assert!(projection.tunnels_visible);
        assert!(projection.commands_visible);
        assert!(!projection.transfers_visible);

        // Panels 菜单显示 / 隐藏（Transfers 按默认侧追加到右栏栈尾）。
        let shown = runtime
            .toggle_panel_visible(PanelId::Transfers)
            .expect("show");
        assert!(shown.transfers_visible);
        assert_eq!(
            runtime.panel_layout().placement(PanelId::Transfers),
            PanelPlacement::Right
        );
        assert!(
            runtime
                .panel_layout_view()
                .frame(PanelId::Transfers)
                .expect("frame")
                .visible
        );
        let hidden = runtime
            .toggle_panel_visible(PanelId::Transfers)
            .expect("hide");
        assert!(!hidden.transfers_visible);
        assert!(!runtime.panel_visible(PanelId::Transfers));
    }

    #[test]
    fn preview_split_is_memory_only_until_commit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut runtime =
            AppRuntime::new_with_keychain(dir.path().to_path_buf(), None).expect("runtime");
        runtime
            .move_panel(PanelId::QuickCommands, PanelSide::Left, None)
            .expect("move");
        let config_path = dir.path().join("config.toml");
        let before = std::fs::read_to_string(&config_path).expect("config before");

        runtime.set_panel_area_size(1200.0, 500.0);
        // 左栈 = [Sessions, QuickCommands]（初始 407/89）；把把手拖到等高（248/248）处。
        runtime.preview_panel_split_pixels(PanelSide::Left, 0, 252.0);
        let after_preview = std::fs::read_to_string(&config_path).expect("config after preview");
        assert_eq!(before, after_preview, "preview 不应落盘");
        let view = runtime.panel_layout_view();
        let sessions = *view.frame(PanelId::Sessions).expect("sessions");
        let commands = *view.frame(PanelId::QuickCommands).expect("commands");
        assert!(
            approx(sessions.height, commands.height),
            "sessions={} commands={}",
            sessions.height,
            commands.height
        );

        runtime.commit_panel_layout().expect("commit");
        let reloaded =
            AppRuntime::new_with_keychain(dir.path().to_path_buf(), None).expect("reload");
        let fractions = reloaded.panel_layout().layout().left_ratios.clone();
        assert_eq!(fractions.len(), 1);
        assert!(approx(fractions[0], 0.5), "fraction = {}", fractions[0]);
    }

    #[test]
    fn toggle_side_expanded_updates_memory_and_persisted_side() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut runtime =
            AppRuntime::new_with_keychain(dir.path().to_path_buf(), None).expect("runtime");
        runtime.set_panel_area_size(1000.0, 600.0);
        assert!(runtime.panel_layout_view().right_collapsed);

        runtime
            .toggle_panel_side_expanded(PanelSide::Right)
            .expect("expand");
        assert!(!runtime.panel_layout_view().right_collapsed);
        assert_eq!(
            runtime.panel_layout().layout().narrow_collapsed_side,
            Some(PanelSide::Left)
        );

        runtime
            .toggle_panel_side_expanded(PanelSide::Right)
            .expect("collapse");
        assert!(runtime.panel_layout_view().right_collapsed);
        assert_eq!(
            runtime.panel_layout().layout().narrow_collapsed_side,
            Some(PanelSide::Right)
        );
    }

    #[test]
    fn panel_area_size_ignores_non_finite_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut runtime =
            AppRuntime::new_with_keychain(dir.path().to_path_buf(), None).expect("runtime");
        runtime.set_panel_area_size(f32::NAN, f32::INFINITY);
        assert_eq!(
            (runtime.panel_area_width, runtime.panel_area_height),
            DEFAULT_PANEL_AREA
        );
    }
}
