# 原型实现计划（`win/ui2`）

> 目标：以 `docs/prototypes/` 为唯一 UI 真源，把每个页面 / 弹窗 / 交互在 `ui/` + `crates/` 中实现并对齐。
> 差距清单：[`IMPLEMENTATION-GAPS.md`](./IMPLEMENTATION-GAPS.md)（2026-11-03 审计：91 项，54 缺口 = P0 5 / P1 36 / P2 13）。
> 本文件随进度更新；波次结束在此记录结果。

## 0. 基线

| 项 | 值 |
|---|---|
| 分支 | `win/ui2`，从 `dev` @ `13c2e6a` 切出。`dev/win/ui2` 与 `dev` 引用命名冲突（git 不允许），CI 的 `dev-build.yml` 接受 `win/**`（即 `<system>/<topic>`） |
| 上一轮成果 | 归档 ref `refs/archive/win-ui-opt`（`18c1cd6`，304 文件 +51k/−34k，17 提交），**只作参考**，不 cherry-pick |
| 构建环境 | `D:\SDK\msys64\ucrt64\bin` 进 PATH；`CARGO_HOME=D:\SDK\cargo-home`；`CARGO_TARGET_DIR=D:\SDK\cargo-target` |
| 工具 | `slint-tr-extractor 1.18.1`；WSL `python3 3.13.5`；Edge headless（原型截图） |
| 基线 `cargo xtask check` | 通过（修基线后）。原基线红：rust 1.99 rustfmt 与仓库 CRLF 冲突 + 4 处 clippy + 过期 `product_smoke` 测试；已加 `rustfmt.toml`（`newline_style = "Windows"`）并清理 |
| 基线 `cargo xtask test` 等价命令 | 通过：`cargo test --workspace --exclude xtask` 27 个测试套件全绿（yshell-app 391 + 集成 6）。原基线红：7 个过期断言（编辑器/会话树文案与顺序、清单格式）、1 个环境相关字体断言、7 个 MCP status kind 缺模板、2 个 token 纪律注释误报、过期 `product_smoke` 测试；已修 |

审计是上一轮中途快照，其中「本轮实施中」的 4 项（Settings → Appearance / Terminal / Security、主密码两窗）在当前 dev 上仍是缺口，执行时按缺口处理。

## 1. 决策（默认执行；要改请在这一波结束前提出）

| # | 决策 | 默认 |
|---|---|---|
| D-1 | Settings → AI / MCP 方向 | 保持现有 stdio 实现：只做 Enable / Auto-approve / 状态；原型的 Port / Listen(LAN) / API key 标「本版不做」并同步原型 |
| D-2 | 主密码范围 | 按 D35 做闭环：Set / Change / Remove / Reset + 启动解锁 + 受保护 session/key 加密；KDF 升级（argon2/AEAD）列为后续安全债，不在本轮 |
| D-3 | i18n | 新文案全部 `@tr`；波次末统一抽取；zh-CN 由 agent 初译，验收时抽审 |
| D-4 | 分支 / 提交 | 本地 `win/ui2` 按波次语义提交；**不 push**，验收通过后再定合并 |
| D-5 | 节奏 | 每波结束停一次，给截图 + 变更摘要 + 下一波计划 |

## 2. 波次与顺序

排序原则：**配置 / 运行时先行 → 页面并行 → 主壳最后（唯一热点文件）→ i18n / 清理 → 统一验收**。

| 波次 | 内容 | 主要执行 | 出口 |
|---|---|---|---|
| 0 | 基线、差距清单、任务板、计划 | Lead | 基线绿、清单可用 |
| 1 | 基础层：config schema + runtime + 共享组件 + i18n 管线 | Lead / 1 人 | 字段与 API 定稿，单测绿 |
| 2 | Settings + 主密码 / Session Editor / 管理器 + 弹窗（3 并行） | 3 个 teammate | 每页功能 / 布局对齐 |
| 3 | 主壳菜单与快捷键 / 停靠面板 / 工作区（3 并行） | 3 个 teammate + Lead | 主流程可点通 |
| 4 | 跨切规范 + i18n + 遗留清理 + 文档（2 并行） | 2 个 teammate + Lead | 规范项清零 |
| 5 | 全量验收：ci、实机逐页截图、修复、提交 | Lead + 全员 | 验收清单全绿 |

## 3. 各波任务

### 波 1 · 基础层（Lead）

1. **config schema + 迁移**（`crates/yshell-config`）：`[ui] appearance_mode / appearance_accent`；`TerminalProfile` 增 `middle_click / right_click / multi_line_paste / auto_copy`；`[sftp] show_hidden / conflict_action / external_editor`；login script 步骤模型；session icon；folder 级继承字段；主密码状态。serde 默认 + sanitize + 迁移测试。
2. **runtime 状态机**（`crates/yshell-app/src/runtime`）：`terminal_input`（点击 / 粘贴 / 自动复制 / 多行确认）纯逻辑 + 单测；settings 草稿；Appearance 预览 / 提交 / 回退；SFTP 默认值应用；login script 连接挂点；SecretVault 状态机 + 启动解锁。
3. **共享组件**：`WinTextInput` / Picker 焦点环、tooltip 覆盖、缺失图标、三档窗口工具。
4. **i18n 管线**：WSL 下 `scripts/i18n/extract.sh` + `catalogs.py` 跑通，确认幂等。

### 波 2 · 数据驱动页面（3 并行）

- **A settings-security**：`ui/pages/settings.slint`、`ui/windows/settings_window.slint`；Appearance / Terminal / Security / SFTP / Advanced 五个分区 + AI/MCP 收口；新增 `master_password_window`、`master_password_unlock_window`（compact 560×400）。
- **B session-editor**：`ui/pages/session_editor.slint` + `ui/windows/session_editor_window.slint`；六个分区 + 页级 Inherit；Login Script 分区 + `login-step-keys` / `login-step-wait` 子窗；`tunnel-editor` 窗；Save 并关闭 / Save and Connect。
- **C managers-dialogs**：`host_keys` / `private_keys` / `proxies` / `theme_manager` 四页 + 各自子窗；host-key 改 compact；auth-prompt 去 Remember；新增 `sftp-chmod` 窗；关标签 / 退出后果清单。

### 波 3 · 主壳 + 面板 + 工作区（3 并行）

- **E shell**（`main_window.slint` 主写者）：6 个一级菜单（补 Tools）、Session 菜单、`Ctrl+Shift+S`、`F11`、Full Screen / Focus Mode、状态栏 UTF-8。
- **F docks-panels**：Sessions 树图标 / 原地改名 / 右键项 / New Folder 禁用；Transfers 批量；Tunnels `+` + 禁用 Start；Commands；dock 宽度。
- **G workspace**：标签图标 / 原地改名 / 分屏上限 4；Find Across；REC 条；N9 banner + pane Receive；Quick Connect Clear；SFTP 原地 New Folder / Rename + chmod 入口。

### 波 4 · 规范 + i18n + 清理（2 并行）

- i18n：抽取 `.pot`、同步 `en-US` / `zh-CN`、初译、零 diff 校验；Rust 侧 I18N-1 kind 补文案。
- 规范：工具条图标化 + 全量 tooltip、禁止打字确认、无省略号、一窗一页脚、分区标题移除、焦点环。
- 清理：删遗留 `known_hosts` 页与 `CLEAR` 路径；原型 / 契约注释同步；status / 契约文档更新。

### 波 5 · 验收

- `cargo xtask ci`；`cargo xtask run` 实机逐页走查；截图对照 `docs/prototypes`；交互清单打勾；修复回归；按波次提交。

## 4. 热点文件与并行规则

| 文件 | 规则 |
|---|---|
| `ui/main_window.slint`（165KB） | 单写者：波 2 为 Lead，波 3 为 shell 队友；别人只交 wiring note |
| `crates/yshell-app/src/{bootstrap,dialog_windows,windows}.rs` | Lead 单写 |
| `crates/yshell-config/src/schema.rs` | 波 1 定稿，之后只读 |
| `runtime/theme.rs` | settings 队友独占 |
| `ui/theme.slint`、`ui/components/basic/*` | 波 1 定稿，之后只加不改 |
| `translations/**` | 波 4 统一生成 |

wiring note 格式：新增 property / callback 名 + 期望绑定 + 插入锚点（函数 / 注释关键字），由热点文件所有者落地。

## 5. 验证

- 每波：`cargo fmt --all --check`；`cargo check -p yshell-app`；`cargo xtask check`；相关 `cargo test`。
- 视觉：`cargo xtask run` 截图 vs 原型 headless 截图；证据存仓库外 `D:\NewSpace\ui-evidence\`。
- 交互：按原型页内状态条与菜单路径逐项走查，禁用项确认仍禁用。
- i18n：抽取脚本重复运行零 diff；`.po` 头部 / msgctxt 按 `scripts/i18n/README.md` 校验。

## 6. 风险

- `main_window.slint` 热点 → 单写者 + wiring note，每次合并必编译。
- Slint 1.18 布局坑（`cross-axis-*` 与手写 `y:` 冲突）。
- 基线测试可能有既有失败（字体环境 / 过期断言）→ 只保证不新增。
- Windows 下 app 运行时不能覆盖 exe → 验收前后关窗再构建。
- 原型与实现冲突项（MCP、Tunnels）→ 先改原型 / status 再实现。

## 7. 波次记录

### 第 0 波 · 完成（2026-11-03）

- 分支 `win/ui2`，基线提交 `fe7f037`（chore: 清 fmt/clippy/test 漂移）。
- 基线修复：`rustfmt.toml` 固定 CRLF + 当前工具链重排；4 处 clippy；7 个 MCP status kind 模板；7 个过期/环境相关断言；删除过期 `product_smoke` 测试。
- 验证：`cargo xtask check` 通过；`cargo test --workspace --exclude xtask` 27 个测试套件全绿；`yshell.exe` 启动冒烟通过（1440×900，见 `D:\NewSpace\ui-evidence\wave0\app-launch.png`）。
- 差距清单恢复：`IMPLEMENTATION-GAPS.md`；任务板：task-1 … task-11。
- 旧一轮成果归档在 `refs/archive/win-ui-opt`（只读参考）。

### 第 1 波 · 完成（2026-11-03）

提交：`c6d1102`（config + runtime 基础）、`edb0243`（Settings 草稿 / SFTP 默认值）。

- **config**（`crates/yshell-config`）：
  - `[ui]` 新字段：`appearance_mode`（system/light/dark）、`appearance_accent`（follow/6 预设，与 `yshell_ui::appearance` 同名）、终端交互四项（middle/right click、multi-line paste、auto copy）。
  - `[sftp]` 新字段：`show_hidden`（默认开）、`conflict_action`（ask/replace/skip/stop）、`external_editor`（None = 系统默认）。
  - `SessionProfile`：`icon`（None = 默认 / 预设 id / 自定义路径）、`login_script`；`LoginScriptProfile` + `LoginStepAction::{Keys, Wait}`。
  - `FolderProfile`：auth / proxy / sftp / tunnel / login-script 五类可继承覆盖；`ResolvedSessionProfile` 解析 nearest-first，并随 `.folder.toml` 持久化。
  - sanitize：图标与外部编辑器去空白、空值回 None；登录脚本名字补齐、等待上限 60s。
- **runtime**：
  - App theme 改为读配置（`YSHELL_THEME` / `YSHELL_ACCENT` 环境变量仍优先，兼容测试与 CI）。
  - 新增 `runtime/terminal_input` 纯逻辑模块（点击语义 / 多行粘贴裁决 / 自动复制）+ 7 单测。
  - Settings → Terminal 草稿新增四项交互（update / Save 写 `[ui]` / Reset 回 D37 默认）。
  - 多行粘贴 `Block` 已在两条粘贴路径生效（新状态 kind + 模板）；选区结束且开自动复制时写入系统剪贴板。
  - 新 SFTP 标签与本地栏按配置继承 Show hidden（默认开，对齐原型）。
  - 新增 TextFormats 模板：MCP 七个状态、`terminal-paste-blocked-multiline`。

验证：`cargo xtask check` 通过；`cargo test --workspace --exclude xtask` 全绿（yshell-config 60 / yshell-app 402 + 集成）。

移交波 2：

- 主密码 / SecretVault 状态机与启动解锁 → 波 2-A（随 Settings → Security 一起做）。
- login script 连接后执行挂点 → 波 2-B（随 Session Editor 一起做）。
- Settings 四项交互的 projection / Slint 接线 → 波 2-A。
- SFTP conflict 默认动作、external editor 启动 → 波 3-G / 波 4 收口。


