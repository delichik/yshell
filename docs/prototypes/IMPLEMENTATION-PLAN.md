# 原型实现计划（`win/ui2`）· 现行版本 2026-11-05

> 目标：以 `docs/prototypes/` 为唯一 UI 真源，把每个页面 / 弹窗 / 交互在 `ui/` + `crates/` 中实现并对齐。
> 差距清单：[`IMPLEMENTATION-GAPS.md`](./IMPLEMENTATION-GAPS.md)（2026-11-03 审计：91 项，54 缺口）。
> 本文件是**现行完整计划**：交付方式、决策、事项状态、剩余顺序、已知问题、变更记录。
> 历史波次记录保留在文末附录。

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
| D-1 | MCP 保持 stdio | Settings → MCP 只做 Enable / Auto-approve / 状态 + stdio 连接片段；Port / Listen / API key / View user / Control user 为禁用占位「本版未提供」。 |
| D-2 | 配置不读环境变量 | 删除 `YSHELL_CONFIG_DIR/THEME/ACCENT/LANG/REDUCE_MOTION/UI_FONT/MASTER_PASSWORD/LOCAL_SHELL/FONTS_DIR` 的读取；只读配置文件；隔离用 `--config-dir`；测试用显式注入。保留 `RUST_LOG` 与测试/打包专用变量。 |
| D-3 | N9 同步按键 = 双向组 | 无固定源/目的；所有开启同步的终端组成一组，任一成员输入广播给组内其它成员；Pause/Stop 保留；**AI/MCP 向某会话写入前自动把该会话移出同步组**。同步横幅按用户要求留到波 3。 |
| D-4 | 布尔控件统一 | 布尔设置一律 `WinSwitch`（40×20 胶囊、12 滑块、167ms）；真正多选用 `WinCheckBox`（20×20）；已删除旧 `WinToggle`/本地 switch 实现（剩余收口中）。 |
| D-5 | 表单统一 | 唯一表单布局 `ui/components/basic/forms.slint`（`FormRow`/`FormHint`/`FormGroupLabel`/`DetailRow`）：label 120 右对齐 13px、gap 12、控件 ≤420、行高 32 整数倍、行距 10、hint 缩进 132、窗内 padding 28。Settings / Session Editor 已用；管理器收口中。 |
| D-6 | 主密码范围 | 本轮只做 GUI 锁 + verifier + Set/Change/Remove/Reset + 启动解锁；session/key 文件整体加密与 KDF（argon2/AEAD）升级为后续安全债。 |
| D-7 | 不 push | 只本地提交/本地构建；未来的 push/CI 流程暂停，等用户通知恢复。 |
| D-8 | 暂不做 UI 自动化测试 | 只跑代码门禁；截图/鼠标自动化暂停，观感由用户核验。 |

## 4. 事项清单与状态

| # | 事项 | 状态 | 提交 / 证据 |
|---|---|---|---|
| 0 | 基线修复（fmt/clippy/测试漂移、CRLF、过期测试） | ✅ 已交付 | `fe7f037`、`f530299` |
| 1 | 基础层：config schema + runtime（外观/终端交互/SFTP/图标/备注/登录脚本/文件夹继承/终端输入） | ✅ 已交付 | `c6d1102`、`edb0243` |
| 2 | Settings 窗口（七分区 + 主密码两窗 + App theme 预览 + Save/Cancel） | ✅ 已交付且用户已接受；后续修复见 #4/#7/#11/#13 | `2d26a06` |
| 3 | i18n：catalogs 重建 + zh-CN 全量翻译 + 多次增量 | ✅ 已交付，随新文案持续增量 | `bed84bc`、`d48fe84` 等 |
| 4 | MCP 分区按原型收敛 + Rust 侧残留英文（状态/外部编辑器下拉） | ✅ 已交付 | `8d442ec` |
| 5 | 窗口行为：拖拽后首击失效、二次拖拽、子窗居中、关闭回焦 | ✅ 已交付；真机脚本 0 failure | `eb3533e` |
| 6 | 环境变量配置清理 + `--config-dir` + `[ui] reduce_motion` + 解锁崩溃修复 | ✅ 已交付 | `936bb10` |
| 7 | N9 同步按键双向组 + AI 自动退组 | ✅ 已交付（待用户核验） | `9489104` |
| 8 | Session Editor 窗口：六分区 + Inherit + Login Script 执行 + 子窗 + Save/Save and Connect | ✅ 已交付（待用户核验） | `8f016d7` |
| 9 | 布尔控件统一为 `WinSwitch`/`WinCheckBox` | ✅ 已交付（待用户核验） | `7b25790` |
| 10 | 管理器：Known Hosts / Private Keys / Proxies / Terminal Styles + 7 个子窗接线 | ✅ 已交付（待用户核验） | `177f634` |
| 11 | 共享表单 + 开关视觉一致性（5 条用户反馈） | ✅ 已交付（待用户核验） | `fa3dece` |
| 12 | 收口：预览位图尺寸统一、tunnel/login-step 本地表单行→共享组件、420 字面量 | 🔄 进行中 | 进行中（teammate） |
| 13 | 收口：管理器/子窗的本地表单行/详情行→共享 `FormRow`/`DetailRow` | 🔄 进行中 | 进行中（teammate） |
| 14 | 剩余弹窗与交互：host-key 紧凑窗确认、auth-prompt 去 Remember、sftp-chmod 入口、关标签/退出后果清单、logging/about/sftp-conflict/properties 收口 | ⬜ 待做 | — |
| 15 | 波 3：主壳菜单（Tools/Session 菜单、Ctrl+Shift+S、F11/Focus Mode）、状态栏、停靠面板（Sessions/Transfers/Tunnels/Commands）、工作区（标签图标/原地改名/分屏上限/Find Across/REC/N9 banner/QC Clear/SFTP 原地操作 + chmod 入口） | ⬜ 待做 | 原型已就绪 |
| 16 | 波 4：i18n 尾部（No Proxy、Discard、Rust 弹窗标题）、遗留清理（旧 known_hosts 页、`CLEAR` 打字确认、focus ring/tooltip 覆盖）、文档同步（status/契约） | ⬜ 待做 | — |
| 17 | 波 5：全量验收（四道门禁 + 用户逐窗核验 + 证据归档） | ⬜ 待做 | — |

## 5. 剩余顺序（按用户「逐项核验」节奏）

1. 当前收口 #12/#13 完成后：Lead 门禁 + 本地提交 + 重建 exe → 交用户核验 #7–#13。
2. 用户确认后：做 #14 剩余弹窗（逐窗一项：host-key、auth-prompt、sftp-chmod 入口、close-tab/quit 后果、logging/about/sftp-conflict/properties）。
3. 再做 #15 波 3（主壳 → 面板 → 工作区，按热点文件串行接线；sync banner 在此项实现）。
4. 最后 #16 清理 + #17 验收。

## 6. 已知未收口 / 风险

| 项 | 说明 |
|---|---|
| 预览位图尺寸 | Settings `(420,198)` vs Session Editor `(480,98)` 的 Rust 常量（#12 收口中） |
| 旧本地 switch/表单实现 | `tunnel_editor_window` 的 `SwitchToggle`（#12）、管理器各类 `*FormRow`/`DetailRow`（#13） |
| Settings 窗口尺寸 | 保持原型 1100×820；label 已从 200 改 120。若用户仍觉得宽，需确认后改为 1000×800（偏离原型 wide 档） |
| Login Wait 子窗 | 现为 standard 720×580，原型是 compact 560×400（待用户决定） |
| MCP failed 状态 | 单测覆盖，未实机造 |
| 残留英文 | `No Proxy`（Rust 投影）、`Discard`（全局硬编码）、部分 Rust 弹窗标题（`Close Session`/`Rename Tab`）、专有名词 |
| e2e | `scripts/e2e/run-ui-sync.sh` 旧同步语义断言默认跳过，组模型 e2e 待移植 |
| 遗留代码 | 旧 `ui/pages/known_hosts.slint`、`CLEAR` 打字确认、focus ring/tooltip 未全覆盖 |
| 安全债 | 主密码未整体加密 session/key 文件；KDF 未升级 |
| 未实机核验 | Session Editor / 管理器 / 开关 / 表单观感（按用户要求未测） |

## 7. 每项由用户核验的要点（简表）

- Settings：七分区、开关/表单观感、App theme 即时预览、Save/Cancel、主密码 Set/解锁。
- Session Editor：六分区、label 右对齐、Host/Port 分行、终端预览整行、Login Script 子窗、Tunnel Editor、Save/Save and Connect。
- 管理器：四周管理器清单/右键/工具条、7 个子窗（Add/Import/Export/Generate/Deploy）。
- N9 同步：组内广播、非成员不广播、Pause/Stop、AI 输入自动退组。
- 环境：无任何 `YSHELL_*`；语言/外观/减少动画只由 `config.toml` 决定。

## 8. 变更记录（本地提交时间线，未 push）

- 2026-11-03：`fe7f037` 基线修复；`f530299` 差距清单+计划；`c6d1102`/`edb0243` 波 1 基础层；`2d26a06` Settings 窗口；`bed84bc` i18n 全量；`8d442ec` MCP 收敛；`eb3533e` 窗口行为修复（此提交及之前曾被意外 push 到 origin）。
- 2026-11-04：`936bb10` 环境变量配置清理；`9489104` N9 双向同步组；`d48fe84` i18n 再同步；`8f016d7` Session Editor 接线；`7b25790` 开关统一；`177f634` 管理器接线；`fa3dece` 共享表单/视觉一致性。
- 2026-11-05：收口 #12/#13（进行中，待提交）。

---

## 附录：历史波次记录（2026-11-03）

### 第 0 波 · 完成

- 基线修复：`rustfmt.toml` 固定 CRLF + 当前工具链重排；4 处 clippy；7 个 MCP status 模板；7 条过期测试断言；删除过期 `product_smoke`。
- 差距清单恢复、计划落盘、旧一轮成果归档 `refs/archive/win-ui-opt`。
- 验证：`cargo xtask check` 通过；workspace 测试 27 套件全绿；GUI 启动冒烟通过。

### 第 1 波 · 完成

- config：`[ui] appearance_mode/accent`、终端交互四项、`[sftp]` 三项、session icon/notes、login script、folder 级 auth/proxy/sftp/tunnel/login-script 继承、主密码 verifier；sanitize + 测试。
- runtime：App theme 读配置、`terminal_input` 纯逻辑、多行粘贴 Block、自动复制、Settings 终端草稿、SFTP Show hidden 默认。
- 验证：`cargo xtask check` 通过；config 60 / app 402 + 集成全绿。
