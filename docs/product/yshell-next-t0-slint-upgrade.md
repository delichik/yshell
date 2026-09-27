# T0 · Slint 升级设计（1.9.2 → 1.18.1）

任务 ID：T0
状态：**已完成（验收通过 2026-09-27）**
设计人：主 agent（设计/验收）
验收人：主 agent
执行：唯一 owner subagent `ses_f1e83b6b0ffexQs70KHJUkHfz7`
依赖：无（平台前置；N0 依赖本任务完成）
验收结论：lint 通过；371 测试全绿；`dist` 产物正常；e2e 双主题 34/34；UI 字重对比通过（Thin 消除）
关联：`yshell-next-features-requirements.md` §12 D1、§11 横切要求

---

## 1. 目标 / 非目标

**目标**

- 把 Slint 从 `=1.9.2` 升级到最新稳定 `=1.18.1`（`slint` + `slint-build` 同步）。
- MSRV 从 1.89 提升到 **1.92**（Slint 1.17+ 要求）。
- 完成所有必要的 API/语言迁移，保持现有功能与视觉不回归。
- 升级后具备 1.17+ 的 `DragArea`/`DropArea` 能力，供后续 N1/N3/N7 使用。

**非目标**

- 不新增任何功能（DnD 落地属于 N1/N3/N7，不在本任务）。
- 不做视觉重设计；Fluent 控件的新版本细节差异只做记录。
- 不处理 fmt 债（L4 单独一轮）。
- 不改翻译文案、不改交互。

---

## 2. 现状事实（核对证据）

| 项 | 现状 |
|---|---|
| 版本锁定 | `crates/yshell-app/Cargo.toml`：`slint = { version = "=1.9.2", default-features = false, features = ["std","compat-1-2","backend-winit-x11","renderer-software"] }`；`slint-build = "=1.9.2"` |
| 编译入口 | `build.rs`：`CompilerConfiguration::new().with_style("fluent").with_bundled_translations("../../translations")`，编译 `ui/main_window.slint` |
| MSRV | workspace `Cargo.toml`：`rust-version = "1.89"`；`README.md` 写作说明提到 MSRV 1.89 |
| CI 工具链 | `.github/actions/setup-build-env` 用 `dtolnay/rust-toolchain@stable`（CI 已是新 stable）；本地需要 rustup ≥1.92 |
| 私有 API | `bootstrap.rs`：`slint::private_unstable_api::re_exports::ColorScheme`，用于显式浅/深色时设置 `Palette.color-scheme`（1.9 无公开类型） |
| 键处理 | `main_window.slint` 注释明确"Slint 1.9 的 KeyEvent 没有 key，只能按 event.text 的键码比较"（F5 `\u{F708}`）；`session_tree.slint` 回车、`terminal_view.slint` 输入同样用 `event.text` |
| Flickable 属性 | `settings.slint` / `host_key_dialog.slint` / `session_tree.slint` 使用 `viewport-width/height`（1.18 已改名 `content-*`，旧名为 deprecated alias） |
| 其他 Slint API | `slint::select_bundled_translation`、`slint::quit_event_loop`、`slint::Image::from_rgba8`、`SharedPixelBuffer::clone_from_slice`、`Timer`、`FocusScope`、`PointerEventKind`（1.17 起公开） |

---

## 3. 变更内容

1. `crates/yshell-app/Cargo.toml`
   - `slint = "=1.18.1"`、`slint-build = "=1.18.1"`。
   - features 保持语义不变；若 1.18 的 feature 名称有拆分（如 winit 后端细分 x11/wayland），按官方 feature 表等价替换（X11 必须保留），在交付说明里记录。
2. workspace `Cargo.toml`：`rust-version = "1.92"`。
3. `Cargo.lock` 更新（保持 `--locked` 可用）。
4. `README.md`：MSRV 1.89 → 1.92 文案同步；如有其它版本说明一并更新。
5. Rust/Slint 侧必要迁移（见 §4 热点表）。

---

## 4. 迁移热点与处理动作

| # | 热点 | 现状 | 1.18 影响 | 处理动作 |
|---|---|---|---|---|
| H1 | `private_unstable_api::re_exports::ColorScheme`（bootstrap） | 私有 API 设置 `Palette.color-scheme` | 私有 API 跨版本不保证；1.15 起有公开 `slint::language::ColorScheme`（`slint::language::ColorScheme`） | 迁移到公开 API；若 `Palette` 的 Rust setter 在新版不可用，按 §7 开放问题 Q2 决策 |
| H2 | `event.text` 当键码（F5、Enter） | 1.9 无 `KeyEvent.key` | **实施核实（1.18.1）：`.slint` 侧 `KeyEvent` 仍只有 `text/modifiers/repeat`，公开 `Key` 枚举仅存在于 Rust 侧；"迁移到 `event.key`"在 1.18.1 不可行** | 保留 `event.text` 方案（仅更新注释）；已回归 F5 重连、回车激活、可打印字符、Ctrl+F，功能正常（设计前提修正，对照 §7 Q1） |
| H3 | Flickable `viewport-width/height` | 1.9 属性名 | 1.18 改名 `content-*`（deprecated alias 保留） | 迁移到 `content-width/content-height`，保持行为一致 |
| H4 | `compat-1-2` + `Palette` re-export | main_window/theme 依赖（`Theme.mode=system` 读 `Palette.color-scheme`） | compat 层行为需验证 | 保留 compat；回归"跟随系统/显式深浅"三态切换 |
| H5 | Fluent 控件视觉 | build.rs pin `fluent` | 9 个版本内的样式修复/细节变化 | 升级前后截图对比，只接受"无布局破坏的细节差异"，逐项记录 |
| H6 | 系统默认字体/字号 | 1.18 起从系统读取应用默认字号 | 窗口已显式 `default-font-size`/`default-font-family`，理论上被覆盖 | 深浅截图核对字号与行高；std-widgets 内部文本抽查 |
| H7 | 软件渲染器改进 | 跳过不透明覆盖、字形间距修复等 | 观感/性能变化 | 截图对比；终端位图来自自绘 `TerminalRenderer`，应不受影响；记录 CPU 观感 |
| H8 | deprecated `Window.x/y`、`try_dispatch_event` | 未使用 | — | 构建时确认无 deprecated 警告 |
| H9 | 翻译管线 | `with_bundled_translations` + polib | 可能行为差异 | 跑翻译测试与中英切换；`.po` 不改内容 |

---

## 5. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| H1 无公开替代 | 升级卡住 | 允许"暂时保留私有 API"作为过渡（见 Q2），但必须验证三态切换正确并记录技术债 |
| 迁移面大导致 UI 回归 | 观感/交互问题 | 升级与功能分离；用 e2e 两条流程 + 全量截图对比 + 手动抽查清单 |
| 本地工具链低于 1.92 | 无法构建 | owner 交付前先 `rustc --version` 确认；README 写明要求 |
| feature 拆分导致 Linux 构建变化 | WSLg/CI 构建失败 | 按官方 feature 表等价替换；`cargo xtask dist` 在 Linux 验证 |
| `Cargo.lock`/`--locked` 不同步 | CI fmt job `cargo metadata --locked` 失败 | 提交前跑 `cargo metadata --locked` 与 `cargo xtask lint` 验证 |
| 与并行任务冲突 | diff 混乱 | T0 期间 `main_window.slint` 归 T0 owner 单写；其它任务冻结 UI 文件 |

---

## 6. 验收标准（主 agent 执行）

**硬门槛**

1. `cargo tree -i slint` 仅 1.18.1；`cargo metadata --locked` 通过。
2. `cargo xtask lint`（clippy `-D warnings`，`--locked`）通过。
3. `cargo xtask test` 全绿；owner 提供**升级前/后测试数量**对照，数量不得下降。
4. `cargo xtask dist` 成功；产物 `--version` 正常。
5. E2E：`bash scripts/e2e/run-ui-ssh.sh`（dark/zh-CN）与 `--theme light --lang en-US` 全过，截图归档到 `dist/ui-checks/`。
6. 无功能夹带：`git diff --stat` 审查，只含版本/迁移/文档；迁移范围超出 §4 时需在设计文档记录并说明理由。

**回归抽查（截图或命令证据）**

7. 主窗口 1440/1120/880 三档宽度，无重叠/截断；菜单栏与右键菜单可正常开合。
8. 终端：键入、回滚滚动、选择复制、查找（next/prev）、F5 重连、Enter 激活会话树节点。
9. SFTP：路径刷新、排序、右键菜单、上传/下载弹窗（fake/native 皆可）。
10. 弹窗：Host key、密码、Settings、Session Editor、Known Hosts、About 均可打开/关闭。
11. 主题三态：`YSHELL_THEME=system|light|dark` 观感正确（`Palette` 同步生效）。
12. 中英切换（`select_bundled_translation`）即时生效、无截断。
13. 升级前后截图对比清单（至少：主窗口深/浅、终端、SFTP、会话编辑器），差异逐项说明（接受/不接受）。
14. **UI 字重/字形回归（D28）**：对比升级前后 UI 文本墨迹统计（基线见 `yshell-font-clarity-findings.md` §4："会话" ink 186px / mean 120），确认 Thin 变量字体问题消除、正文笔画与字距恢复正常。

**交付物**

- 变更 + `Cargo.lock` + README/MSRV 更新。
- 证据包：命令输出摘要、e2e 截图、前后对比说明、偏离记录（H1/H2/H3 的实际处理方式）。
- 提交信息：`upgrade Slint 1.9.2 -> 1.18.1 (MSRV 1.92)`；单一提交（或纯升级拆分提交）。

---

## 7. 设计冻结确认（2026-09-27）

| # | 问题 | 用户结论 |
|---|---|---|
| Q1 | 键处理迁移（`event.text` → `KeyEvent.key`）？ | **尽量使用新版本建议的模式**（不只保证编译） |
| Q2 | 若 1.18 没有 `Palette.color-scheme` 的公开 Rust setter？ | 允许"暂时保留私有 API + 记录技术债"，不阻塞升级 |
| Q3 | Fluent 非破坏性视觉差异？ | 不要求像素级一致，但**尽量追求一致**；布局破坏不允许 |
| Q4 | 验收主场景？ | WSLg 为主（e2e + 截图），Windows/macOS 由 CI 矩阵覆盖 |

---

## 8. 执行与返工规则

- 确认后分配**唯一 owner subagent**；本任务所有实现/返工都由该 owner 完成，主 agent 不改代码。
- owner 交付时必须附带 §6 证据；主 agent 验收，失败项以"复现步骤 + 期望/实际"形式交回同一 owner。
- 升级期间 `ui/main_window.slint` 归 T0 owner 单写；其它任务不得并行修改 UI 文件。
- 完成后本任务状态记入任务台账，N0 方可开工。
