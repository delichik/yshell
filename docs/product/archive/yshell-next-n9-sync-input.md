# N9 同步发送按键（设计）

> **归档**：N9 已验收（2026-09-28）。现行缺口（Visible 仍等同 All）→ [`yshell-current-status.md`](./yshell-current-status.md) §3。

状态：**已完成（验收通过）**；§1 Visible 收紧仍属遗留。依赖：N0 ✅、R2 ✅  
关联决策：D23、D24；需求 §10  
文件落位：`runtime/tabs.rs`；`ui/main_window.slint`；`status_bar.slint`；`terminal_workspace.slint`；`bootstrap.rs`；`text_formats.slint`

## 1. 语义与作用域
- 源：一个终端标签；目标：当前窗口内的终端标签集合。
  - `Send to All`：当前窗口所有已连接 SSH 终端（排除源、排除未连接）；
  - `Send to Visible`：设计曾「分屏前等同 All」；**分屏已落地，实现尚未按可见 pane 过滤**（已知缺口）；
  - `Send to Current`：取消同步（只保留当前终端，退出同步态）。
- 标签级 `receives_key_input`：右键勾选/取消"接收键输入"；断开自动移出并提示。

## 2. 运行时
- `InputSyncState { source: Option<TabId>, targets: BTreeSet<TabId>, mode }`（运行时会话态，不持久化；可选记住上次目标集合）。
- 扇出：在所有输入路径的**唯一汇聚点**（`send_active_terminal_key/input/bytes`）处，源发送成功后同步写入目标（顺序一致；目标只写输入、其回显不再回灌）。
- 不变量：源必须在 targets 之外；源关闭/断开、窗口关闭 → 停止同步；目标断线自动移除。

## 3. UI 与安全
- 入口：终端右键 `Send Key Input To…` / `Stop Send Key Input To`；标签右键勾选"接收"；状态栏 chip `SYNC → n targets` + 一键停止。
- 不弹确认（D23）；`Ctrl+C` 等控制键广播时给出一次性非阻塞提示（toast/状态栏闪烁文案）。
- 可键盘操作：`Stop` 入口在 chip 上可点。

## 4. 验收
1. 源输入 `echo hi` → 全部目标得到同样输入与输出；停止后不再复制；目标断开自动移除；源标签关闭自动停止。
2. chip/角标与实际状态一致；无"幽灵同步"（错误态可见）。
3. IME 组合在提交后才同步（组合中不发送）。
4. `cargo xtask lint/test` 全绿；e2e 双主题（Fake 场景可跑）；截图 `dist/ui-checks/n9-*`（目标选择/chip/角标/停止/断开移除）。
