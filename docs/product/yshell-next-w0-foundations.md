# YShell W0 地基任务包（并行设计）

状态：**待用户确认**（确认后每个任务分配唯一 owner 立即执行）
设计/验收：主 agent
执行：各任务唯一 owner subagent
目的：在 T0（Slint 升级）执行期间，把**不依赖 UI 热文件、不新增第三方依赖、不写 `Cargo.lock`** 的 crates 级工作并行推进，为 N0/N1/N2/N4/N5/N6 铺路。

## 通用约束（所有任务）

- **不碰**：`ui/**`、`crates/yshell-app/**`（含 bootstrap/runtime/session_runtime——这些是 T0/N0/S1/S2 的热文件）。
- **不改**：根 `Cargo.toml`、`Cargo.lock`、`deny.toml`、`.github/**`。
- 并行期间 cargo 命令一律加 `--locked`（避免与 T0 的 lock 更新互相覆盖）；开发期用 `cargo check -p <crate> --locked`，交付前跑一次 `cargo test -p <crate> --all-features --locked` + `cargo clippy -p <crate> --all-targets --locked -- -D warnings`。
- 若与其它 owner 同时构建，使用独立 `CARGO_TARGET_DIR`（例如 `$HOME/yt-<task>`）。
- 接口以本文档为准；实现中确需调整接口 → 停下报告主 agent，不得自行扩大范围。
- 交付物：代码 + 单测 + 交付报告（变更摘要/命令证据/偏离记录）。

---

## C0 配置地基（crates/yshell-config）

**目标**：为 N0/N2/N3/N4/N5 提供 schema 与解析，不写 UI/运行时。

**范围与接口**

1. 新增 `[ui]` 节：
   - `new_tab_mode: "quick-connect" | "session-editor"`（默认 `quick-connect`）。
   - `layout`：`left: Vec<PanelSlot { panel, collapsed }>`、`right: Vec<PanelSlot>`、`left_ratios: Vec<f32>`、`right_ratios: Vec<f32>`、`left_width: u32`、`right_width: u32`、`narrow_collapsed_side: Option<"left"|"right">`。
   - `multi_window_enabled: bool`（默认 true）。
   - `PanelId` 枚举放本 crate（`sessions|sftp|tunnels|quick_commands|transfers`，snake_case 序列化），供后续 runtime/UI 复用。
2. 快速连接：
   - `quick_connect: { enabled: bool = true, limit: usize = 20, history: Vec<QuickConnectEntry { target, last_used_at, use_count }> }`。
   - `quick_links: Vec<QuickLink { id, label, target, sort_order }>`。
   - `target` 一律存规范化字符串（复用 `QuickConnectTarget` 解析校验，非法项读取时跳过并告警）。
3. 文件夹继承：`FolderProfile` 新增 `terminal: Option<TerminalProfile>`、`logging: Option<LoggingProfile>`、`appearance: Option<AppearanceProfile>`；扩展 `ResolvedSessionProfile` 解析为 **会话 > 最近祖先文件夹 > … > 根文件夹 > 全局 > 内置默认**（逐字段继承）。
4. 终端主题：`TerminalProfile` 增加可选字段：`color_scheme: Option<String>`、`foreground/background/cursor/selection: Option<String>`（`#RRGGBB`）、`ansi: Option<[String; 16]>`、`font_family: Option<String>`、`font_size: Option<u16>`、`fallback_fonts: Option<Vec<String>>`；保留 scrollback 字段。
5. 密钥：新增 `keys: Vec<KeyProfile { id, label, algorithm, fingerprint, public_key, secret_ref, comment }>`（私钥材料/口令只存 secret store）；`AuthMethod::PrivateKey` 增加 `Option<key_id>`（兼容旧 `path`，解析优先 key_id）。
6. 迁移：`schema_version` 升级；旧文件可读；缺省字段用默认值；提供 `migrate` 单测。

**不做**：UI、runtime、密钥文件读写、rfd。

**验收**：round-trip 序列化；继承解析（含三级嵌套、缺字段回退、非法值）；旧 schema 迁移；`--locked` 下单测/clippy 全绿。

---

## F0 SFTP 后端能力（crates/yshell-sftp）

**目标**：为 N1 提供递归、批量与传输控制能力，不接 UI。

**范围与接口**

1. 单文件接口保持不变；新增 `remove_dir(path)`（仅空目录，递归删除由 App 层编排或显式 `recursive` 参数）。
2. 目录传输：
   - `upload_tree(local_root, remote_root, options)` / `download_tree(remote_root, local_root, options)`。
   - `options: TreeTransferOptions { overwrite: Ask|Overwrite|Rename|Skip, follow_symlinks: bool }` + 进度回调 `FnMut(TransferProgress { path, kind: File|Dir|Symlink, bytes_done, bytes_total, direction })` + 取消信号（`AtomicBool`）。
   - 单项失败不中断（默认 `ContinueOnError`），最终返回汇总 `TreeTransferReport { completed, skipped, failed: Vec<(path, reason)> }`。
3. 批量：`delete_many`、`chmod_many`：逐项结果返回，不因单项失败中断。
4. `TransferQueue` 状态机补：`pause/resume/remove/clear_completed`、失败重试上限与"重试后可再失败"语义；纯模型 + 单测。
5. Fake 后端补齐上述能力，行为与 Real 对齐（供 UI 与单测使用）。

**不做**：本地文件选择器、UI、rfd、远程→远程中转（属 N1 编排）。

**验收**：树传输覆盖 成功/覆盖策略/取消/部分失败继续；批量部分失败语义；fake/real trait 对齐（编译期 + 行为测试）；`--locked` 全绿。

---

## A0 SSH 认证层（crates/yshell-ssh）

**目标**：为 N4 提供"按服务端能力认证"的底层接口，UI/runtime 后置。

**范围与接口**

1. 握手后查询 `Session::auth_methods(username)` → `AuthMethods { password: bool, publickey: bool, keyboard_interactive: bool, agent: bool }`；服务端未返回时 `None`；加入连接结果（成功/失败均带回，供 runtime 弹认证窗）。
2. Keyboard-interactive 通用化：
   - `KeyboardInteractiveChallenge { name, instruction, prompts: Vec<Prompt { text, echo }> }`。
   - prompter trait 支持多轮（libssh2 回调可多次触发）与取消；保留现有"单 secret 填充"为默认实现之一。
3. 认证尝试：`authenticate_with(attempt: AuthAttempt)`，`AuthAttempt { method: Password{..}|PublicKey{..}|Agent|KeyboardInteractive{responses}, username }`；返回细化错误（凭据错误 / 方法不允许 / 需要其它方式），供弹窗内换方式重试。
4. Host key 校验与现有默认连接路径行为不变（向后兼容）。

**不做**：密钥文件解析/生成、UI、runtime 状态机、secret store。

**验收**：Fake 传输后端可模拟 `auth_methods` 与多轮 prompt；错误分类单测；现有连接相关测试不回归；`--locked` 全绿。

---

## G0 日志核心（crates/yshell-logging）

**目标**：为 N6 提供"运行中开启到指定文件"的日志核心。

**范围与接口**

1. 新增 `SessionLogger::open_with(SessionLogOptions { path, format: Raw|Sanitized, timestamps: bool, mode: Append|Truncate })`。
2. 时间戳（默认关）：行首 `[HH:MM:SS]`；按字节流扫描行边界，只在行首插入，不破坏 ANSI 序列（sanitized 模式在转义序列剥离后加）。
3. 生命周期：`flush()`、`close(self) -> io::Result<()>`；`Drop` 兜底 flush；`last_error()` 暴露最近写失败。
4. 兼容：保留现有 `open(path, format, redactor, rotation)` 签名（内部委托新实现），现有调用方零改动。
5. 只读路径/磁盘写失败注入测试。

**不做**：UI、runtime 接线、路径选择、轮转策略改动。

**验收**：时间戳/追加/截断/失败注入单测；raw 模式字节保真；既有测试不回归；`--locked` 全绿。

---

## L3 对比度自动检查（xtask/scripts，复用现有依赖）

**目标**：设计语言 §8/§9 与验收 #6 的自动检查。

**范围与接口**

1. 解析 `ui/theme.slint` 的浅/深两套 token：`text-primary/secondary/tertiary` × `window-background/layer-fill/card-fill`、accent 文字组合、`success/critical` 等状态色。
2. 按 WCAG 相对亮度计算对比度；正文 ≥4.5:1、大字号/图标 ≥3:1；输出失败组合明细（token 名、颜色值、比值）+ 非零退出码。
3. 入口二选一（实现时定，倾向）：`cargo xtask contrast`（Rust，复用 xtask 依赖）或 `scripts/contrast-check.*`；不新增第三方依赖。
4. 可对自己的临时拷贝做"越界能被抓到"的自测；**不修主题色**（发现问题报回主 agent 另立任务）。

**不做**：CI 接线（先本地入口，验收后再定）。

**验收**：一键运行；输出可读；对故意越界样例能报失败；`--locked` 全绿。

---

## 依赖入库任务 D0（T0 完成后执行，单列）

各任务按设计**不引入新依赖**；等 T0 交付后，由 **D0 owner** 一次性把后续任务需要的新依赖加入 `[workspace.dependencies]` + `Cargo.lock`（预计：`rfd`，以及 N5a 光栅选型确定后的 `swash` 或等价物、密钥解析的 `ssh-key`），避免多任务并行修改根 `Cargo.toml`/lock。

## 资源与节奏提示

- 5 个 owner 同时构建会占满 CPU/内存；建议开发期频繁 `cargo check -p`、交付前各自跑一次完整 `-p` 测试，`CARGO_TARGET_DIR` 分开。
- 与 T0 唯一的共享面是 `Cargo.lock`：一律 `--locked`；若遇到 lock 竞争，重试一次即可，不要手动改 lock。
- 主 agent 按任务逐项验收；返工回原 owner。

---

## N5a-W0 终端字形质量（renderer 部分，提前开工）

背景：`yshell-font-clarity-findings.md`（调研验收通过）确认终端字形问题：fontdue 无 hinting 导致竖笔拆列发灰、CJK 呈 .notdef 灰框、粗体只提亮、斜体不生效、1x 光栅被 Slint 缩放（B5）。UI 接线（bootstrap/terminal_view）与最终验收在 **T0 之后**；本阶段只做 `crates/yshell-terminal` 内可独立交付的部分。

**本阶段范围（W0）**

1. 渲染器接口（默认行为不变）：
   - `TerminalRenderer` 支持 `scale_factor: f32`（默认 1.0）与逻辑 `font_size`；位图与 cell 尺寸按物理像素计算；`scale_factor=1.0` 且非粗体/斜体的输出与现状**逐像素一致**（快照回归）。
   - 字形缓存键从 `char` 改为 `(char, bold, italic)`。
2. 真粗体/斜体：
   - 新增内置资产 `DejaVuSansMono-Bold.ttf`、`DejaVuSansMono-Oblique.ttf`（系统 `/usr/share/fonts/truetype/dejavu/` 可取，许可自由；各约 300KB）。
   - `cell.bold`/`cell.italic` 使用对应字面；字面缺失时回退现有"提亮"策略并在代码注释/报告记录。粗体/斜体为**有意变更**（附 before/after 像素证据）。
3. 字体回退链机制：
   - `TerminalFontStack { primary, fallbacks: Vec<FontFace> }`，逐字形选择首个 `lookup_glyph_index != 0` 的字体。
   - 为可测试性：字形选择抽象为可注入的 `GlyphSource`（或用 trait），单测覆盖"主字体缺字形 → 回退命中"；真实 CJK 资产在后续阶段补。
   - 默认栈暂为 DejaVu 常规/粗/斜；CJK 回退资产策略（subset 体积/许可/生成工具）由本任务调研并给建议，依赖引入走 D0。
4. 光栅质量 spike（**不新增依赖**）：
   - 独立探针比较 fontdue（现状）与候选（swash / zeno / 2x 超采样降采样）的竖笔覆盖率分布与 ≥90% 覆盖墨点占比，给选型建议 + 数据证据；结论写报告，真正引入依赖走 D0。
5. e2e 缩放修复（不碰 UI 热文件）：
   - `scripts/e2e/run-ui-ssh.sh`（及 `lib.sh`）固定 Xvfb 几何 / `WINIT_X11_SCALE_FACTOR=1`，确保 winit 缩放 = 1.0；运行一次验证。
6. 明确**不做**：bootstrap/terminal_view/UI 接线、CJK 资产落地、D0 依赖引入、Slint 侧任何文件。

**验收（本阶段）**

- `cargo test -p yshell-terminal --all-features --locked` + `cargo clippy -p yshell-terminal --all-targets --locked -- -D warnings` 全绿。
- 回归：`scale_factor=1.0`、非粗斜体的渲染帧与现状逐像素一致（快照）。
- 粗体/斜体：`\033[1m` / `\033[3m` 使用对应字面（像素证据）。
- 回退：可注入 `GlyphSource` 的单测覆盖；真实资产路径留待 CJK 阶段。
- spike 报告：候选方案对比数据 + 推荐 + 风险。
- e2e：缩放固定为 1.0（输出证据）。

**后续（T0 后，仍由同一 owner）**

- UI 接线：`bootstrap.rs` 传 `scale_factor`、`terminal_view.slint` 1:1 显示；CJK 资产与 subset 工具；D0 引入光栅依赖；完整 N5a 验收（scale=1.25/2.0 无 12/13px 交替、中文可读、粗体可见）。
