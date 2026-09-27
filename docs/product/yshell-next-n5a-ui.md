# N5a-UI 终端渲染接线（scale_factor + 1:1 显示）

状态：**已完成（验收通过 2026-09-28）**；owner `ses_f1e2e0941ffdGGtVQywUnI1xNF`
验收：1.0/1.25/2.0 三档"应用截图行与渲染器直出帧**逐字节一致**、字距漂移 0px、选区 bbox 精确"；394 测试；e2e 34/34×2。
偏离：ceil 网格（最右/下保留半格、不丢内容）、位图左上锚定、scale 经 `phx` 解析（未加 TerminalView 属性）。
范围：`crates/yshell-app/src/bootstrap.rs`（TerminalSurface / 网格与 resize）、`ui/components/terminal_view.slint`（显示与坐标）；不改 `crates/yshell-terminal` 内部、不动其它 UI 组件。
并行：与 **L2** 同时进行（L2 改 dialogs / `main_window` / `session_sidebar`；本任务改 `bootstrap` / `terminal_view`，文件零重叠）。

## 背景
N5a-B 已把终端光栅器换成 swash + CJK；遗留 B5：位图按 **1x 逻辑像素**光栅，Slint 在非 1.0 缩放时以最近邻放大（1.25 → 字距 12/13px 交替；2.0 → 方块感）。当前用户环境 scale=1.0，所以以前不明显；这次把接线补上。

## 范围
1. **取缩放**：`window.window().scale_factor()`；`TerminalRenderer::with_scale_factor(16.0, scale)`；`scale` 变化时 `set_scale_factor`（重建 cell 度量 + 清字形缓存）。
2. **网格与 PTY**：`columns = floor(viewport_logical_px * scale / cell_物理宽)`、`rows` 同理；PTY size 用该网格；resize 触发沿用现有路径。
3. **1:1 显示（禁拉伸，硬性）**：`terminal_view.slint` 中终端图像必须以**精确像素尺寸**显示（1 图像像素 = 1 设备像素），**删除 `image-fit: fill` 的拉伸**。网格建议改 `ceil`（位图 ≥ 视口、贴 `(0,0)`、父级裁剪多余半格），或 `floor` + 位图贴角显示、余数留白；鼠标/选区坐标以位图原点换算。
   背景：现有 `fill` 把 800×684 位图拉伸到 ~807×692 视口（0.9–1.1% 非整数重采样），是终端字发虚的直接来源（渲染器直出帧清晰、应用截图发虚）。
4. **坐标换算**：鼠标/选区/查找高亮映射到网格时统一走物理像素（`terminal-viewport-*-px` 改为物理像素或在使用处乘 scale）。
5. **scale 变化监听**：Slint `Window.scale-factor` 变化 → 重建渲染器、重算网格、resize PTY、清缓存；窗口初次显示前也要用最终 scale 初始化。

## 验收
1. `cargo xtask lint` + `cargo xtask test` 全绿（≥392）。
2. **scale=1.0 回归**：截图与现状一致（主窗口/终端内容；可用现有 e2e 截图对比）。
3. **scale=1.25 / 2.0**：`WINIT_X11_SCALE_FACTOR=1.25|2.0` 截图：字距均匀（无 12/13px 交替）、无像素块放大、终端网格与窗口对齐；鼠标点选/拖选坐标正确（抽测）。
4. e2e dark/zh-CN 与 light/en-US 全过；截图 `dist/ui-checks/n5aui-*`（含 1.0 / 1.25 / 2.0 三组）。
5. 不动 `yshell-terminal` 内部与其它组件；告警数 ≤21。

## 风险
- 逻辑尺寸与物理尺寸混用导致 1px 抖动：统一"位图 = 物理像素、Slint 逻辑尺寸 = 物理/scale"。
- 高 DPI 下窗口尺寸/断点仍按逻辑像素（现有布局不变），只改终端与网格。
