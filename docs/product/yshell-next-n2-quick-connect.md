# N2 快速连接页（设计）

状态：**待确认**；依赖：R2（runtime 模块化，接线落位）；时机：R2 后
关联决策：D11（`+` 默认打开快速连接页）、D12（历史默认开、上限 20、可关可清空）、D13（快速链接只存目标）；需求 §3
文件落位：新 `ui/pages/quick_connect.slint`；`ui/main_window.slint`（最小接线）；`ui/components/command_bar.slint`（移除连接输入）；`runtime/tabs.rs` + `runtime/sessions.rs` + `runtime/mod.rs`；`bootstrap.rs`（回调接线）；`translations`

## 1. 模型与运行时
1. `TabKind::QuickConnect`（N0 已预留枚举；扩展 `TabEntry` 显示标题为 `@tr("Quick Connect")`、无会话）。
2. `ui.new_tab_mode`（`quick-connect` 默认 | `session-editor`，C0 已备）：`+` 按钮与 `Ctrl+N` 不冲突——`+` 按设置；`Ctrl+N` 仍直接打开 Session Editor；启动无会话时打开 QC 页；关闭最后一个终端标签后回到 QC 页（无标签则不新建）。
3. 历史：连接成功后记录 `QuickConnectEntry{target,last_used_at,use_count}`（规范化 `[user@]host:port`、去重、上限 20、`enabled=false` 时不记录）；提供 Clear。
4. 快速链接：`QuickLink{id,label,target,sort_order}` 的 pin/移除/复制链接（重命名与排序可后置）。
5. 持久化：即时写回 `config.toml`（历史最多 20 条，写入频率低）；保存失败走状态栏提示，不阻断连接。
6. "保存为会话…"：连接后提供入口，打开 Session Editor 预填 target（复用现有编辑器草稿）。

## 2. 页面组件（新文件，组件段可先行）
`ui/pages/quick_connect.slint`：
- 大输入框（`ssh://user@host:port` 等形式，Enter 直连；非法输入内联提示）；
- "最近连接"列表（Runtime/已保存会话最近使用 + QC 历史，去重按时间倒序；行操作：连接 / pin 为快速链接 / 复制）；
- "快速链接"列表（连接 / 复制 / 移除）；
- 空态与错误态文案（@tr）；
- 回调（透传）：`submit(string)`、`connect-target(string)`、`pin-target(string)`、`remove-link(string)`、`copy-target(string)`、`save-as-session(string)`、`clear-history()`。
组件只接收渲染好的行数据（`[QuickConnectRow]`/`[QuickLinkRow]` 结构体），不内嵌逻辑。

## 3. CommandBar 与快捷键
- 删除 CommandBar 的快速连接输入与"连接"按钮（S1+S2 未动的部分）；
- `Ctrl+O` → 打开/激活 QC 页并聚焦输入框（`focus-quick-connect()` 移植到页面组件）；
- File 菜单 "Quick Connect" 同步为"打开 QC 页"。

## 4. 投影与回调（runtime）
新增（只增）：
- `quick_connect_rows: Vec<QuickConnectRow{ kind(recent|saved|link), title, target, last_used_text }>`、`quick_links_rows`、`quick_connect_history_enabled: bool`、`quick_connect_summary_text`。
- 回调：`open_quick_connect_tab()`、`submit_quick_connect(string)`（复用解析）、`quick_connect_pin(string)`、`quick_link_remove(string)`、`quick_connect_history_clear()`、`new_tab_default()`（`+` 行为）。
- 记录时机：`handle_quick_connect` 成功路径与 saved session 连接成功路径。

## 5. 不做
- 多窗口下的 QC 页行为（先单窗口）；快速链接的拖拽排序/重命名（后置）；QC 页作为"欢迎页"以外的定制。

## 6. 验收
1. 输入 `ssh://root@1.1.1.1:12345` 直连成功且**不写入** sessions 树；历史出现该条（上限/去重/Clear 生效；`enabled=false` 时不记录）。
2. 快速链接 pin/移除/复制；"保存为会话…"预填正确。
3. `+` 行为按 `new_tab_mode` 切换；启动落 QC 页；关闭最后一个终端标签回 QC 页；`Ctrl+O` 聚焦。
4. CommandBar 无连接输入；File 菜单/快捷键一致。
5. `cargo xtask lint/test` 全绿（≥R2 后基数）；e2e 双主题全过；截图 `dist/ui-checks/n2-*`（QC 页/历史/链接/`+` 行为/关闭最后标签/非法输入）。
