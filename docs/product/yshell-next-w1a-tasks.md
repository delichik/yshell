# W1a 任务包（E0 / D0 / S1+S2 / L3b）

状态：**设计已确认（2026-09-27）**，四个任务同时启动，每个任务一个唯一 owner。
执行状态（2026-09-27）：**E0 ✅ / D0 ✅（含 D0-F）/ L3b ✅ / S1+S2 ✅（验收通过）**。
设计/验收：主 agent。执行：各任务唯一 owner。
通用约束：仓库在 `/root/yshell`（不要在旧 `D:` 副本上工作）；构建统一：

```bash
export PATH="$HOME/.cargo/bin:$PATH"          # 非登录 shell 会拿到 rustc 1.85，必须显式
export CARGO_TARGET_DIR=/root/yshell-target   # 共享 target，禁止自建
cargo <check/test/clippy> ... --locked
```

- 并行期间 `Cargo.lock` 可能被 D0 更新；其它任务遇到 lock 竞争/不一致，**重试一次**，不要手改 lock。
- 禁止 `cargo fmt --all`；不得夹带范围外改动；接口如需调整先停下报告。

---

## E0 构建环境收口

权威设计：`docs/product/yshell-next-e0-build-env.md`（照其范围与验收执行）。
范围：`scripts/wsl-env.sh`（新增）、`scripts/README.md`、`README.md`（第 105 行旧路径）、清理 `/root/yt-*`（保留 `yt-n5a-evidence` 与 `yshell-target`）。
不做：代码逻辑、CI、`Cargo.toml`/`Cargo.lock`。
验收（主 agent 复核）：非登录 shell `source scripts/wsl-env.sh && cargo xtask lint` 通过；仓内 `/mnt/d/NewSpace` 零命中；清理前后磁盘对比。

## D0 依赖入库（`time` + `swash`）

范围：根 `Cargo.toml` 的 `[workspace.dependencies]` 增加：
- `time = { version = "0.3", features = ["local-offset", "formatting", "macros"] }`（G0-tz 用）
- `swash = "0.2.10"`（N5a UI 阶段替换 fontdue 光栅用）
更新 `Cargo.lock` 并验证。**不新增使用方、不改其它 crate**。
- `rfd` **本次不入库**：其 Linux 后端（GTK3 vs xdg-portal）特性会影响全仓构建，选型并入 N1 设计先行 spike，再统一入库。
验收：`cargo metadata --locked` 通过；`cargo check --workspace --all-features --locked` 通过（依赖未被使用，应无新编译对象以外的变化）；`cargo xtask deny`（若可用）无新增许可问题；报告给出 lock 变更摘要。

## S1+S2 会话栏收敛 + 后端默认 native / 去 UI 入口

范围：`ui/main_window.slint`、`crates/yshell-app/src/{bootstrap.rs,runtime.rs}`（以及必要的 `crates/yshell-app/tests`）。

**S1 会话栏（§14.1）**
- 删除 `RECENT`、`ACTIVE` 区块；删除标题栏 `+`、列表下方 `…`、窄窗口图标条 `+`（保留会话树主体与 `SAVED` 标题）。
- 新增会话树**空白区右键菜单**：`New Session`（打开 Session Editor）、`New Folder…`（弹窗输入名称，在根目录创建）、`Refresh`（重读配置/刷新会话树）；`Import Config...`/`Expand All`/`Collapse All` 显示为 disabled + tooltip（本版不实现）。
- 新增 runtime 命令：根目录建文件夹（参考现有 `create_folder_under_editor_target` 实现）、`refresh_saved_sessions()`（重新 `load` 配置并投影）。
- 图标条模式：右键弹出同一菜单的最小版（`New Session` / 展开会话栏）。
- 保留其它入口：CommandBar `New Session`、File 菜单、Ctrl+N、标签条 `+`。

**S2 后端（§14.2）**
- `resolve_startup_ssh_backend`：无环境变量时默认 **native SSH（Real）**；`YSHELL_SSH_BACKEND=fake|native-ssh` 仍可覆盖（e2e 用）。
- View 菜单删除 `Fake Backend` / `Native SSH Backend` 两项；状态栏删除后端色点与 `Backend: {0}` 文案；`transport_backend_text` 不再投影到 UI。
- 删除 `.slint` 侧 `select_fake_backend` / `select_native_ssh_backend` 回调及 bootstrap 接线；runtime 方法保留给测试。
- 更新受影响的单测/断言；`--locked` 全绿。

验收（主 agent 复核）：
1. `cargo xtask lint`、`cargo xtask test` 全绿；
2. 会话栏无 `+`/`…` 按钮；空白区右键能新建会话、新建文件夹（树中立即可见）、刷新；
3. 菜单/状态栏无后端字样；无环境变量启动走 real（终端出现真实登录横幅而非 fake-shell）；`YSHELL_SSH_BACKEND=fake` 仍可用；
4. 截图证据：会话栏、空白区菜单、View 菜单、状态栏（dark/zh-CN 一份即可）。

## L3b 浅色 text-tertiary 修正

范围：`ui/theme.slint` 一处——浅色 `text-tertiary` 的 alpha `0x72` → `0x8B`（`#0000008B`）。
不做：其它 token、CI 接线、dark 侧。
验收：`cargo xtask contrast` 浅色 **57/57**、深色 57/57；差异截图抽查（设置页/会话树 tertiary 文本仍可分辨且不刺眼）；`cargo test -p xtask --locked` 全绿。

---

## 依赖与并行

- 四个任务文件不重叠：E0（docs/scripts）、D0（根 Cargo.toml/Cargo.lock）、S1+S2（app/UI）、L3b（theme.slint）。
- D0 与其它任务的唯一接触面是 `Cargo.lock`：D0 尽快完成；其余任务 `--locked` 重试即可。
- 完成后主 agent 逐项验收；返工回原 owner。
