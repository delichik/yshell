# docs/prototypes

现行 **UI 设计稿**目录（功能 / 布局 / 交互 / **WinUI 3 观感与准确几何**）。规则见 [`docs/agents/prototypes.md`](../agents/prototypes.md)。

产品范围仍以 [`yshell-current-status.md`](../product/yshell-current-status.md) 为准。冲突时以本目录原型 + status 为准。

**视觉真源（设计稿）**：对齐 [`ui-winui3-design-language.md`](../product/ui-winui3-design-language.md)。条带高度 Fluent **32**（title/menu/tab）；实现 `Theme.bar-height-menubar = 32` 已对齐。共享样式：

| 文件 | 用途 | 样板 |
|---|---|---|
| [`_theme.css`](./_theme.css) | Light/Dark token + 几何 | — |
| [`form.css`](./form.css) | 表单控件（唯一） | [`forms.html`](./forms.html) · [`components/buttons.html`](./components/buttons.html) · [`form-fields.html`](./components/form-fields.html) |
| [`_chrome.css`](./_chrome.css) | 窗壳 / Nav / Tab / Flyout / InfoBar | [`chrome.html`](./chrome.html) |
| [`_proto.css`](./_proto.css) + [`_embed.js`](./_embed.js) | 舞台脚手架、iframe 定高、query/`theme` | — |
| [`components/`](./components/) | **组件真源**（壳 / 控件 / 注释 … iframe + query） | [`components.html`](./components.html) |
| [`_anno.css`](./_anno.css) | 通用注释（tag / callout / legend） | [`annotations.html`](./annotations.html) · `components/anno-*` |

**浏览器目录页：** [`index.html`](./index.html) — 左侧列表、右侧 iframe 预览。组件对照：[`components.html`](./components.html)。

主壳统一在 [`main-shell/`](./main-shell/)：同功能目录多 HTML，可再套一层分组（`panels/`、`workspace/`）。

## 约定

- 每个功能或页面一个子目录，入口一般为 `index.html`（浏览器直接打开）。
- **同功能多文件**：弹窗 / 独立区用同目录下的 `modals.html`、`sections/…` 等。
- **可再套一层**：同一产品壳内按区分子目录，仍用多 HTML。
- **视觉**：WinUI 3 / Fluent 2；钉死窗档与控件几何；禁止页内线框色板 / 控件样式分叉。
- **任何 UI 设计阶段直接改这里的文件**；确认后再改 `ui/`。
- 固定窗档尺寸；舞台可横滚，**不压缩**对话框；仅清单等明确区域可滚。
- 原地编辑类交互必须用页内状态钮把输入框钉在画面上。
- **禁止**再把 Sessions 写成固定「左 sidebar」、工具面板写成固定「RightDock Tab」。
- **注释**：统一 `A · Short name`（[`annotations.html`](./annotations.html) + [`_anno.css`](./_anno.css)）。
- **表单 / 壳层**：引用根 `form.css` + `_chrome.css`；`session-editor/form.css` 仅 `@import` 根文件。

## 弹窗尺寸

弹窗只用三档；新增弹窗不得自造尺寸。确需特殊尺寸时在文件 meta 与本表标注原因。

| 档位 | 尺寸 | 目录 |
|---|---|---|
| compact | 560×340 | `compact-confirm` · `close-tab-confirm` · `host-key` · `sftp-chmod` · `settings/mcp-consent` · `settings/master-password` · `settings/master-password-unlock` · `known-hosts/modals` · `session-editor/sections/login-step-wait` |
| standard | 720×520 | `auth-prompt` · `logging` · `about` · `sftp-conflict` · `sftp-properties` · `tunnel-editor` · `private-keys/modals` · `session-editor/sections/login-step-keys` |
| wide | 1100×700 | `settings` · `session-editor` · `known-hosts` · `private-keys` · `proxies` · `terminal-styles` |

**非弹窗尺寸（切片/主窗）**：主窗 `1440×900`；工作区切片 `960×560`；tab bar 切片 `960×200`；Quick Connect 切片 `720×480`；split / tool-dock 切片 `1100×640`；Settings / Session Editor 内容切片 `800×640`（实际窗仍 wide `1100×700`）；dock 面板 `280–320` 宽。

## 索引

### Main shell — [`main-shell/`](./main-shell/)

```text
main-shell/
  index.html              # 整壳拼装
  tool-dock.html          # 左右栈壳
  panels/                 # 可停靠面板内容
    sessions.html
    transfers.html
    tunnels.html
    commands.html
  workspace/              # 工作区内容
    tab-bar.html
    terminal.html
    sftp.html
    quick-connect.html
    split.html
    sync-input.html
```

N3：Left stack | Workspace | Right stack。可停靠面板可换边。

| 路径 | 覆盖 |
|---|---|
| [`main-shell/index.html`](./main-shell/index.html) | 整壳 1440×900：菜单 / 快捷键 / 双栈示意 |
| [`main-shell/tool-dock.html`](./main-shell/tool-dock.html) | 左右栈壳、分栏、折叠、⋯ Move L/R |
| [`main-shell/panels/sessions.html`](./main-shell/panels/sessions.html) | Sessions；Session 原地改名；Folder New/Rename 本版禁用 |
| [`main-shell/panels/transfers.html`](./main-shell/panels/transfers.html) | Transfers · Pause/Cancel |
| [`main-shell/panels/tunnels.html`](./main-shell/panels/tunnels.html) | Tunnels · Add → tunnel-editor |
| [`main-shell/panels/commands.html`](./main-shell/panels/commands.html) | Commands 空态 |
| [`main-shell/workspace/tab-bar.html`](./main-shell/workspace/tab-bar.html) | 标签条；Rename Tab 原地输入 |
| [`main-shell/workspace/terminal.html`](./main-shell/workspace/terminal.html) | 终端工作区 · 查找/REC |
| [`main-shell/workspace/sftp.html`](./main-shell/workspace/sftp.html) | SFTP 双栏 · 拖放 |
| [`main-shell/workspace/quick-connect.html`](./main-shell/workspace/quick-connect.html) | 快速连接 + Recent |
| [`main-shell/workspace/split.html`](./main-shell/workspace/split.html) | 分屏 1/2/4 |
| [`main-shell/workspace/sync-input.html`](./main-shell/workspace/sync-input.html) | N9 同步按键 |

旧入口 stub：[`session-sidebar/`](./session-sidebar/index.html) → `panels/sessions`；[`right-dock/`](./right-dock/index.html) → `tool-dock`。

### Compact（560×340）

| 目录 | 覆盖 |
|---|---|
| [`compact-confirm/`](./compact-confirm/index.html) | 确认：普通 / 破坏性 / 多行粘贴（禁止打字确认词） |
| [`close-tab-confirm/`](./close-tab-confirm/index.html) | 关标签 / 退出 |
| [`host-key/`](./host-key/index.html) | Host key 首次信任 / 变更（compact） |
| [`sftp-chmod/`](./sftp-chmod/index.html) | SFTP 权限：八进制 + Owner / Group / Other 勾选框 |

### Standard（720×520）

| 目录 | 覆盖 |
|---|---|
| [`auth-prompt/`](./auth-prompt/index.html) | 认证：Password / Multi / Key / Challenge |
| [`logging/`](./logging/index.html) | 终端日志 |
| [`about/`](./about/index.html) | 关于 |
| [`sftp-conflict/`](./sftp-conflict/index.html) | SFTP 冲突 |
| [`sftp-properties/`](./sftp-properties/index.html) | SFTP 属性 |
| [`tunnel-editor/`](./tunnel-editor/index.html) | 隧道表单 · 来自 panels/tunnels Add |

### Wide（1100×700）

对齐 main-shell：`index` 整壳 + 一层 `sections/` 切片（不是每区复制整窗）。

```text
settings/                         session-editor/
  index.html                        index.html
  sections/                         sections/
    general.html                      general.html
    appearance.html                   authentication.html
    terminal.html                     login-script.html
    security.html                     proxy.html
    advanced.html                     tunnels.html
    sftp.html                         terminal-style.html
                                    login-step-keys.html
                                    login-step-wait.html
```

| 目录 | 覆盖 |
|---|---|
| [`settings/`](./settings/) | `index` 整壳 hub + `sections/*`；`security`（主密码卡片 + 管理器清单）；`master-password`（Set/Change/Remove/Reset）；`master-password-unlock`（启动解锁）；`mcp-consent` |
| [`forms.html`](./forms.html) + [`form.css`](./form.css) | 表单控件规范页 + 唯一样式源（含动画） |
| [`session-editor/`](./session-editor/) | `index` 整壳 hub + `sections/*`；[`form.css`](./session-editor/form.css) → 根 `form.css`（表单控件 420；清单/Keys 大编辑除外）；弹窗 `sections/login-step-*` |
| [`terminal-styles/`](./terminal-styles/index.html) | 终端样式 |
| [`known-hosts/`](./known-hosts/index.html) | Known Hosts · [`modals.html`](./known-hosts/modals.html) |
| [`private-keys/`](./private-keys/index.html) | Private Keys · [`modals.html`](./private-keys/modals.html) |
| [`proxies/`](./proxies/index.html) | Proxies |

历史观感稿：[`docs/product/ui-sketches/`](../product/ui-sketches/)。

## 实现对齐缺口

| 项 | 设计稿 | 当前实现 |
|---|---|---|
| Menubar / 顶栏条带 | Fluent **32** | Slint `Theme.bar-height-menubar = 32`（已对齐） |

设计验收以本目录为准。
