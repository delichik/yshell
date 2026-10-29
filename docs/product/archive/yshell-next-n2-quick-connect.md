# N2 快速连接页（设计）

> **归档**：已验收。`new_tab_mode` 实现漂移见 [`yshell-current-status.md`](./yshell-current-status.md) §3。

状态：**已完成（归档）**（快速链接与"保存为会话…"入口已下线，见下方"变更记录"）；依赖：R2
关联决策：D11（`+` 默认打开快速连接页）、D12（历史默认开、上限 20、可关可清空）；需求 §3
文件落位：新 `ui/pages/quick_connect.slint`；`ui/main_window.slint`（最小接线）；`ui/components/command_bar.slint`（移除连接输入）；`runtime/tabs.rs` + `runtime/sessions.rs` + `runtime/mod.rs`；`bootstrap.rs`（回调接线）；`translations`

## 1. 模型与运行时
1. `TabKind::QuickConnect`（N0 已预留枚举；扩展 `TabEntry` 显示标题为 `@tr("Quick Connect")`、无会话）。
2. `ui.new_tab_mode`（`quick-connect` 默认 | `session-editor`，C0 已备）：`+` 按钮与 `Ctrl+N` 不冲突——`+` 按设置；`Ctrl+N` 仍直接打开 Session Editor；启动无会话时打开 QC 页；关闭最后一个终端标签后回到 QC 页（无标签则不新建）。
3. 历史：连接成功后记录 `QuickConnectEntry{target,last_used_at,use_count}`（规范化 `[user@]host:port`、去重、上限 20、`enabled=false` 时不记录）；提供 Clear。
4. 持久化：即时写回 `config.toml`（历史最多 20 条，写入频率低）；保存失败走状态栏提示，不阻断连接。
5. "最近使用"是相对时间文案（Rust 侧拼装）：必须随时间刷新，否则一直停在 `just now`；
   `quick_connect_relative_time_due()` 给出下一次刷新时刻，由应用级 tick 到期触发重新投影。

## 2. 页面组件（新文件，组件段可先行）
`ui/pages/quick_connect.slint`：
- 大输入框（`ssh://user@host:port` 等形式，Enter 直连；非法输入内联提示）；
- "最近连接"列表（Runtime/已保存会话最近使用 + QC 历史，去重按时间倒序；行操作：连接 / 复制）；
- 空态与错误态文案（@tr）；
- 回调（透传）：`submit(string)`、`connect-target(string)`、`copy-target(string)`、`clear-history()`。
组件只接收渲染好的行数据（`[QuickConnectRow]` 结构体），不内嵌逻辑。

## 3. CommandBar 与快捷键
- 删除 CommandBar 的快速连接输入与"连接"按钮（S1+S2 未动的部分）；
- `Ctrl+O` → 打开/激活 QC 页并聚焦输入框（`focus-quick-connect()` 移植到页面组件）；
- File 菜单 "Quick Connect" 同步为"打开 QC 页"。

## 4. 投影与回调（runtime）
新增（只增）：
- `quick_connect_rows: Vec<QuickConnectRow{ kind(recent|saved), title, target, last_used_text }>`、`quick_connect_history_enabled: bool`、`quick_connect_summary_text`。
- 回调：`open_quick_connect_tab()`、`submit_quick_connect(string)`（复用解析）、`quick_connect_history_clear()`、`new_tab_default()`（`+` 行为）。
- 记录时机：`handle_quick_connect` 成功路径与 saved session 连接成功路径。

## 5. 不做
- 多窗口下的 QC 页行为（先单窗口）；QC 页作为"欢迎页"以外的定制。

## 6. 验收
1. 输入 `ssh://root@1.1.1.1:12345` 直连成功且**不写入** sessions 树；历史出现该条（上限/去重/Clear 生效；`enabled=false` 时不记录）。
2. "最近使用"的相对时间随时间刷新（`just now` → `1 minute ago` → …），不需要用户交互触发。
3. `+` 行为按 `new_tab_mode` 切换；启动落 QC 页；关闭最后一个终端标签回 QC 页；`Ctrl+O` 聚焦。
4. CommandBar 无连接输入；File 菜单/快捷键一致。
5. `cargo xtask lint/test` 全绿（≥R2 后基数）；e2e 双主题全过；截图 `dist/ui-checks/n2-*`（QC 页/历史/`+` 行为/关闭最后标签/非法输入）。

## 7. 变更记录
- 快速链接（`QuickLink` / `quick_links` 配置 / pin 与移除按钮）整块下线：功能价值不足，
  相关配置字段、投影、回调与文案一并删除（旧配置里的 `[[quick_links]]` 直接忽略）。
- 连接成功后的"Connected to …"卡片（含"保存为会话…"入口）下线：直连后 New Tab 已原地
  变成终端标签，这段文案没有信息量；对应的状态字段与编辑器预填入口一并删除。
- "最近使用"改为随时间自动刷新（见 §1.5）。
