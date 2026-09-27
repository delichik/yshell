# 并行执行重构计划（R2 + 组件优先并行）

状态：**待用户确认**（R2 时机、组件优先策略）
背景：用户质疑"为什么没有设计、为什么不能并行"。复盘结论：

1. **设计滞后是我的流程问题**：此前按波次"到点才写"，应改为**一次性把剩余设计全部写出来**（上游依赖 C0/F0/A0/G0/N5a/R1 都已交付，剩余设计不再受上游阻塞）。
2. **并行受限是事实**：单一工作树、无分支合并流程，`crates/yshell-app/src/runtime.rs`（~390KB）与 `ui/main_window.slint` 是全局热文件，两个 owner 同时改必然互相覆盖。UI 已通过 R1 组件化缓解，但 runtime 仍是瓶颈。

## 一、结构解法

### R2：`runtime.rs` 模块化拆分（对标 R1）
- 目标：把 `runtime.rs` 机械拆分为 `runtime/` 子模块，按领域划分文件，**行为零变化**：
  `mod.rs`（结构体/字段与 re-export）、`tabs.rs`、`sessions.rs`、`connection.rs`、`sftp.rs`、`transfers.rs`、`logging.rs`、`auth.rs`、`editor.rs`、`panels.rs`、`projection.rs`、`menus.rs`、`shortcuts.rs`、`tests/` 等（最终边界由设计定）。
- 约束：纯搬迁（函数签名/调用点不变）；`pub(crate)`/`pub` 可见性按最小改动；每步编译通过；验收 = 全门禁 + e2e 双主题 + 截图无差异 + 测试数不降。
- 收益：后续任务按模块落位，可 3–5 条并行（每个任务"组件/模块 + 自己的接线文件"）。
- 成本：一次高风险机械重构（~4000 行），建议独占一个短窗口。

### 组件优先并行（所有 UI 任务统一策略）
- 每个任务拆成两段：
  1. **组件/模块段（可并行）**：新页面/弹窗放在**新文件**；本 crate/本模块的逻辑（如 `yshell-config`、`yshell-logging`、`yshell-terminal`、`sftp_panel.slint`、`tool_dock.slint`）独立完成并自验。
  2. **接线段（串行短窗口）**：只改 `main_window.slint`/`runtime` 对应模块的最小切片（回调透传、菜单项、状态接线），由该任务 owner 在排到的窗口里完成。
- 排队规则：**接线段同一时刻只有 1 个 shared-file 持有者**（当前 N2 持有 `main_window`/`bootstrap`/`projection`/`tabs`/`sessions` 等）；其它任务做"模块段/组件段"（自域文件 + 新文件）并行；模块段完成后在接线段排队。
- 当前并行（2026-09-28）：**N2 Phase 2（接线段）∥ N4 模块段 ∥ N6 模块段**；N3/N5/N1/N9/N7 待接线段让出后依次进入。
- **交付规则（2026-09-27 追加）**：owner 必须冻结本次验收用的二进制（`dist/<task>-work/yshell-<task>-verified` + sha256），验收一律用冻结副本——并行构建会覆盖 `target/debug/yshell`（L2 验收时已踩过一次）。
- **图片预算（2026-09-28 追加）**：子会话最多读 ≤5 张图片；证据用文件路径 + ImageMagick 裁剪；报告不嵌入图片。（原 N4 owner 因上下文累计 31 张图片超限报错，已由新会话接管。）

## 二、设计批（一次性写齐，按此顺序）
**状态：设计批已完成（2026-09-27）**，文档如下：
1. `yshell-next-r2-runtime-split.md`（R2）
2. `yshell-next-n2-quick-connect.md`（N2）
3. `yshell-next-rfd-spike.md`（rfd 选型）
4. `yshell-next-n4-keys-auth.md`（N4）
5. `yshell-next-n1-sftp.md`（N1）
6. `yshell-next-n6-logging-ui.md`（N6）
7. `yshell-next-n3-panels.md`（N3）
8. `yshell-next-n5-terminal-theme.md`（N5）
9. `yshell-next-n9-sync-input.md`（N9）
10. `yshell-next-n7-multi-window.md`（N7）
**rfd spike：已完成**（报告 `yshell-next-rfd-spike-report.md`）：选 `rfd 0.17.2` + **GTK3 透传 feature**（`dialog-gtk3 = ["rfd/gtk3"]`）、调用必须走 worker 线程 + `invoke_from_event_loop`（UI 线程直调会冻结）、系统依赖/打包/降级触发条件见报告；**D0b 已派发**（工作区依赖入库，暂不加 features）。
**待决**：N9 的 `Visible` 语义（无分屏时暂等同 All，或等分屏后再做）；L1 待方案文档定位；L4（fmt 冻结窗口）计划排在 **R2 之后、N2 接线段之前**。

## 三、确认后立即并行的候选（互不碰热文件）
- rfd spike（临时工程 + 报告，零仓库冲突）
- N2 组件段：`quick_connect` 页面新文件 + `yshell-config` 接线
- N4 组件段：密钥管理页面 + 认证弹窗新文件 + config keys 解析
- N5 组件段：Folder Editor 新文件
- N6 组件段：日志设置弹窗新文件
- N3 组件段：`tool_dock.slint` 面板栈
- N1 组件段：`sftp_panel` 双栏/紧凑化
（每个都需先确认其设计；实现阶段按"组件段并行、接线段排队"执行）

## 四、待确认
| # | 问题 | 建议 |
|---|---|---|
| Q1 | 是否执行 R2（runtime 模块化）？时机？ | 执行；**L2 完成后立刻**，先于 N2（N2/N4/N6 等都受益） |
| Q2 | 组件优先并行策略是否现在启用？ | 启用：设计批写完 → 确认 → 组件段立即并行 |
