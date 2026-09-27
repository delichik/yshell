# N7 多窗口（设计，探索性）

状态：**待确认**；依赖：R2、N0；真·拖出等 winit 0.31 正式版 + Slint 跟进版本
关联决策：D5（首版只做右键"移动到新窗口"）、D6（不做新会话自动开窗）、D7（关闭窗口=确认后断开）、D8（布局全局一套）；需求 §8
文件落位：新 `crates/yshell-app/src/windows.rs`（WindowManager）；`bootstrap.rs`（`run()` → `show()` + `run_event_loop()`；按窗口 wiring）；`runtime/tabs.rs`（标签→窗口归属）；`ui/main_window.slint`（窗口级最小接线：Move to New Window 菜单项）

## 1. 阶段 1（本任务范围）
- `WindowManager`：持有 `Vec<WindowEntry>`（`MainWindow` 实例 + 该窗口标签集合/活动标签/布局投影）；`AppRuntime` 单实例共享（单线程 UI）。
- 生命周期：`bootstrap` 改为 `show()` + `run_event_loop()`；最后一个窗口关闭 = 退出（沿用 Quit 确认）；关闭带活动连接的窗口 → 确认后断开（D7）。
- 标签迁移：`Move to New Window` / `Move to Main Window`（右键菜单）；迁移仅换 UI 所有权，`SessionRuntime`（shell/grid/日志/日志文件句柄）**不重建**；迁入后按新视口触发一次 resize。
- 刷新：单一应用级 Timer 遍历所有窗口/脏标签（沿用 N0 的 `poll_all_terminal_outputs`，按 window 归属投影）。
- 主题/字体：新窗口创建时应用当前主题/强调色/UI 字体；主题变更广播全部窗口。
- 布局：全局一套（D8）；每窗口记录内存态（尺寸/位置）。
- 明确不做：跨窗口拖拽（等 winit 0.31）；多线程窗口；"新会话自动开窗"（D6）。

## 2. 验收
1. `Move to New Window`：终端画面/回滚/运行中命令状态不变；原窗口标签消失且不影响其它标签；新窗口可继续输入/输出/resize。
2. 两窗口并行：输入、输出、resize、日志各自正确；关闭一个窗口不影响另一个；关闭带连接窗口有确认并断开。
3. 主题广播、UI 字体、面板显隐在两个窗口一致。
4. `cargo xtask lint/test` 全绿；e2e 双主题；截图 `dist/ui-checks/n7-*`（迁移前后/双窗口/关闭确认）。
5. 跟踪项：winit 0.31 正式版 → OS 拖放与真·拖出（另任务）。
