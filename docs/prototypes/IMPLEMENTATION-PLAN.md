# 原型实现计划（`win/ui2`）· 现行版本 2026-11-05

> 目标：以 `docs/prototypes/` 为唯一 UI 真源，把每个页面 / 弹窗 / 交互在 `ui/` + `crates/` 中实现并对齐。
> 差距清单：[`IMPLEMENTATION-GAPS.md`](./IMPLEMENTATION-GAPS.md)（2026-11-03 审计：91 项，54 缺口）。
> 本文件是**现行完整计划**：交付方式、决策、事项状态、波次状态、剩余顺序、已知问题、变更记录。
> 波次总览在 §0；每波结束在文末「波次记录」追加一条记录（含提交范围、门禁、证据、遗留）。

---

## 0. 波次状态总览

| 波次 | 范围 | 状态 | 完成日期 / 提交 |
|---|---|---|---|
| 0 | 基线修复、差距审计、计划与任务板 | ✅ 完成 | 2026-11-03 · `fe7f037`、`f530299` |
| 1 | 配置 / runtime 基础层（外观、终端交互、SFTP、登录脚本、文件夹继承） | ✅ 完成 | 2026-11-03 · `c6d1102`、`edb0243` |
| 2 | 页面与弹窗：Settings + 主密码、Session Editor、管理器、剩余弹窗、控件/表单/预览规范 | ✅ 完成（待用户核验） | 2026-11-05 · `2d26a06` … `2fbc72d`（#2–#16） |
| 3 | 主壳菜单/快捷键/sync banner、停靠面板、工作区 | 🔍 待用户核验 | #17 `de3aef8`、#18 `1125e6a`、#19 `07ed35d` |
| 4 | i18n 尾部、遗留清理、文档同步 | 🔍 待用户核验 | #20/#21 `1ee6b45` |
| 5 | 全量验收（门禁 + 用户逐窗核验 + 证据归档） | ⬜ 未开始 | — |

> 波次状态以本表为准；事项级细节见 §4，历史记录见文末附录。

---

## 1. 交付方式（现行，2026-11-04 起）

- **Lead 不写实现代码**：每个窗口/弹窗/交互作为一个「项」，委派 teammate 实现与接线；Lead 负责范围拆分、核验、提交、构建、汇报。
- **单项流程**：定义范围 → teammate 实现 + 单测 → Lead 跑四道门禁 → **本地语义化提交** → 本地重建 exe → 用户核验 → 通过后才进入下一项。
- **不 push**（用户 2026-11-04 要求）：只本地提交、本地构建；`origin/win/ui2` 目前停在 `eb3533e`（此前一次意外 push），本地领先 7 个提交。
- **暂不做 UI 测试/截图**（用户 2026-11-04 要求）：teammate 只做代码 + 单测 + 门禁，观感由用户实机核验。
- 每项完成后向用户汇报：改动摘要、提交号、exe 路径、门禁结果、已知遗留、需要核验的点。

## 2. 环境与命令

```powershell
$env:PATH='D:\SDK\msys64\ucrt64\bin;D:\SDK\msys64\usr\bin;'+$env:PATH
$env:CARGO_HOME='D:\SDK\cargo-home'
$env:CARGO_TARGET_DIR='D:\SDK\cargo-target'
```

- `D:\SDK\msys64\usr\bin` 提供 `perl`，vendored OpenSSL 构建需要；缺了会重编/失败。
- 门禁：`cargo check -p yshell-app`、`cargo test -p yshell-app`、`cargo fmt --all --check`、`cargo xtask check`、`bash scripts/i18n/check.sh`。
- 本地运行：`D:\SDK\cargo-target\debug\yshell.exe --config-dir <隔离目录>`（**配置不读任何 `YSHELL_*` 环境变量**；语言/外观等写在 `config.toml` 的 `[ui]`）。
- 旧路径 `D:\NewSpace\yshell\target\debug\yshell.exe` 也已替换为新构建；桌面快捷方式 `YShell (dev)` 指向新路径。

## 3. 决策记录（现行）

| # | 决策 | 说明 |
|---|---|---|
| D-1 | MCP 以 status **D34** / 原型为准 | Settings → MCP：**Enable / Port / Listen(Local\|LAN) / API key / View user / Control user / Auto-approve**（不是「stdio 本版砍掉这些字段」）。传输实现细节可仍走现有 MCP 通道，但 **UI 不得按旧 D-1 把 Port 等标成未提供**。 |
| D-2 | 配置不读环境变量 | 删除 `YSHELL_CONFIG_DIR/THEME/ACCENT/LANG/REDUCE_MOTION/UI_FONT/MASTER_PASSWORD/LOCAL_SHELL/FONTS_DIR` 的读取；只读配置文件；隔离用 `--config-dir`；测试用显式注入。保留 `RUST_LOG` 与测试/打包专用变量。 |
| D-3 | N9 同步按键 = 双向组 | 无固定源/目的、无 SRC/RCV、无 All/Visible 预设、**无 Pause**；成员用 banner **On/Off** 进/出组；**End all sync** 解散整组；**AI/MCP 写入前自动把该会话 Off**。banner 已随波 3 #17 落地。 |
| D-4 | 布尔控件统一 | 布尔设置一律 `WinSwitch`（40×20 胶囊、12 滑块、167ms）；真正多选用 `WinCheckBox`（20×20）；已删除旧 `WinToggle`/本地 switch 实现（剩余收口中）。 |
| D-5 | 表单统一 | 唯一表单布局 `ui/components/basic/forms.slint`（`FormRow`/`FormHint`/`FormGroupLabel`/`DetailRow`）：label 120 右对齐 13px、gap 12、控件 ≤420、行高 32 整数倍、行距 10、hint 缩进 132、窗内 padding 28。Settings / Session Editor 已用；管理器收口中。 |
| D-6 | 主密码范围 | 本轮只做 GUI 锁 + verifier + Set/Change/Remove/Reset + 启动解锁；session/key 文件整体加密与 KDF（argon2/AEAD）升级为后续安全债。 |
| D-7 | 不 push | 只本地提交/本地构建；未来的 push/CI 流程暂停，等用户通知恢复。 |
| D-8 | 暂不做 UI 自动化测试 | 只跑代码门禁；截图/鼠标自动化暂停，观感由用户核验。 |

## 4. 事项清单与状态

状态图例：✅ 已完成（代码实现+门禁+本地提交，未 push）· 🔍 已完成、待用户核验 · 🔄 进行中（未提交）· ⬜ 未完成 · ⛔ 本版不做。

### 4.1 已完成（本地提交，未 push）

| # | 事项 | 状态 | 提交 / 证据 |
|---|---|---|---|
| 0 | 基线修复（fmt/clippy/测试漂移、CRLF、过期测试） | ✅ | `fe7f037`、`f530299` |
| 1 | 基础层：config schema + runtime（外观/终端交互/SFTP/图标/备注/登录脚本/文件夹继承/终端输入） | ✅ | `c6d1102`、`edb0243` |
| 2 | Settings 窗口（七分区 + 主密码两窗 + App theme 预览 + Save/Cancel） | ✅ 用户已接受 | `2d26a06` |
| 3 | i18n：catalogs 重建 + zh-CN 全量翻译 + 多次增量 | ✅ 随新文案增量 | `bed84bc`、`d48fe84` 等 |
| 4 | MCP 分区按原型收敛 + Rust 侧残留英文（状态/外部编辑器下拉） | ✅ | `8d442ec` |
| 5 | 窗口行为：拖拽后首击失效、二次拖拽、子窗居中、关闭回焦 | ✅ 真机脚本 0 failure | `eb3533e` |
| 6 | 环境变量配置清理 + `--config-dir` + `[ui] reduce_motion` + 解锁崩溃修复 | ✅ | `936bb10` |
| 7 | N9 同步按键双向组 + AI 自动退组 | 🔍 待用户核验 | `9489104` |
| 8 | Session Editor 窗口（六分区 + Inherit + Login Script 执行 + 子窗 + Save/Save and Connect） | 🔍 待用户核验 | `8f016d7` |
| 9 | 布尔控件统一为 `WinSwitch`/`WinCheckBox` | 🔍 待用户核验 | `7b25790` |
| 10 | 管理器：Known Hosts / Private Keys / Proxies / Terminal Styles + 7 个子窗接线 | 🔍 待用户核验 | `177f634` |
| 11 | 共享表单 + 开关视觉一致性（用户 5 条反馈） | 🔍 待用户核验 | `fa3dece` |
| 12 | 现行完整计划文档化（交付方式/决策/状态/覆盖清单） | ✅ | `bde53dd` |
| 13 | 预览尺寸统一（单一 `PREVIEW_AREA_PX` + 两页一致性回归测试）、tunnel/login-step 子窗改用共享 FormRow/WinSwitch、Session Editor 残留字面量收口 | ✅ | `72c4189` |
| 14 | 管理器表单/详情行全面改用共享 `FormRow`/`DetailRow`/`FormHint`（本地行实现清零），3 条新文案补译 | ✅ | `3d3b37f` |
| 15 | 剩余弹窗 A：sftp-chmod 窗入口/注册/投影、关标签与退出的后果清单、auth-prompt 收口、About 开源许可正文 | ✅ | `ef940f5` |
| 16 | 剩余弹窗 B：About / Logging / SFTP Conflict / SFTP Properties 按原型收口 | ✅ | `6635820` |
| 17 | 波 3 · 主壳：6 个一级菜单（补 Tools）、Session 菜单项、`Ctrl+Shift+S`、`F11`、Full Screen/Focus Mode、状态栏 UTF-8、sync banner | 🔍 待用户核验 | `de3aef8`（含修正 `5f7cbb2`/`034f3d3`/`ae0abba`/`9150b1e`/`a7f72ad`/`64da296`） |
| 18 | 波 3 · 停靠面板：Sessions 图标/原地改名/右键项/New Folder 禁用；Transfers 批量 Pause/Resume All；Tunnels `+`；dock 宽度 | 🔍 待用户核验 | `1125e6a` |
| 19 | 波 3 · 工作区：标签图标/原地改名/AI 标志；Find Across；REC 底条；QC Clear；SFTP 原地 New Folder/Rename + chmod 入口；SSH PTY 视口初尺寸；Don't ask again 勾选布局（**无**分屏叶数硬上限、**无** N9 Pause / pane Receive） | 🔍 待用户核验 | `07ed35d` |
| 20 | 波 4 · i18n 尾部：`No Proxy` sentinel + `@tr`；Discard / Close Session·Tabs 走 TextFormats；删除死路径 Rename Tab 弹窗 | 🔍 待用户核验 | `1ee6b45` |
| 21 | 波 4 · 遗留清理：删旧 `known_hosts` 页与 CLEAR/`RESET SECRETS` 打字 API；确认窗去 token；FocusRing/tooltip；文档与 i18n 同步 | 🔍 待用户核验 | `1ee6b45` |

### 4.2 进行中（未提交）

| # | 事项 | 状态 | 当前进度 |
|---|---|---|---|
| — | （无） | — | — |

### 4.3 未完成（待做）

| # | 事项 | 状态 | 备注 |
|---|---|---|---|
| 22 | 波 5 · 验收：全量门禁 + 用户逐窗核验 + 证据归档 | ⬜ | — |

### 4.4 本版不做（⛔，来自 status §2 与决策）

Quick Commands 内容（仅空态）· 文件夹 New/Rename/Delete/Connect All · Import/Export Config · Settings Shortcuts · Session Editor SFTP/Advanced 分区 · Expand/Collapse All · OS 拖出 / 标签拖出成窗 · 主题导入导出 / 光标形状 · Tunnels 转发运行时（Start/Stop 禁用）· 会话 Proxy 真连 · 主密码整体文件加密 / KDF 升级（后续安全债）。  
（MCP Port/Listen/API key/View/Control：**不做砍项**，见 status D34 与本表 D-1。）

### 4.5 原型覆盖清单（`docs/prototypes/` → 实现状态）

| 原型目录 | 状态 | 说明 |
|---|---|---|
| `settings/`（hub + sections + master-password + mcp-consent） | ✅/🔍 | 已实现，待用户核验观感 |
| `session-editor/`（hub + sections + login-step-*） | ✅/🔍 | 已实现，待核验 |
| `tunnel-editor/` | ✅/🔍 | 已接入 Session Editor / Tunnels 流程，待核验 |
| `known-hosts/`、`private-keys/`、`proxies/`、`terminal-styles/` | ✅/🔍 | 已实现，待核验 |
| `host-key/`（compact 560×400）、`auth-prompt/` | ✅/🔍 | host-key 已 compact；auth-prompt 已去 Remember |
| `logging/`、`about/`、`sftp-conflict/`、`sftp-properties/`、`password-prompt/` | ✅/🔍 | 已按原型收口，待核验 |
| `compact-confirm/`、`close-tab-confirm/` | ✅/🔍 | 已补后果清单/Don't ask again 位置，待核验 |
| `sftp-chmod/` | ✅/🔍 | 窗已注册并接入 SFTP 行右键，待核验 |
| `main-shell/`（index、tool-dock、menu、status、快捷键） | 🔍 | 波 3 #17：菜单/快捷键/全屏/Focus/UTF-8/banner；待核验 |
| `main-shell/panels/`（sessions/transfers/tunnels/commands） | 🔍 | 波 3 #18 已实现，待用户核验 |
| `main-shell/workspace/`（tab-bar/terminal/sftp/quick-connect/split/sync-input） | 🔍 | #19 工作区项已实现；split **无**叶数硬上限；待用户核验 |
| `forms.html`、`annotations.html`、`index.html`、`_form-audit.html` | — | 设计参考页，非应用页面 |

## 5. 剩余顺序（按用户「逐项核验」节奏）

1. **第 2 波已全部完成**：#0–#16 均已本地提交（`fe7f037` … `ee9c807` / `2fbc72d`）。
2. **第 3 波**：#17 / #18 / #19 已本地提交，待用户核验（不含分屏上限 4 / N9 Pause·Receive）。
3. **第 4 波**：#20 / #21 已实现并过门禁，待用户核验；接着 #22 验收。
4. **设计权威**：仅 `docs/prototypes/**` 及其文档为设计稿；交互契约 / archive 仅参考（见根 `AGENTS.md`）。

## 6. 已知未收口 / 风险

| 项 | 说明 |
|---|---|
| Settings 窗口尺寸 | 保持原型 1100×820；label 已从 200 改 120。若用户仍觉得宽，需确认后改为 1000×800（偏离原型 wide 档） |
| Login Wait 子窗 | 现为 standard 720×580，原型是 compact 560×400（待用户决定） |
| MCP failed 状态 | 单测覆盖，未实机造 |
| 残留英文 | 其余 CompactConfirm kind（Quit / Close Window / Host Keys / MCP 等）标题与正文仍由 Rust 英文直投；专有名词除外 |
| e2e | `scripts/e2e/run-ui-sync.sh` 旧同步语义断言默认跳过，组模型 e2e 待移植 |
| 安全债 | 主密码未整体加密 session/key 文件；KDF 未升级 |
| SFTP 权限入口 | 当前入口仅 SFTP 行右键；工具条 `⋯ More` 菜单（原型）尚未实现，随波 3 工作区补 |
| Close Window 确认 | 关标签/退出已用后果清单；“关闭窗口”确认未加后果行（原型无，保留现状） |
| 开源许可资源 | `assets/open_source_licenses.txt` 由 `scripts/licenses/generate_open_source_licenses.py` 按 Windows GNU target 生成（含各 crate 全文）；发布 Linux/macOS 包需按对应 target 重生成 |
| 未实机核验 | Session Editor / 管理器 / 开关 / 表单 / 剩余弹窗观感（按用户要求未测） |

## 7. 每项由用户核验的要点（简表）

- Settings：七分区、开关/表单观感、App theme 即时预览、Save/Cancel、主密码 Set/解锁。
- Session Editor：六分区、label 右对齐、Host/Port 分行、终端预览整行、Login Script 子窗、Tunnel Editor、Save/Save and Connect。
- 管理器：四周管理器清单/右键/工具条、7 个子窗（Add/Import/Export/Generate/Deploy）。
- 剩余弹窗：SFTP 行右键 Change Permissions（compact 560×400，八进制↔勾选）；关标签/退出后果清单与 Don't ask again；About 开源许可展开；Logging；SFTP Conflict/Properties。
- N9 同步：组内广播、非成员不广播、banner On/Off + End all（无 Pause）、AI 输入自动退组。
- 环境：无任何 `YSHELL_*`；语言/外观/减少动画只由 `config.toml` 决定。

## 8. 变更记录（本地提交时间线，未 push）

- 2026-11-03：`fe7f037` 基线修复；`f530299` 差距清单+计划；`c6d1102`/`edb0243` 波 1 基础层；`2d26a06` Settings 窗口；`bed84bc` i18n 全量；`8d442ec` MCP 收敛；`eb3533e` 窗口行为修复（此提交及之前曾被意外 push 到 origin）。
- 2026-11-04：`936bb10` 环境变量配置清理；`9489104` N9 双向同步组；`d48fe84` i18n 再同步；`8f016d7` Session Editor 接线；`7b25790` 开关统一；`177f634` 管理器接线；`fa3dece` 共享表单/视觉一致性。
- 2026-11-05：`bde53dd`/`0c82331` 现行计划与完成状态文档化；`72c4189` 预览尺寸统一 + tunnel/login-step 表单行收口；`3d3b37f` 管理器表单/详情行收口；`ef940f5` sftp-chmod 入口 + 关标签/退出后果清单 + auth-prompt + 开源许可正文；`6635820` About/Logging/SFTP Conflict/Properties 收口；`ee9c807` 弹窗文案 i18n 同步；`2fbc72d` 波 2 完成状态；`de3aef8` 波 3 #17 主壳（Tools / Session / F11 / Focus / UTF-8 / sync banner / dock 260·280）；波 3 #18 停靠面板（Sessions 图标/原地改名/右键/空白 New Folder 禁用；Transfers Pause·Resume All；Tunnels `+` / Start·Stop 禁用；见本地提交）。同日用户裁定：设计稿仅原型；scrub 分屏「最多 4 叶」、N9 Pause/Visible、Favorite 禁用进原型。

---

## 附录：波次记录

### 第 0 波 · 完成（2026-11-03）

- 基线修复：`rustfmt.toml` 固定 CRLF + 当前工具链重排；4 处 clippy；7 个 MCP status 模板；7 条过期测试断言；删除过期 `product_smoke`。
- 差距清单恢复、计划落盘、旧一轮成果归档 `refs/archive/win-ui-opt`。
- 验证：`cargo xtask check` 通过；workspace 测试 27 套件全绿；GUI 启动冒烟通过。

### 第 1 波 · 完成（2026-11-03）

- config：`[ui] appearance_mode/accent`、终端交互四项、`[sftp]` 三项、session icon/notes、login script、folder 级 auth/proxy/sftp/tunnel/login-script 继承、主密码 verifier；sanitize + 测试。
- runtime：App theme 读配置、`terminal_input` 纯逻辑、多行粘贴 Block、自动复制、Settings 终端草稿、SFTP Show hidden 默认。
- 验证：`cargo xtask check` 通过；config 60 / app 402 + 集成全绿。

### 第 2 波 · 完成（2026-11-05，待用户核验）

- 交付事项：#2–#16 —— Settings 七分区 + 主密码两窗、Session Editor 全分区 + Login Script + 子窗、四个管理器 + 7 个子窗、布尔控件统一、共享表单/详情行、终端预览尺寸统一、剩余弹窗（sftp-chmod 入口、关标签/退出后果清单、auth-prompt、About 开源许可、Logging/Conflict/Properties）、i18n 随文案增量。
- 提交范围：`2d26a06` … `2fbc72d`（共 15 个本地提交；其中含用户反馈修复：开关/表单/预览、拖拽/居中回焦、环境变量配置清理、N9 双向同步组）。
- 门禁：`cargo check`、`cargo test -p yshell-app`（462 + 集成 2/3/3）、`cargo fmt --all --check`、`cargo xtask check`、i18n `check.sh` 全绿；**未做 UI 自动化测试**（用户要求）。
- 证据：`D:\NewSpace\ui-evidence\wiring\`（form-consistency / preview-switch-closeout / manager-forms-closeout / dialogs-closeout-a/b 等）与 `wave2\*` 截图/日志。
- 遗留：SFTP 工具条 `⋯ More` 菜单（权限入口暂仅行右键）；开源许可清单按平台 target 生成；Settings 窗口尺寸待用户确认；Login Wait 子窗尺寸待定。

### 第 3 波 · 完成（2026-11-05，待用户核验）

- #17 主壳（待用户核验）：`de3aef8` 主壳菜单/快捷键/F11/Focus/UTF-8/sync banner/dock 260·280；随后修正 `5f7cbb2`、`034f3d3`、`ae0abba`、`9150b1e`、`a7f72ad`、`64da296`。按用户裁定：sync 横幅改为 On/Off 开关 + End all sync，**无 Pause/Visible**，AI 写入前自动 Off；`main-shell` 设计稿已同步。
- #18 停靠面板（待用户核验）：`1125e6a` Sessions 图标/原地改名/右键/空白处 New Folder 禁用；Transfers Pause/Resume All；Tunnels `+`，Start/Stop 保持禁用。
- #19 工作区（待用户核验）：标签图标/原地改名/AI 标志；Find Across；REC 底条；QC Clear；SFTP 原地 New Folder/Rename + chmod；SSH 开壳用视口尺寸（避免 MOTD 顶行进 scrollback）；`WinCheckBox` Don't ask again 布局；原型 scrub（无分屏叶数硬上限 / 无 N9 Pause·Receive）。提交号见 §4.1。
- 用户裁定（2026-11-05）：设计稿只以 `docs/prototypes/` 为准；scrub 分屏「最多 4 叶」、N9 Pause/Visible、Favorite 禁用等愿景条款写入原型/文档，不作为实现必做项。

### 第 4 波 · 完成（2026-11-06，待用户核验）

- #20 i18n 尾部：Session Editor Proxy 投影 `no-proxy` + `TextFormats.proxy-option-label`；CompactConfirm 的 `editor-discard` / `close-tabs` 走 TextFormats（含既有 Close Session 模板）；删除死路径 Rename Tab 弹窗（原地改名保留）。
- #21 遗留清理：删除 `known_hosts` 页/窗与 CLEAR 打字 API；删除死路径 `RESET SECRETS` API（活路径仍是主密码 Reset 破坏性按钮）；CompactConfirm 去掉 token 输入；删除未使用的 `confirm_dialog.slint`；WinTextInput / ManagedKeyPicker FocusRing；标签「＋」与菜单栏停靠开关 tooltip；status / GAPS / catalogs 同步。
- 门禁：`cargo xtask check`、`cargo test -p yshell-app`（463 + 集成 2/3/3）、`bash scripts/i18n/check.sh` 全绿。
- 遗留：其余确认框（Quit / Close Window / Host Keys / MCP 等）标题正文仍为 Rust 英文直投（非本波点名项）。
