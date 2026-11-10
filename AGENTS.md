# AGENTS

本文件是仓库级 **通用** Agent 规则。前后端、模块专项约定不写在这里，见 [`docs/agents/`](docs/agents/README.md)。

## 必读顺序

1. [`docs/product/yshell-current-status.md`](docs/product/yshell-current-status.md) — **产品范围**真相源（已交付 / 本版不做 / 真缺口 / 决策）
2. 改 UI 功能 / 布局 / 交互时：[`docs/prototypes/`](docs/prototypes/README.md) + [`docs/agents/prototypes.md`](docs/agents/prototypes.md) — **唯一设计稿**
3. 按任务类型读对应专项规则：[`docs/agents/`](docs/agents/README.md)
4. 其余文档（交互契约、设计语言、archive、旧计划等）**仅供参考**，不得单独当成待实现设计

### 文档权威（2026-11-05）

| 角色 | 文档 | 用途 |
|---|---|---|
| 产品范围 | `yshell-current-status.md` | 有没有、砍没砍、真缺口、决策 |
| **设计稿** | `docs/prototypes/**` 及其目录文档（`README`、`IMPLEMENTATION-PLAN` / `GAPS` 仅作原型→实现跟踪）与 `docs/agents/prototypes.md` | 页面长什么样、交互怎么走 |
| 参考 | `yshell-ui-interaction-contract.md`、`ui-winui3-design-language.md`、`archive/**`、ui-sketches 等 | 可查阅；**冲突时不得压过原型或 status** |

冲突时：**status（范围）> 原型及其文档（设计）> 实现与单测 > 参考文档 / archive**。  
禁止把契约或 archive 里的愿景条款（如「第一版最多 4 pane」）直接写成实现必做项；若原型误抄了参考稿，先改原型再实现。

## 通用行为

- **先对齐范围再动手**：以 status 的「已交付 / 本版不做 / 真缺口」为准；勿把 archive / 契约愿景或禁用入口当成待实现需求。
- **UI 设计只改原型**：功能 / 布局 / 交互以 `docs/prototypes/` 为准；参考文档有出入时改原型（或 status），不要按契约「补齐」。
- **小步、可验证**：改动应可 `cargo xtask check` / 相关测试覆盖；避免无关重构与顺手「美化」。
- **文档与代码同向**：范围或决策变更更新 status；设计变更先改原型（及计划跟踪），不要只改代码留口述。
- **本地问题跟踪**：`problems.md` 仅本机用，**不得提交**。
- **密钥与假后端**：不引入 Fake SSH / 系统 `ssh`/`sftp` 作为发布路径；临时验证桥不得写成完成度。
- **提交**：仅在用户明确要求时 commit；不改 git config、不 force push、不跳过 hooks。
- **禁止打字确认破坏操作**：删除 / 清空 / 覆盖等不得要求用户手输确认词（如 `CLEAR`）；用文案 + 明确破坏性按钮（或二次确认窗）。

## 专项规则入口

| 文档 | 何时阅读 |
|---|---|
| [`docs/agents/frontend.md`](docs/agents/frontend.md) | 改 `ui/`、交互、视觉、弹窗、布局 |
| [`docs/agents/backend.md`](docs/agents/backend.md) | 改 `crates/`、协议、runtime、配置、测试 |
| [`docs/agents/prototypes.md`](docs/agents/prototypes.md) | **任何 UI 设计 / 新页面 / 改布局交互**（前端必经） |

## 构建与检查

```bash
cargo xtask check    # fmt + clippy -D warnings
cargo xtask test
cargo xtask ci
cargo xtask doctor
```

细节见根 [`README.md`](README.md)。
