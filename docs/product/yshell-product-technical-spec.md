# YShell 产品与技术规格说明

## 1. 项目目标

YShell 是一个开源、跨平台、现代化的 SSH 终端与 SFTP 客户端，目标是在 Windows、Linux、macOS 上提供接近 Xshell 免费版核心工作流的体验。

本项目不复制 Xshell 的商标、视觉资产、专有协议实现、脚本接口或受版权保护的界面细节，而是实现功能兼容和使用体验对标。第一版只支持 SSH 相关能力，但 SSH 范围内必须包含交互式终端、SFTP 文件管理、端口转发、会话管理、认证管理和日志能力。

配套文档：

- 详细开发拆分计划：`docs/product/yshell-detailed-development-plan.md`
- GitHub Actions 自动发布配置：`docs/product/github-actions-release.md`
- UI 草图 HTML：`docs/product/ui-sketches/yshell-ui-sketches.html`
- UI 逐控件交互契约：`docs/product/yshell-ui-interaction-contract.md`

## 2. 设计原则

- 开源优先：代码、配置格式、构建流程和扩展接口应便于社区审计与贡献。
- 原生跨平台：不使用 JavaScript、Electron、Tauri WebView 或浏览器运行时。
- 安全优先：凭据、私钥、主密码、主机密钥和日志都必须有明确的安全边界。
- 专业工具质感：界面应服务于高频运维、开发和教学场景，布局密集但不杂乱。
- 功能兼容而非像素复刻：对标 Xshell 免费版的核心能力，但避免复制专有 UI、文案和品牌。
- 渐进交付：先做稳定可用的 SSH/SFTP 客户端，再扩展脚本、插件、同步和高级自动化。

## 3. 竞品能力参考

Xshell 官方功能清单包括多标签、标签分组、Session Manager、SSH/SFTP/TELNET/RLOGIN/SERIAL/RDP 等协议、终端仿真、快捷命令、认证配置、端口转发、日志、主题、颜色方案、状态栏和文件传输等能力。免费授权页面说明 Xshell/Xftp 的免费授权面向家庭和学校使用，并且免费版已移除标签数量限制。

YShell 第一版只覆盖 SSH 协议族能力：

- 覆盖：SSH2 终端、SFTP、端口转发、X11 转发配置入口、会话管理、认证、主机密钥、日志、快捷命令、终端主题和布局。
- 暂不覆盖：Telnet、Rlogin、Serial、RDP、FTP、X/Y/ZMODEM、VB/JScript/Python 脚本录制、与 Xmanager/Xagent/Xftp 的专有集成。
- 后续评估：本地 Shell、插件系统、命令触发器、云同步、团队共享配置、Mosh。

参考来源：

- Xshell 产品页：https://www.xshell.com/en/xshell/
- Xshell 功能规格：https://www.xshell.com/en/xshell-all-features/
- Xshell 免费授权说明：https://www.xshell.com/en/free-for-home-school/

## 4. 目标用户

- 开发者：日常连接 Linux 服务器、容器宿主机、云主机和开发环境。
- 运维与 SRE：需要管理大量会话、批量执行命令、查看日志和转发端口。
- 学生与教学用户：需要免费、开源、跨平台的 SSH/SFTP 学习工具。
- 开源社区用户：希望避免闭源商业客户端，同时需要比 PuTTY 更现代的体验。

## 5. 第一版功能范围

### 5.1 SSH 终端

- 支持 SSH2 交互式 Shell。
- 支持密码、键盘交互、私钥和 ssh-agent 认证。
- 支持 Ed25519、ECDSA、RSA 私钥；DSA 仅作为兼容选项，不作为默认推荐。
- 支持主机密钥首次信任、变更告警、已知主机管理。
- 支持 PTY 尺寸同步、窗口 resize、环境变量、终端类型配置。
- 支持 keepalive、连接超时、重连提示和断线状态显示。

### 5.2 终端仿真

- 默认终端类型为 `xterm-256color`。
- 支持 UTF-8、多字节字符、宽字符、Emoji 显示策略和中文字体回退。
- 支持 ANSI 颜色、256 色、TrueColor、粗体、斜体、下划线、反色。
- 支持鼠标模式、括号粘贴、备用屏幕、滚屏缓冲。
- 支持查找、复制、粘贴、矩形选择、全选、清屏、软换行。
- 支持可配置字体、字号、行距、光标形状、光标闪烁、透明度。

### 5.3 SFTP 文件管理

- 每个 SSH 会话可打开关联 SFTP 面板。
- 支持远程目录浏览、上传、下载、删除、重命名、新建目录。
- 支持文件权限显示和 chmod 修改。
- 支持拖放上传、批量传输、覆盖确认、传输队列、失败重试。
- 支持远程文件查看和用系统默认编辑器打开临时副本。
- 支持断点续传作为后续增强；第一版至少支持失败后重新排队。
- 支持通过会话配置指定默认远程目录和本地下载目录。

### 5.4 会话管理

- 支持会话树：文件夹、标签、搜索、收藏、最近连接。
- 支持会话配置：主机、端口、用户名、认证方式、代理、终端、编码、颜色方案、SFTP 默认目录。
- 支持导入/导出 YShell 配置文件。
- 支持批量编辑常见字段，例如用户名、端口、代理、颜色方案。
- 支持快速连接栏：输入 `ssh://user@host:22` 或 `user@host` 快速打开会话。

### 5.5 多标签与布局

- 支持单窗口多标签。
- 支持标签拖拽排序、关闭、复制会话、重连。
- 支持左右/上下分屏标签组。
- 支持递归分屏布局：一个工作区可继续向右或向下拆分，但第一版最大分屏叶子数限制为 4，避免状态管理失控。
- 支持将标签拖入已有分屏区域，也支持通过标签右键菜单执行 Split Right、Split Down、Move to Pane。
- 支持分屏比例拖拽调整，并在应用状态中保存最后一次布局。
- 支持分屏内 Send Key Input To 键输入复制，但默认关闭，必须由用户显式选择目标窗口。
- 支持标签颜色、会话状态图标、未读输出提示。
- 支持全屏模式和专注模式。

### 5.6 快捷命令、广播命令与发送键输入到

- 支持每个会话或全局快捷命令。
- 快捷命令可显示为按钮栏或命令面板。
- 支持向当前会话发送命令。
- 支持向选中的多个会话广播输入，默认需要二次确认。
- 支持 Send Key Input To：用户直接在一个 SSH 终端窗口输入时，把同一批终端输入事件实时复制到其它指定 SSH 终端窗口。
- Send Key Input To 不是发送一条命令，也不是发送 Compose Pane 文本；它复制用户按下的终端输入操作，包括普通字符、Enter、Backspace、Tab、方向键、Home/End、PageUp/PageDown、功能键、Ctrl 组合、Alt 组合、粘贴内容和 bracketed paste 包装。
- 每个 SSH 终端窗口必须能在窗口上直接选择是否接收当前源窗口的键输入。可见窗口显示接收开关，隐藏标签可在目标选择弹窗中勾选。
- 预设目标包括：发送到所有 SSH、发送到所有可见 SSH、发送到所有已连接 SSH、发送到当前分屏、发送到当前标签组、发送到同一文件夹、手动选择。
- 发送目标只包含处于 connected 状态且拥有交互式 Shell channel 的 SSH 会话，不包含 SFTP、断开会话、错误会话、连接中会话。
- Send Key Input To 启用后必须有醒目的状态条，显示源窗口、目标数量、当前预设、停止按钮；开始前必须二次确认。
- Send Key Input To 的复制方向是单向的：源窗口输入复制到目标窗口，目标窗口本地输入不会反向复制，避免循环。
- Broadcast Command 与 Send Key Input To 必须在 UI 和代码中分开命名：前者发送一次性文本，后者复制持续的终端输入事件。
- 支持多行 Compose Pane：用户先编辑多行文本，再发送到一个或多个会话。

### 5.7 端口转发

- 支持本地端口转发、本地监听地址配置。
- 支持远程端口转发。
- 支持动态 SOCKS5 转发。
- 支持转发状态面板：监听地址、目标地址、连接数、错误状态、停止/重启。
- X11 转发第一版提供配置项和 SSH 请求支持；不捆绑 X Server。

### 5.8 代理

- 支持 SOCKS4 代理。
- 支持 SOCKS4a 代理，允许代理端解析域名。
- 支持 SOCKS5 代理。
- 支持 SOCKS5 用户名/密码认证。
- 支持 HTTP CONNECT 代理。
- 支持无代理、全局代理、会话代理三种模式。
- 支持每个会话单独配置代理，也支持全局代理配置。
- 支持代理连接测试，必须展示 TCP 连接、代理握手、SSH 握手三个阶段的状态。

### 5.9 日志与审计

- 支持会话日志自动保存。
- 支持记录连接到的 SSH Shell channel 的所有远端输出，包括普通输出、stderr 合并输出、控制序列清洗后的文本。
- 支持两种日志格式：raw transcript 和 sanitized text。raw transcript 保留原始字节或 escape sequence，sanitized text 去除 ANSI 控制序列，便于阅读。
- 支持纯文本日志和带时间戳日志，时间戳可按行前缀写入。
- 支持日志路径模板：日期、会话名、主机名、用户名。
- 支持敏感输入保护：密码提示期间不记录用户输入。
- 日志默认记录远端输出，不记录本地用户键盘输入；如果用户开启 input logging，必须二次确认并在状态栏标红显示。
- 支持每个 session、每个文件夹、全局默认日志策略；优先级为 session > folder > global default。
- 支持日志查看入口和打开所在目录。

### 5.10 主题与外观

- 支持浅色、深色、跟随系统。
- 支持终端配色方案导入/导出。
- 支持默认外观配置、文件夹外观配置、每个保存的 session 外观配置。
- 外观配置采用继承优先级：session appearance > folder appearance > global default appearance > built-in default。
- 文件夹外观可作用于其子文件夹和保存的 session，子文件夹可继续覆盖。
- 每个 session 可独立设置应用主题、终端颜色方案、字体、字号、行距、光标形状、标签颜色、SFTP 面板默认可见性。
- 支持现代化主界面：左侧会话树、中央终端工作区、右侧可停靠工具面板、底部状态栏。
- 支持高 DPI 和多显示器。

## 6. 非目标范围

第一版不做以下功能：

- 不支持 Telnet、Rlogin、Serial、RDP、FTP。
- 不做 JavaScript 脚本引擎。
- 不做浏览器 WebView UI。
- 不做云同步和账号体系。
- 不做 Xshell 配置文件的完全兼容导入；可后续支持手动迁移工具。
- 不复制 Xshell 的图标、名称、布局细节和专有文案。

## 7. 技术选型

### 7.1 推荐栈

- 语言：Rust
- UI：Slint
- SSH/SFTP：优先评估 `russh`、`ssh2`、`openssh-sftp-client` 等 Rust 生态库
- 终端解析：优先评估 `alacritty_terminal` 或 `vte`
- 配置序列化：TOML 或 JSON；内部模型使用 Rust 强类型
- 加密存储：平台密钥链优先，主密码加密作为跨平台兜底
- 打包：Windows MSI/EXE、macOS DMG、Linux AppImage/Deb/RPM

### 7.2 为什么选择 Rust + Slint

Rust 更适合 SSH、SFTP、终端仿真、加密和高并发 I/O。它能降低内存安全风险，并且在跨平台系统集成上有长期维护优势。Slint 提供原生、轻量、现代化的 UI 能力，支持 Rust 绑定，不需要 JavaScript 或 WebView。

Fyne 的优势是 Go 开发速度快、跨平台简单，但复杂终端渲染、GPU 文本性能、现代化 UI 自定义和长期安全边界不如 Rust 路线稳。Iced 也适合 Rust，但要实现停靠面板、复杂列表、文件管理器和专业工具界面，Slint 的声明式 UI 和设计迭代体验更合适。

参考来源：

- Slint 官网：https://www.slint.dev/
- Fyne 官网：https://fyne.io/
- Iced 文档：https://docs.iced.rs/

## 8. 架构设计

YShell 应按清晰边界拆分为以下模块：

```text
app-shell
  管理窗口、菜单、标签、分屏、命令面板、全局快捷键。

ui-components
  提供会话树、终端视图、SFTP 面板、转发面板、日志视图、设置页。

session-core
  管理会话生命周期、连接状态、重连、keepalive、事件分发。

ssh-core
  封装 SSH 握手、认证、PTY、Shell channel、端口转发和代理。

sftp-core
  封装 SFTP 浏览、传输队列、权限、错误恢复和进度事件。

terminal-core
  负责终端解析、屏幕缓冲、文本选择、搜索、颜色和渲染数据模型。

config-store
  管理会话配置、用户设置、主题、快捷命令、已知主机和迁移。

secret-store
  管理密码、私钥 passphrase、主密码和平台密钥链。

logging-core
  管理会话日志、传输日志、敏感输入屏蔽和日志轮转。
```

## 9. 数据模型

### 9.1 SessionProfile

```text
id
name
folder_id
host
port
username
auth_profile_id
proxy_profile_id
terminal_profile_id
sftp_profile_id
appearance_profile_id
logging_profile_id
tunnel_profiles
tags
color
created_at
updated_at
```

### 9.1.1 FolderProfile

```text
id
parent_folder_id
name
appearance_profile_id
logging_profile_id
proxy_profile_id
sort_order
created_at
updated_at
```

### 9.2 AuthProfile

```text
id
name
method: password | private_key | agent | keyboard_interactive
username_override
private_key_path
secret_ref
agent_forwarding_enabled
```

### 9.3 SftpProfile

```text
default_remote_dir
default_local_dir
confirm_overwrite
preserve_timestamp
show_hidden_files
transfer_parallelism
```

### 9.4 TunnelProfile

```text
id
type: local | remote | dynamic
listen_host
listen_port
target_host
target_port
enabled_on_connect
```

### 9.5 ProxyProfile

```text
id
name
mode: none | global | session
protocol: socks4 | socks4a | socks5 | http_connect
host
port
username
secret_ref
resolve_dns_by_proxy
```

### 9.6 AppearanceProfile

```text
id
name
scope: global_default | folder | session
theme_mode: system | light | dark
terminal_color_scheme_id
font_family
font_size
line_height
cursor_shape
tab_color
show_sftp_on_connect
show_quick_commands_on_connect
```

### 9.7 LoggingProfile

```text
id
name
scope: global_default | folder | session
enabled
format: raw_transcript | sanitized_text
include_timestamps
record_remote_output
record_local_input
path_template
rotate_policy
```

## 10. UI 信息架构

主窗口采用专业工具布局：

- 顶部：菜单栏、快速连接栏、全局搜索/命令面板入口。
- 左侧：Session Manager，会话树、收藏、最近连接、标签过滤。
- 中央：终端标签组，支持单标签、多标签、左右/上下分屏。
- 右侧：可停靠工具面板，显示 SFTP、快捷命令、端口转发、会话信息。
- 底部：状态栏，显示连接状态、主机、用户、延迟、上传/下载速率、日志状态、代理状态。

SFTP 面板可以作为右侧停靠面板，也可以作为独立标签打开。默认建议与终端并排显示，方便一边执行命令一边传文件。

## 11. 安全策略

- 主机密钥采用 TOFU 策略：首次连接提示信任，后续变更必须强提醒。
- 密码和私钥 passphrase 默认进入系统密钥链。
- 如果系统密钥链不可用，使用主密码派生密钥加密本地 secret store。
- 私钥文件不复制进配置目录，只保存路径和必要元数据。
- 日志默认不记录密码输入和键盘交互中的敏感字段。
- 广播输入、批量删除远程文件、覆盖上传等高风险动作需要确认。
- 远程文件临时编辑必须使用隔离临时目录，并在关闭后提示是否上传变更。

## 12. 配置与存储

建议路径：

- Windows：`%APPDATA%\YShell`
- macOS：`~/Library/Application Support/YShell`
- Linux：`~/.config/yshell`

建议文件：

- `sessions.toml`：会话、文件夹、标签。
- `profiles.toml`：认证、代理、终端、SFTP、隧道配置。
- `appearance.toml`：全局默认、文件夹、session 外观配置和继承关系。
- `logging.toml`：全局默认、文件夹、session 日志策略。
- `proxies.toml`：SOCKS4、SOCKS4a、SOCKS5、HTTP CONNECT 代理配置。
- `themes.toml`：主题和终端配色。
- `known_hosts`：主机密钥。
- `commands.toml`：快捷命令和命令组。
- `state.toml`：最近连接、窗口布局、标签恢复状态。

## 13. 里程碑

### Milestone 0：工程骨架

- 建立 Rust workspace。
- 建立 Slint 主窗口。
- 建立配置目录、日志目录、错误处理和基础 CI。
- 完成 Windows/Linux/macOS 最小打包验证。

### Milestone 1：SSH 终端 MVP

- 支持快速连接 SSH。
- 支持密码和私钥认证。
- 支持终端渲染、输入、resize、复制粘贴。
- 支持主机密钥确认。
- 支持断开、重连、关闭标签。

### Milestone 2：会话管理

- 支持会话树、保存、编辑、删除、搜索。
- 支持认证配置、终端配置和颜色方案。
- 支持导入/导出 YShell 配置。

### Milestone 3：SFTP 核心

- 支持远程目录浏览。
- 支持上传、下载、删除、重命名、新建目录。
- 支持传输队列、进度、失败重试。
- 支持远程文件临时编辑。

### Milestone 4：高级 SSH 工作流

- 支持本地、远程、动态端口转发。
- 支持 SOCKS4、SOCKS4a、SOCKS5、HTTP CONNECT 代理。
- 支持快捷命令、Compose Pane、广播命令、Send Key Input To 实时键输入复制。
- 支持自动日志。

### Milestone 5：体验打磨

- 支持分屏标签组、状态栏、主题、全屏。
- 完成高 DPI、多显示器和跨平台快捷键打磨。
- 完成文档、发行包和贡献指南。

## 14. 测试计划

- 单元测试：配置解析、路径模板、密钥存储、终端解析、SFTP 队列。
- 集成测试：用本地测试 SSH server 验证认证、PTY、SFTP、端口转发。
- 跨平台测试：Windows、Linux、macOS 分别运行基础 UI 和 SSH 连接测试。
- 安全测试：主机密钥变更、错误密码、密钥 passphrase、日志敏感输入屏蔽。
- UI 回归测试：主窗口、会话编辑器、SFTP 面板、转发面板、设置页截图对比。
- 性能测试：大输出滚屏、长时间连接、大文件传输、多会话并发。

## 15. 开源治理

- 许可证建议：Apache-2.0 或 MIT；如果使用 GPL 依赖，需要重新评估整项目许可证。
- 仓库应包含贡献指南、行为准则、问题模板和路线图。
- 明确声明项目与 NetSarang/Xshell 无关联。
- 文档中使用“对标”“兼容工作流”，避免使用“复刻 UI”“克隆品牌”等表达。

## 16. 验收标准

第一版可发布时必须满足：

- 用户能在三大桌面平台安装并启动 YShell。
- 用户能创建 SSH 会话并完成交互式终端操作。
- 用户能保存会话、重新打开会话、管理会话树。
- 用户能打开同一会话的 SFTP 面板并完成上传/下载。
- 用户能配置私钥、代理、端口转发、日志和终端配色。
- 用户能同时打开多个标签，并在断线后清楚看到状态和重连入口。
- 凭据不会明文保存在普通配置文件中。
- 项目文档明确说明支持范围、非目标范围和安全模型。
