# YShell UI 交互契约

> **性质（2026-11-05）：参考文档，不是设计稿。**  
> - **产品范围** → [`yshell-current-status.md`](./yshell-current-status.md)  
> - **UI 设计（功能 / 布局 / 交互）** → [`docs/prototypes/`](../prototypes/README.md) 及原型规则  
> 下文含大量早期愿景（如「第一版最多 4 pane」、Favorites 等）。与原型或 status 冲突时 **以原型 / status 为准**；不得按本文「补齐」实现。

更新注记（2026-10-29，仍有效）：范围与砍项以 status 为准；标注「本版不做」或与 status §2 冲突的，实现须 disabled/空态/无入口。

原独立 `context-menu-design.md` 已并入各区域菜单节并迁入 `archive/`。

## 1. 全局规则

### 1.1 鼠标规则

| 操作 | 全局含义 |
|---|---|
| 左键单击 | 选择、聚焦、触发主动作。 |
| 左键双击 | 打开、连接、进入目录、编辑当前对象。 |
| 右键单击 | 默认打开当前对象的上下文菜单；终端可被 Settings → Terminal → Right click 覆盖（Paste / Do nothing）。 |
| 鼠标悬停 | 显示 tooltip；危险按钮 tooltip 必须说明风险。 |
| 拖拽 | 只用于标签排序、分屏移动、SFTP 上传、SFTP 下载、面板宽度调整。 |
| 中键单击 | 标签栏：关闭对应标签；终端：按 Settings → Terminal → Middle click（默认 Paste）；其他区域无动作。 |

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

主窗口从上到下、从左到右为以下区域（**N3 ToolDock**：两侧是可换边的面板栈，不是固定 sidebar）：

```text
AppMenu
MainToolbar
Left stack | WorkspaceTabs + TerminalWorkspace | Right stack
StatusBar
```

- **Left / Right stack**：有序竖排 `PanelFrame`（Sessions / Tunnels / QuickCommands / Transfers）。每面板 `PanelPlacement = Left | Right | Hidden`；标题 `⋯`：Move to Left / Move to Right、Collapse、Hide；同栈上下分栏可拖。
- **默认布局（非锁死）**：左栈 Sessions；右栈 Tunnels + QuickCommands。Transfers 常通过 View 或传输任务显示。SFTP **不是** dock 面板，是工作区标签。
- 布局持久化见 status「面板停靠 N3」与 `[ui.layout]`。

默认尺寸：

- 最小窗口：`1120 x 720`。
- 默认窗口：`1440 x 900`。
- 左栈默认宽度：`260px`，可拖拽范围 `200-420px`（按边记忆）。
- 右栈默认宽度：`330px`，可拖拽范围 `280-560px`（按边记忆）。
- StatusBar 高度：`30px`。

## 3. AppMenu

### 3.1 File 菜单

| 控件 ID | 文案 | 类型 | 左键行为 | 快捷键 | 状态规则 |
|---|---|---|---|---|---|
| `menu.file.new_session` | New Session | 菜单项 | 打开 Session Editor，模式为 create。 | `Ctrl+N` | 始终启用。 |
| `menu.file.quick_connect` | Quick Connect | 菜单项 | 打开/聚焦快速连接页输入框。 | `Ctrl+O` | 始终启用。 |
| `menu.file.save_active` | Save Active Session | 菜单项 | 将当前活动会话写入 sessions/。 | 视实现 | 有活动会话时启用。 |
| `menu.file.settings` | Settings | 菜单项 | 打开 Settings 页面。 | `Ctrl+,` | 始终启用。 |
| `menu.file.quit` | Quit | 菜单项 | 若存在活动会话，打开 Quit Confirmation；否则退出。 | `Ctrl+Q` / `Cmd+Q` | 始终启用。 |

> **本版 File 菜单**：无 Import / Export Config、无 New Folder 菜单项。新建根文件夹走会话树空白区「New Folder」。Import/Export 属本版不做（见 `yshell-current-status.md` §2）。

### 3.2 Edit 菜单

| 控件 ID | 文案 | 类型 | 左键行为 | 快捷键 | 状态规则 |
|---|---|---|---|---|---|
| `menu.edit.copy` | Copy | 菜单项 | 复制当前终端选区或 SFTP 选中文件路径。 | `Ctrl+C` | 有可复制内容时启用。 |
| `menu.edit.paste` | Paste | 菜单项 | 粘贴到当前焦点输入框或终端。 | `Ctrl+V` | 剪贴板非空时启用。 |
| `menu.edit.select_all` | Select All | 菜单项 | 终端全选可见缓冲，SFTP 全选列表。 | `Ctrl+A` | 当前区域支持选择时启用。 |
| `menu.edit.find` | Find | 菜单项 | 打开当前区域搜索栏。 | `Ctrl+F` | 终端或 SFTP 有焦点时启用。 |
| `menu.edit.clear_terminal` | Clear Terminal | 菜单项 | 清屏但不清除 scrollback。 | `Ctrl+L` | 当前标签已连接时启用。 |
| `menu.edit.clear_scrollback` | Clear Scrollback | 菜单项 | 打开确认弹窗，确认后清除 scrollback。 | 无 | 当前标签存在终端时启用。 |

### 3.3 Session 菜单

| 控件 ID | 文案 | 类型 | 左键行为 | 快捷键 | 状态规则 |
|---|---|---|---|---|---|
| `menu.session.connect` | Connect | 菜单项 | 连接当前选中会话。 | `Enter` on selected session | 选中未连接会话时启用。 |
| `menu.session.disconnect` | Disconnect | 菜单项 | 断开当前标签 SSH 和关联 SFTP。 | 无 | 当前标签已连接时启用。 |
| `menu.session.reconnect` | Reconnect | 菜单项 | 断开后重新连接当前会话。 | `F5` when disconnected | 当前标签断开或错误时启用。 |
| `menu.session.duplicate_tab` | Duplicate Tab | 菜单项 | 使用相同 SessionProfile 打开新标签并连接。 | 无 | 当前标签有会话 profile 时启用。 |
| `menu.session.edit` | Edit Session | 菜单项 | 打开 Session Editor，模式为 edit。 | 无 | 选中保存的会话时启用。 |
| `menu.session.open_sftp` | Open SFTP | 菜单项 | 在当前会话上打开 SFTP 标签（固定本地/远程左右两栏）。 | `Ctrl+Shift+S` | 当前标签已连接时启用。 |
| `menu.session.tunnels` | Tunnels | 菜单项 | 显示 Tunnels 面板（落在其当前栈侧；默认右栈）。 | 无 | 当前标签存在时启用。 |

### 3.4 View 菜单

| 控件 ID | 文案 | 类型 | 左键行为 | 快捷键 | 状态规则 |
|---|---|---|---|---|---|
| `menu.view.session_manager` | Sessions | 勾选菜单项 | 显示/隐藏 Sessions 面板（可停左或右栈；默认左）。 | `Ctrl+B` | 始终启用。 |
| `menu.view.transfers` | Transfers | 勾选菜单项 | 显示/隐藏 Transfers 面板（可停任一栈）。 | 无 | 始终启用。 |
| `menu.view.quick_commands` | Quick Commands | 勾选菜单项 | 显示/隐藏 Quick Commands 面板。 | 无 | 始终可勾选；**面板内容本版未提供**（空态说明）。 |
| `menu.view.tunnel_panel` | Tunnels | 勾选菜单项 | 显示/隐藏 Tunnels 面板。 | 无 | 始终启用。配置可编辑；**Start/Stop 转发 runtime 尚未 provision**（见 status §3）。 |
| `menu.view.fullscreen` | Full Screen | 勾选菜单项 | 切换全屏。 | `F11` | 始终启用。 |
| `menu.view.focus_mode` | Focus Mode | 勾选菜单项 | 隐藏左右栈与工具栏，仅保留标签、终端、状态栏。 | 无 | 当前存在终端标签时启用。 |

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

## 5. Sessions panel

Sessions 是可停靠工具面板（`PanelId::Sessions`），内容为会话树；可放在 **左栈或右栈**（默认左）。面板外壳（标题 / ⋯ Move·Collapse·Hide / 折叠）见 §8 与 N3 `PanelFrame`；本节只规定树内容。实现组件名可能仍为 `session_sidebar.slint`。

### 5.1 顶部区域

| 控件 ID | 文案/占位 | 类型 | 左键行为 | 右键行为 | 状态规则 |
|---|---|---|---|---|---|
| `sidebar.header.title` | SAVED / 会话树标题 | 文本 | 无 | 无 | 始终显示。 |
| `sidebar.search.input` | Search sessions | 单行输入框 | 聚焦搜索。 | 清空/粘贴上下文菜单。 | 始终启用。 |

> **本版（S1）**：面板内常驻 `+` / `…` 树工具已删除（面板级 `⋯` 仍负责 Move/Collapse/Hide）；新建会话/刷新走空白区右键（及 File / CommandBar New Session）。Folder New Folder / Rename / Delete 本版不做（见 status §2）。

### 5.2 会话树项目

| 对象 | 左键单击 | 左键双击 | 右键单击 | 拖拽 |
|---|---|---|---|---|
| Folder | 选中。 | 展开/折叠。 | 菜单：New Session Here、Edit（共用 Session Editor）、New Folder / Rename / Delete（本版不做）。 | 可拖到其它 Folder 下。 |
| Session | 选中并在详情区显示摘要。 | 连接：活动标签已断开/未连接（且不是快速连接页）时占用该标签，否则打开新标签。 | 菜单：Connect、Open in New Tab、Open SFTP、Edit、Duplicate、Favorite、Delete、Export。 | 可拖到 Folder 下或改变排序。 |
| Favorite Session | 选中。 | 打开新标签并连接。 | 菜单：Connect、Remove Favorite、Edit。 | 可在 Favorites 内排序。 |
| Recent Session | 选中。 | 打开新标签并连接。 | 菜单：Connect、Pin to Favorites、Remove from Recent。 | 不支持拖拽。 |

### 5.3 会话树上下文菜单

| 菜单项 | 目标对象 | 行为 | 确认 |
|---|---|---|---|
| Connect | Session | 当前工作区打开新标签并连接。 | 无 |
| Open in New Tab | Session | 新建标签但不切断其它标签。 | 无 |
| Open SFTP | Session | 新建 SFTP 标签；未连接时先连接。 | 未保存认证时打开认证弹窗。 |
| Edit | Session / Folder | 打开共用 Session Editor（会话或文件夹模式）。文件夹模式下 Host/Port/Username 不可编辑。 | 无 |
| Duplicate | Session | 复制配置，名称追加 `Copy`，打开编辑器。 | 无 |
| Rename | Folder/Session | 行内编辑名称。 | Enter 保存，Esc 取消。 |
| Delete | Folder/Session | 移入确认弹窗。Folder 删除会列出子项数量。 | 必须确认 |
| Favorite | Session | 加入收藏。 | 无 |
| Export | Folder/Session | 打开导出向导。 | 无 |

## 6. WorkspaceTabs

### 6.1 标签项

标签有三种模式：New Tab（快速连接）、Terminal、SFTP。三者共享同一套标签条、分屏、拖拽和关闭机制；模式差异只体现在内容区、标签图标和模式专属菜单项。`+` / `Ctrl+T` / 启动应用都创建一个 New Tab；多个 New Tab 相互独立。

**来源标志**：由 MCP/AI 创建的连接在标签名称旁显示 **AI** 标志（实心 chip）；用户创建的标签不显示。Rename / Switch mode 不清除该标志。

| 区域 | 左键单击 | 左键双击 | 右键单击 | 中键单击 | 拖拽 |
|---|---|---|---|---|---|
| 标签主体 | 切换到该标签。 | 已断开/未连接的标签：重连该标签；仍连着的标签：复制同源会话（新标签）。 | 打开标签上下文菜单。 | 关闭标签。 | 调整顺序或拖入分屏。 |
| 关闭按钮 `x` | 关闭标签。 | 无 | 无 | 无 | 无 |
| 状态图标 | 显示状态 tooltip。 | 无 | 无 | 无 | 无 |
| AI 标志 | 仅展示（tooltip：Created by AI）。 | 无 | 无 | 无 | 无 |

关闭窗口内最后一个标签等同点击窗口关闭：主窗口进入退出确认，次窗口直接关闭。

### 6.2 标签上下文菜单

| 菜单项 | 行为 | 状态 |
|---|---|---|
| Reconnect | 重新连接当前会话。 | Terminal / SFTP 标签；断开或错误时启用。 |
| Disconnect | 断开 SSH / SFTP 连接。 | Terminal / SFTP 标签；已连接时启用。 |
| Rename Tab | 行内编辑标签名，仅影响当前 tab。 | 始终启用。 |
| Split Right | 将标签移入右侧分屏。 | 当前工作区可分屏时启用。 |
| Split Down | 将标签移入下方分屏。 | 当前工作区可分屏时启用。 |
| Sync Input for This Tab | 勾选/取消该标签是否加入输入同步组（组模型，无源/目的角色）。 | 仅 Terminal 标签（connected SSH shell）。 |
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

模式过滤：New Tab 隐藏 Reconnect / Disconnect / Sync Input / Switch；SFTP 隐藏 Sync Input。隐藏项不占菜单高度。

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
| 中键单击 | 按 Settings → Terminal → Middle click：Paste（默认）/ Do nothing。 |
| 右键（Context menu）单击选区 | 打开 Copy/Paste/Select Screen/Select All/Find/Find Across SSH/Clear 菜单。 |
| 右键（Context menu）单击无选区 | 打开 Paste/Select Screen/Select All/Find/Find Across SSH/Clear 菜单。 |
| 右键（Paste / Do nothing） | 不弹菜单；Paste 走剪贴板并遵循 Multi-line paste，Do nothing 无动作。 |
| 鼠标滚轮 | 滚动 scrollback。 |
| Shift + 鼠标滚轮 | 水平滚动，若终端启用横向滚动。 |

### 7.2 终端上下文菜单

| 菜单项 | 行为 | 状态 |
|---|---|---|
| Copy | 复制选区。 | 有选区时启用。 |
| Copy Current Line | 复制当前光标行。 | 当前行存在内容时启用。 |
| Copy Screen | 复制当前可见屏幕内容。 | 当前屏幕存在内容时启用。 |
| Paste | 粘贴剪贴板；多行时按 Settings → Terminal → Multi-line paste。 | 剪贴板非空且已连接时启用。 |
| Paste as Bracketed | 使用 bracketed paste 包裹粘贴。 | 剪贴板非空且终端支持时启用。 |
| Select Screen | 仅选择当前可见屏幕内容。 | 当前屏幕存在内容时启用。 |
| Select All | 选择可见终端和 scrollback。 | 始终启用。 |
| Find | 打开终端搜索栏。 | 始终启用。 |
| Find Regex | 打开终端搜索栏并启用正则模式。 | 始终启用。 |
| Find Across SSH | 打开跨 SSH 查找面板。 | 至少存在 2 个 SSH terminal 时启用。 |
| Clear Screen | 发送清屏并清空当前屏幕。 | 已连接时启用。 |
| Clear Scrollback | 清除 scrollback，弹确认。 | scrollback 非空时启用。 |
| Save Output As | 将 scrollback 保存为文本。 | scrollback 非空时启用。 |
| Sync Input for This Session | 把当前终端加入/移出输入同步组（无源/目的角色）。 | 当前终端已连接时启用。 |
| Pause Sync Input / Resume Sync Input | 暂停/恢复广播，组成员保留、连接不断。 | 同步组非空时启用。 |
| Stop Sync Input | 清空同步组（所有成员退出同步）。 | 同步组非空时启用。 |

> 同步输入是**组模型**（见 §8.1）：没有 SRC/RCV 角色，也没有 All SSH / All Visible / All Connected 预设；启停只改变“哪些会话在组里”。

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

## 8. ToolDock panels（N3）

可停靠面板不是固定「右侧 Tab」：每个面板是独立 `PanelFrame`，可出现在左栈或右栈，同栈内上下分栏。活动 dock 面板：`Sessions`、`Tunnels`、`QuickCommands`、`Transfers`。`Local` / `SFTP` **不是** dock 面板；SFTP 是工作区标签（本地左栏 / 远程右栏）。默认右栈常显示 `Tunnels` + `QuickCommands`；`Transfers` 多由 View 或传输任务打开。

| Panel | 内容要点 | 面板级菜单（⋯） |
|---|---|---|
| Sessions | 会话树（见 §5） | Move to Left/Right · Collapse · Hide |
| Transfers | 传输队列；队列项可 Clear Completed / Pause / Resume（内容区或上下文） | 同上 |
| Tunnels | 端口转发列表；Add / Start/Stop（runtime 本版未 provision，见 status §3） | 同上 |
| Quick Commands | 本版仅空态（见 status §2） | 同上 |

面板拖拽换边/换序与栏宽、分栏比例持久化见 status N3 与 `[ui.layout]`。原型：`docs/prototypes/main-shell/tool-dock.html`、`main-shell/panels/*`。

## 8.1 Input Sync（同步输入，N9）

同步输入是**双向的组模型**，没有固定的源/目的地址、没有 SRC/RCV 角色、也没有“选择目标”的预设（All SSH / All Visible / All Connected 等一律废弃）。

规则：

- 每个终端会话有一个 **Sync Input 开关**（终端右键 / 标签右键 / pane header 开关，语义相同）。开关为 On 的已连接 SSH 会话组成**同一个同步组**。
- 任意组成员里的终端输入（字符、Enter、Backspace、Tab、方向键、Ctrl/Alt 组合、粘贴等）都会复制给**其它所有组成员**。实际使用中焦点会话只有一个，因此等价于“焦点会话输入 → 组内其它会话”。
- 注入到其它成员的输入**不能再触发广播**（输入注入路径与用户按键路径分开），不产生回环。
- 不在组里的会话完全不受影响；断开/关闭的会话自动退出组。
- **Pause**：保留组、暂停广播；**Stop**：清空组。
- **AI/MCP 输入自动退出同步**：AI/MCP 向某会话发送输入、按键或执行命令时，该会话的 Sync Input 自动关闭（AI 输入不得被广播），并在状态栏留一条一次性说明。注入的 AI 输入同样不广播。
- 不弹 KEYS / 大段粘贴 / Ctrl+C 打字确认（D23）；控制键广播只在状态栏 chip 提示。

状态栏 chip（本版实现）：

| 控件 ID | 类型 | 行为 |
|---|---|---|
| `key_broadcast.chip` | 状态栏 chip | 同步组非空时显示 `SYNC → n targets`，`n` = 相对焦点会话的其它组成员数；Pause 时标 `PAUSED`。点击 = 停止同步（`Stop Sync Input`）。 |
| `key_broadcast.notice` | 状态栏 chip 提示位 | 一次性提示：控制键已广播、成员因断开退出、AI 输入导致该会话退出同步等；点击 = 清除提示。 |

终端上方 banner（原型目标，**本波不做**）：

原型 `main-shell/workspace/sync-input.html` 保留 Input Sync banner（组会话数、Pause、Stop、控制键警示）作为目标形态；**本波不实现 banner**，入口与反馈只走状态栏 chip + 菜单项。banner 留待波 3。

pane header（分屏时）与标签徽标：

| 控件 ID | 类型 | 行为 |
|---|---|---|
| `pane.sync_input_toggle` | switch + On/Off | 将当前 pane 加入/移出同步组；仅 connected SSH shell pane 启用。 |
| `tab.sync_badge` | 静态徽标 | 组成员标签显示 `SYNC` 徽标（不再是 SRC/RCV）；非成员不显示。 |

原型：`docs/prototypes/main-shell/workspace/sync-input.html`。

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
| Edit | 文件 | 下载临时副本，用 Settings → SFTP External editor（System default / 已检测编辑器 / Custom 路径）打开；每次保存立即自动上传（对齐 Xftp）。监视在编辑器进程退出或 SFTP/SSH 断开后结束并清临时文件。 | 无（保存即传；不另弹确认） |
| Download | 文件/目录 | 加入下载队列。 | 覆盖本地文件时确认 |
| Upload Here | 目录/空白 | 打开文件选择器并上传到目标目录。 | 覆盖远程文件时确认 |
| Rename | 文件/目录 | 行内编辑名称。 | 名称冲突时确认覆盖或取消 |
| Delete | 文件/目录/多选 | 打开删除确认。 | 必须确认 |
| New Folder | 空白/目录 | 列表行内插入新行编辑。 | 无 |
| Permissions | 远程文件/目录 | 打开独立 compact 权限窗（`sftp-chmod`）。 | Apply 应用 |
| Copy Remote Path | 文件/目录 | 复制远程绝对路径。 | 无 |
| Refresh | 任意 | 刷新当前目录。 | 无 |

### 9.5 SFTP 弹窗

Rename / New Folder 均为列表行内编辑，不存在独立弹窗。

#### Chmod Dialog

独立 compact `560×400`；原型：`sftp-chmod/index.html`。

| 控件 ID | 类型 | 默认 |
|---|---|---|
| `chmod.path` | 只读文本 | 目标绝对路径。 |
| `chmod.octal` | 单行输入框 | `755`；与九个 checkbox 双向同步。 |
| `chmod.owner.read/write/execute` | checkbox | 从当前权限读取。 |
| `chmod.group.read/write/execute` | checkbox | 从当前权限读取。 |
| `chmod.other.read/write/execute` | checkbox | 从当前权限读取。 |
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
| `tunnel.enabled_on_connect` | switch + Enabled/disabled | false | 无。 |
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

## 13. Session Editor（Session / Folder 共用）

Session Editor 是 modal dialog（原型壳 `1100×820`），包含左侧 section nav 和右侧表单。**文件夹与会话共用同一编辑器**：可配置字段均可从父级 Inherit（会话 → 最近祖先文件夹 → 全局）；**Host / Port / Username 是连接身份，不可继承**——编辑文件夹时这些字段不可用（隐藏）。Folder 右键 **Edit** 打开本编辑器（文件夹模式），不再有独立 Folder Editor。右栏分区内容区**不再重复** section 大标题（导航已标明）；可 Inherit 页仅保留 Inherit 开关。Known Hosts / Private Keys / Proxies / Terminal Styles 管理器左右栏同样**不要** Managed / Details 类分区标题。

**表单控件规范**：原型唯一样式源 [`docs/prototypes/form.css`](../prototypes/form.css)，验收样板 [`forms.html`](../prototypes/forms.html)。各页直接引用这些 class / token / 开关动画（`.12s` 拇指位移）；禁止另写一套控件样式。`session-editor/form.css` 仅 re-export。  
**表单宽度**：普通表单控件与**只读展示块**列统一 **420**（`--field-w`；Proxies / Terminal Styles / Tunnel editor / Private Keys 等右栏或弹窗同此）：input / select / 短 textarea / 公钥预览 / 路径只读框 / 终端预览。Port `100`、Icon 下拉 `220` 等窄控件另计。旁路钮 `.ctl-with-btn` 可伸出 420：字段旁无歧义时仅「管理」，独立入口用全称（如「管理终端样式」），选文件用 Browse（见 [`forms.html`](../prototypes/forms.html)）。Notes 不得再加宽。  
**表单行高**：`--ctrl-h`（32）为最小行高单位；`form-row` / `detail-row` 高度必须是其**整数倍**（默认 min = 1×；清单 `--row-h` = 2×；textarea / 终端预览 `--field-h-md` = 4×；公钥等 `--field-h-lg` = 5×）。行内始终顶对齐。左 `lbl` 用固定 `padding-top: calc((ctrl-h − 1.2em) / 2)`，在最小高度时与单行控件视觉垂直居中；值列变高时 **label 上边距不变**。  
**展示表单（管理器右栏等内嵌 part）**：不强制左标签缩进；宽度仍 420，只读块与输入对齐；行高规则同上。  
**例外（不要套 420）**：Tunnels / Login Script / 管理器**左**清单（全宽）；Login Keys **正文与 Tokens**、Import **粘贴 PEM**（大编辑值列全宽；Name 仍 420）。  
**布尔字段**：与其它字段同一 `form-row`——左短 `lbl` | 右 **开关 + 状态文案**（按上下文：On/Off、Enabled/Disabled、Use/Don't use 等）；禁止 checkbox 与游离「☐ + 长文案」行。补充说明用 `hint-indent`。页级 Inherit、pane 接收开关同用开关控件。菜单勾选项除外。口令/密码**禁止** Remember（填写即保存或本次连接用完即弃）。  
**按钮悬停提示**：所有可点按钮（文字 / 图标 / 主按钮 / 危险钮 / 工具栏 / 页脚）都必须有 **`title`**（图标钮另加 `aria-label`）。悬停片刻显示含义；正面案例：Terminal Styles 字体旁 `+` → Add Font File。  
**清单 / 旁路图标钮**：上下文无歧义时，添加 / 删除 / 复制 / 清空等用图标（`+` / `⌫` / `⎘` / `⌀` 等），勿写长文案。多种添加并存时用短区分文案（`Keys` / `Wait`）。右键菜单仍用文字（建议也有 `title`）。  
**单字段子窗**（Known Hosts Add / Import / Export 等）：只有一项输入时**不适用**表单列规则——无左侧 label、无缩进；控件顶满内容区。

### 13.1 左侧 section nav

| 项 | 左键行为 |
|---|---|
| General | 显示通用连接信息（不继承）。 |
| Authentication | 显示认证设置。 |
| Login Script | 登录成功后、用户可输入前自动执行的脚本。 |
| SFTP | 显示 SFTP 设置（本版不做）。 |
| Tunnels | 显示端口转发设置。 |
| Proxy | 显示代理设置。 |
| Advanced | 显示 keepalive、环境变量等高级设置（本版不做）。 |
| Terminal Style | 选择该 session 的终端样式 Profile（继承 / 自定义）。 |

> **无 Terminal / Logging 分区**：终端外观走 **Terminal Style**（选 Profile）+ Terminal Styles 管理器；日志走独立 Logging 工具窗 / 终端右键。Session Editor 不再设这两页。

### 13.2 General 表单

General **不继承**。Username 在 Authentication。

| 控件 ID | 类型 | 默认 | 校验 / 行为 |
|---|---|---|---|
| `session.name` | 单行输入框 | 空 | 非空，同文件夹下不重名。控件列宽 **420**。 |
| `session.icon` | 预览 + 下拉 (+ Browse) | Default（内置） | 行容器宽 **420**；下拉固定 **220**；Default / 预设 / 自定义图片。会话树与 session 标签展示此图标。 |
| `session.host` | 单行输入框 | 空 | 非空，可为域名/IP。宽 **420**。文件夹模式隐藏。 |
| `session.port` | 数字输入框 | `22` | `1-65535`；宽固定 **100**（勿用 `width:100%` 撑开）。文件夹模式隐藏。 |
| `session.notes` | 多行文本区 | 空 | 可选备注；宽与 Name 对齐 **420**（较高 `min-height`）。 |
| `session.folder` | 下拉/选择器 | 当前文件夹 | 必须存在（会话）。 |

### 13.3 Authentication 表单

页级 Inherit 开关（默认开）。Username 在本页（不在 General）。

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `auth.username` | 单行输入框 | 空 | 允许空；空时连接时提示。 |
| `auth.method` | 分段按钮 | Private Key | Password / Private Key。**Agent 本版不做。** |
| `auth.password` | password input | 空 | 仅 Password：**填写即保存**到 SecretStore；留空 = 连接时再问。无「Remember」勾选。 |
| `auth.key` | 下拉 | 首项托管密钥 | 仅 Private Key：从 Private Keys 选；无手输路径（D19）。 |
| `auth.key.manage` | 按钮 | 无 | 与下拉同一行末尾；文案仅 **管理** → Private Keys（字段旁无歧义不加 Keys）。 |
| `auth.key.passphrase` | password input | 空 | 仅 Private Key：**填写即保存**；留空 = 需要时再问。无「Remember」勾选。 |

连接时实际口令/挑战 UI 见 Auth Prompt；本页只存偏好与托管密钥引用。

### 13.3a Login Script 清单

页级 Inherit 开关（默认开）。**不是** shell 脚本文件；是代用户输入的**有序步骤列表**（主页是清单，不是表单列；列表全宽，不套 `--field-w`。Keys/Wait 分窗才是表单）。

**时机**：SSH 认证成功之后，终端对用户输入开放之前，按列表顺序执行。空列表 = 跳过。

主列表**只读**。Keys 与 Wait **分窗**（勿用 Type 下拉混在同一表单）：

- Keys：`session-editor/sections/login-step-keys.html`（720×580；大编辑区 + Token 说明）
- Wait：`session-editor/sections/login-step-wait.html`（560×400）

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `login_script.add_keys` | 按钮 | 无 | **列表上方** 文案 `Keys`（须与 Wait 区分，故不用裸 `+`）→ keys 弹窗。 |
| `login_script.add_wait` | 按钮 | 无 | **列表上方** 文案 `Wait` → wait 弹窗。 |
| `login_script.steps` | 只读列表 | 空 | 点摘要 → 对应分窗 Edit；行拖拽调序。 |
| `login_script.keys.name` | 单行 | 空 | **可选**。 |
| `login_script.keys.body` | 多行等宽 | 空 | 代用户击键；`{Enter}` / `{Ctrl+C}` 等 Token；字面量花括号双写：`{{`→`{`、`}}`→`}`（弹窗内附转换表）。**值列全宽**，不套 `--field-w`。 |
| `login_script.wait.name` | 单行 | 空 | **可选**。 |
| `login_script.wait.ms` | 数字 | `500` | 下一步前等待毫秒。 |
| `*.dlg.delete` / `cancel` / `save` | 按钮 | 无 | 编辑态 Delete；Cancel \| Save。 |

### 13.4 Terminal 表单

**已删除。** Session Editor 无 Terminal 分区；见 Terminal Style / Terminal Styles / Settings → Terminal。

### 13.5 SFTP 表单

**Session Editor 分区本版不做**（nav disabled）。全局 SFTP 默认见 **§14.2 Settings → SFTP**（不再在会话级配路径 / 并行等旧字段）。

### 13.6 Proxy 表单

页级 Inherit 开关（默认开）。会话侧只**选择**已有 Proxy Profile，不内嵌完整代理编辑表单。切片：`session-editor/sections/proxy.html`。

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `proxy.profile` | 下拉 | 继承解析值 / No Proxy | 选已有托管 Profile（含 No Proxy）。 |
| `proxy.manage` | 按钮 | 无 | Profile 右侧文案仅 **管理** → Proxies 管理器（字段旁无歧义不加全称；不挤进左栏标签列）。 |

### 13.6a Tunnels 清单（Session Editor）

页级 Inherit 开关（默认开）。**配置清单**（非表单字段列；非 Dock 运行态 Start/Stop）。切片原型：`session-editor/sections/tunnels.html`；编辑弹窗：`tunnel-editor/`。列表宽 = 右栏内容区，不套 `--field-w`。

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `session_tunnels.add` | 按钮 | 无 | **列表上方** 图标 **`+`**（`aria-label` Add Tunnel；仅一种隧道，无歧义）→ tunnel-editor（create）。 |
| `session_tunnels.clear` | 按钮 | 无 | 列表上方 Clear Tunnels；切 Custom。 |
| `session_tunnels.list` | 列表 | 空 / 继承 | 左键选中；双击 → tunnel-editor（edit）。 |
| 行右键 Edit | 菜单 | 无 | → tunnel-editor（edit）。 |
| 行右键 Duplicate | 菜单 | 无 | 复制该条配置（名称加 copy）；切 Custom。 |
| 行右键 Delete | 菜单 | 无 | 删除该条；切 Custom。 |

### 13.7 Logging 表单

**已从 Session Editor 删除。** 会话日志配置与启停走独立 Logging 工具窗 / 终端右键（见 Logging 原型），不在 Session Editor 设页。

日志按钮右键菜单：

| 菜单项 | 行为 | 状态 |
|---|---|---|
| Start Logging | 开启当前会话日志。 | 未开启时启用。 |
| Stop Logging | 停止当前会话日志。 | 已开启时启用。 |
| Open Current Log | 打开当前日志文件。 | 已生成日志文件时启用。 |
| Open Log Folder | 打开日志目录。 | 始终启用。 |
| Logging Settings | 打开 Logging 工具窗。 | 始终启用。 |

### 13.8 Terminal Style 表单（原 Appearance）

页级 Inherit 开关（默认开）。会话侧只**选择**已有 Terminal Style Profile；字体 / 配色在 Terminal Styles 管理器编辑。切片：`session-editor/sections/terminal-style.html`。

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `terminal_style.profile` | 下拉 | 继承解析值 | 选已有 Style（如 Campbell / Custom — ops）。 |
| `terminal_style.manage` | 按钮 | 无 | Style 右侧文案仅 **管理** → Terminal Styles 管理器。独立入口（如 Settings Appearance）才用全称 **管理终端样式**。 |
| `terminal_style.preview` | 只读预览 | 随 Profile | 与管理器 Live preview **同构**（高 `--field-h-md` = 4×32、实线边、ANSI 样例）；`max-width` 对齐表单列（`420`），`overflow: hidden`，不得撑破右栏。 |

> Settings 里的 **Appearance**（应用壳主题等）与本页无关，勿混名。

### 13.9 底部按钮

| 控件 ID | 文案 | 类型 | 行为 | 状态 |
|---|---|---|---|---|
| `session_editor.test` | Test Connection | 按钮 | 使用当前表单发起测试连接。 | host/port 基本合法时启用。 |
| `session_editor.cancel` | Cancel | 按钮 | 若有未保存改动，弹 Discard Changes。 | 始终启用。 |
| `session_editor.save` | Save | 主按钮 | 校验并保存；**成功后关闭**编辑器。分区内无独立 Save。 | 表单合法且有改动时启用。 |
| `session_editor.save_connect` | Save and Connect | 主按钮 | 保存后打开标签并连接（编辑器关闭）。 | 表单合法时启用。 |

## 13.10 Folder 模式（共用 Session Editor）

无独立 Folder Editor。文件夹级默认配置使用 §13 同一壳：

| 规则 | 说明 |
|---|---|
| 入口 | 会话树 Folder 右键 **Edit**（文案不再用 Edit Folder Defaults）。 |
| 可编辑 | Name / Icon / Notes；以及 Authentication / Login Script / Proxy / Tunnels / Terminal Style 等可 Inherit 字段。 |
| 不可编辑 | Host / Port / Username（连接身份；文件夹无连接目标）。 |
| 继承 | 子会话未覆盖时解析自本文件夹 → 更上层文件夹 → 全局。 |
| 页脚 | Cancel \| Save；无 Test Connection / Save and Connect。 |
| 影响范围 | 保存时不逐个确认子会话；可选显示受影响子项数量（预览）。 |

## 13.11 Input Sync 入口（无对话框）

同步输入没有设置对话框，也没有“选择目标”的向导。全部入口落在菜单与逐会话开关上（语义见 §8.1）：

| 入口 | 行为 |
|---|---|
| 终端右键 `Sync Input for This Session` | 把当前终端加入/移出同步组。 |
| 终端右键 `Pause Sync Input` / `Resume Sync Input` | 暂停/恢复广播（组成员保留）。 |
| 终端右键 `Stop Sync Input` | 清空同步组。 |
| 标签右键 `Sync Input for This Tab` | 同上，作用于该标签对应会话。 |
| pane header switch | 分屏时逐 pane 加入/移出同步组。 |
| 状态栏 chip | 显示 `SYNC → n targets`；点击 = 停止同步 / 清除一次性提示。 |

启用期间：

- 组成员每次终端输入事件先发送给本会话，再复制到其它组成员；注入路径不再广播。
- 复制的事件包括普通字符、Enter、Backspace、Tab、方向键、Home/End、PageUp/PageDown、功能键、Ctrl/Alt 组合和粘贴。
- 不复制 UI 操作，例如切换标签、打开菜单、拖拽 pane、SFTP 操作。
- **AI/MCP 输入**：向某会话写入前自动关闭该会话的同步标志并提示；AI 输入不广播。
- **本版（D23 / N9）**：不弹 KEYS / 大段粘贴 / Ctrl+C 确认；用状态栏 chip + 控制键警示替代。普通终端粘贴的确认由 Settings → Terminal → Multi-line paste 决定（§14.5）。
- 停止后不关闭任何 SSH 会话。

## 14. Settings

Settings 是 dialog，左侧分类，右侧内容（表单 / 设置卡片）。

分类：

- General：语言（system / zh-CN / en-US）+ Confirm close / quit + Reduce motion（见 §14.7）。
- Appearance：**本版启用**（见 §14.6）：App theme（Mode / Accent）+ Terminal style（Style 下拉 + 管理）。
- Terminal：全局终端默认值（scrollback 行数 / 单元上限，见 §14.5）。
- Security：**本版启用**（见 §14.4）：Master password 卡片 + 启动解锁 + Known Hosts / Private Keys / Proxies 管理器行（整行点击，不内嵌）。
- MCP：**本版启用**（独立分区，见 §14.3）；Enable / Port / Listen / API key / View user / Control user / Auto-approve；AI 创建标签带 AI 标志（§6.1）。
- Advanced：Debug logging、配置目录展示；本版本不提供整库 wipe secrets / 重置布局。MCP 不放本页。
- SFTP（Settings 分类）：**本版启用**（全局 SFTP 默认，见 §14.2）；与 Session Editor 内 SFTP 分区「本版不做」不同。
- Shortcuts：本版本禁用，tooltip 说明未提供。

### 14.1 底部按钮

| 控件 ID | 文案 | 类型 | 行为 | 状态 |
|---|---|---|---|---|
| `settings.cancel` | Cancel | 按钮 | 弃稿关闭（× / Esc 同路径）。 | 始终启用。 |
| `settings.save` | Save | 主按钮 | 校验并持久化**全部**可编辑分区草稿；**成功后关闭**。任一分区校验失败则保持打开。 | 始终启用（失败时留在窗内）。 |

分区内**禁止**再放 Save / Save terminal settings / Save SFTP 等；Reset defaults、管理入口等旁路动作可保留。

### 14.2 SFTP（Settings）

全局默认；切片原型：`settings/sections/sftp.html`。无分区 Save。

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `settings.sftp.show_hidden` | 开关 | On | 新开 SFTP 标签的默认隐藏文件显示；栏内仍可临时切换。 |
| `settings.sftp.conflict_action` | 下拉 | Ask | **Ask** → 弹出 conflict 窗；**Replace** 覆盖；**Skip** 跳过冲突项；**Stop** 中止整批传输。非 Ask 不弹窗。 |
| `settings.sftp.external_editor` | 下拉 | System default 或首个已检测项 | 选项固定含 **System default** 与 **Custom**；其余为启动时自动检测到的知名编辑器（如 VS Code、Zed、Notepad++、Sublime Text——未安装的不出现）。选非 Custom 时不显示路径。 |
| `settings.sftp.external_editor_path` | 路径 + Browse | 空 | **仅 Custom** 可见/必填：用户 Browse 选可执行文件。Remote Edit 用所选编辑器打开临时副本；**每次保存立即自动上传**（对齐 Xftp）。监视在**编辑器进程退出**或 **SFTP/SSH 断开**后结束并清理临时文件；不因单次保存而结束。 |

### 14.3 MCP（Settings）

独立分区；切片原型：`settings/sections/mcp.html`。无分区 Save。默认关闭。

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `settings.mcp.enabled` | 开关 | Off | 开启后 GUI 托管 MCP endpoint。Off 时下列控件禁用。 |
| `settings.mcp.port` | 数字（宽 Port） | `18765` | `1–65535`。 |
| `settings.mcp.listen` | 分段 Local \| LAN | Local | **Local** = `127.0.0.1`；**LAN** = `0.0.0.0`。 |
| `settings.mcp.api_key` | password + Generate / Copy | 空 | 可选；**LAN 推荐**。Generate 写入新密钥；Copy 复制当前值。 |
| `settings.mcp.view_user_connections` | 开关 | Off | On 时允许 AI **查看**用户创建的连接；Off 时 AI 仅见自身创建的连接。开 Control 时强制 On。 |
| `settings.mcp.control_user_connections` | 开关 | Off | On 时允许 AI **操作**用户创建的连接（输入/开关等）；隐含 View。关 View 时强制 Off。 |
| `settings.mcp.auto_approve` | 开关 | Off | On 时跳过 **MCP control consent**（§14.3a）；Off 时每次 `request_control` 弹窗。 |

#### 14.3a MCP control consent

原型：`settings/mcp-consent.html`。壳：CompactConfirm（`560×400`）。触发：MCP Enable 且 Auto-approve = Off，AI 请求控制。

| 控件 ID | 文案 | 行为 |
|---|---|---|
| `mcp_consent.message` | 说明正文 | 可跑命令 / 读屏 / 开关标签；**不能**代填密码或接受 host key。 |
| `mcp_consent.dont_ask` | Don't ask again 勾选框（footer 最左，与 Deny / Allow 同一行） | 仅与 Allow 一并生效：写入 `settings.mcp.auto_approve = On`。Deny 不改设置。 |
| `mcp_consent.deny` | Deny | 拒绝控制；Esc / × 同路径。 |
| `mcp_consent.allow` | Allow | 主按钮（非破坏性）；授予控制锁。 |

### 14.4 Security（Settings）

切片：`settings/sections/security.html`；主密码子窗：`settings/master-password.html`；启动解锁：`settings/master-password-unlock.html`。无分区 Save。

页面结构（WinUI Settings card）：

- **Master password 卡片**：图标 + 标题 + 状态 pill（Not set / Set）+ 一句状态说明 + 动作区；Set 态额外露出 `Forgot password?`（→ Reset）。
- **Managers**：Known Hosts / Private Keys / Proxies 三行；整行可点（图标 + 名称 + 计数 + chevron），不做内嵌管理，也不在行下堆按钮。
- 配置目录只在 Advanced 展示，Security 不重复。

Master password 语义：

- **可选**。未设 = 不启用主密码加密层；session / key 文件按默认存储（凭证可能仍由 OS keychain 保护）。
- **设** = 用主密码加密全部 session 文件与 key 文件（含保存的口令 / passphrase）；启动后首次读取机密时弹 `master-password-unlock`。
- 主密码不写入磁盘；只保存用于校验的 verifier / KDF 参数。
- Set / Change 校验口令至少 8 字符；Change 必须校验当前口令。
- **Remove** 必须校验当前主密码；成功后解密并回到 Not set，不删除数据。
- **Reset** 用于忘记主密码：删除受其保护的 session / key 数据后移除主密码；Known Hosts 不受影响，proxy profile 保留但已存口令属于受保护数据会被清除。无打字确认词；危险按钮。

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `settings.security.master_password.status` | 只读 pill | Not set | **Not set** = 未启用主密码加密层；**Set** = 已启用（对齐 Xshell）。 |
| `settings.security.master_password.set` | 主按钮 | — | 仅 Not set：→ Set Master Password。 |
| `settings.security.master_password.change` | 按钮 | — | 仅 Set：→ Change（Current + New + Confirm）。 |
| `settings.security.master_password.remove` | 按钮 | — | 仅 Set：→ Remove（Current；解密并清除主密码，不删数据）。 |
| `settings.security.master_password.forgot` | 危险文字链 | — | 仅 Set：→ Reset（删除受保护数据；无确认词）。 |
| `settings.security.managers.known_hosts` | 可点行 | 计数 | → Known Hosts 管理器。 |
| `settings.security.managers.private_keys` | 可点行 | 计数 | → Private Keys 管理器。 |
| `settings.security.managers.proxies` | 可点行 | 计数 | → Proxies 管理器。 |

#### 14.4a Master password 子窗

`master-password.html`（compact `560×400`）：

| 子窗 | 字段 | 主按钮 | 说明 |
|---|---|---|---|
| Set | New / Confirm | Set | 写 verifier，并加密既有 session / key 文件。 |
| Change | Current / New / Confirm | Change | 校验当前口令后重加密；错误留在窗内。 |
| Remove | Current | Remove | 解密后回到 Not set；不删数据。 |
| Reset | 无 | Reset and Delete（danger） | 删除受保护数据；默认焦点 Cancel。 |

#### 14.4b Startup unlock

`master-password-unlock.html`（compact `560×400`）：启动后首次读取机密时出现。

| 控件 ID | 类型 | 行为 |
|---|---|---|
| `master_password_unlock.password` | password（单字段） | Enter = Unlock。 |
| `master_password_unlock.unlock` | 主按钮 | 校验；错误在字段上方显示错误条，窗体尺寸不变。 |
| `master_password_unlock.forgot` | 危险文字链 | → Reset；删除受保护数据后重新开始。 |
| `master_password_unlock.quit` | 按钮 / × / Esc | 退出 YShell，不读取任何受保护文件。 |

Headless（CLI / MCP）不弹 GUI 解锁：主密码无法通过环境变量提供；无头路径使用 OS keychain 或显式注入的密钥库，否则以 locked 错误失败，不读取受保护文件。

Settings **不**使用 Danger Zone，也 **不**提供打字令牌确认的 `RESET SECRETS` / `CLEAR` 入口（Reset Master Password 用明确破坏性按钮，无确认词）。启动解锁原型见 `master-password-unlock.html`；本分区只管设置入口与状态。Known Hosts / Private Keys 亦可从 Session 菜单打开。

Known Hosts / Private Keys 由 Session 菜单打开（与 Settings 同级独立弹窗）。破坏性操作用普通 `ConfirmDialog`（`destructive`，无确认词）：

| 控件 ID | 行为 | 确认 |
|---|---|---|
| `known_hosts.remove` | 删除选中 `host:port` 整组 known hosts。 | 普通确认。 |
| `known_hosts.clear_all` | 清空全部 known hosts。 | 普通确认。 |
| `private_keys.remove` | 删除选中托管私钥。 | 普通确认（含被引用告警）。 |
| `private_keys.deploy` | 向当前会话部署公钥。 | 普通确认。 |
| `settings.advanced.open_config_dir` | 打开配置目录。 | 无（本版本可仅展示路径）。 |

口令（Generate / Import / 详情）：**填写即写入** SecretStore，**无**「Remember」勾选（对齐 Authentication）。详情可用 Forget 清除已存口令。Auth Prompt 本次连接口令同样无 Remember。

### 14.5 Terminal（Settings）

全局终端默认值；切片原型：`settings/sections/terminal.html`。无分区 Save；Reset defaults 留在分区，Save 用整窗页脚。

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `settings.terminal.scrollback_lines` | 数字 | `10000` | `100–1,000,000` 行；越界时字段红框 + 行内错误，不落盘。 |
| `settings.terminal.scrollback_max_cells` | 数字 | `2000000` | `100,000–100,000,000` 单元；与行数一起构成 per-session 上限。 |
| `settings.terminal.middle_click` | 下拉 | Paste | Paste / Do nothing；Paste 走剪贴板，并遵循 Multi-line paste 策略。 |
| `settings.terminal.right_click` | 下拉 | Context menu | Context menu / Paste / Do nothing；Paste 不弹菜单，并遵循 Multi-line paste 策略。 |
| `settings.terminal.multiline_paste` | 下拉 | Ask | Ask → ConfirmDialog（`Paste N lines?`，Cancel \| Paste）；Paste 直接发送；Block 忽略多行粘贴。 |
| `settings.terminal.auto_copy` | 开关 | Off | On = 选中即复制到剪贴板；Off = 需 Ctrl+C / 菜单 Copy。 |
| `settings.terminal.reset_defaults` | 图标按钮 | — | 恢复 scrollback + 鼠标 / 粘贴 / Auto copy 默认值；不关闭窗、不触发分区 Save。 |

- 这些设置是全局默认值；Save 后应用到现有与新开终端。
- Middle / Right = Paste 与 Ctrl+V 走同一 Multi-line paste 策略；Ask 的确认框复用 CompactConfirm（`compact-confirm`），不出现打字确认词。
- 错误就在字段下方/表单状态中显示，窗尺寸不变。
- 分区内不出现 `Save` / `Save terminal settings`。

### 14.6 Appearance（Settings）

切片原型：`settings/sections/appearance.html`。无分区 Save。页面分两组：**App theme**（Mode / Accent）与 **Terminal style**（Style 下拉 + 管理）。

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `settings.appearance.mode` | 分段 System / Light / Dark | System | 整窗明暗；选择后即时预览，Save 持久化，Cancel 回滚。 |
| `settings.appearance.accent` | 色板 | Windows Blue | Follow system + 6 预设（Blue / Teal / Purple / Green / Orange / Magenta）；即时预览，Save 持久化，Cancel 回滚。 |
| `settings.appearance.terminal_style` | 下拉 | Campbell | 全局 Terminal Style Profile；Save 后对新开终端生效；已开终端保持捕获样式。 |
| `settings.appearance.manage_terminal_styles` | 图标按钮 | — | → Terminal Styles 管理器；字体 / 配色在管理器编辑。 |

- App theme 作用于整个应用；Terminal style 独立于 UI 明暗。
- Follow system accent 仅 Windows 读取系统强调色；Linux / macOS 回退当前预设（设计规范 §3.4）。
- 本分区不内联字体 / 配色编辑；这些在 Terminal Styles 管理器。
- 分区内不出现 Save。

### 14.7 General（Settings）

切片原型：`settings/sections/general.html`。无分区 Save。

| 控件 ID | 类型 | 默认 | 行为 |
|---|---|---|---|
| `settings.general.language` | 下拉 | System default | System / 中文 / English；切换立即生效。 |
| `settings.general.confirm_close_session` | 开关 | On | 关闭有活动连接的会话 / 标签前确认。 |
| `settings.general.confirm_quit_app` | 开关 | On | 退出 YShell 前确认。 |
| `settings.general.reduce_motion` | 开关 | Off | On = 关闭所有过渡动画。 |

- Save 持久化；分区内不出现 Save。

## 15. 安全弹窗

### 15.1 Host Key First Trust

壳：compact `560×400`。触发：首次连接未知 host key。

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

壳：compact `560×400`。触发：已知 host key 与当前不一致。

必须是阻断弹窗，不能自动继续。

按钮：

| 控件 ID | 文案 | 行为 |
|---|---|---|
| `host_key_changed.cancel` | Cancel | 取消连接。 |
| `host_key_changed.copy_details` | Copy Details | 复制旧/新 fingerprint 和路径。 |
| `host_key_changed.replace` | Replace and Trust | 替换 known_hosts 并连接；破坏性主按钮，无打字确认词。 |

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
| `close_tab.dont_ask` | Don't ask again | 勾选框，footer 最左、与 Cancel 同一行；勾选并确认后写 Settings → General → Confirm close = Off。Cancel / × 不改。 |

### 16.2 Quit Confirmation

触发：退出应用且存在活动会话。

按钮：

| 控件 ID | 文案 | 行为 |
|---|---|---|
| `quit.cancel` | Cancel | 返回应用。 |
| `quit.disconnect_quit` | Disconnect and Quit | 断开所有连接并退出。 |
| `quit.dont_ask` | Don't ask again | 勾选框，footer 最左、与 Cancel 同一行；勾选并确认后写 Settings → General → Confirm quit = Off。Cancel / × 不改。 |

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
| Sync Input for This Session | 左键 | Runtime Sync Group（无对话框） | tab/session id（加入或移出组）。 |
| Pause / Resume Sync Input | 左键 | Runtime Sync Group | 无（切组内 `paused`）。 |
| Stop Sync Input | 左键 | Runtime Sync Group | 无（清空组）。 |
| Tab / Pane Sync Input Toggle | 左键 | Runtime Sync Group | session id + On/Off。 |
| Status Bar SYNC chip | 左键 | Runtime Sync Group | 无（Stop Sync Input / 清除提示）。 |
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
