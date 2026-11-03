# 原型 vs 实现差距审计（2026-11-03）

> 只读审计报告。基准 = `docs/prototypes/`（现行唯一 UI 真源）；参考实现 = `ui/` 与 `crates/yshell-app/src/`。
> 行号是 2026-11-03 上午快照；`main_window.slint` / `settings.slint` / `dialog_windows.rs` / `bootstrap.rs` 正被并行任务改动，相关行号可能漂移，引用时以函数/注释关键字复核。
> 本次不修改任何 `ui/`、`crates/`、`docs/product/` 文件。

## 0. 总览

### 0.1 统计

| 状态 | 数量 | 说明 |
|---|---:|---|
| 已实现（含等价/超集实现） | 33 | 与原型结构或行为可用 |
| 部分实现 | 21 | 主功能在，缺原型要求的一部分 |
| 缺失 | 24 | 原型有、实现没有 |
| 与原型相反 / 规范冲突 | 9 | 实现了但方向与原型或硬性规则相反 |
| 本轮实施中（未计入缺口） | 4 | Settings Appearance / Terminal / Security、主密码 set/change/remove/reset + 启动解锁 |
| **审计项合计** | **91** | 计入缺口 54 项 |

优先级只落在缺口行上：**P0 = 5 · P1 = 36 · P2 = 13**。

### 0.2 本轮已覆盖范围（不计入缺口）

- **Settings → Appearance / Terminal / Security** 三个分区，以及 **master-password（Set/Change/Remove/Reset）**、**master-password-unlock（启动解锁）**：由本轮并行任务实现中，相关 `settings.slint` / `settings_window.slint` / `dialog_windows.rs` 行号仍在变动，本报告只登记「实施中」，不列缺口。
- 已完成的近似实现：ToolDock 双栈换边/折叠/隐藏、分屏每叶 TabBar、终端查找栏、N9 发送核心 + 状态栏 chip、SFTP 传输队列、日志窗、About、SFTP 冲突/属性窗、MCP consent、三档弹窗尺寸 token。

### 0.3 建议的下一批垂直切片（3–5 个，均可独立验收）

1. **切片 A（P0 收口）**：会话编辑器补 `Login Script` 分区 + `login-step-keys/wait` 子窗；`sessions/tab/SFTP` 改回**就地输入**（去掉 NewFolderDialog / SftpEntryDialog 的改名/新建路径）；`host-key` 窗改回 compact 560×400。
2. **切片 B（主壳导航）**：补 `Tools` 一级菜单与 `Session` 菜单的 Connect / Duplicate Tab / Open SFTP / Tunnels 入口；补 `Ctrl+Shift+S`、`F11`、Focus Mode；菜单项与快捷键一并回归。
3. **切片 C（SFTP 收口）**：独立 `sftp-chmod` compact 窗（八进制 + 9 勾选联动）；Tunnels 面板 `+` → `tunnel-editor` standard 窗；Transfers 批量 Pause All/Resume All；Settings → SFTP 三字段（Show hidden / Conflict / External editor）。
4. **切片 D（终端体验）**：`Find Across SSH` 侧栏；N9 同步 banner（预设下拉 + Pause/Stop）与 pane 头 Receive 开关；REC 底部提示条；Clear Scrollback 改窗内确认。
5. **切片 E（管理器规范收口）**：私钥 `Generate` 流程；管理器工具条改图标钮（正文只进 title）、行级动作改右键菜单；Rust 侧窗口标题/确认文案进 i18n；删除无入口的遗留 KnownHosts 页与 `CLEAR` 打字确认。

---

## 1. 主壳 / 全局（12 项）

| 原型要求 | 现状 | 证据（文件:行） | 优先级 |
|---|---|---|---|
| 顶栏 6 个一级菜单：File / Edit / Session / View / **Tools** / Help | 部分：只有 5 个，缺 Tools；管理器入口折进 Session | `ui/components/menu_bar.slint:150-189`；`docs/prototypes/main-shell/index.html:687-691` | P0 |
| 菜单栏 Import/Export 禁用占位项 | 缺失 | `docs/prototypes/main-shell/index.html:359`；`menu_bar.slint:150-189` 无该项 | P2 |
| Session 菜单含 Connect / Duplicate Tab / Open SFTP / Tunnels；Edit 菜单含 Clear Scrollback | 部分：Session 菜单为 Disconnect / Reconnect / Edit / Update / Delete / 管理器入口，缺 Connect、Duplicate、Open SFTP、Tunnels | `ui/main_window.slint:1408-1447`；`docs/prototypes/main-shell/index.html:671-679` | P0 |
| 快捷键 Ctrl+N / Ctrl+O / Ctrl+, / Ctrl+Q / Ctrl+B / F5 / Ctrl+F / Ctrl+K；另有 Ctrl+Shift+S、F11 | 部分：缺 `Ctrl+Shift+S`（Open SFTP）与 `F11`（全屏） | `ui/main_window.slint:1179-1190,1263-1347`；`docs/prototypes/main-shell/index.html:428-436` | P1 |
| View → Full Screen（F11）/ Focus Mode | 缺失 | `docs/prototypes/main-shell/index.html:684-686`；全仓无 fullscreen/focus-mode 实现 | P1 |
| 状态栏：连接态词 + 目标 + 尺寸 + 光标 + SYNC chip + REC + MCP chip | 已实现 | `ui/components/status_bar.slint:60-294` | — |
| 状态栏右侧 UTF-8 指示 | 缺失 | `docs/prototypes/main-shell/index.html:409`；`status_bar.slint` 全文件无编码项 | P2 |
| ToolDock：左右栈、换边、折叠、隐藏、面板间拖拽分隔 | 已实现 | `ui/components/tool_dock.slint:425-720`；`ui/components/panel_frame.slint:247-284` | — |
| 默认落位：Sessions 左；Tunnels + QuickCommands 右；Transfers 隐藏 | 已实现 | `crates/yshell-config/src/schema.rs:1423-1435` | — |
| 左右栈默认宽度 260 / 280 | 部分：实现 240 / 320 | `crates/yshell-app/src/runtime/panels.rs:639-640`；`docs/prototypes/main-shell/index.html:15-16` | P2 |
| Sessions 面板搜索框常驻可用 | 已实现 | `ui/components/session_sidebar.slint:184-189` | — |
| 面板头 ⋯ 菜单：Move to Left/Right、Collapse/Expand、Hide | 已实现 | `ui/components/panel_frame.slint:247-284` | — |

## 2. 停靠面板（Sessions / Transfers / Tunnels / Commands，10 项）

| 原型要求 | 现状 | 证据（文件:行） | 优先级 |
|---|---|---|---|
| 会话树显示 `session.icon`（Default / 预设 / 自定义图） | 缺失：固定 `terminal` 图标，无图标数据模型 | `ui/components/session_tree.slint:147-161`；`docs/prototypes/main-shell/panels/sessions.html:384-398` | P1 |
| Session 原地改名（右键 Rename / 双击名称） | 缺失：菜单无 Rename，双击只打开 | `ui/main_window.slint:1725-1758`；`sessions.html:431-439` | P1 |
| Session 右键：Connect / Open in New Tab / Open SFTP / Edit / Rename / Duplicate / Favorite / Export(disabled) / Delete | 部分：有 Open / Open SFTP / Edit / Update From Active / Delete / New Session | `ui/main_window.slint:1725-1758` | P1 |
| 会话树空白区 New Folder 本版禁用 | 冲突：实现启用（可创建根文件夹） | `ui/main_window.slint:1794-1815`；`sessions.html:417-420` | P1 |
| Folder 右键 New Folder / Rename / Delete / Connect All 禁用 + 原因 | 已实现 | `ui/main_window.slint:1764-1791` | — |
| Transfers 行状态/进度/行内 Pause·Resume·Retry·Cancel + Clear completed | 已实现 | `ui/components/transfer_queue.slint:209-243` | — |
| Transfers 批量 Pause All / Resume All | 缺失：只有逐行动作 + Clear completed | `docs/prototypes/main-shell/panels/transfers.html:251-255`；`transfer_queue.slint:18-23` | P1 |
| Tunnels Start/Stop：运行时就绪前 **禁用** + 悬停「runtime not provisioned」 | 部分：按钮可点，点击后走状态栏提示「not provisioned」 | `crates/yshell-app/src/runtime/panels.rs:392-445`；`panels/tunnels.html:236,270-273` | P1 |
| Tunnels 面板 `+`（Add Tunnel）→ `tunnel-editor` | 缺失：面板无添加入口，无 tunnel-editor 窗 | `ui/components/tool_dock.slint:599-627`；`panels/tunnels.html:227` | P1 |
| Commands 面板空态（Quick Commands 本版不可用） | 已实现 | `ui/components/tool_dock.slint:629-658` | — |

## 3. 工作区（Tab / 终端 / SFTP / 分屏 / N9 / Quick Connect，17 项）

| 原型要求 | 现状 | 证据（文件:行） | 优先级 |
|---|---|---|---|
| 标签 mode（new/terminal/sftp）、Switch 同标签改 mode、关闭族、重连/断开、分屏、接收按键 | 已实现 | `ui/main_window.slint:1622-1724` | — |
| 标签显示会话图标（与树同套）+ AI 标志（AI 创建的连接） | 部分：图标缺失（见上）；AI 标志无数据来源，未实现 | `workspace_tab_item.slint:86-143`；`tab-bar.html:276-278,297` | P1（图标）/ P2（AI） |
| 标签原地改名（右键 Rename Tab） | 部分：使用独立 Rename Tab 弹窗，不是就地输入 | `ui/main_window.slint:1893-1896`；`dialog_windows.rs:942-975`；`tab-bar.html:298` | P1 |
| 分屏：每叶自有 TabBar、拖标签到边缘分屏、空 pane 提示、关掉末标签合并 | 已实现 | `ui/components/workspace_split.slint:290,603` | — |
| 分屏上限 **4 叶** | 冲突：实现允许 10 叶，但 UI 文案写 max 4 | `crates/yshell-app/src/runtime/split_tree.rs:358`；`ui/main_window.slint:1467`；`split.html:346` | P1 |
| 终端 Find 栏（Ctrl+F，含 regex） | 已实现 | `ui/components/terminal_view.slint:464-540` | — |
| 终端 Find Across SSH（跨会话匹配侧栏） | 缺失 | `docs/prototypes/main-shell/workspace/terminal.html:375-385,398`；全仓无实现 | P1 |
| REC 底部提示条（REC chip + 文件路径 + 停止入口） | 部分：REC 只在状态栏/标签角标体现，无底部提示条 | `status_bar.slint:251-294`；`terminal.html:387-391` | P2 |
| 终端右键菜单（Copy/Paste/Find/Clear Scrollback/Logging/Send Key Input To） | 已实现（项比原型更多） | `ui/main_window.slint:1483-1594` | — |
| Clear Scrollback 用**窗内**确认 | 部分：走独立 compact 确认窗 | `dialog_windows.rs:751-757`；`terminal.html:411-420` | P2 |
| 本地终端（Local terminal） | 已实现 | `crates/yshell-app/src/runtime/local_terminal.rs` | — |
| N9：发送按键核心 + 状态栏 `SYNC → n` chip + 标签 SRC/RCV + 标签菜单 Receive Key Input | 已实现 | `status_bar.slint:199-249`；`workspace_tab_item.slint:156-181`；`main_window.slint:1685-1696` | — |
| N9 同步 banner（预设下拉 + Pause + Stop，无弹窗确认） | 缺失：只保留右键菜单与 chip | `docs/prototypes/main-shell/workspace/sync-input.html:321-332`；全仓无 banner | P1 |
| N9 pane 头部 Receive 开关（源 pane 禁用） | 缺失：接收开关只在标签右键菜单 | `sync-input.html:334-366` | P2 |
| Quick Connect：Target + Connect + Recent 列表 + 空态 + Copy target + 历史开关 | 已实现 | `ui/pages/quick_connect.slint:152-333`；`crates/yshell-app/src/runtime/quick_connect.rs:146-166` | — |
| Quick Connect 输入框 Clear 图标钮 | 缺失 | `quick-connect.html:219` | P2 |

## 4. 弹窗（compact / standard，16 项）

| 原型要求 | 现状 | 证据（文件:行） | 优先级 |
|---|---|---|---|
| compact-confirm：普通 / 破坏性 / 多行粘贴三态，无打字确认词 | 已实现 | `ui/windows/compact_confirm_window.slint:67-119` | — |
| close-tab-confirm / quit：列出将断开的 SSH/SFTP/transfers/tunnels 后果清单 | 部分：只有一句摘要，无逐项后果与 tunnel note | `crates/yshell-app/src/dialog_windows.rs:1555-1572`；`close-tab-confirm/index.html:343-370` | P1 |
| Don't ask again → 写回 Settings → General 的 Confirm close / quit | 已实现 | `dialog_windows.rs:719-733` | — |
| host-key 内容：首次/变更、警告条、Known hosts 路径、Expected/Presented、Trust Once / Trust and Save / Replace and Trust | 已实现 | `ui/components/host_key_dialog.slint:51-152` | — |
| host-key 尺寸 = compact **560×400** | 缺失：窗口用 standard 720×580 | `ui/windows/host_key_window.slint:11-16`；`docs/prototypes/host-key/index.html:11-14` | P0 |
| host-key 变更态 `Copy Details` 按钮 | 缺失 | `docs/prototypes/host-key/index.html:101,112` | P2 |
| auth-prompt：Password / Public key / Keyboard interactive 切换、错误条、agent、challenge 轮次、Continue | 已实现 | `ui/windows/auth_prompt_window.slint:10-16`；`ui/components/auth_prompt_dialog.slint:129-131,190-208` | — |
| 口令字段不加 Remember（原型与表单规范） | 冲突：密码页有 Remember password | `auth_prompt_dialog.slint:236-238`；`docs/agents/prototypes.md:118` | P1 |
| sftp-chmod：独立 compact 窗，八进制输入 + Owner/Group/Other × R/W/X 9 个勾选框双向同步 | 缺失：只有 SFTP 内 octal 文本框（`kind == "chmod"`） | `ui/components/sftp_panel.slint:1237-1242`；`ui/windows/sftp_entry_window.slint:49`；`sftp-chmod/index.html:63-94` | P1 |
| logging：开始表单（Save to / File name / Format / Timestamp / Record input）+ 进行中 Stop 视图 | 已实现（另有覆盖确认、输入警告，超集） | `ui/components/logging_dialog.slint:137-417` | — |
| about：版本 + 说明 + Config/License 网格 + 可展开开源许可 | 已实现 | `ui/windows/about_window.slint:52-157` | — |
| sftp-conflict：冲突数 + source→dest + 路径列表 + Skip/Rename/Overwrite | 已实现 | `ui/windows/sftp_conflict_window.slint:45-55` | — |
| sftp-properties：六行只读网格 + Copy Path / Close | 已实现 | `ui/windows/sftp_properties_window.slint:51-57` | — |
| tunnel-editor：standard 720×580 独立窗（Name/Type/Bind/Destination/On connect） | 缺失：无独立窗，仅 Session Editor 内联表单 | `docs/prototypes/tunnel-editor/index.html:148-205`；`ui/windows/` 无 tunnel editor | P1 |
| 弹窗只用三档尺寸 token（560×400 / 720×580 / 1100×820） | 已实现（唯一例外 host-key，见上） | `ui/theme.slint:215-222` | — |
| MCP control consent（Deny/Allow + Don't ask again，仅随 Allow 生效） | 已实现 | `dialog_windows.rs:780-800,1476-1494` | — |

## 5. Settings（10 项，含 4 项本轮）

| 原型要求 | 现状 | 证据（文件:行） | 优先级 |
|---|---|---|---|
| wide 1100×820 整壳、左导航 7 分区 + Shortcuts(禁用)、页脚仅 Cancel \| Save（Save=保存并关闭） | 已实现 | `ui/windows/settings_window.slint:11-14`；`ui/pages/settings.slint:483-530,1400-1413` | — |
| General：Language / Confirm close / Confirm quit / Reduce motion | 已实现 | `settings.slint:564-675` | — |
| Appearance：App theme（Mode/Accent）+ Terminal style 下拉 + 预览 | **本轮实施中，未计入缺口** | `settings.slint:676-805` | — |
| Terminal：Scrollback lines/max cells + Reset + Mouse&paste（middle/right/multi-line/auto copy） | **本轮实施中，未计入缺口** | `settings.slint:806-966` | — |
| Security：主密码卡片（Not set/Set，Set/Change/Remove/Forgot）+ Managers 清单计数 | **本轮实施中，未计入缺口** | `settings.slint:1009-1186` | — |
| master-password（Set/Change/Remove/Reset）与启动解锁窗 | **本轮实施中，未计入缺口** | `ui/pages/settings.slint:366-369`；`ui/windows/master_password_window.slint`、`master_password_unlock_window.slint`（本轮新增） | — |
| MCP：Enable + Port + Listen(Local/LAN) + API key(Generate/Copy) + View user + Control user + Auto-approve | 部分：仅 Enable + Auto-approve；实现为 stdio MCP（`--mcp`），无端口/监听/API key | `settings.slint:1187-1302`；`crates/yshell-app/src/runtime/mcp.rs:145-177`；`settings/sections/mcp.html:52-116` | P1 |
| Advanced：Debug logging 开关 + 打开日志目录 + Config directory 只读 + 打开目录 | 部分：缺 Config directory 的「打开目录」按钮 | `settings.slint:1303-1376`；`settings/sections/advanced.html:48-72` | P2 |
| SFTP：Show hidden（默认开）/ Conflict（Ask/Replace/Skip/Stop）/ External editor（含 Custom 路径） | 缺失：仅有原型没有的「Max parallel transfers」 | `settings.slint:967-1007`；`settings/sections/sftp.html:52-92` | P1 |
| 分区内容区不再放 General/Authentication 等大标题 | 冲突：SFTP / AI MCP / Advanced 三个分区顶部仍复述分区名大标题（Security 分区本轮正按规则收口） | `ui/pages/settings.slint:974-979,1191-1196,1307-1312`；`docs/agents/prototypes.md:81` | P1 |

## 6. Session Editor（10 项）

| 原型要求 | 现状 | 证据（文件:行） | 优先级 |
|---|---|---|---|
| wide 整壳 + 左导航 + 页脚 Test Connection / Cancel / Save / Save and Connect；SFTP 与 Advanced 禁用并注明 | 已实现（SFTP/Advanced 禁用带 tooltip） | `ui/windows/session_editor_window.slint:14-17`；`ui/pages/session_editor.slint:520-527,1030-1040` | — |
| General：Name / Host / Port / **Icon**（Default·预设·自定义 + Browse/Reset）/ **Notes**；Folder 无 Host/Port | 部分：有名称/主机/端口/用户名/只读 Folder；缺 Icon 与 Notes | `session_editor.slint:588-668`；`session-editor/sections/general.html:63-83` | P1 |
| Authentication：页级 Inherit 开关 + 「Using parent: …」来源 + Username + Password/Private Key + Passphrase | 部分：方法为 Agent/Password/Keyboard Interactive/Key，无页级 Inherit/source；Passphrase 仅 legacy 提示 | `session_editor.slint:669-812`；`sections/authentication.html:54-99` | P1 |
| Login Script 分区：Inherit + 步骤列表（全宽）+ 工具条 Keys/Wait + 右键 Edit/Delete/调序 + 拖拽排序 | 缺失：导航无该分区，无任何 login script 数据/UI | `session_editor.slint:520-527`；`session-editor/index.html:105`；全仓无 `login_script` | P0 |
| login-step-keys / login-step-wait 独立 standard 子窗（Name/Keys/Tokens 速查；Wait ms；Delete 入口） | 缺失 | `docs/prototypes/session-editor/sections/login-step-keys.html:94-141`；`login-step-wait.html:68-87` | P1 |
| Proxy：Inherit + Profile 下拉 + 管理器旁路 | 部分：No Proxy / Profile / Manage proxies 有；无页级 Inherit/source | `session_editor.slint:813-907`；`sections/proxy.html:53-74` | P1 |
| Tunnels：Inherit + 配置清单（全宽）+ 顶栏 plus/clear + 行右键 Edit/Duplicate/Delete + Add → tunnel-editor | 部分：内联 Kind/Host/Port/Target 表单 + Add/Clear；无 Inherit、无行右键、无独立 editor | `session_editor.slint:908-974`；`sections/tunnels.html:85-118` | P1 |
| Terminal Style：Inherit + Style 下拉 + 预览 | 部分：分区改名 Appearance，按字段 Inherit/Explicit/Reset 覆盖 + 预览（超集），但无页级 Inherit | `session_editor.slint:975-1009`, `547-587`；`sections/terminal-style.html:77-107` | P1 |
| 分区内容区不再放大标题（General/Authentication/Proxy/Tunnels/Appearance） | 冲突：每分区都有 SectionTitle | `session_editor.slint:592,673,817,912,979`；`docs/agents/prototypes.md:81` | P1 |
| 分区内不出现 Save，只用整窗页脚 | 已实现 | `session_editor.slint:1030-1040` | — |

## 7. 管理器（known-hosts / private-keys / proxies / terminal-styles，10 项）

| 原型要求 | 现状 | 证据（文件:行） | 优先级 |
|---|---|---|---|
| Known Hosts：按 host:port 分组的左清单 + 右详情（Host/Port/Algorithm/Fingerprint）+ Import/Export/Clear All | 部分：现由 `host_keys` 页承载，分组清单/详情/导入/导出/清空齐全；缺 Add（单字段粘贴窗） | `ui/pages/host_keys.slint:232-450`；`known-hosts/index.html:76-136`；`known-hosts/modals.html:61-74` | P1 |
| Known Hosts 行级动作走右键、工具栏只放图标钮 | 冲突：Remove 是清单下文字按钮；Import/Export/Clear All/Paste text 是工具条文字钮 | `ui/pages/host_keys.slint:236-264,365-376`；`docs/agents/prototypes.md:80,86` | P1 |
| Private Keys：Generate 流程（表单 → 结果页公钥 + Copy） | 缺失：只有 Import/Test/Export/Deploy/Remove | `ui/pages/private_keys.slint:120-136,236-353`；`private-keys/modals.html:69-129` | P1 |
| Private Keys：Import（文件/粘贴互跳）、Export、Deploy 用独立 standard 子窗 | 部分：全部内联在管理器页右栏，无子窗 | `ui/pages/private_keys.slint:287-353`；`private-keys/modals.html:131-208` | P1 |
| Private Keys 右栏：Algorithm/Fingerprint/Used by/Public key + Copy/Passphrase + Forget；空列表右栏空态 | 已实现 | `ui/pages/private_keys.slint:373-580` | — |
| Proxies：清单 + 右栏表单（Protocol/Host/Port/User/Password/DNS/Test） | 已实现 | `ui/pages/proxies.slint:235-602` | — |
| Proxies：工具条只放 `+`，Duplicate/Delete 走行右键 | 冲突：New/Edit/Duplicate/Test/Delete 全是文字按钮 | `ui/pages/proxies.slint:305-353`；`proxies/index.html:66-131` | P1 |
| Terminal Styles：Font（family/size/Fallback + Add Font File）+ Colors + ANSI + 预览 | 缺失：编辑器只有配色/ANSI；字体配置散在 Settings→Appearance（本轮） | `ui/components/theme_editor.slint:101-212`；`terminal-styles/index.html:182-229` | P2 |
| Terminal Styles：Built-in 只读 + Clone + Delete + live preview | 已实现 | `ui/pages/theme_manager.slint:60-183,240-370` | — |
| 遗留 `known_hosts` 页（无入口）含「Type CLEAR」打字确认 | 冲突：仍在仓库且违反「禁止打字确认」；当前无 UI 入口 | `ui/main_window.slint:980`（callback 无调用点）；`ui/pages/known_hosts.slint:238-256`；`crates/yshell-app/src/runtime/auth.rs:119` | P2 |

## 8. 跨切面规范（6 项）

| 原型/规范要求 | 现状 | 证据（文件:行） | 优先级 |
|---|---|---|---|
| Rust 侧不再把英文句子直接投到 UI（窗口标题/确认文案/i18n） | 部分：确认窗标题与正文、错误状态仍硬编码英文 | `crates/yshell-app/src/dialog_windows.rs:568-570,601,634-636,646,701-703,715,726,739,1558-1570`；`problems.md:100-101` | P1 |
| 原地编辑：Sessions 改名、Tab 改名、SFTP New Folder/Rename 一律就地输入，禁止另开对话框 | 缺失：SFTP 走 `SftpEntryDialog`（compact 窗），Sessions 无改名，Tab 用弹窗 | `ui/components/sftp_panel.slint:1074-1078`；`ui/windows/sftp_entry_window.slint:68`；`docs/agents/prototypes.md:94` | P0 |
| 禁止打字确认破坏操作（CLEAR/DELETE 等） | 冲突：遗留 KnownHosts 页仍在用 `CLEAR`；确认窗保留了未使用的 token 输入位 | `crates/yshell-app/src/runtime/auth.rs:113-121`；`ui/pages/known_hosts.slint:238-256`；`compact_confirm_window.slint:97-113` | P2 |
| WinTextInput / ManagedKeyPicker / ProxyPicker 有焦点环 | 缺失：文本输入仅底部 accent 条，两个 Picker 无焦点反馈 | `problems.md:86-88` | P1 |
| 可点按钮均有 tooltip / 禁用项有原因 | 部分：菜单项已普遍带 tooltip，面板折叠、菜单栏右栏开关、标签「＋」等图标钮仍缺 | `problems.md:85`；`ui/components/panel_frame.slint:56-65` | P1 |
| 清单元件优秀实现（清单/键盘对齐/尺寸 token） | 已实现：清单行高、表单行高、三档窗宽均已 token 化 | `ui/theme.slint:207-222`；`ui/components/basic/controls.slint` | — |

---

## 9. 不建议近期做（下一轮别当需求）

以下项原型本身标注为禁用/空态，或依赖上游能力；实现侧即使「未做」也不算缺口：

1. **Tunnels 转发运行时（引擎）**：原型明写 Start/Stop 在 runtime provisioned 前禁用，转发引擎属后续里程碑（`panels/tunnels.html:236,270-273`）；配置编辑/Add 仍按 §2 计缺口。
2. **Quick Commands / Commands 面板内容**：本版空态即基准（`panels/commands.html:152`）。
3. **Balance Panes**：原型里就是 disabled 按钮（`split.html:238`）。
4. **Folder 的 New Folder / Rename / Delete / Connect All，以及树的 Import Config / Expand All / Collapse All**：原型明确「本版不做/禁用」（`sessions.html:419,435-437`）。
5. **Session Editor 的 SFTP 与 Advanced 分区**：原型为 `not this version`（`session-editor/index.html:109-110`）。
6. **AI 标签标志**：依赖 MCP 控制链路定型（`tab-bar.html:297`），当前无 tab 来源模型，建议随后续 MCP 批次一起做。
7. **MCP 的 Port / Listen(LAN) / API key**：实现已选 stdio MCP 方向（`mcp.rs:145-177`），与原型端口方案冲突；先确认方向并改原型，再谈实现。
8. **终端内边距 12/12、圆角 8、遮罩色、树箭头旋转**：problems.md 已标记「待定」，与 Rust 像素契约耦合，不属 UI 缺口（`problems.md:62-67,82-84`）。
9. **主密码 / Settings 三分区**：本轮实施中，验收前不列为缺口。
