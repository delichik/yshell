# YShell UI 交互契约

更新注记（2026-10-29）：**范围与砍项以 [`yshell-current-status.md`](./yshell-current-status.md) 为准。** 下文含愿景级控件时，凡标注「本版不做」或与 status §2 冲突的，实现须 disabled/空态/无入口，不得按「始终启用」验收。

本文档是**唯一现行 UI 契约**（原独立 `context-menu-design.md` 已并入各区域菜单节并迁入 `archive/`）。右键以各节「实现状态」与 status 为准。

## 1. 全局规则

### 1.1 鼠标规则

| 操作 | 全局含义 |
|---|---|
| 左键单击 | 选择、聚焦、触发主动作。 |
| 左键双击 | 打开、连接、进入目录、编辑当前对象。 |
| 右键单击 | 打开当前对象的上下文菜单。 |
| 鼠标悬停 | 显示 tooltip；危险按钮 tooltip 必须说明风险。 |
| 拖拽 | 只用于标签排序、分屏移动、SFTP 上传、SFTP 下载、面板宽度调整。 |
| 中键单击 | 仅标签栏支持：关闭对应标签。其他区域中键无动作。 |

### 1.2 键盘规则

| 快捷键 | 行为 |
|---|---|
| `Ctrl+N` / `Cmd+N` | 新建会话，打开 Session Editor。 |
| `Ctrl+Shift+N` / `Cmd+Shift+N` | 新建文件夹。 |
| `Ctrl+O` / `Cmd+O` | 打开 Quick Connect。 |
| `Ctrl+W` / `Cmd+W` | 关闭当前标签。 |
| `Ctrl+Shift+W` / `Cmd+Shift+W` | 关闭当前窗口。 |
| `Ctrl+T` / `Cmd+T` | 新建空标签，聚焦 Quick Connect。 |
| `Ctrl+Tab` | 切到下一个标签。 |
| `Ctrl+Shift+Tab` | 切到上一个标签。 |
| `Ctrl+F` / `Cmd+F` | 当前终端或 SFTP 列表内搜索。 |
| `Ctrl+Shift+F` / `Cmd+Shift+F` | 跨 SSH 查找；如果焦点在终端，以当前终端 query 初始化。 |
| `Ctrl+C` / `Cmd+C` | 终端有选区时复制；无选区时发送 interrupt。 |
| `Ctrl+Shift+C` / `Cmd+Shift+C` | 强制复制终端选区。 |
| `Ctrl+V` / `Cmd+V` | 粘贴到当前终端或输入框。 |
| `Ctrl+Shift+V` / `Cmd+Shift+V` | 强制粘贴到终端。 |
| `F5` | 刷新当前 SFTP 目录或重连断开的当前会话。 |
| `F11` | 全屏。 |
| `Esc` | 关闭当前非阻断弹窗、搜索框、命令面板。阻断安全弹窗不能用 Esc 关闭。 |

macOS 菜单显示 `Cmd`，Windows/Linux 显示 `Ctrl`。终端内快捷键冲突时，以终端语义优先；应用级快捷键仅在终端没有焦点或使用 `Ctrl+Shift` 组合时触发。

### 1.3 控件状态

| 状态 | 视觉 | 行为 |
|---|---|---|
| Enabled | 正常对比度 | 可点击、可聚焦。 |
| Disabled | 40% 透明度 | 不可点击，tooltip 说明为什么不可用。 |
| Loading | 显示 spinner 或进度文字 | 禁止重复提交。 |
| Dangerous | 红色强调 | 点击后必须二次确认，除非只是打开详情。 |
| Active | 蓝色强调或选中背景 | 表示当前区域、标签、会话、目录或模式。 |

## 2. 主窗口布局

主窗口从上到下、从左到右固定为以下区域：

```text
AppMenu
MainToolbar
SessionSidebar | WorkspaceTabs + TerminalWorkspace | RightDock
StatusBar
```

默认尺寸：

- 最小窗口：`1120 x 720`。
- 默认窗口：`1440 x 900`。
- SessionSidebar 默认宽度：`260px`，可拖拽范围 `200-420px`。
- RightDock 默认宽度：`330px`，可拖拽范围 `280-560px`。
- StatusBar 高度：`30px`。

## 3. AppMenu

### 3.1 File 菜单

| 控件 ID | 文案 | 类型 | 左键行为 | 快捷键 | 状态规则 |
|---|---|---|---|---|---|
| `menu.file.new_session` | New Session | 菜单项 | 打开 Session Editor，模式为 create。 | `Ctrl+N` | 始终启用。 |
| `menu.file.quick_connect` | Quick Connect | 菜单项 | 打开/聚焦快速连接页输入框。 | `Ctrl+O` | 始终启用。 |
| `menu.file.save_active` | Save Active Session | 菜单项 | 将当前活动会话写入 sessions/。 | 视实现 | 有活动会话时启用。 |
| `menu.file.settings` | Settings... | 菜单项 | 打开 Settings 页面。 | `Ctrl+,` | 始终启用。 |
| `menu.file.quit` | Quit | 菜单项 | 若存在活动会话，打开 Quit Confirmation；否则退出。 | `Ctrl+Q` / `Cmd+Q` | 始终启用。 |

> **本版 File 菜单**：无 Import / Export Config、无 New Folder 菜单项。新建根文件夹走会话树空白区「New Folder…」。Import/Export 属本版不做（见 `yshell-current-status.md` §2）。

### 3.2 Edit 菜单

| 控件 ID | 文案 | 类型 | 左键行为 | 快捷键 | 状态规则 |
|---|---|---|---|---|---|
| `menu.edit.copy` | Copy | 菜单项 | 复制当前终端选区或 SFTP 选中文件路径。 | `Ctrl+C` | 有可复制内容时启用。 |
| `menu.edit.paste` | Paste | 菜单项 | 粘贴到当前焦点输入框或终端。 | `Ctrl+V` | 剪贴板非空时启用。 |
| `menu.edit.select_all` | Select All | 菜单项 | 终端全选可见缓冲，SFTP 全选列表。 | `Ctrl+A` | 当前区域支持选择时启用。 |
| `menu.edit.find` | Find... | 菜单项 | 打开当前区域搜索栏。 | `Ctrl+F` | 终端或 SFTP 有焦点时启用。 |
| `menu.edit.clear_terminal` | Clear Terminal | 菜单项 | 清屏但不清除 scrollback。 | `Ctrl+L` | 当前标签已连接时启用。 |
| `menu.edit.clear_scrollback` | Clear Scrollback | 菜单项 | 打开确认弹窗，确认后清除 scrollback。 | 无 | 当前标签存在终端时启用。 |

### 3.3 Session 菜单

| 控件 ID | 文案 | 类型 | 左键行为 | 快捷键 | 状态规则 |
|---|---|---|---|---|---|
| `menu.session.connect` | Connect | 菜单项 | 连接当前选中会话。 | `Enter` on selected session | 选中未连接会话时启用。 |
| `menu.session.disconnect` | Disconnect | 菜单项 | 断开当前标签 SSH 和关联 SFTP。 | 无 | 当前标签已连接时启用。 |
| `menu.session.reconnect` | Reconnect | 菜单项 | 断开后重新连接当前会话。 | `F5` when disconnected | 当前标签断开或错误时启用。 |
| `menu.session.duplicate_tab` | Duplicate Tab | 菜单项 | 使用相同 SessionProfile 打开新标签并连接。 | 无 | 当前标签有会话 profile 时启用。 |
| `menu.session.edit` | Edit Session... | 菜单项 | 打开 Session Editor，模式为 edit。 | 无 | 选中保存的会话时启用。 |
| `menu.session.open_sftp` | Open SFTP | 菜单项 | 在当前会话上打开 SFTP 标签（固定本地/远程左右两栏）。 | `Ctrl+Shift+S` | 当前标签已连接时启用。 |
| `menu.session.tunnels` | Tunnels... | 菜单项 | 打开右侧 Tunnel 面板。 | 无 | 当前标签存在时启用。 |

### 3.4 View 菜单

| 控件 ID | 文案 | 类型 | 左键行为 | 快捷键 | 状态规则 |
|---|---|---|---|---|---|
| `menu.view.session_manager` | Session Manager | 勾选菜单项 | 显示/隐藏左侧会话树。 | `Ctrl+B` | 始终启用。 |
| `menu.view.transfers` | Transfers | 勾选菜单项 | 显示/隐藏右侧传输队列面板。 | 无 | 始终启用。 |
| `menu.view.quick_commands` | Quick Commands | 勾选菜单项 | 显示/隐藏 Quick Commands 面板。 | 无 | 始终可勾选；**面板内容本版未提供**（空态说明）。 |
| `menu.view.tunnel_panel` | Tunnel Panel | 勾选菜单项 | 显示/隐藏 Tunnels 面板。 | 无 | 始终启用。配置可编辑；**Start/Stop 转发 runtime 尚未 provision**（见 status §3）。 |
| `menu.view.fullscreen` | Full Screen | 勾选菜单项 | 切换全屏。 | `F11` | 始终启用。 |
| `menu.view.focus_mode` | Focus Mode | 勾选菜单项 | 隐藏左右面板和工具栏，仅保留标签、终端、状态栏。 | 无 | 当前存在终端标签时启用。 |

## 4. MainToolbar

从左到右：

| 控件 ID | 文案/占位 | 类型 | 默认值 | 左键行为 | 回车行为 | 右键行为 | 状态规则 |
|---|---|---|---|---|---|---|---|
| `toolbar.new_session` 等 | — | — | — | 顶部 CommandBar Quick Connect **输入条已移除**（N2）；连接入口在快速连接页标签。 | — | — | — |

> 历史「MainToolbar Quick Connect / Commands / Log」控件表保留作愿景参考；现行主工具条以实现为准（New Session、面板开关、查找、设置等）。

### 4.1 Quick Connect 解析规则

| 输入 | 解析结果 |
|---|---|
| `host` | host=`host`, port=`22`, username=当前系统用户名或最近用户名。 |
| `user@host` | username=`user`, host=`host`, port=`22`。 |
| `host:2222` | host=`host`, port=`2222`。 |
| `user@host:2222` | username=`user`, host=`host`, port=`2222`。 |
| `ssh://user@host:2222` | username=`user`, host=`host`, port=`2222`。 |

错误反馈：

- 空输入：Connect 按钮 disabled。
- 端口不是 `1-65535`：输入框下方显示 `Port must be between 1 and 65535`。
- host 为空：输入框下方显示 `Host is required`。
- 解析成功但没有认证信息：打开 Quick Auth Dialog。

## 5. SessionSidebar

### 5.1 顶部区域

| 控件 ID | 文案/占位 | 类型 | 左键行为 | 右键行为 | 状态规则 |
|---|---|---|---|---|---|
| `sidebar.header.title` | SAVED / 会话树标题 | 文本 | 无 | 无 | 始终显示。 |
| `sidebar.search.input` | Search sessions | 单行输入框 | 聚焦搜索。 | 清空/粘贴上下文菜单。 | 始终启用。 |

> **本版（S1）**：侧栏常驻 `+` / `…` 已删除；新建会话/文件夹/刷新走空白区右键（及 File / CommandBar New Session）。

### 5.2 会话树项目

| 对象 | 左键单击 | 左键双击 | 右键单击 | 拖拽 |
|---|---|---|---|---|
| Folder | 选中。 | 展开/折叠。 | 菜单：New Session、New Folder、Rename、Delete、Export Folder。 | 可拖到其它 Folder 下。 |
| Session | 选中并在详情区显示摘要。 | 连接：活动标签已断开/未连接（且不是快速连接页）时占用该标签，否则打开新标签。 | 菜单：Connect、Open in New Tab、Open SFTP、Edit、Duplicate、Favorite、Delete、Export。 | 可拖到 Folder 下或改变排序。 |
| Favorite Session | 选中。 | 打开新标签并连接。 | 菜单：Connect、Remove Favorite、Edit。 | 可在 Favorites 内排序。 |
| Recent Session | 选中。 | 打开新标签并连接。 | 菜单：Connect、Pin to Favorites、Remove from Recent。 | 不支持拖拽。 |

### 5.3 会话树上下文菜单

| 菜单项 | 目标对象 | 行为 | 确认 |
|---|---|---|---|
| Connect | Session | 当前工作区打开新标签并连接。 | 无 |
| Open in New Tab | Session | 新建标签但不切断其它标签。 | 无 |
| Open SFTP | Session | 新建 SFTP 标签；未连接时先连接。 | 未保存认证时打开认证弹窗。 |
| Edit | Session | 打开 Session Editor(edit)。 | 无 |
| Duplicate | Session | 复制配置，名称追加 `Copy`，打开编辑器。 | 无 |
| Rename | Folder/Session | 行内编辑名称。 | Enter 保存，Esc 取消。 |
| Delete | Folder/Session | 移入确认弹窗。Folder 删除会列出子项数量。 | 必须确认 |
| Favorite | Session | 加入收藏。 | 无 |
| Export | Folder/Session | 打开导出向导。 | 无 |

## 6. WorkspaceTabs

### 6.1 标签项

标签有三种模式：New Tab（快速连接）、Terminal、SFTP。三者共享同一套标签条、分屏、拖拽和关闭机制；模式差异只体现在内容区、标签图标和模式专属菜单项。`+` / `Ctrl+T` / 启动应用都创建一个 New Tab；多个 New Tab 相互独立。

| 区域 | 左键单击 | 左键双击 | 右键单击 | 中键单击 | 拖拽 |
|---|---|---|---|---|---|
| 标签主体 | 切换到该标签。 | 已断开/未连接的标签：重连该标签；仍连着的标签：复制同源会话（新标签）。 | 打开标签上下文菜单。 | 关闭标签。 | 调整顺序或拖入分屏。 |
| 关闭按钮 `x` | 关闭标签。 | 无 | 无 | 无 | 无 |
| 状态图标 | 显示状态 tooltip。 | 无 | 无 | 无 | 无 |

关闭窗口内最后一个标签等同点击窗口关闭：主窗口进入退出确认，次窗口直接关闭。

### 6.2 标签上下文菜单

| 菜单项 | 行为 | 状态 |
|---|---|---|
| Reconnect | 重新连接当前会话。 | Terminal / SFTP 标签；断开或错误时启用。 |
| Disconnect | 断开 SSH / SFTP 连接。 | Terminal / SFTP 标签；已连接时启用。 |
| Rename Tab | 行内编辑标签名，仅影响当前 tab。 | 始终启用。 |
| Split Right | 将标签移入右侧分屏。 | 当前工作区可分屏时启用。 |
| Split Down | 将标签移入下方分屏。 | 当前工作区可分屏时启用。 |
| Receive Key Input | 勾选/取消该标签作为键输入接收目标。 | 仅 Terminal 标签。 |
| Move to New Window | 将标签迁移到新窗口。 | 当前窗口存在标签时启用。 |
| Move to Main Window | 将标签迁回主窗口。 | 标签不在主窗口时启用。 |
| Switch to SFTP | 关闭 shell 通道、保留 SSH transport，原标签转为 SFTP。 | Terminal 标签且连接可用时启用。 |
| Switch to Terminal | 关闭 SFTP 通道，在同一 SSH transport 上打开新 shell，原标签转为 Terminal。 | SFTP 标签且连接可用时启用。 |
| Close Others | 关闭其它标签。 | 标签数大于 1 时启用。 |
| Close Tabs to Left | 关闭左侧标签。 | 左侧存在标签时启用。 |
| Close Tabs to Right | 关闭右侧标签。 | 右侧存在标签时启用。 |
| Close All Tabs | 关闭当前 pane 全部标签。 | 标签数大于 0 时启用。 |
| Close Disconnected Tabs | 关闭断开或错误标签。 | 存在断开/错误标签时启用。 |
| Close | 关闭当前标签。 | 始终启用。 |

模式过滤：New Tab 隐藏 Reconnect / Disconnect / Receive Key Input / Switch；SFTP 隐藏 Receive Key Input。隐藏项不占菜单高度。

## 6.3 分屏交互

| 控件/区域 | 左键行为 | 右键行为 | 拖拽行为 | 状态 |
|---|---|---|---|---|
| `workspace.pane_splitter` | 无 | 无 | 拖动调整相邻 pane 比例。 | 两个 pane 同时存在时显示。 |
| `workspace.empty_pane` | 聚焦 pane。 | 菜单：Close Pane、New Session Here。 | 可接收标签拖入。 | pane 无标签时显示。 |
| `workspace.pane_header` | 聚焦 pane。 | 菜单：Split Right、Split Down、Close Pane、Move All Tabs。 | 可拖拽整个 pane 中的 active tab。 | 始终显示。 |
| `tab.drag_to_right_edge` | 无 | 无 | 在目标 pane 右侧创建竖向分屏。 | 分屏叶子数小于 4 时允许。 |
| `tab.drag_to_bottom_edge` | 无 | 无 | 在目标 pane 下方创建横向分屏。 | 分屏叶子数小于 4 时允许。 |

Split 下拉菜单：

| 菜单项 | 行为 | 状态 |
|---|---|---|
| Split Right | 当前 active tab 移到右侧新 pane。 | pane 数小于 4 时启用。 |
| Split Down | 当前 active tab 移到下方新 pane。 | pane 数小于 4 时启用。 |
| Move Tab to Previous Pane | 将 active tab 移到上一个 pane。 | 存在其它 pane 时启用。 |
| Move Tab to Next Pane | 将 active tab 移到下一个 pane。 | 存在其它 pane 时启用。 |
| Close Current Pane | 关闭当前 pane，内部标签逐个触发关闭确认。 | pane 数大于 1 时启用。 |
| Balance Panes | 将同级 pane ratio 重置为 50%。 | 存在 split 时启用。 |

分屏规则：

- 第一版最多 4 个 pane。
- New Tab、Terminal、SFTP 三种标签都可以进入分屏；分屏叶子数与标签模式无关。
- 每个 SFTP 标签独立保存远程路径、列表、选择、排序、隐藏文件开关和本地目录；分屏中的两个 SFTP 叶子互不同步。
- SFTP 叶子固定为本地左栏 / 远程右栏，默认 1:1；拖动分隔条只影响当前 SFTP 叶子，比例不跨叶子同步、不持久化。
- 关闭 pane 内最后一个标签后，自动合并相邻 pane。
- 调整 pane 比例后立即更新内存状态，500ms debounce 写入 `state.toml`。
- 分屏状态只保存布局和 tab/session id，不保存 SSH 连接 secret。

## 7. TerminalWorkspace

### 7.1 终端视图

| 操作 | 行为 |
|---|---|
| 左键单击 | 聚焦终端。 |
| 左键拖拽 | 创建普通文本选区。 |
| Alt + 左键拖拽 | 创建矩形选区。 |
| 左键双击 | 选择当前单词。 |
| 左键三击 | 选择当前行。 |
| 右键单击选区 | 打开 Copy/Paste/Select Screen/Select All/Find/Find Across SSH/Clear 菜单。 |
| 右键单击无选区 | 打开 Paste/Select Screen/Select All/Find/Find Across SSH/Clear 菜单。 |
| 鼠标滚轮 | 滚动 scrollback。 |
| Shift + 鼠标滚轮 | 水平滚动，若终端启用横向滚动。 |

### 7.2 终端上下文菜单

| 菜单项 | 行为 | 状态 |
|---|---|---|
| Copy | 复制选区。 | 有选区时启用。 |
| Copy Current Line | 复制当前光标行。 | 当前行存在内容时启用。 |
| Copy Screen | 复制当前可见屏幕内容。 | 当前屏幕存在内容时启用。 |
| Paste | 粘贴剪贴板。 | 剪贴板非空且已连接时启用。 |
| Paste as Bracketed | 使用 bracketed paste 包裹粘贴。 | 剪贴板非空且终端支持时启用。 |
| Select Screen | 仅选择当前可见屏幕内容。 | 当前屏幕存在内容时启用。 |
| Select All | 选择可见终端和 scrollback。 | 始终启用。 |
| Find | 打开终端搜索栏。 | 始终启用。 |
| Find Regex | 打开终端搜索栏并启用正则模式。 | 始终启用。 |
| Find Across SSH | 打开跨 SSH 查找面板。 | 至少存在 2 个 SSH terminal 时启用。 |
| Clear Screen | 发送清屏并清空当前屏幕。 | 已连接时启用。 |
| Clear Scrollback | 清除 scrollback，弹确认。 | scrollback 非空时启用。 |
| Save Output As... | 将 scrollback 保存为文本。 | scrollback 非空时启用。 |

### 7.3 终端搜索栏

| 控件 ID | 类型 | 行为 |
|---|---|---|
| `terminal.search.input` | 单行输入框 | 输入后 200ms debounce 搜索。 |
| `terminal.search.prev` | 按钮 | 跳到上一个匹配。 |
| `terminal.search.next` | 按钮 | 跳到下一个匹配。 |
| `terminal.search.case_sensitive` | checkbox | 切换大小写敏感。 |
| `terminal.search.regex` | checkbox | 切换正则模式。正则非法时显示错误。 |
| `terminal.search.close` | 按钮 | 关闭搜索栏并保留终端焦点。 |

### 7.4 跨 SSH 查找面板

跨 SSH 查找用于在多个 SSH 终端的 scrollback 和当前屏幕中搜索字符串。

| 控件 ID | 类型 | 行为 |
|---|---|---|
| `terminal.search_all.query` | 单行输入框 | 输入普通文本或正则 query。 |
| `terminal.search_all.mode` | segmented control | Text / Regex。 |
| `terminal.search_all.scope` | segmented control | Current SSH / All Visible SSH / All Connected SSH / Manual。 |
| `terminal.search_all.highlight_all` | checkbox | 在所有匹配终端中高亮结果。 |
| `terminal.search_all.case_sensitive` | checkbox | 切换大小写敏感。 |
| `terminal.search_all.results` | 列表 | 按 session 分组显示匹配行、行号或 scrollback offset。 |
| `terminal.search_all.open_result` | 结果行 | 左键跳转到对应 SSH 标签/pane 并定位匹配。 |
| `terminal.search_all.close` | 按钮 | 关闭面板并清除跨 SSH 高亮。 |

规则：

- 普通文本查找按字面量匹配。
- Regex 模式使用 Rust regex 语法；正则非法时不执行搜索，并显示错误。
- Highlight All 开启时，目标终端标签显示匹配数量 badge。
- 搜索不发送任何内容到 SSH，不影响连接状态。

## 8. RightDock

RightDock 是 tabbed dock，第一版包含 `Transfers`、`Tunnels`、`Commands`、`Session Info`。`Local` / `SFTP` 不再作为 RightDock 面板；SFTP 是独立标签，固定本地左栏 / 远程右栏。默认显示 `Tunnels` 和 `Commands`，`Transfers` 通过 View 菜单或传输任务打开。

| Dock Tab | 左键行为 | 右键行为 |
|---|---|---|
| Transfers | 切换到传输队列。 | 菜单：Clear Completed、Pause All、Resume All。 |
| Tunnels | 切换到端口转发面板。 | 菜单：Add Tunnel、Stop All。 |
| Commands | 切换到快捷命令。 | 菜单：New Command、Manage Groups。 |
| Session Info | 切换到当前会话信息。 | 无。 |

## 8.1 Send Key Input To 状态条

当 Send Key Input To 启用时，终端上方必须显示状态条。该功能表示用户在源 SSH 窗口直接按下的终端输入事件会实时复制到目标 SSH 窗口，不是发送命令文本。

| 控件 ID | 类型 | 行为 |
|---|---|---|
| `key_broadcast.banner` | 状态条 | 显示 `Send Key Input To: source -> N targets`。 |
| `key_broadcast.preset` | 下拉 | 选择 All SSH / All Visible / All Connected / Current Split / Current Tab Group / Same Folder / Manual。 |
| `key_broadcast.target_list` | 链接按钮 | 左键打开目标窗口列表。 |
| `key_broadcast.pause` | toggle 按钮 | 暂停/恢复复制键输入，不断开会话。 |
| `key_broadcast.stop` | 危险按钮 | 立即停止复制键输入。 |

状态条必须使用警示色，不允许和普通连接状态混淆。

每个可见 SSH pane 的 pane header 必须显示接收开关：

| 控件 ID | 类型 | 左键行为 | 状态 |
|---|---|---|---|
| `pane.receive_key_input_toggle` | checkbox/toggle | 将当前 pane 加入或移出 Send Key Input To 目标。 | 仅 connected SSH shell pane 启用；源 pane disabled。 |

## 9. SFTP Tab

### 9.0 SFTP 标签布局

SFTP 标签固定为左右两栏：左栏本地文件，右栏远程文件。顶部各自有路径输入、刷新/上级/主目录按钮；远程栏额外提供上传、下载、新建目录和更多菜单。默认列宽 1:1，中间分隔条可拖动；比例只属于当前 SFTP 标签，不跨标签同步、不持久化。传输任务只进入独立 Transfers 面板，SFTP 标签内不再嵌入传输抽屉。

### 9.1 SFTP 顶部工具栏

| 控件 ID | 文案/图标 | 类型 | 左键行为 | 状态规则 |
|---|---|---|---|---|
| `sftp.path.input` | 当前远程路径 | 单行输入框 | 聚焦后可编辑路径。Enter 跳转。 | SFTP 已连接时启用。 |
| `sftp.back` | Back | 图标按钮 | 跳到历史上一个目录。 | 有历史时启用。 |
| `sftp.forward` | Forward | 图标按钮 | 跳到历史下一个目录。 | 有前进历史时启用。 |
| `sftp.up` | Up | 图标按钮 | 跳到父目录。 | 当前不是根目录时启用。 |
| `sftp.refresh` | Refresh | 图标按钮 | 重新列出当前目录。 | SFTP 已连接时启用。 |
| `sftp.upload` | Upload | 按钮 | 打开本地文件选择器。 | SFTP 已连接时启用。 |
| `sftp.download` | Download | 按钮 | 下载选中文件到默认本地目录。 | 选中远程文件时启用。 |
| `sftp.new_folder` | New Folder | 按钮 | 打开新建目录弹窗。 | SFTP 已连接且当前目录可写时启用。 |
| `sftp.more` | More | 下拉按钮 | 打开更多菜单。 | SFTP 已连接时启用。 |

### 9.2 SFTP 文件列表列定义

| 列 | 左键单击表头 | 右键单击表头 | 内容 |
|---|---|---|---|
| Name | 按名称排序，再次点击反向。 | 打开列显示菜单。 | 文件夹加 `/`，隐藏文件按设置显示。 |
| Size | 按大小排序。 | 打开列显示菜单。 | 文件夹显示 `-`。 |
| Permissions | 按权限字符串排序。 | 打开列显示菜单。 | 如 `rwxr-xr-x`；Windows 本地栏不显示该列。 |
| Owner | 按 owner 排序。 | 打开列显示菜单。 | 服务器支持时显示。 |
| Modified | 按修改时间排序。 | 打开列显示菜单。 | 本地化显示。 |

列宽可自定义：拖动列头左缘的分隔线调整该列宽度（Name 列吸收差值）。

### 9.3 SFTP 文件行交互

| 对象 | 左键单击 | 左键双击 | 右键单击 | 拖入 | 拖出 |
|---|---|---|---|---|---|
| 目录 | 选中。 | 进入目录。 | 打开目录菜单。 | 上传文件到该目录。 | 下载整个目录，第一版可 disabled 并提示后续支持。 |
| 文件 | 选中。 | 使用默认动作：View 或 Edit，取设置。 | 打开文件菜单。 | 覆盖上传需确认。 | 下载到本地默认目录。 |
| 多选文件 | Ctrl/Cmd 或 Shift 多选。 | 无。 | 打开批量菜单。 | 批量上传覆盖确认。 | 批量下载。 |
| 空白区域 | 清空选择。 | 无。 | 打开目录菜单。 | 上传到当前目录。 | 无。 |
| 空白区域拖动 | 框选（橡皮筋）：拖出的矩形覆盖的行被选中。 | 无。 | 无。 | 无。 | 无。 |

框选的手势细节：按下的位置是矩形的**一个角**（横竖都能伸展）；行上按住
**文件图标**才是"拖文件"（复制/移动），行上其它位置都是框选——这样列表被填满
时也能框选。单击列表空白处取消选择。

拖放的复制/移动语义与 Windows 资源管理器一致：同卷默认移动、跨卷默认复制；
按住 Ctrl 强制复制、按住 Shift 强制移动。

打开远端文件时下载在后台进行，列表上覆盖一层加载指示器（转圈 + 文件名），
界面不会卡住。

### 9.4 SFTP 文件上下文菜单

| 菜单项 | 目标 | 行为 | 确认 |
|---|---|---|---|
| Open/View | 文件 | 下载临时副本并用只读查看器或系统默认程序打开。 | 无 |
| Edit | 文件 | 下载临时副本，用系统默认编辑器打开，监听变更并提示上传。 | 上传时确认 |
| Download | 文件/目录 | 加入下载队列。 | 覆盖本地文件时确认 |
| Upload Here | 目录/空白 | 打开文件选择器并上传到目标目录。 | 覆盖远程文件时确认 |
| Rename | 文件/目录 | 行内编辑名称。 | 名称冲突时确认覆盖或取消 |
| Delete | 文件/目录/多选 | 打开删除确认。 | 必须确认 |
| New Folder | 空白/目录 | 打开新建目录弹窗。 | 无 |
| Chmod | 文件/目录 | 打开权限编辑弹窗。 | Apply 时确认 |
| Copy Remote Path | 文件/目录 | 复制远程绝对路径。 | 无 |
| Refresh | 任意 | 刷新当前目录。 | 无 |

### 9.5 SFTP 弹窗

#### New Folder Dialog

| 控件 ID | 类型 | 默认 | 校验 |
|---|---|---|---|
| `new_folder.name` | 单行输入框 | `New Folder` | 非空，不包含 `/` 或空字符。 |
| `new_folder.cancel` | 按钮 | 无 | 关闭弹窗。 |
| `new_folder.create` | 主按钮 | 无 | 校验通过后创建目录。 |

#### Chmod Dialog

| 控件 ID | 类型 | 默认 |
|---|---|---|
| `chmod.owner.read/write/execute` | checkbox | 从当前权限读取。 |
| `chmod.group.read/write/execute` | checkbox | 从当前权限读取。 |
| `chmod.other.read/write/execute` | checkbox | 从当前权限读取。 |
| `chmod.octal` | 单行输入框 | 如 `755`。 |
| `chmod.cancel` | 按钮 | 关闭。 |
| `chmod.apply` | 主按钮 | 应用 chmod。 |

## 10. Transfer Queue

| 控件 ID | 类型 | 左键行为 | 右键行为 | 状态规则 |
|---|---|---|---|---|
| `transfer.pause_all` | 按钮 | 暂停所有 queued/running 任务。 | 无 | 有活动任务时启用。 |
| `transfer.resume_all` | 按钮 | 恢复所有 paused 任务。 | 无 | 有暂停任务时启用。 |
| `transfer.clear_completed` | 按钮 | 清除 completed 任务。 | 无 | 有 completed 任务时启用。 |
| `transfer.task_row` | 列表行 | 选中任务并显示详情。 | 打开任务菜单。 | 始终可选。 |

任务行菜单：

| 菜单项 | 状态 | 行为 |
|---|---|---|
| Pause | running/queued | 暂停任务。 |
| Resume | paused | 恢复任务。 |
| Retry | failed | 重新排队。 |
| Cancel | queued/running/paused | 取消任务。 |
| Remove | completed/failed/cancelled | 从列表移除。 |
| Open Local Folder | download completed | 打开本地目录。 |
| Copy Error | failed | 复制错误详情。 |

## 11. Tunnel Panel

> **本版实现状态**：会话编辑器可配置 forwards；Dock 列表与 Start/Stop **已接线**，但 SSH runtime **尚未 provision 端口转发**（状态栏 `tunnel-not-provisioned` / `tunnel-not-running`）。Proxy 管理可测连通性，native 连接路径仍拒绝带 proxy 的会话。详见 `yshell-current-status.md` §3。

### 11.1 Tunnel 列表

| 控件 ID | 类型 | 行为 |
|---|---|---|
| `tunnel.add` | 按钮 | 打开 Tunnel Editor(create)。 |
| `tunnel.stop_all` | 危险按钮 | 弹确认后停止当前会话所有 tunnel。 |
| `tunnel.row` | 列表行 | 左键选中；右键打开菜单；双击编辑。 |

Tunnel 行菜单：

| 菜单项 | 状态 | 行为 |
|---|---|---|
| Start | stopped/failed | 启动 tunnel。 |
| Stop | running | 停止 tunnel。 |
| Restart | running/failed | 停止后重新启动。 |
| Edit | stopped/failed | 打开编辑器。 |
| Duplicate | 任意 | 复制配置并打开编辑器。 |
| Delete | stopped/failed | 删除配置，需确认。 |
| Copy Listen Address | running | 复制监听地址。 |

### 11.2 Tunnel Editor

| 控件 ID | 类型 | 默认 | 校验 |
|---|---|---|---|
| `tunnel.type` | segmented control | Local | Local/Remote/Dynamic。 |
| `tunnel.listen_host` | 单行输入框 | `127.0.0.1` | 非空。 |
| `tunnel.listen_port` | 数字输入框 | 空 | `1-65535`。 |
| `tunnel.target_host` | 单行输入框 | 空 | Dynamic 类型 disabled；其它类型非空。 |
| `tunnel.target_port` | 数字输入框 | 空 | Dynamic 类型 disabled；其它类型 `1-65535`。 |
| `tunnel.enabled_on_connect` | checkbox | false | 无。 |
| `tunnel.test` | 按钮 | 无 | 尝试绑定监听端口，不保存。 |
| `tunnel.cancel` | 按钮 | 无 | 关闭。 |
| `tunnel.save` | 主按钮 | 无 | 校验通过后保存。 |

## 12. Quick Commands Panel

> **本版不做命令预设**：Dock 卡片仅展示空态（*Command presets are not available in this build yet.*）。下列控件表为愿景规格，**不得按已交付验收**。同步输入走终端右键 N9，不依赖本面板。

| 控件 ID | 类型 | 左键行为 | 右键行为 |
|---|---|---|---|
| `commands.group_selector` | 下拉 | 切换命令组。 | 无 |
| `commands.new` | 按钮 | 打开 Command Editor(create)。 | 无 |
| `commands.command_button` | 按钮 | 发送命令到当前终端。 | 菜单：Edit、Duplicate、Delete、Send to Selected Sessions。 |
| `commands.compose.textarea` | 多行输入框 | 编辑待发送文本。 | Cut/Copy/Paste/Select All。 |
| `commands.send_current` | 主按钮 | 发送 compose 内容到当前终端。 | 无 |
| `commands.broadcast` | 危险按钮 | 打开 Broadcast Confirmation。 | 无 |
| `commands.send_key_input_to` | 危险按钮 | 打开 Send Key Input To Dialog。 | 无 |

### 12.1 Broadcast Confirmation

必须显示：

- 目标会话数量。
- 每个目标会话名称、主机、用户。
- 待发送文本预览，超过 20 行折叠。
- 确认输入框：用户必须输入 `BROADCAST`。

控件：

| 控件 ID | 类型 | 行为 |
|---|---|---|
| `broadcast.confirm_text` | 单行输入框 | 输入 `BROADCAST` 后启用 Send。 |
| `broadcast.cancel` | 按钮 | 关闭。 |
| `broadcast.send` | 危险主按钮 | 向目标会话发送文本。 |

## 13. Session Editor

Session Editor 是 modal dialog，宽度 `760px`，高度不超过屏幕 `85%`，包含左侧 section nav 和右侧表单。

### 13.1 左侧 section nav

| 项 | 左键行为 |
|---|---|
| General | 显示通用连接信息。 |
| Authentication | 显示认证设置。 |
| Terminal | 显示终端设置。 |
| SFTP | 显示 SFTP 设置。 |
| Tunnels | 显示端口转发设置。 |
| Proxy | 显示代理设置。 |
| Logging | 显示日志设置。 |
| Advanced | 显示 keepalive、环境变量等高级设置。 |
| Appearance | 显示该 session 的外观覆盖设置。 |

### 13.2 General 表单

| 控件 ID | 类型 | 默认 | 校验 |
|---|---|---|---|
| `session.name` | 单行输入框 | 空 | 非空，同文件夹下不重名。 |
| `session.folder` | 下拉/选择器 | 当前文件夹 | 必须存在。 |
| `session.host` | 单行输入框 | 空 | 非空，可为域名/IP。 |
| `session.port` | 数字输入框 | `22` | `1-65535`。 |
| `session.username` | 单行输入框 | 空 | 允许空；空时连接时提示。 |
| `session.tags` | token input | 空 | 每个 tag 最长 32 字符。 |
| `session.color` | 色块选择 | 自动 | 从预设色板选择。 |

### 13.3 Authentication 表单

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `auth.method` | segmented control | Password | Password / Private Key / Agent / Keyboard Interactive。 |
| `auth.password.save` | checkbox | false | 勾选后密码保存到 SecretStore。 |
| `auth.password.input` | password input | 空 | 仅 Password 显示。 |
| `auth.key.path` | 文件路径输入 | 空 | Browse 选择私钥文件。 |
| `auth.key.browse` | 按钮 | 无 | 打开文件选择器。 |
| `auth.key.passphrase.save` | checkbox | false | 保存 passphrase 到 SecretStore。 |
| `auth.agent_forwarding` | checkbox | false | 启用 agent forwarding。 |
| `auth.test` | 按钮 | 无 | 使用当前表单执行测试连接，不保存。 |

### 13.4 Terminal 表单

| 控件 ID | 类型 | 默认 |
|---|---|---|
| `terminal.type` | 下拉 | `xterm-256color` |
| `terminal.encoding` | 下拉 | `UTF-8` |
| `terminal.font_family` | 字体选择 | 系统 monospace |
| `terminal.font_size` | 数字输入 | `13` |
| `terminal.line_height` | 数字输入 | `1.2` |
| `terminal.color_scheme` | 下拉 | Default Dark |
| `terminal.cursor_shape` | 下拉 | Block |
| `terminal.blink_cursor` | checkbox | true |
| `terminal.scrollback_lines` | 数字输入 | `10000` |

### 13.5 SFTP 表单

| 控件 ID | 类型 | 默认 | 校验 |
|---|---|---|---|
| `sftp.default_remote_dir` | 单行输入框 | 空 | 空表示登录目录；非空必须以 `/` 开头。 |
| `sftp.default_local_dir` | 路径输入 | Downloads | 必须是本地有效目录。 |
| `sftp.confirm_overwrite` | checkbox | true | 无。 |
| `sftp.preserve_timestamp` | checkbox | true | 无。 |
| `sftp.show_hidden_files` | checkbox | true | 无。 |
| `sftp.transfer_parallelism` | 数字输入 | `2` | `1-8`。 |

### 13.6 Proxy 表单

| 控件 ID | 类型 | 默认 | 校验 |
|---|---|---|---|
| `proxy.mode` | segmented control | Inherit | Inherit / None / Global / Custom。 |
| `proxy.protocol` | 下拉 | SOCKS5 | SOCKS4 / SOCKS4a / SOCKS5 / HTTP CONNECT。 |
| `proxy.host` | 单行输入框 | 空 | Custom 时非空。 |
| `proxy.port` | 数字输入 | 空 | Custom 时 `1-65535`。 |
| `proxy.resolve_dns_by_proxy` | checkbox | true | SOCKS4 disabled；SOCKS4a/SOCKS5 可用。 |
| `proxy.username` | 单行输入框 | 空 | SOCKS5/HTTP CONNECT 可用。 |
| `proxy.password.save` | checkbox | false | 勾选后保存到 SecretStore。 |
| `proxy.password.input` | password input | 空 | SOCKS5/HTTP CONNECT 可用。 |
| `proxy.test` | 按钮 | 无 | 执行 TCP、proxy handshake、SSH handshake 三阶段测试。 |

### 13.7 Logging 表单

| 控件 ID | 类型 | 默认 | 校验 |
|---|---|---|---|
| `logging.mode` | segmented control | Inherit | Inherit / Disabled / Enabled。 |
| `logging.format` | 下拉 | Sanitized Text | Raw Transcript / Sanitized Text。 |
| `logging.record_remote_output` | checkbox | true | 第一版默认 true，且为日志核心能力。 |
| `logging.record_local_input` | checkbox | false | 勾选时弹安全确认。 |
| `logging.include_timestamps` | checkbox | true | 无。 |
| `logging.path_template` | 单行输入框 | `{date}/{session}-{host}.log` | 非空，变量必须来自允许列表。 |
| `logging.open_folder` | 按钮 | 无 | 打开日志目录。 |

日志按钮右键菜单：

| 菜单项 | 行为 | 状态 |
|---|---|---|
| Start Logging | 开启当前会话日志。 | 未开启时启用。 |
| Stop Logging | 停止当前会话日志。 | 已开启时启用。 |
| Open Current Log | 打开当前日志文件。 | 已生成日志文件时启用。 |
| Open Log Folder | 打开日志目录。 | 始终启用。 |
| Logging Settings | 跳转 Session Editor Logging section。 | 当前 session 可编辑时启用。 |

### 13.8 Appearance 表单

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `appearance.mode` | segmented control | Inherit | Inherit / Custom。 |
| `appearance.theme_mode` | 下拉 | Inherit | System / Light / Dark。 |
| `appearance.terminal_color_scheme` | 下拉 | Inherit | 选择终端配色。 |
| `appearance.font_family` | 字体选择 | Inherit | 选择终端字体。 |
| `appearance.font_size` | 数字输入 | Inherit | `8-36`。 |
| `appearance.line_height` | 数字输入 | Inherit | `1.0-2.0`。 |
| `appearance.cursor_shape` | 下拉 | Inherit | Block / Bar / Underline。 |
| `appearance.tab_color` | 色块选择 | Inherit | 设置标签颜色。 |
| `appearance.show_sftp_on_connect` | checkbox | Inherit | 连接后自动显示 SFTP。 |

继承提示：

- 每一项显示最终 resolved 值，例如 `Inherited from folder Production: Dark`。
- 用户点击 Reset Field 恢复单项继承。
- 用户点击 Reset All 恢复整个 session 的外观继承。

### 13.9 底部按钮

| 控件 ID | 文案 | 类型 | 行为 | 状态 |
|---|---|---|---|---|
| `session_editor.test` | Test Connection | 按钮 | 使用当前表单发起测试连接。 | host/port 基本合法时启用。 |
| `session_editor.cancel` | Cancel | 按钮 | 若有未保存改动，弹 Discard Changes。 | 始终启用。 |
| `session_editor.save` | Save | 主按钮 | 校验并保存。 | 表单合法且有改动时启用。 |
| `session_editor.save_connect` | Save and Connect | 主按钮 | 保存后打开标签并连接。 | 表单合法时启用。 |

## 13.10 Folder Editor

Folder Editor 用于保存文件夹级默认配置。

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `folder.name` | 单行输入框 | 空 | 非空，同级不重名。 |
| `folder.parent` | 下拉/选择器 | 当前父级 | 选择父文件夹。 |
| `folder.appearance.mode` | segmented control | Inherit | Inherit / Custom。 |
| `folder.logging.mode` | segmented control | Inherit | Inherit / Disabled / Enabled。 |
| `folder.proxy.mode` | segmented control | Inherit | Inherit / None / Global / Custom。 |
| `folder.apply_preview` | 只读列表 | 无 | 显示受影响的子文件夹和 session 数量。 |
| `folder.cancel` | 按钮 | 无 | 关闭。 |
| `folder.save` | 主按钮 | 无 | 保存文件夹配置。 |

保存文件夹级配置时，不弹出逐个 session 确认；但必须在 preview 中显示影响范围。

## 13.11 Send Key Input To Dialog

触发入口：

- Quick Commands Panel 的 `Send Key Input To`。
- 终端右键菜单 `Send Key Input To...`。
- 标签右键菜单 `Send Key Input To...`。
- 每个可见 SSH pane header 上的接收开关。

控件：

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `key_broadcast.source_session` | 只读文本 | 当前 active SSH pane | 显示源窗口。 |
| `key_broadcast.preset` | segmented/dropdown | All Visible | All SSH / All Visible / All Connected / Current Split / Current Tab Group / Same Folder / Manual。 |
| `key_broadcast.target_toggles` | checkbox list | 按 preset 自动计算 | 每一行表示一个 SSH 窗口；用户可直接勾选/取消。 |
| `key_broadcast.filter.username` | checkbox + 输入 | false | 可选：只匹配同用户名或输入的用户名。 |
| `key_broadcast.filter.host_prefix` | checkbox + 输入 | false | 可选：按 host 前缀匹配。 |
| `key_broadcast.filter.tags` | token input | 空 | 可选：匹配 session tag。 |
| `key_broadcast.event_scope` | 只读说明 | All terminal input events | 明确会复制字符、Enter、Backspace、方向键、Ctrl/Alt 组合、粘贴等。 |
| `key_broadcast.preview` | 只读文本 | 无 | 显示将接收键输入的目标数量和名称。 |
| `key_broadcast.confirm_text` | 单行输入框 | 空 | 必须输入 `KEYS` 才启用 Start。 |
| `key_broadcast.cancel` | 按钮 | 无 | 关闭。 |
| `key_broadcast.start` | 危险主按钮 | disabled | 启动 Send Key Input To。 |

排除规则：

- 排除源会话自身。
- 排除 disconnected、connecting、error 状态会话。
- 排除没有交互式 Shell channel 的会话。
- 排除 SFTP-only 操作。
- 排除当前正在接收其它 Send Key Input To 的会话，避免循环。

启用期间：

- 源终端每次终端输入事件先发送给源 SSH，再复制到目标 SSH。
- 复制的事件包括普通字符、Enter、Backspace、Tab、方向键、Home/End、PageUp/PageDown、功能键、Ctrl/Alt 组合和粘贴。
- 不复制 UI 操作，例如切换标签、打开菜单、拖拽 pane、SFTP 操作。
- **本版（D23 / N9）**：不弹 KEYS / 大段粘贴 / Ctrl+C 确认；用状态栏 chip + 控制键警示替代。下列「粘贴确认 / Ctrl+C 确认」为旧契约残留，**不以本版验收**。
- 停止后不关闭任何 SSH 会话。

## 14. Settings

Settings 是 dialog，左侧分类，右侧表单。

分类：

- General：语言（system / zh-CN / en-US）。
- Appearance：终端外观入口（配色、字体；保存后对新开终端生效）。
- Terminal：全局终端默认值（scrollback 行数 / 单元上限）。
- Security：SecretStore 状态、配置目录；跳转独立 **Host Keys** / **Private Keys** 管理页（不内嵌）。
- Advanced：配置目录展示；本版本不提供整库 wipe secrets / 重置布局。
- SFTP（Settings 分类）：**本版启用**（全局 SFTP 默认）；与 Session Editor 内 SFTP 分区「本版不做」不同。
- Shortcuts：本版本禁用，tooltip 说明未提供。
- Secret Store：Security 展示状态；解锁依赖环境变量（见 status §3），本版无 in-app 主密码向导；亦不提供 wipe。

Settings **不**使用 Danger Zone，也 **不**提供打字令牌确认的 `RESET SECRETS` / `CLEAR` 入口。

Host Keys / Private Keys 由 Session 菜单打开（与 Settings 同级独立弹窗）。破坏性操作用普通 `ConfirmDialog`（`destructive`，无确认词）：

| 控件 ID | 行为 | 确认 |
|---|---|---|
| `host_keys.remove` | 删除选中 `host:port` 整组 known hosts。 | 普通确认。 |
| `host_keys.clear_all` | 清空全部 known hosts。 | 普通确认。 |
| `private_keys.remove` | 删除选中托管私钥。 | 普通确认（含被引用告警）。 |
| `private_keys.deploy` | 向当前会话部署公钥。 | 普通确认。 |
| `settings.advanced.open_config_dir` | 打开配置目录。 | 无（本版本可仅展示路径）。 |

## 15. 安全弹窗

### 15.1 Host Key First Trust

触发：首次连接未知 host key。

必须显示：

- host、port、username。
- key type。
- SHA256 fingerprint。
- known_hosts 保存路径。

按钮：

| 控件 ID | 文案 | 行为 |
|---|---|---|
| `host_key_first.cancel` | Cancel | 取消连接。 |
| `host_key_first.trust_once` | Trust Once | 本次连接信任，不写入 known_hosts。 |
| `host_key_first.trust_save` | Trust and Save | 写入 known_hosts 并继续。 |

默认焦点：Cancel。Enter 不触发 Trust，必须鼠标点击或 Tab 选择后确认。

### 15.2 Host Key Changed

触发：已知 host key 与当前不一致。

必须是阻断弹窗，不能自动继续。

按钮：

| 控件 ID | 文案 | 行为 |
|---|---|---|
| `host_key_changed.cancel` | Cancel | 取消连接。 |
| `host_key_changed.copy_details` | Copy Details | 复制旧/新 fingerprint 和路径。 |
| `host_key_changed.replace` | Replace Key | 需输入 `REPLACE` 后启用，替换 known_hosts 并连接。 |

## 16. 退出与关闭确认

### 16.1 Close Tab Confirmation

触发：关闭有活动 SSH/SFTP/transfer/tunnel 的标签。

显示：

- 会话名。
- 活动 SSH 状态。
- SFTP 传输任务数量。
- 端口转发数量。

按钮：

| 控件 ID | 文案 | 行为 |
|---|---|---|
| `close_tab.cancel` | Cancel | 返回。 |
| `close_tab.disconnect_close` | Disconnect and Close | 停止 SSH/SFTP/tunnel 并关闭。 |
| `close_tab.keep_running` | Keep Running in Background | 第一版 disabled，tooltip 说明不支持后台会话。 |

### 16.2 Quit Confirmation

触发：退出应用且存在活动会话。

按钮：

| 控件 ID | 文案 | 行为 |
|---|---|---|
| `quit.cancel` | Cancel | 返回应用。 |
| `quit.disconnect_quit` | Disconnect and Quit | 断开所有连接并退出。 |

## 17. 交互跳转表

| 起点 | 动作 | 目标 | 数据传递 |
|---|---|---|---|
| Quick Connect input | Enter | Quick Auth Dialog 或 Terminal Tab | 解析后的 host/port/username。 |
| Session Tree Session | 双击 | Terminal Tab（活动标签已断开/未连接时占用该标签，否则新开） | SessionProfile id。 |
| Terminal Tab | 切换为 SFTP / 点击 SFTP | SFTP Tab（同一标签） | 当前 SessionHandle id + SSH transport。 |
| SFTP Tab | 切换为 Terminal | Terminal Tab（同一标签） | 当前 SessionHandle id + SSH transport。 |
| SFTP 文件双击 | 文件 | Remote Edit Flow | remote path。 |
| SFTP 文件双击 | 目录 | SFTP 当前目录 | remote path。 |
| Tunnel Add | 左键 | Tunnel Editor | 当前 SessionProfile id。 |
| Command Button | 左键 | Terminal | command text。 |
| Broadcast Button | 左键 | Broadcast Confirmation | selected session ids + command text。 |
| Send Key Input To Button | 左键 | Send Key Input To Dialog | source pane id + target preset。 |
| Pane Receive Key Input Toggle | 左键 | Update Send Key Input Targets | pane session id + checked state。 |
| Host Key Prompt | Trust and Save | Terminal Tab | host key record。 |
| Session Editor Save and Connect | 左键 | Terminal Tab | saved SessionProfile id。 |

## 18. UI 验收清单

- 每个可点击控件必须有唯一控件 ID。
- 每个 disabled 控件必须有 tooltip。
- 每个危险操作必须有确认。
- 每个表单输入必须有校验规则。
- 每个右键菜单必须有定义。
- 每个双击动作必须有定义。
- 每个拖拽动作必须有定义；未定义区域不得响应拖拽。
- 每个跳转必须定义目标页面和传递数据。
- 每个安全弹窗默认焦点必须是安全按钮。
- SFTP 标签必须能从保存会话右键、终端标签右键和终端内容右键直接打开，并与 Terminal 标签原位互切。
