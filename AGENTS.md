# AGENTS

本文件是仓库级 **通用** Agent 规则。前后端、模块专项约定不写在这里，见 [`docs/agents/`](docs/agents/README.md)。

## 必读顺序

1. [`docs/product/yshell-current-status.md`](docs/product/yshell-current-status.md) — 产品真相源（已交付 / 本版不做 / 真缺口 / 决策）
2. 改 UI / 交互时：[`docs/product/yshell-ui-interaction-contract.md`](docs/product/yshell-ui-interaction-contract.md)
3. 改视觉 token / Fluent 观感时：[`docs/product/ui-winui3-design-language.md`](docs/product/ui-winui3-design-language.md)
4. 按任务类型读对应专项规则：[`docs/agents/`](docs/agents/README.md)

冲突时：**status > 实现与单测 > UI 契约 > archive 历史稿**。

## 通用行为

- **先对齐范围再动手**：以 status 的「已交付 / 本版不做 / 真缺口」为准；勿把 archive 愿景或禁用入口当成待实现需求。
- **小步、可验证**：改动应可 `cargo xtask check` / 相关测试覆盖；避免无关重构与顺手「美化」。
- **文档与代码同向**：行为或产品决策变更时，更新 status（或明确指向的现行契约），不要只改代码留口述。
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
