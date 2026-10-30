# 前端 Agent 规则

适用范围：`ui/**`、与 UI 投影相关的 `crates/yshell-app` / `crates/yshell-ui` 改动、交互与视觉调整。

## 0. 原型优先（最重要）

**先原型，后实现。** 完整约定见 [`prototypes.md`](./prototypes.md)。

摘要：

- 设计或调整页面功能 / 布局 / 交互时，**直接修改** [`docs/prototypes/`](../prototypes/) 下原型并展示预期效果。
- 原型只表达功能、布局、交互逻辑；不要堆复杂 CSS / 动效。
- 无对应原型（或不先更新原型）不要大改 `ui/`。

## 1. 现行文档

| 优先级 | 文档 |
|---|---|
| 范围 | [`yshell-current-status.md`](../product/yshell-current-status.md) |
| 交互 | [`yshell-ui-interaction-contract.md`](../product/yshell-ui-interaction-contract.md) |
| 视觉 | [`ui-winui3-design-language.md`](../product/ui-winui3-design-language.md) |

本版不做的入口保持 disabled + tooltip 或空态，禁止按愿景稿「全部启用」验收。

## 2. 技术栈与目录

- UI 为 **Slint**（见 status 中的版本决策），源文件在 `ui/`。
- 主题 / token：`ui/theme.slint`；基础控件：`ui/components/basic/`。
- 页面：`ui/pages/`；壳与弹窗：`ui/windows/`、`ui/components/`。
- 业务状态与命令多在 Rust（`yshell-app` runtime + `yshell-ui` 模型）；Slint 侧保持展示与回调，避免在 `.slint` 里堆业务规则。

## 3. 实现约束

- **Token 优先**：颜色、间距、字号、时长走 `Theme`；避免散落魔法数（既有 `ui_token_discipline` 测试需继续满足）。
- **动效**：可关（减少动画 / `YSHELL_REDUCE_MOTION`）；新动效必须尊重 `Theme.animations-enabled`。
- **i18n**：用户可见文案走既有翻译流程，勿硬编码中英混用新串而不入库。
- **无障碍与对比度**：改对比度相关时参考设计语言与 xtask 对比度检查（若已有）。
- **弹窗**：遵循 L2 约定（Esc / 遮罩 / 阻断安全对话框不可 Esc 关掉等），与契约一致。
- **拖放**：应用内用原生 DragArea/DropArea；OS 级拖出等上游能力，勿私自平台层（见决策 D2/D3）。

## 4. 改 UI 的检查清单

1. status：是否属于本版范围？
2. 原型：`docs/prototypes/` 是否已反映目标布局与交互？
3. 契约：菜单 / 快捷键 / 鼠标语义是否一致？
4. 实现：Slint + 必要 Rust 接线；禁用项有说明。
5. 验证：相关 UI 测试 / 手工路径；`cargo xtask check`。
