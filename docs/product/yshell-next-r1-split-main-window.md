# R1 `main_window.slint` 拆分（结构重构）

状态：**已完成（验收通过 2026-09-27）**；owner `ses_f1d6684ebffemBTJ0xVHtw8n9x`
背景：`main_window.slint` 2376 行 / 125KB；T0、S1+S2、N0、N2、N3 全都要改它，"单写者串行"已成为吞吐瓶颈；且 Slint 每次改动整文件重编译。
目标：按职责拆出可独立演进的组件；`main_window` 只保留窗口状态、回调、快捷键、菜单模型与组合布局。
非目标：**行为与视觉零变化**（纯重构）；不引入新功能；不解决 binding-loop（N3）；不改文案。

## 1. 拆分边界（提案）

| 新组件 | 内容 | 关键接口（示意） |
|---|---|---|
| `components/menu_bar.slint` | `MenuTitle` + 5 个一级菜单（含 MenuPopup 实例与悬停切换） | in: `open-index`、锚点、各菜单 entries；out: `activated(index)`、`hovered(index)`、`request-close` |
| `components/command_bar.slint` | New Session、面板开关、查找/设置/更多 | in: 布尔态与输入值；out: 各动作回调 |
| `components/session_sidebar.slint` | 左栏全量：头部、搜索、SessionTree、空白区菜单、图标条、收起按钮 | in: rows/selected/搜索词/显隐/宽度/rail 态；out: select/activate/toggle/context-menu/blank-menu/new-session/new-folder/refresh/collapse |
| `components/terminal_workspace.slint` | 标签条（当前单标签占位，N0 重写其内部）+ `TerminalView` + 内边距 | in: tab 文案/终端投影/查找态；out: 终端与查找回调、tab 回调 |
| `components/tool_dock.slint` | 右栏容器 + SFTP / Tunnels / Transfers / Quick Commands 卡片 | in: 显隐/宽度/各面板投影；out: 面板回调透传 |
| `components/status_bar.slint` | 状态栏（替换现有 11 行死壳） | in: 状态文案/连接信息；out: 现有上下文菜单等入口 |
| 保留在 `main_window` | Window 属性块、display 函数、快捷键 `dispatch-global-shortcut`、菜单模型数组、全部弹窗实例（Confirm/HostKey/Password/About/Settings/SessionEditor/KnownHosts/SFTP 弹窗/Quit/Delete）与 scrim 层 | — |

补充：
- 左/右栏宽度拖拽把手随对应栏组件搬移；拖拽状态仍在 `main_window`，用属性/回调传递。
- 菜单/右键菜单锚点沿用"回调传 x/y"的现有模式（`absolute-position` 在组件内取）。
- 跨组件输入框用 `in-out property` + `<=>` 双向绑定。
- `Theme` 等 globals 在各文件 import 使用，不新建全局状态（避免每实例隔离带来的不一致）。

## 2. 死组件清理（需先证实无引用）

候选：`terminal_tab_bar.slint`、`toolbar.slint`、旧 `status_bar.slint`（由真实版替换）、`quick_commands.slint`、`tunnel_panel.slint`（被 `tool_dock` 内容取代时）。清理前先 `grep` 全仓引用与测试引用，确认无引用再删；有引用则保留待后续。

## 3. 约束与风险

- **零行为/零视觉变化**：不顺手改布局、间距、禁用逻辑、文案；`@tr` 原样搬移（msgid 不变）。
- 快捷键 `FocusScope` 仍由 `main_window` 持有；子组件只上报事件。
- 弹窗暂不搬（降低风险）；若 main_window 仍偏大，后续再单独抽 `dialog_host`。
- 验收需要 before/after 截图逐张对比；Slint 布局可能因组件化产生 1px 级别差异，须逐项解释或修正到一致。

## 4. 验收

1. `cargo xtask lint` + `cargo xtask test` 全绿（数量 ≥ 当前 371）；`ui_token_discipline` 通过。
2. e2e：dark/zh-CN 与 light/en-US 全过。
3. before/after 截图对比（复用 T0 的对比方法）：除光标闪烁/时间类噪声外应零差异；每处差异给出解释。
4. 体积目标：`main_window.slint` ≤ 1500 行；每个新组件 ≤ 450 行（超出需说明）。
5. 死组件确认无引用后删除，并在交付报告列出引用检查命令与结果。
6. 文档更新：单写者规则改为"`main_window` 与 6 个新组件各自单写者"，后续任务按文件切分并行。

## 5. 顺序与影响

- 排期：**S1+S2 完成后 → R1 → N0**（N0 将改 `terminal_workspace` 内部与少量 main_window，拆分后冲突面显著变小）。
- R1 期间占用：`ui/main_window.slint` + 新增组件文件；其它任务不并行改这些文件。
- R1 完成后：N0/N2/N3 可分别落在不同组件文件上，UI 并行度提高一个档位。

## 6. 已确认（2026-09-27）

| # | 问题 | 结论 |
|---|---|---|
| Q1 | 拆分时机 | **S1+S2 完成后、N0 之前**（R1 → N0） |
| Q2 | 拆分粒度 | **按 §1 提案的 6 个区段组件** |
| Q3 | 死组件清理 | **本次一并清理**（先 grep 证实无引用） |

## 7. 验收记录（2026-09-27，主 agent）

| 项 | 结果 |
|---|---|
| main_window 行数 | 2376 → **1423**（≤1500 ✅） |
| 新组件 | menu_bar 193 / command_bar 146 / session_sidebar 313 / terminal_workspace 147 / tool_dock 276 / status_bar 31 / text_formats 333（显示串函数库，第 7 个文件） |
| 门禁 | lint ✅；`cargo xtask test` **373 passed / 0 failed** |
| e2e | dark/zh-CN 34/34、light/en-US 34/34 |
| 像素对比 | 23/32 全等；9 张仅终端时间噪声；**非终端区域 32/32 AE=0** |
| 死组件 | terminal_tab_bar / toolbar / quick_commands / tunnel_panel / sftp_operation_dialog 已删（grep 证据见 owner 报告） |
| 偏离（已接受） | ① binding-loop 18→21（环未新增，仅组件路径节点；根因消除归 **N3**）；② 新增 `text_formats.slint` 以满足行数目标；③ main_window 保留 5 个无渲染方显示串属性（i18n 键集合对齐，清理归 **L1**）。 |

## 8. 单写者规则（R1 后生效）

以下文件各自"任何时刻一个 owner"，不同文件可并行：
`ui/main_window.slint`、`ui/text_formats.slint`、`ui/theme.slint`、`ui/components/{menu_bar,command_bar,session_sidebar,terminal_workspace,tool_dock,status_bar}.slint`。

后续 UI 任务的落位约定：
- N0 → `terminal_workspace` + `main_window`（当前 owner）；
- N2 → `command_bar` + `terminal_workspace` + `main_window`（N0 完成后）；
- N3 → `tool_dock` + `session_sidebar` + `main_window`；
- N5a-UI → `terminal_view` + `yshell-terminal` + `bootstrap`（与 N0 协调后）。
