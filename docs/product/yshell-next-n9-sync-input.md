# N9 同步发送按键（设计）

状态：**待确认**（含 §1 的 Visible 语义待定）；依赖：N0（标签模型）✅、R2
关联决策：D23（不弹确认，仅状态栏提示 + 一键停止）、D24（作用域当前窗口；Send to Current = 取消；IME 提交后同步；不做暂停键）；需求 §10
文件落位：`runtime/tabs.rs`（`InputSyncState` 与扇出）；`ui/main_window.slint`（终端/标签右键菜单项 + 状态 chip 最小接线）；`ui/components/status_bar.slint`（同步状态）；`ui/components/terminal_workspace.slint`（标签角标）；`bootstrap.rs`（输入汇聚点）；`ui/text_formats.slint`

## 1. 语义与作用域
- 源：一个终端标签；目标：当前窗口内的终端标签集合。
  - `Send to All`：当前窗口所有已连接 SSH 终端（排除源、排除未连接）；
  - `Send to Visible`：**待定** —— 当前无分屏，推荐：暂等同 `All` 并在 UI 标注"分屏后收紧"；备选：等分屏（E-029+）实现后再提供 `Visible`；
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
