# 后端 / Rust Agent 规则

适用范围：`crates/**`、`xtask/`、配置 schema、协议与 runtime、测试与打包相关改动。

## 1. 产品边界

- 真相源仍是 [`yshell-current-status.md`](../product/yshell-current-status.md)。
- 发布路径面向 **应用内 SSH/SFTP 实现**；系统 `ssh`/`sftp` 或 Fake 后端不得当作完成度。
- 「本版不做」与真缺口分开：前者不要偷偷做成半吊子入口。

## 2. Crate 边界（简图）

| Crate | 职责 |
|---|---|
| `yshell-app` | 进程入口、窗口、runtime、与 UI 接线 |
| `yshell-ui` | 视图模型、命令/事件类型 |
| `yshell-ssh` / `yshell-sftp` | 连接、认证、传输、错误分类 |
| `yshell-terminal` | 终端网格 / 解析 / 渲染相关 |
| `yshell-config` | 配置 schema、会话、密钥元数据等持久化形状 |
| `yshell-secret` / `yshell-logging` / `yshell-local` / `yshell-mcp` | 密钥、日志、本地 PTY、MCP |
| `yshell-test-support` / `xtask` | 测试辅助与仓库任务 |

跨 crate 依赖保持单向、职责清晰；UI 模型类型放 `yshell-ui`，协议细节放 ssh/sftp，不要在 app 里复制一套平行抽象。

## 3. 工程约定

- **MSRV / Edition**：以 workspace `Cargo.toml` 为准（当前 rust-version 1.92）。
- **unsafe**：workspace `unsafe_code = "deny"`；不要为图省事打开。
- **检查入口**：`cargo xtask check` / `test` / `ci`；不要发明第二套本地门禁脚本替代 xtask。
- **Live 测试**：需要真机 SSH 时用 xtask 的 live 选项与文档中的目标格式；默认 CI 路径以仓库现有 workflow 为准。
- **配置**：改 schema 须考虑迁移与默认值；幽灵字段（有 schema 无接线）不要继续扩大，能删则跟 status 对齐。

## 4. 与前端协作

- 仅改后端协议/行为但影响 UI 状态机时：同步更新契约或原型中的状态说明，并确保投影层（runtime projection）一致。
- 新增用户可见状态 / 错误：前端要有对应展示；若属新页面或新流程，走 [`prototypes.md`](./prototypes.md)。

## 5. 改动检查清单

1. 是否落在 status 允许范围？
2. 公共 API / 错误类型是否稳定、可测？
3. 单测或 smoke 是否覆盖主路径与失败路径？
4. 是否误引入系统 ssh 委托或假后端当主路径？
5. `cargo xtask check`（及必要 `test`）是否通过？
