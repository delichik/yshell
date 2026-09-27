# R2 `runtime.rs` 模块化拆分（设计）

状态：**已完成（验收通过 2026-09-28）**；owner `ses_f1c92bbdfffeE6a15zYTD5t9Gr`
目标：`crates/yshell-app/src/runtime.rs`（≈390KB / 单文件）机械拆分为 `crate::runtime` 模块目录，**行为零变化**；解锁"按模块并行"。
设计/验收：主 agent；执行：唯一 owner

## 1. 方法（纯搬迁）

- `runtime.rs` → `runtime/mod.rs` + 子模块；函数体不改（仅可见性与 `use` 路径调整）。
- 对外 API 不变：`crate::runtime::AppRuntime` / `AppProjection` / 各类型导入路径保持不变（通过 `pub use` 保持）。
- 字段可见性：结构体私有字段改 `pub(crate)`（或 `pub(super)`），保证子模块可访问。
- 每搬一个模块：`cargo check -p yshell-app --locked` + `cargo test -p yshell-app --locked` 通过后再搬下一个。
- 不做：任何逻辑/行为/文案/测试断言改动；不顺手优化；tests 就近放到各模块（超大测试文件可 `runtime/tests/` 分文件）。

## 2. 建议模块边界（首版；实现时可微调并记录）

| 模块 | 内容 |
|---|---|
| `mod.rs` | `AppRuntime` 结构体/字段/构造/Debug、常量、共用小工具、`pub use` |
| `tabs.rs` | `TabEntry/TabKind`、打开/切换/关闭（单关+批量）、轮询游标、关闭确认状态 |
| `sessions.rs` | `sessions` 表、打开/删除/保存会话、最近会话、会话树投影支撑 |
| `connection.rs` | 后端选择、连接/重连/断开、host key 弹窗状态、密码弹窗状态、连接状态投影 |
| `sftp.rs` | SFTP 列表/选择/排序/操作/远程编辑 |
| `transfer.rs` | 传输队列投影与操作 |
| `logging.rs` | 全局日志设置、每会话日志开关（N6 在此扩展） |
| `auth.rs` | 认证请求状态（N4 在此扩展）；密钥清单解析（只读） |
| `editor.rs` | SessionEditor 草稿/字段更新/持久化/文件夹选择 |
| `panels.rs` | 面板显隐/位置/布局状态（N3 在此扩展） |
| `keys.rs` | 活动终端输入/按键/查找/选择/剪贴板/清屏 |
| `projection.rs` | `AppProjection` 组装与各子投影函数 |
| `menus.rs` | 菜单旗标、右键菜单准备、tab 菜单旗标 |
| 其余（shortcuts/secret 等） | 内容少则并入 `mod.rs`，不强行拆 |

## 3. 约束与风险

- 独占期：其它任务不得改 `runtime.rs`/`runtime/**`；`bootstrap.rs` 只允许 R2 owner 做必要的路径调整。
- 风险：机械搬迁编译错误多、单文件测试耗时长。缓解：逐模块搬迁 + 小步 `cargo check`；如单窗口风险过高，可拆成两步提交（先 `tabs/sessions/sftp/editor` 四个大域，再其余）。
- 目标体积：每个子模块 ≤ 800 行；`mod.rs` ≤ 1000 行（超出说明并接受）。

## 4. 验收

1. `cargo xtask lint` + `cargo xtask test` 全绿（数量 ≥392，不降）。
2. e2e dark/en 与 light/en 全过；与 before 截图逐张对比（仅允许终端时间类噪声）。
3. 变更审查：只有搬迁（函数内容等价；可见性/`use`/文件路径为唯一差异）；`pub` API 路径不变。
4. 交付报告给出模块表（文件/行数/职责）+ before/after 行数。
5. 单写者规则更新：`runtime/mod.rs` 与各子模块分别单写者；后续任务按模块落位。

## 5. 验收记录与模块落位（2026-09-28，主 agent）

- 等价校验：`python3 /root/r2-tool/verify.py` → **`VERIFY: OK (pure move)`**（509 项 missing/extra/text diff 全 0）。
- 门禁：lint 通过；`cargo xtask test` **394 passed**；e2e dark/light **34/34×2**；隔离 before/after 截图差异仅终端时间噪声（0.02–0.04%）。
- 模块表：15 个域模块 + `tests/` 12 文件（详见 owner 报告）；`bootstrap.rs` 未改；对外路径 `crate::runtime::*` 不变。
- **单写者与任务落位**：
  - **N2（进行中）** → `tabs.rs`/`sessions.rs`/`menus.rs`/`projection.rs` + `main_window`/`command_bar`/`bootstrap`（当前接线唯一持有者）
  - N4（模块段进行中） → `auth.rs`/`connection.rs` + 新页面；接线段排队
  - N6（模块段进行中） → `logging.rs` + 新弹窗；接线段排队
  - N3 → `panels.rs`；N1 → `sftp.rs`/`sftp_ops.rs`/`transfer.rs`；N5 → 渲染器 + 页面；N9 → `tabs.rs`（须等 N2 让出）
- 新增跨模块项：子模块内声明 `pub(crate)`；仅在其它模块按名引用时再于 `mod.rs` 重导出（避免 `-D warnings` 未使用导入）。
- 测试文件落位：`runtime/tests/{domain}.rs`，新增测试加进对应域文件。
