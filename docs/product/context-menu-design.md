# YShell 右键菜单设计矩阵

本文档专门定义第一版所有右键菜单。右键菜单必须按上下文区分，禁止实现成一套通用菜单。每个菜单项都必须有明确的触发区域、启用条件、点击行为和危险确认。

## 1. 全局规则

- 右键菜单只显示与当前上下文相关的操作。
- 不适用的常用操作可以 disabled，但必须有 tooltip 说明原因。
- 危险操作使用红色文字或危险图标，并且点击后进入确认弹窗。
- 菜单项顺序固定：打开/连接类、编辑类、发送/传输类、视图类、危险类。
- macOS 支持 `Ctrl + 左键` 等同右键。
- 菜单打开时不改变当前 SSH 输入焦点；关闭菜单后焦点回到触发区域。

## 2. SSH 终端右键菜单

### 2.1 终端无选区

触发区域：SSH terminal viewport，当前没有文本选区。

| 菜单项 | 启用条件 | 左键点击行为 | 备注 |
|---|---|---|---|
| Paste | 剪贴板非空且 SSH connected | 粘贴到当前 SSH。 | 大段粘贴按设置触发确认。 |
| Paste as Bracketed | 剪贴板非空且 bracketed paste 可用 | 使用 bracketed paste 粘贴。 | 默认推荐。 |
| Send Key Input To... | 当前 SSH connected | 打开 Send Key Input To Dialog，源为当前 pane。 | 复制实时终端输入事件。 |
| Stop Send Key Input To | 当前 pane 是源 pane | 停止键输入复制。 | 危险色但不需要确认。 |
| Select Screen | 当前屏幕存在内容 | 仅选择当前可见屏幕内容，不包含 scrollback。 | 对应“全选屏幕内容”。 |
| Select All | 当前终端存在内容 | 选择 scrollback 和当前屏幕。 | 对应“全选全部终端内容”。 |
| Find... | 当前终端存在内容 | 打开终端搜索栏。 | 普通文本查找。 |
| Find Regex... | 当前终端存在内容 | 打开终端搜索栏并启用 regex。 | 正则非法时显示错误。 |
| Find Across SSH... | 至少存在 2 个 SSH terminal | 打开跨 SSH 查找面板。 | 支持当前窗口所有 SSH 或可见 SSH。 |
| Clear Screen | SSH connected | 清除当前屏幕，不清 scrollback。 | |
| Clear Scrollback | scrollback 非空 | 打开 Clear Scrollback Confirmation。 | 危险操作。 |
| Save Output As... | scrollback 非空 | 打开保存文件对话框。 | 保存 sanitized text。 |
| Logging Settings... | 当前 session 可编辑 | 打开 Session Editor 的 Logging section。 | |

### 2.2 终端有选区

触发区域：SSH terminal viewport，当前存在文本选区。

| 菜单项 | 启用条件 | 左键点击行为 | 备注 |
|---|---|---|---|
| Copy | 有选区 | 复制选区。 | 默认菜单第一项。 |
| Copy with ANSI | 有选区 | 复制包含 ANSI 样式的文本。 | 第一版可 disabled。 |
| Copy as HTML | 有选区 | 复制 HTML 富文本。 | 第一版可 disabled。 |
| Copy Current Line | 当前光标行存在内容 | 复制当前行。 | 无选区时也可用。 |
| Copy Screen | 当前屏幕存在内容 | 复制可见屏幕内容。 | 不包含 scrollback。 |
| Paste | 剪贴板非空且 SSH connected | 粘贴到当前 SSH。 | |
| Send Selection to Current | 有选区且 SSH connected | 将选区文本发送到当前 SSH。 | 需要确认。 |
| Send Selection to... | 有选区 | 打开 Broadcast Confirmation。 | 一次性文本，不是 Send Key Input To。 |
| Find Selection | 有选区 | 用选区填充搜索栏并搜索。 | |
| Find Selection Across SSH | 有选区且至少存在 2 个 SSH terminal | 打开跨 SSH 查找面板，以选区为 query。 | |
| Clear Selection | 有选区 | 清除选区。 | |

### 2.3 终端接收目标 pane header

触发区域：pane header。

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| Receive Key Input | 当前 pane 是 connected SSH 且不是源 pane | 勾选/取消接收当前源 pane 的键输入。 |
| Make Source for Send Key Input To | 当前 pane 是 connected SSH | 将当前 pane 设为源并打开目标选择。 |
| Split Right | pane 数小于 4 | 当前 active tab 移到右侧新 pane。 |
| Split Down | pane 数小于 4 | 当前 active tab 移到下方新 pane。 |
| Move Tab to Previous Pane | 存在其它 pane | 移动 active tab。 |
| Move Tab to Next Pane | 存在其它 pane | 移动 active tab。 |
| Close Pane | pane 数大于 1 | 关闭 pane，内部活动会话逐个确认。 |

## 3. 标签右键菜单

触发区域：WorkspaceTabs 的标签主体。

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| Reconnect | 标签断开或错误 | 重连该标签的 session。 |
| Disconnect | 标签 connected | 断开 SSH、SFTP、tunnels。 |
| Duplicate Tab | 有 SessionProfile | 复制标签并连接。 |
| Rename Tab | 始终启用 | 行内编辑标签显示名。 |
| Send Key Input To... | 标签 connected SSH | 以该标签为源打开 Send Key Input To Dialog。 |
| Receive Key Input | 存在 active source 且该标签可接收 | 勾选/取消接收。 |
| Split Right | pane 数小于 4 | 移到右侧新 pane。 |
| Split Down | pane 数小于 4 | 移到下方新 pane。 |
| Move to Pane | 存在其它 pane | 展开 pane 子菜单。 |
| Close Others | 标签数大于 1 | 关闭其它标签。 |
| Close Tabs to Left | 左侧有标签 | 关闭左侧标签。 |
| Close Tabs to Right | 右侧有标签 | 关闭右侧标签。 |
| Close All Tabs | 标签数大于 0 | 关闭当前 pane 内全部标签，活动会话逐个确认。 |
| Close Disconnected Tabs | 存在 disconnected/error 标签 | 关闭所有已断开或错误标签。 |
| Close | 始终启用 | 关闭当前标签。 |

## 4. SFTP 右键菜单

### 4.1 远程文件

触发区域：SFTP 文件行，类型为文件。

| 菜单项 | 启用条件 | 左键点击行为 | 确认 |
|---|---|---|---|
| Open/View | 文件可读 | 下载临时副本并打开。 | 无 |
| Edit | 文件可读写 | 下载临时副本，编辑后提示上传。 | 上传时确认 |
| Download | SFTP connected | 加入下载队列。 | 覆盖本地文件时确认 |
| Download To... | SFTP connected | 选择本地目录后下载。 | 覆盖时确认 |
| Rename | 当前目录可写 | 行内重命名。 | 名称冲突时确认 |
| Delete | 当前目录可写 | 打开删除确认。 | 必须确认 |
| Chmod... | server 支持权限修改 | 打开 chmod dialog。 | Apply 时确认 |
| Copy Remote Path | 始终启用 | 复制远程绝对路径。 | 无 |
| Copy Name | 始终启用 | 复制文件名。 | 无 |
| Properties | 始终启用 | 打开属性弹窗。 | 无 |

### 4.2 远程目录

触发区域：SFTP 文件行，类型为目录。

| 菜单项 | 启用条件 | 左键点击行为 | 确认 |
|---|---|---|---|
| Open | SFTP connected | 进入目录。 | 无 |
| Open in New SFTP Tab | 第一版可 disabled | 新 SFTP tab 打开目录。 | 无 |
| Upload Here... | 当前目录可写 | 选择本地文件上传到该目录。 | 覆盖时确认 |
| Download Directory | 第一版 disabled | 后续递归下载目录。 | tooltip 说明后续支持 |
| New Folder | 当前目录可写 | 在该目录创建子目录。 | 无 |
| Rename | 当前目录可写 | 行内重命名。 | 冲突时确认 |
| Delete | 当前目录可写 | 删除目录。 | 必须确认 |
| Chmod... | server 支持权限修改 | 打开 chmod dialog。 | Apply 时确认 |
| Copy Remote Path | 始终启用 | 复制目录路径。 | 无 |
| Properties | 始终启用 | 打开属性弹窗。 | 无 |

### 4.3 SFTP 空白区域

触发区域：SFTP 文件列表空白处。

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| Upload Files... | SFTP connected 且当前目录可写 | 打开文件选择器。 |
| Upload Folder... | 第一版 disabled | 后续目录上传。 |
| New Folder | 当前目录可写 | 打开 New Folder Dialog。 |
| Refresh | SFTP connected | 刷新当前目录。 |
| Show Hidden Files | SFTP connected | 切换隐藏文件显示。 |
| Copy Current Path | SFTP connected | 复制当前远程目录。 |
| Open Local Download Folder | 始终启用 | 打开默认下载目录。 |

### 4.4 SFTP 多选

触发区域：多个文件/目录被选中。

| 菜单项 | 启用条件 | 左键点击行为 | 确认 |
|---|---|---|---|
| Download Selected | 至少一个文件 | 加入下载队列。 | 覆盖时确认 |
| Delete Selected | 当前目录可写 | 打开批量删除确认，显示数量。 | 必须确认 |
| Chmod Selected... | server 支持 | 打开 chmod dialog。 | Apply 时确认 |
| Copy Remote Paths | 始终启用 | 复制多行路径。 | 无 |
| Properties | 始终启用 | 打开批量属性摘要。 | 无 |

## 5. Session Manager 右键菜单

### 5.1 保存的 Session

触发区域：SessionSidebar 中的保存会话。

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| Connect | 未连接或允许重复连接 | 新标签打开并连接。 |
| Open in New Tab | 始终启用 | 打开新标签。 |
| Open in Split Right | 当前工作区 pane 数小于 4 | 新建右侧 pane 并连接。 |
| Open in Split Down | 当前工作区 pane 数小于 4 | 新建下方 pane 并连接。 |
| Open SFTP | 可连接 SSH | 连接后打开 SFTP。 |
| Edit Session... | 始终启用 | 打开 Session Editor。 |
| Duplicate | 始终启用 | 复制 session，名称追加 `Copy`，并打开编辑器。 |
| Duplicate Without Secrets | 始终启用 | 复制 session 但不复制 password/passphrase secret_ref。 |
| Duplicate to Folder... | 至少存在一个 folder | 选择目标 folder 后复制 session。 |
| Copy Connection String | 始终启用 | 复制 `ssh://user@host:port`。 |
| Copy Host | 始终启用 | 复制 host。 |
| Copy User@Host | username 非空 | 复制 `user@host`。 |
| Favorite / Unfavorite | 始终启用 | 切换收藏。 |
| Send Key Input To Target | 存在 active source 且该 session connected | 勾选/取消作为键输入接收目标。 |
| Export Session... | 始终启用 | 导出该 session。 |
| Delete | 始终启用 | 删除确认。 |

### 5.2 Folder

触发区域：SessionSidebar 文件夹。

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| New Session | 始终启用 | 在该 folder 下新建 session。 |
| New Folder | 始终启用 | 在该 folder 下新建子 folder。 |
| Connect All | folder 内存在 session | 逐个连接。必须确认数量。 |
| Open All Visible | 当前筛选结果有 session | 打开筛选可见 session。必须确认数量。 |
| Send Key Input To Folder | 存在 active source | 将该 folder 下 connected SSH 加入目标。 |
| Edit Folder Defaults... | 始终启用 | 打开 Folder Editor。 |
| Duplicate Folder... | 始终启用 | 复制 folder、子 folder 和 session，可选择是否复制 secrets。 |
| Copy Folder Path | 始终启用 | 复制 session tree 路径。 |
| Rename | 始终启用 | 行内改名。 |
| Export Folder... | 始终启用 | 导出 folder 配置。 |
| Delete Folder | 始终启用 | 删除确认，显示子项数量。 |

### 5.3 SessionSidebar 空白区域

触发区域：左侧会话树空白处。

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| New Session | 始终启用 | 在根目录新建 session。 |
| New Folder | 始终启用 | 在根目录新建 folder。 |
| Import Config... | 始终启用 | 打开导入。 |
| Expand All | 有折叠 folder | 展开全部。 |
| Collapse All | 有展开 folder | 折叠全部。 |
| Refresh | 始终启用 | 重新加载配置。 |

## 6. 传输队列右键菜单

触发区域：Transfer Queue 任务行。

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| Pause | queued/running | 暂停任务。 |
| Resume | paused | 恢复任务。 |
| Retry | failed | 重新排队。 |
| Cancel | queued/running/paused | 取消任务。 |
| Remove | completed/failed/cancelled | 从队列移除。 |
| Open Local Folder | download completed | 打开本地目录。 |
| Open Remote Folder | upload completed 且 SFTP connected | 跳转远程目录。 |
| Copy Error | failed | 复制错误详情。 |
| Clear Completed | 有 completed 任务 | 清除所有完成任务。 |

## 7. Tunnels 右键菜单

触发区域：Tunnel row。

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| Start | stopped/failed | 启动 tunnel。 |
| Stop | running | 停止 tunnel。 |
| Restart | running/failed | 重启 tunnel。 |
| Edit... | stopped/failed | 打开 Tunnel Editor。 |
| Duplicate | 始终启用 | 复制 tunnel 配置。 |
| Delete | stopped/failed | 删除确认。 |
| Copy Listen Address | running | 复制监听地址。 |
| Copy Target Address | local/remote tunnel | 复制目标地址。 |
| Show Connections | running | 显示连接数详情。 |

## 8. Quick Commands 右键菜单

触发区域：快捷命令按钮。

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| Run in Current Session | 当前 SSH connected | 发送该命令到当前 SSH。 |
| Run in Selected Sessions... | 有多个 connected SSH | 打开 Broadcast Confirmation。 |
| Edit Command... | 始终启用 | 打开 Command Editor。 |
| Duplicate | 始终启用 | 复制命令。 |
| Move to Group | 存在多个 group | 展开 group 子菜单。 |
| Delete | 始终启用 | 删除确认。 |

## 9. 状态栏右键菜单

### 9.1 连接状态

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| Reconnect | 当前 session disconnected/error | 重连。 |
| Disconnect | 当前 session connected | 断开。 |
| Copy Host | 当前 session 存在 | 复制 host。 |
| Copy User@Host | 当前 session 存在 | 复制 user@host。 |

### 9.2 日志状态

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| Start Logging | 未启用日志 | 开启日志。 |
| Stop Logging | 已启用日志 | 停止日志。 |
| Open Current Log | 已生成日志文件 | 打开日志文件。 |
| Open Log Folder | 日志目录存在 | 打开目录。 |
| Logging Settings... | 当前 session 可编辑 | 打开 Logging section。 |

### 9.3 代理状态

| 菜单项 | 启用条件 | 左键点击行为 |
|---|---|---|
| Proxy Details | 当前 session 存在 | 显示 resolved proxy。 |
| Test Proxy | 使用代理 | 执行三阶段代理测试。 |
| Proxy Settings... | 当前 session 可编辑 | 打开 Proxy section。 |

## 10. 禁止行为

- SSH 终端右键不得显示 SFTP 的 Rename/Delete/Chmod。
- SFTP 右键不得显示 Send Key Input To。
- Session Manager 右键不得直接执行 SFTP 文件操作。
- Transfer Queue 右键不得显示 SSH reconnect。
- Quick Commands 的 Broadcast 不能和 Send Key Input To 混为一个菜单项。
- 任何菜单项不得在 disabled 状态下静默无反馈。
