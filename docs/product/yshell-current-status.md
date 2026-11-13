# YShell 本 build 产品现状

更新时间：2026-11-12  
性质：**唯一现行产品范围文档**（有什么 / 砍了什么 / 还缺什么 / 仍生效的决策）

冲突时：**本文（产品范围）> [`docs/prototypes/`](../prototypes/README.md) 及其文档（设计稿）> 实现与单测 > 参考文档**（[`yshell-ui-interaction-contract.md`](./yshell-ui-interaction-contract.md)、设计语言、[`archive/`](./archive/README.md) 等）。  
交互契约与 archive **不是设计真源**，仅供参考；不得把其中愿景条款直接当成待实现需求。

---

## 1. 已交付

| 域 | 概要 |
|---|---|
| 标签 / 分屏 / N8 批量关闭 | 多标签、分屏、Close Other/Left/Right/All |
| 快速连接 N2 | QC 页、历史、Ctrl+O；顶部 QC 输入条已移除 |
| 面板停靠 N3 | 左右 Dock、上下分栏、布局持久化 |
| SFTP N1 | 双栏、应用内拖放、队列、冲突、批量删除、rfd；可拖入远端 |
| 密钥 / 认证 N4 | 认证弹窗、私钥页、Known Hosts 管理页 |
| 终端样式 N5 / N5a | 继承链、配色、字体（Terminal Styles 管理器）；物理像素光栅 / CJK |
| 日志 UI N6 | 右键启停、REC、弹窗、rfd |
| 多窗口 N7 Phase 1 | 菜单迁窗、关窗确认 |
| 同步输入 N9 | **双向组模型**：逐会话 Sync 开关（进/出组）；组一旦开启，**所有会话都显示 banner**（组外为 Off）；**结束全部同步**解散整组（不做 Pause）；状态栏 chip；AI/MCP 输入自动退出同步；不弹 KEYS 确认（D23） |
| 本地终端 | runtime/PTY 已有；**本版无 UI 入口**（`+` 开 QC；`open_local_terminal` 仅测试/预留） |
| 基建 | Slint 1.18、R1/R2 拆分、L2 Esc/遮罩、对比度 xtask、真实 SSH only |
| MCP（终端控制） | Settings → **MCP** 独立分区（原型）：Enable / Port / Listen(Local\|LAN) / API key / View user / Control user / Auto-approve；默认关。AI 创建的标签带 AI 标志（TabBar 原型）。实现仍含授权锁 + `session_run`/blocker；不含 SFTP、不含代填密码/host key（见 D34） |
| D38 四角色多进程 | Host（拥有 AppRuntime）/ SessionWorker / McpDaemon / Shell；**角色二进制已分离**（`yshell` 用户入口 + `yshell-host` / `yshell-shell` / `yshell-session-worker` / `yshell-mcp-daemon`，argv 角色 flag 保留）；默认 ensure Host + Shell 附着；单实例锁 + run-state pid 身份校验 + 子进程回收 + 优雅关机序列；零窗关机；二次启动聚焦已有窗口；Host 重启后 Shell 懒重连。未闭环见 §3「远程 Shell UI 完整度」。`YSHELL_TEST_INPROCESS_HOST=1` 同进程完整 GUI |

---

## 2. 本版明确不做

UI 为 disabled + tooltip、空态，或决策延后。**不是待补闭环。**

| 项 | 说明 |
|---|---|
| Quick Commands 内容 | Dock 仅空态；View 仍可显隐面板 |
| Import / Export Config | File 菜单禁用占位（Not in this version）；空白区 Import disabled |
| Expand All / Collapse All | 空白区 disabled |
| 文件夹：New Folder / Rename / Delete / Connect All | 仅 New Session Here、Edit…（与 Session 共用编辑器） |
| Session Editor：SFTP / Advanced 分区 | nav disabled |
| Settings：Shortcuts；整库 wipe secrets / 重置布局 | Advanced 只展示路径等；主密码 Reset 仅清受保护 session/key（见 D35），不等于整库 wipe |
| 会话栏 RECENT/ACTIVE、`+`/`…` | 已删（D25） |
| Fake SSH / 后端切换 | 已删（D26） |
| OS 拖出到资源管理器 | 等 winit 0.31 + Slint（D3） |
| 标签拖出成新窗 | 等上游；现仅菜单迁窗（D5） |
| 主题导入导出 / 光标形状 | 首版不做（D16） |
| 仅路径引用私钥（不托管内容） | 不做（D19） |

---

## 3. 真缺口（仍露出、未闭环）

| 项 | 缺口 |
|---|---|
| SSH 端口转发 | 可配 + Dock Start/Stop；runtime 不 provision |
| 会话 Proxy 真连 | Manager/探测可用；native SSH/SFTP 拒绝 proxy |
| Secret Store / 主密码 | 原型已补：Security 卡片、Set/Change/Remove/Reset、启动解锁（D35）。主密码状态机与启动解锁已接线；不再读 `YSHELL_MASTER_PASSWORD`（生产默认 OS keychain，文件加密库只在测试中显式注入），仍未加密 session/key 文件（真缺口） |
| Appearance 主题 | 原型已有 App theme（Mode / Accent）+ Terminal style 下拉，Settings 已有控件并即时预览 / Save 持久化；外观只读 `[ui] appearance_mode / appearance_accent`（不再有 `YSHELL_THEME` / `YSHELL_ACCENT` 环境变量）；Appearance 仍在页面内内联编辑字体 / 主题，未改为 Profile 下拉 |
| Terminal 交互设置 | 原型已有 Middle / Right click、Multi-line paste、Auto copy；runtime 未接线 |
| 弹窗尺寸 / 特殊窗 | 原型已统一为 compact / standard / wide；Host key Slint 仍 standard，SFTP Permissions 新窗未接线；旧 NewFolder / SftpEntry 窗口仍在代码中 |
| N9 Visible | ~~目标集等同 All~~ 已随组模型改版删除（无 All/Visible 预设）；终端上方 banner 已随波 3 #17 落地 |
| N6 遗留 | R-107 存为会话默认；日志大小上限 |
| `ui.new_tab_mode` | schema 有；`+` 忽略 session-editor；Settings 无项（相对 D11） |
| `ui.multi_window_enabled` | schema 幽灵字段，未接线 |
| 远程 Shell UI 完整度 | D38 默认分进程：完整投影 + 终端 snapshot（含选区）；**主窗回调 202/359**（其余为独立 OS 窗，已在 `remote_dialog_wire.rs` 接线）；SFTP 面板 / 终端选择复制粘贴查找 / 本地栏 / 传输 / tab 菜单 / 面板拖动 / viewport-DPI / 终端外观投影 / 原生文件选择器已接通；**多窗已落地**（Host 单调 window id + tab 归属、move-tab-to-new-window / move-to-main、关闭次窗、零窗关机；关闭确认窗仍按 window 0 简化）；Worker/MCP 退出经投影变 Failed；Shell 不再构造 AppRuntime。Host 拥有 SFTP worker 与唯一落盘权；周期探针 / 延迟列表 / 用户导航均在独立 SFTP control 线程。远程 OS 窗：解锁 / 密码 / host-key、Settings、主密码子窗、会话编辑器（含 Login Script / Tunnel）、文件夹默认值、私钥（含生成/导入/导出/部署）、代理、终端样式、Host Keys（含增/导入/导出）、日志、认证、SFTP 冲突 / 属性 / chmod、About、新建文件夹、SFTP 条目，以及删除确认、关标签、退出、关窗、清 scrollback、MCP consent。Settings 的 L 档在 Shell 本地直到 Save（I/R 与其它窗的控件仍每次 UiCall）。**未闭环**：副 pane 位图未绘制（N7 既有限制）、SFTP/终端解析/本地 PTY 仍在 Host、新增 `@tr` 文案未回填 `.po/.pot`。`YSHELL_TEST_INPROCESS_HOST=1` 仍为完整 DialogWindowManager。见 [`process-architecture.md`](../prototypes/process-architecture.md) |
| L1 状态栏 i18n 方案 | 方案文档未落盘 |
| L4 fmt + cargo-deny 冻结 | 等独占窗口 |

---

## 4. 仍生效的产品决策（原 requirements §12）

| # | 决策 |
|---|---|
| D1 | Slint 1.18.1；MSRV 1.92 |
| D2 | 应用内拖放用原生 DragArea/DropArea |
| D3 | OS 拖入/拖出等 winit 0.31 + Slint；不自研平台层 |
| D4 | 文件对话框用 rfd，失败降级路径输入 |
| D5–D8 | 多窗：菜单迁窗；关窗确认断开；布局全局一套；不做自动开窗 |
| D9–D10 | SFTP 可折叠本地栏；远端→远端=下载再上传 |
| D11–D13 | `+` 默认可切 QC/编辑器；历史上限 20；快速链接只存目标（链接入口后来下线，以实现为准） |
| D14 | SFTP 页只留导航 + ⋯ |
| D15–D17 | 预设主字体 + 备用链；主题只影响新开终端；不做导入导出/光标 |
| D18–D21 | 私钥与 host key 分栏；加密托管；应用内生成 + 导入（文件/粘贴）（D20 修订）；不做仅路径引用（D19）；按 auth_methods 弹窗 |
| D22–D24 | 日志默认 sanitized；N9 不弹确认；关标签 flush；IME 提交后同步 |
| D25–D26 | 会话栏收敛；仅真实 SSH |
| D27–D32 | 状态栏 i18n / Esc 遮罩 / 对比度 / 字形 N5a / swash / WSL 构建路径等（多数已落地，L1/L4 见 §3） |
| D33 | Settings → SFTP 本版只保留：Show hidden 默认、Conflict 默认动作（Ask / Replace / Skip / Stop，默认 Ask）、External editor（下拉：System default + 自动检测知名编辑器 + Custom；仅 Custom 才 Browse 路径）。Remote Edit 对齐 Xftp：保存即自动上传；监视在编辑器进程退出或 SFTP/SSH 断开后结束。路径 / 并行 / 时间戳等旧全局项本版不进 Settings。 |
| D34 | Settings → **MCP** 独立分区：Enable、Port、Listen（Local / LAN）、API key（可选；LAN **推荐**）、View user、Control user（隐含 View）、Auto-approve。默认 Off。Auto-approve Off 时 `request_control` 弹 MCP control consent（Deny \| Allow；Don't ask again + Allow → 打开 Auto-approve）。MCP/AI 创建的工作区标签显示 **AI** 标志。 |
| D35 | Settings → Security：Master password 卡片（状态 + Set / Change / Remove / Forgot password?）；管理器入口用整行可点清单（图标 + 名称 + 计数 + chevron），不写摘要。Master password 可选（对齐 Xshell）：Not set = 不启用主密码加密层；Set = 加密全部 session/key 文件与保存的口令，启动时解锁；Change / Remove 校验当前口令；Reset 删除受保护数据，无打字确认词。 |
| D36 | Settings → Appearance 分两组：**App theme**（Mode System / Light / Dark + Accent Follow system / 6 预设）与 **Terminal style**（全局 Style 下拉 + 管理；字体 / 配色在 Terminal Styles 管理器）。App theme 即时预览，Save 持久化；Terminal style 保存后对新开终端生效。 |
| D37 | Settings → Terminal 新增交互项：Middle click（Paste 默认 / Do nothing）、Right click（Context menu 默认 / Paste / Do nothing）、Multi-line paste（Ask 默认 / Paste / Block）、Auto copy（Off 默认）；Reset defaults 一并重置这些值。 |
| D38 | **进程拆分**：Host / SessionWorker（一连一进程）/ McpProcess（监听单例）/ Shell（纯 UI）。MCP **全局**可见全部 session、可随时切换，**不**绑定「宿主窗」；新建连接的 tab 仅临时落在某窗展示。清理权**只在 Host**；零 UI 窗 → Host 关机收尸；Job Object + 心跳 + `run-state` 启动收尸防野进程。默认启动：ensure Host OS 进程 → Shell 附着；`YSHELL_TEST_INPROCESS_HOST=1` 保留同进程完整 GUI。配置 L/I/R：L 档 Shell 本地草稿至 Save，I/R 经 UiCall；Host 是唯一落盘者。细节见 [`docs/prototypes/process-architecture.md`](../prototypes/process-architecture.md) §7。 |

完整原文与 N0–N9 需求叙述见 [`archive/yshell-next-features-requirements.md`](./archive/yshell-next-features-requirements.md)。

---

## 5. 维护约定

- 砍功能 → 改实现 + 更新 §2；相关原型标禁用/空态；参考契约可事后同步，非必须先改。
- 补闭环 → 改代码 + 从 §3 删行或移入 §1；设计面先对齐 [`docs/prototypes/`](../prototypes/README.md)。
- 新决策 → 写入 §4（递增 Dn），不必复活 requirements 长文。
- UI 设计变更 → 只改原型（及计划跟踪）；勿把契约/archive 愿景直接写成缺口。
- 历史任务设计、spike、早期规格一律只放 [`archive/`](./archive/README.md)，不在本目录平铺。
