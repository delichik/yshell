# N3 工具面板停靠系统 + 绑定环清理（设计）

状态：**待确认**；依赖：R2（`runtime/panels.rs`）、R1（tool_dock 已独立）、S1+S2、N0
关联决策：D8（布局全局一套；Transfers 普通面板）、需求 §4；**同时清理 21 条 binding-loop 告警**
文件落位：重写 `ui/components/tool_dock.slint` → 泛化为左右栏容器；`ui/components/session_sidebar.slint`（作为 Sessions 面板容器保留）；新 `ui/components/panel_frame.slint`（标题/折叠/移动/关闭菜单 + 拖拽把手）；`runtime/panels.rs`；`ui/main_window.slint`（布局组合/断点）；`C0` 的 `UiProfile.layout` 读写

## 1. 模型（runtime/panels.rs）
- `PanelId = Sessions | Sftp | Tunnels | QuickCommands | Transfers`；`PanelPlacement = Left | Right | Hidden`；每侧一个有序面板栈 + 每面板高度比例（`left_ratios/right_ratios`，C0 已备）；栏宽（`left_width/right_width`）。
- 持久化：`UiProfile.layout` 读写（全局一套，所有窗口共享）；自动折叠仅内存态。
- 绑定环清理：把 `nav_rail_active`/`dock_toggle_visible`/`dock_auto_collapsed` 等**断点判定改为 Rust 投影**（`window.width` 变化 → Rust 计算 → 属性下发），或让组件不在布局上依赖这些标志；验收要求告警数 **≤ 3**（目标 0）。

## 2. UI
- `panel_frame.slint`：标题条（图标+标题+`⋯` 菜单：Move to Left/Right、Collapse、Hide）+ 折叠态（28–32px）+ 分栏把手（4px，复用现有 resize 模式）。
- 左右栏容器：渲染各自面板栈；面板间上下分割比例可拖；栏宽可拖（沿用现有夹取与断点逻辑）。
- 移动换边/换序：面板标题菜单 +（Slint 1.18）应用内 `DragArea/DropArea` 拖拽；拖拽期间显示插入位置。
- 面板内容绑定：SFTP/Tunnels 跟随活动标签会话；Transfers/Sessions/QuickCommands 应用级；无会话空态。
- CommandBar 的四个开关收敛为 `Panels` 菜单（View 菜单同步）；`dock_user_expanded` 等窄窗规则泛化为左右两侧。
- 最小可用尺寸与折叠规则沿用现状（左 200–420 / 右 280–560，按边独立记忆）。

## 3. 验收
1. 任一面板可放左/右、上下分栏、拖比例、折叠/隐藏；重启后布局还原（读 C0 配置）。
2. 280px 单栏内容不溢出；窄窗自动折叠规则两侧一致。
3. **binding-loop 告警 ≤3（目标 0）**；无 deprecated 告警。
4. `cargo xtask lint/test` 全绿；e2e 双主题；截图 `dist/ui-checks/n3-*`（移动面板/分割/折叠/持久化/窄窗）。
