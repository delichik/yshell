# docs/prototypes

现行 **UI 功能 / 布局 / 交互** 原型目录。规则见 [`docs/agents/prototypes.md`](../agents/prototypes.md)。

**浏览器目录页：** [`index.html`](./index.html)

主窗是 **N3 ToolDock**：Left stack | Workspace | Right stack。可停靠面板（Sessions / Tunnels / QuickCommands / Transfers）可换边；默认布局不是锁死侧别。整壳菜单/快捷键见 [`main-window/`](./main-window/index.html)；停靠壳见 [`tool-dock/`](./tool-dock/index.html)。

## 约定

- 每个功能或页面一个子目录，入口一般为 `index.html`（浏览器直接打开）。
- 低保真：块级布局 + 文案标签 + 简单状态切换；不要复杂 CSS / 动效。
- **任何 UI 设计阶段直接改这里的文件** 来展示预期效果；确认后再改 `ui/`。
- 固定 Theme 尺寸；舞台可横滚，**不压缩**对话框；仅清单等明确区域可滚。
- 原地编辑类交互必须用页内状态钮（或默认演示态）把输入框钉在画面上，不能只靠右键偶发出现。
- **禁止**再把 Sessions 写成固定「左 sidebar」、工具面板写成固定「RightDock Tab」。

## 索引

### Main shell

| 目录 | 覆盖 |
|---|---|
| [`main-window/`](./main-window/index.html) | 整壳 1440×900：菜单 / 快捷键 / 双栈示意；细节见 tool-dock 与 panel-* |

### ToolDock（N3）

| 目录 | 覆盖 |
|---|---|
| [`tool-dock/`](./tool-dock/index.html) | 左右栈壳、分栏、折叠、⋯ Move L/R 演示 |
| [`panel-sessions/`](./panel-sessions/index.html) | Sessions 内容；Session 原地改名；Folder New/Rename 本版禁用 |
| [`panel-transfers/`](./panel-transfers/index.html) | Transfers · 320×520 · Running/Mixed/Empty · 批量 Pause/Resume/Clear |
| [`panel-tunnels/`](./panel-tunnels/index.html) | Tunnels · 320×560 · Add → tunnel-editor；Start/Stop disabled |
| [`panel-commands/`](./panel-commands/index.html) | Commands 空态 |

旧入口（跳转 stub）：[`session-sidebar/`](./session-sidebar/index.html) → panel-sessions；[`right-dock/`](./right-dock/index.html) → tool-dock。

### Workspace（非 dock）

| 目录 | 覆盖 |
|---|---|
| [`tab-bar/`](./tab-bar/index.html) | 标签条；Rename Tab 原地输入 |
| [`terminal-workspace/`](./terminal-workspace/index.html) | 终端工作区 960×560 · 查找/跨会话/REC/本地 shell |
| [`sftp-panel/`](./sftp-panel/index.html) | SFTP 双栏；原地 Rename/New；折叠本地；拖放；Chmod |
| [`quick-connect/`](./quick-connect/index.html) | 快速连接 + Recent；校验空 Host / 坏端口 |
| [`workspace-split/`](./workspace-split/index.html) | 工作区分屏：1 / 2v / 2h / 4 · 空 pane · Balance |
| [`sync-input/`](./sync-input/index.html) | N9 同步按键：状态条 + chip + pane 接收开关（无 D23 弹窗） |

### Compact（560×400）

| 目录 | 覆盖 |
|---|---|
| [`compact-confirm/`](./compact-confirm/index.html) | 确认：普通 / 破坏性（禁止打字确认词） |
| [`close-tab-confirm/`](./close-tab-confirm/index.html) | 关标签 / 退出：SSH · SFTP+传输 · Quit |
| [`new-folder/`](./new-folder/index.html) | 历史短表单壳；非主路径 |
| [`sftp-entry/`](./sftp-entry/index.html) | 历史短表单壳；主路径见 `sftp-panel` |

### Standard（720×580）

| 目录 | 覆盖 |
|---|---|
| [`auth-prompt/`](./auth-prompt/index.html) | 认证（含密码单方法）：Password only / Multi / Key / Challenge |
| [`host-key/`](./host-key/index.html) | 主机密钥信任：first-trust / changed |
| [`logging/`](./logging/index.html) | 终端日志：开启表单 vs 停止 |
| [`about/`](./about/index.html) | 关于：版本、Config / License |
| [`sftp-conflict/`](./sftp-conflict/index.html) | SFTP 冲突（仅路径列表可滚） |
| [`sftp-properties/`](./sftp-properties/index.html) | SFTP 属性网格 |
| [`tunnel-editor/`](./tunnel-editor/index.html) | 隧道表单 · 720×580 · 来自 panel-tunnels Add |

### Wide（1100×820）

| 目录 | 覆盖 |
|---|---|
| [`settings/`](./settings/index.html) | 设置：左导航 + 右表单；Shortcuts disabled |
| [`session-editor/`](./session-editor/index.html) | 会话编辑器；SFTP/Advanced nav disabled（§2） |
| [`folder-editor/`](./folder-editor/index.html) | 文件夹编辑 |
| [`theme-manager/`](./theme-manager/index.html) | 主题管理：列表 + 编辑/预览 |
| [`host-keys/`](./host-keys/index.html) | 主机密钥：列表+详情；Import/Export 模态冻结父窗 |
| [`known-hosts/`](./known-hosts/index.html) | 已知主机：清单 + 详情 + Danger Zone |
| [`private-keys/`](./private-keys/index.html) | 私钥：清单+详情；Import/Export 模态冻结父窗 |
| [`proxies/`](./proxies/index.html) | 代理：清单+表单；页脚仅 Cancel\|Save（无叠 Close） |

历史观感稿：[`docs/product/ui-sketches/`](../product/ui-sketches/)。
