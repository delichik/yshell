# N0 多标签模型 + N8 批量关闭（设计）

状态：**已完成（验收通过 2026-09-27）**；owner `ses_f1d23e1f8ffexVR71k4oVIk3ML`
验收记录：392 测试（+19 新增）、e2e 34/34×2 + 标签 25/25×2、10 标签性能抽查通过（~28% 单核 / ~90MB）；偏差：`real.rs::write_input` 非阻塞修复（必要，含 live 回归）、`+` 钉在标签条右侧、新增 `confirm/cancel_close_tabs` 回调。
依赖：T0（已验证通过）、C0（`[ui]` 配置与 folder 结构已就绪）
设计/验收：主 agent；执行：唯一 owner
关联：`yshell-next-features-requirements.md` N0/N8（§1、§14）、`context-menu-design.md` §3
已确认决策：**N8 并入 N0**；标签溢出**横向滚动**；**允许**同一已保存会话重复打开。

## 1. 目标 / 非目标

**目标**
- 真实多标签：多个终端标签并存、任意切换、关闭（单关 + 批量）。
- 每个标签持有独立 `SessionRuntime`，后台标签持续接收输出。
- 标签条右键菜单：`Close`、`Close Others`、`Close Tabs to Left`、`Close Tabs to Right`、`Close All`、`Close Disconnected Tabs`（启用条件见 §7）。
- 关闭活动标签后的选择语义、投影顺序稳定。

**非目标**
- 分屏 pane（E-029~E-038）、标签拖拽/多窗口（N7）、快速连接页（N2，届时扩 `TabKind`）、未读徽标与重命名（后续）、标签状态持久化（不落配置）。

## 2. 现状

- 运行时已有 `sessions: BTreeMap<String, SessionRuntime>` 与 `active_session_id`；`SessionRuntime` 自带 `session_id`/`tab_id`/`display_name`/`state`/`shell_session`/`terminal_grid`。
- UI 只有一个固定标签；投影里 `terminal_image`、`terminal_visible_lines`、`tab_text` 等均只描述活动会话。
- 120ms Timer 只轮询活动会话（`poll_active_terminal_output_passive`）。
- 打开会话（Quick Connect / Saved / Draft）都会直接设为 active。

## 3. 运行时模型

新增：

```rust
enum TabKind { Terminal { session_id: String } }   // N2 再加 QuickConnect
struct TabEntry { tab_id: String, kind: TabKind, unread: u32 /*V1 恒 0*/ }
tabs: Vec<TabEntry>,        // 顺序 = 显示顺序
active_tab_id: Option<String>,
```

- **不变量**：`active_tab_id` 要么为 `None`（无标签），要么存在于 `tabs`；`active_session_id` 与活动标签指向同一会话。
- **打开**：创建 `SessionRuntime`（现有路径）→ `tabs.push` → 激活；**允许同一 saved session 重复打开**（重复连接）。
- **切换**：`activate_tab(tab_id)` → 更新 `active_tab_id` 与 `active_session_id`；`unread` 清零（V1 占位）。
- **关闭（单个）**：`close_tab(tab_id)`：
  - 该会话 `connected/connecting` → UI 弹确认（E-057，默认焦点安全按钮）；
  - 确认后：断开并移除 `SessionRuntime`（沿用现有清理语义，含 SFTP/日志/远程编辑），移除 `TabEntry`；
  - 关的是活动标签：优先选右侧相邻，否则左侧；无标签则空态（N2 后回快速连接页）。
- **批量关闭**（N8 并入）：`close_tabs(scope)`，`scope ∈ {Others(tab_id), LeftOf(tab_id), RightOf(tab_id), All, Disconnected}`：
  - 先计算目标集合；有活动连接则弹一次确认（显示"将关闭 N 个标签，其中 M 个活动连接"）；
  - 按右→左的顺序逐个关闭目标，保持剩余顺序稳定；关闭完如活动标签被关，按 §3 单关规则选相邻。

## 4. 投影

`AppProjection` 新增（只增不减）：

```rust
struct TabData { id: String, title: String, state_text: String, connected: bool, active: bool }
tabs: Vec<TabData>,
active_tab_id: String,
tab_count: i32,
```

- 现有活动会话投影（`terminal_image`/`terminal_visible_lines`/`terminal_*`、`tab_text` 等）语义不变：始终描述**活动标签**，把 UI 改动面压到标签条一处。
- 新回调：`activate_tab(string)`、`close_tab(string)`、`close_tabs(string)`（scope 编码）、`tab_context_menu_requested(string, length, length)`。

## 5. UI（`ui/main_window.slint`）

- 标签条改为 repeater，并放入**横向可滚动容器**（Flickable/ScrollView 横向；滚轮横向或按住拖动，见验收）：

```slint
for tab in root.tabs : WinTab {
    text: tab.title + "  " + tab.state_text;
    active: tab.active;
    clicked => { root.activate_tab(tab.id); }
    context-menu => { root.tab_context_menu_requested(tab.id, x, y); }
}
```

- 标签右键菜单（完整项，按 §7 启用条件；disabled 必须带 tooltip 原因）。
- 关闭确认复用 `ConfirmDialog`：单关显示会话名；批量显示数量与活动连接数。
- `+` 暂保持现状（N2 改为打开快速连接页）。

## 6. 后台轮询

- Timer 改为 `poll_all_terminal_outputs()`：遍历所有 `sessions` 收输出；活动标签优先，其余按游标轮转，单 tick 上限（默认 8 个会话）避免标签过多时卡顿。
- 只有活动标签触发全量投影与位图渲染；非活动标签仅更新 grid/parser（已有能力）。
- `frame_id` 继续用于判断重绘；「有输出但非活动」只累计（V1 不做未读角标）。

## 7. 菜单启用条件（对照 `context-menu-design.md` §3）

| 项 | 启用条件 |
|---|---|
| Close | 恒可用 |
| Close Others | 标签数 > 1 |
| Close Tabs to Left | 左侧有标签 |
| Close Tabs to Right | 右侧有标签 |
| Close All | 标签数 > 0 |
| Close Disconnected Tabs | 存在 disconnected/error 标签 |

- 关闭带活动连接：单关/批量均有确认；纯已断开标签直接关。
- 关闭标签联动日志/远程编辑/传输的现有清理路径，不引入新语义。

## 8. 测试与验收

- 单测：打开/激活/关闭/关闭活动后的相邻选择；重复打开同名会话；轮询游标与活动优先；投影顺序与 `active` 标记；`active_tab_id` 不变量；批量 scope 的集合计算（Others/Left/Right/All/Disconnected）与"跳过已关标签"的顺序稳定性。
- 门禁：`cargo xtask lint` + `cargo xtask test` 全绿（数量不低于当前 371）。
- e2e/截图（`dist/ui-checks/`，`n0-` 前缀）：
  1. 打开两个标签 → 切换 → 后台标签持续输出（切回内容在）；
  2. 标签条右键菜单全项（含 disabled 状态与 tooltip）；
  3. 批量关闭确认弹窗（数量/活动连接数）；
  4. 关闭活动标签后的相邻选择；
  5. 标签横向滚动（开 8+ 个标签）。
- 性能：10 个标签同时输出时无卡顿（CPU 观察 + 操作可响应即可）。
