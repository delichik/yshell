# YShell 技术架构设计

## 1. 技术目标

YShell 使用 Tauri 作为桌面外壳和系统集成层，使用 xterm.js 作为终端渲染层。前端负责窗口、布局、交互状态和终端呈现；Rust 后端负责本地 PTY、SSH 协议、文件系统、密钥与凭据、配置读写、日志、系统集成和安全边界。

架构目标：

- 将终端渲染、会话模型、连接协议和持久化配置解耦。
- 使用强类型 IPC 避免前后端命令漂移。
- 支持 Windows、Linux、macOS 的本地终端与 SSH 行为。
- 为后续 SFTP、端口转发、插件系统和团队配置源预留扩展点。

## 2. 推荐目录结构

```text
yshell/
├── src/                         # 前端应用
│   ├── app/                     # 应用启动、路由、全局状态
│   ├── components/              # Fluent 风格基础组件
│   ├── features/
│   │   ├── sessions/            # 会话树、会话编辑、快速连接
│   │   ├── terminal/            # xterm.js 包装、窗格、搜索、复制粘贴
│   │   ├── workspace/           # 标签页、分屏、窗口状态
│   │   ├── settings/            # 设置中心
│   │   └── logs/                # 日志 UI
│   ├── bindings/                # Tauri IPC 类型绑定
│   └── styles/                  # 主题、设计令牌、全局样式
├── src-tauri/
│   ├── src/
│   │   ├── commands/            # Tauri command 入口
│   │   ├── config/              # 配置模型、导入导出、迁移
│   │   ├── crypto/              # 加密与安全存储适配
│   │   ├── pty/                 # 本地 PTY
│   │   ├── ssh/                 # SSH 连接、认证、通道、SFTP
│   │   ├── terminal/            # 终端会话运行时、事件分发
│   │   ├── logging/             # 会话日志与审计
│   │   └── platform/            # 平台差异适配
│   └── tauri.conf.json
├── docs/design/                 # 设计文档
└── tests/                       # 集成与端到端测试
```

实际工程可按所选前端框架微调，但必须保持“UI 状态、终端渲染、协议运行时、配置存储”四层边界清晰。

## 3. 核心模块

### 3.1 前端 Shell

职责：

- 应用启动、全局主题、窗口布局。
- 标签页、窗格、侧边栏、状态栏。
- 命令面板、设置中心、会话编辑器。
- 调用 IPC 命令并订阅后端事件。

非职责：

- 不直接处理 SSH 密码认证。
- 不直接保存明文凭据。
- 不在 UI 状态中保存大体量终端输出。

### 3.2 xterm.js 终端层

职责：

- 创建、销毁和复用 xterm.js 实例。
- 处理输入、输出、resize、搜索、复制粘贴、链接识别。
- 维护每个窗格的终端渲染设置。

设计要点：

- 后端输出以二进制或 UTF-8 文本事件流推送给对应终端实例。
- 前端输入通过 IPC 发送到指定 `terminal_runtime_id`。
- 终端缓冲区由 xterm.js 管理，应用状态只保存必要元数据。
- resize 事件需要节流，避免拖拽分屏时频繁请求后端调整 PTY/SSH 通道大小。

### 3.3 会话配置模块

会话配置是持久化对象，与运行中的终端实例分离。

推荐核心字段：

```text
SessionProfile
- id: string
- name: string
- folder_id: string | null
- tags: string[]
- protocol: local | ssh
- host: string | null
- port: number | null
- username: string | null
- auth: AuthConfig
- proxy: ProxyConfig | null
- terminal: TerminalConfig
- appearance: AppearanceConfig | null
- logging: LoggingConfig | null
- created_at: datetime
- updated_at: datetime
- last_connected_at: datetime | null
```

运行实例使用独立对象：

```text
TerminalRuntime
- runtime_id: string
- profile_id: string | null
- kind: local | ssh
- status: connecting | connected | disconnected | failed
- pane_id: string
- tab_id: string
- process_ref/channel_ref: backend owned
```

### 3.4 Rust 运行时模块

Rust 后端维护真实连接与进程句柄：

- 本地 PTY：启动 shell、写入 stdin、读取 stdout/stderr、resize、kill。
- SSH：建立 TCP 连接、认证、打开 session channel、处理 shell、exec、resize、关闭。
- 日志：将输出流按配置写入文件。
- 安全存储：读写密码、私钥口令和代理凭据。
- 配置：读写 JSON/TOML/SQLite，并执行版本迁移。

后端应使用 runtime registry 管理活跃终端，避免前端伪造句柄访问其他会话。

## 4. IPC 设计

### 4.1 命令原则

- IPC 输入输出必须有稳定类型。
- 每个命令只做一个清晰动作。
- 长生命周期任务通过事件推送结果，命令只返回启动状态。
- 后端必须校验 `runtime_id`、路径、配置 ID 和权限边界。

### 4.2 建议命令

| 命令 | 方向 | 说明 |
| --- | --- | --- |
| `sessions.list` | 前端 -> 后端 | 获取会话树和会话摘要。 |
| `sessions.save` | 前端 -> 后端 | 创建或更新会话配置。 |
| `sessions.delete` | 前端 -> 后端 | 删除会话配置，可选择是否删除凭据。 |
| `terminal.open_local` | 前端 -> 后端 | 启动本地 PTY。 |
| `terminal.open_ssh` | 前端 -> 后端 | 启动 SSH shell。 |
| `terminal.write` | 前端 -> 后端 | 向运行中终端写入用户输入。 |
| `terminal.resize` | 前端 -> 后端 | 调整终端尺寸。 |
| `terminal.close` | 前端 -> 后端 | 关闭运行中终端。 |
| `terminal.start_log` | 前端 -> 后端 | 为终端开启日志。 |
| `terminal.stop_log` | 前端 -> 后端 | 停止日志。 |
| `credentials.set` | 前端 -> 后端 | 写入安全凭据。 |
| `credentials.delete` | 前端 -> 后端 | 删除安全凭据。 |
| `config.export` | 前端 -> 后端 | 导出配置。 |
| `config.import` | 前端 -> 后端 | 导入配置。 |

### 4.3 建议事件

| 事件 | 方向 | 说明 |
| --- | --- | --- |
| `terminal.output` | 后端 -> 前端 | 终端输出数据。 |
| `terminal.status_changed` | 后端 -> 前端 | 连接状态变化。 |
| `terminal.exit` | 后端 -> 前端 | 进程或 SSH 通道退出。 |
| `terminal.error` | 后端 -> 前端 | 运行时错误。 |
| `sessions.changed` | 后端 -> 前端 | 会话配置变化。 |
| `host_key.verify_required` | 后端 -> 前端 | 主机密钥需要用户确认。 |
| `log.status_changed` | 后端 -> 前端 | 日志状态变化。 |

## 5. 数据存储

### 5.1 配置文件

推荐早期使用可读 JSON 或 TOML，并预留迁移版本号：

```text
config_version: 1
profiles: SessionProfile[]
folders: SessionFolder[]
settings: AppSettings
keybindings: Keybinding[]
```

优点是易于调试、导入导出和社区贡献。后续如果会话数量很大或需要复杂查询，可迁移到 SQLite。

### 5.2 凭据存储

- Windows：优先系统凭据管理器。
- macOS：优先 Keychain。
- Linux：优先 Secret Service/libsecret，缺失时提示用户选择加密文件存储或不保存。
- 凭据通过 `credential_ref` 与会话配置关联，配置文件不保存明文。

### 5.3 主机密钥

主机密钥数据库独立存储，字段包含：

- host、port、algorithm、fingerprint、public_key、first_seen_at、last_seen_at。
- 主机密钥变化时必须阻断连接并要求用户确认。

## 6. SSH 与本地 PTY

### 6.1 SSH 能力

MVP：

- 密码认证。
- 私钥认证。
- SSH Agent 认证。
- 交互式 shell。
- resize。
- keepalive。
- 主机密钥校验。

后续：

- 跳板机链路。
- HTTP/SOCKS 代理。
- 本地/远程/动态端口转发。
- SFTP。
- X11 转发。

### 6.2 本地 PTY

- Windows 使用 ConPTY。
- Linux/macOS 使用系统 PTY。
- shell 配置包含命令、参数、环境变量和工作目录。
- 所有平台都需要处理 resize、退出码、编码和信号差异。

## 7. 安全设计

### 7.1 敏感数据

- 前端只在用户输入过程中短暂持有敏感字段。
- 保存凭据时通过 IPC 交给后端写入系统安全存储。
- 导出配置默认排除凭据引用对应的秘密内容。
- 加密导出必须要求用户设置导出口令，并提示保管风险。

### 7.2 危险操作

以下操作默认需要确认：

- 删除会话或文件夹。
- 关闭含活跃连接的窗口。
- 粘贴多行命令。
- 开启广播输入。
- 接受未知主机密钥。
- 在主机密钥变化后继续连接。

### 7.3 日志安全

- 日志开启时在状态栏显示明显状态。
- 日志文件默认不记录用户键盘输入，仅记录远端输出；若需要输入日志必须单独开启。
- 日志路径应避免默认落在公开同步目录。

## 8. 可扩展性

### 8.1 插件预留

早期不实现完整插件系统，但接口设计应避免封死扩展能力：

- 命令面板命令注册。
- 会话上下文菜单扩展。
- 终端链接处理器扩展。
- 配色与主题包。
- 命令片段库。

### 8.2 协议扩展

协议运行时抽象应允许新增：

- Telnet。
- 串口。
- Kubernetes exec。
- Docker exec。
- WSL 发行版终端。

## 9. 测试策略

### 9.1 Rust 后端

- 会话配置序列化与迁移测试。
- 凭据引用和敏感字段过滤测试。
- PTY 启动、resize、关闭测试。
- SSH 可使用本地测试服务器或容器化 OpenSSH 进行集成测试。

### 9.2 前端

- 会话树增删改查测试。
- 标签页和分屏状态 reducer 测试。
- 快捷键冲突检测测试。
- xterm.js 包装层使用 mock IPC 验证输入输出绑定。

### 9.3 端到端

- 打开本地终端并执行命令。
- 创建 SSH 会话并连接测试服务器。
- 保存、重启应用、恢复会话树。
- 分屏、广播输入、日志开启关闭。

## 10. 发布与更新

- 使用 Tauri 打包 Windows、Linux、macOS 安装包。
- 发布产物需要包含校验和。
- 自动更新应可关闭，并支持稳定版与预览版通道。
- 安装包签名策略按平台逐步完善，早期至少保证发布说明和校验文件清晰。
