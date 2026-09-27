# YShell UI 设计规范 · WinUI 3 / Fluent 2

状态：**设计已确认**（迭代 W0 完成，2026-09-26；五项决策见 §12）。本文件确定"长什么样、遵循什么规则"，实现按 §9 的 W1–W5 推进。
上游：`docs/product/yshell-product-technical-spec.md` §5.10、`docs/product/ui-redesign-roadmap.md`
配套视觉稿：`docs/product/ui-sketches/winui3-mockup.html`

---

## 1. 目标与非目标

### 1.1 目标
1. 视觉层整体对齐 **Windows 11 / WinUI 3（Fluent 2）**：配色、圆角、描边、状态层、字体排印、图标、动效。
2. 在 Windows 上"像原生应用"；在 Linux/macOS 上保持同一套观感（不依赖 Windows 专有能力）。
3. 消除硬编码：颜色、间距、圆角、字号、动效全部走语义 token。
4. 结构对齐 WinUI 3 控件模型：NavigationView / CommandBar / TabView / InfoBar / ContentDialog / Flyout / Card。
5. 多语言一等公民：可运行时切换、无字符串拼接、布局可容纳 +40% 文案膨胀与 CJK。

### 1.2 非目标（本期不做）
- 不做 Mica/Acrylic 真实模糊、云母材质、亚克力背板（软件渲染器做不到）。
- 不改交互契约与后端能力（`yshell-ui-interaction-contract.md` 保持有效），只改呈现。
- 不做 RTL 布局（预留规则，不实现）。
- 不追求 Windows 专属控件（如 JumpList、Taskbar 缩略图工具栏）。

### 1.3 设计原则
1. **语义 token 先行**：组件只引用 token，禁止写死色值（现状 189 处）。
2. **信息层级靠"层"不靠"阴影"**：window → layer → card → control，四级实色 + 1px 描边表达层级。
3. **状态必须可见**：hover / pressed / selected / focus / disabled 五态统一实现，不可用即 disabled（不允许"静默失效"）。
4. **文字即数据**：任何用户可见字符串只有一处来源（`.slint` 的 `@tr()`），禁止拼接成句。
5. **可达性不回归**：每轮改动后既有入口全部可达，测试不回归。

---

## 2. 技术边界（约束决定设计手段）

| 约束 | 现状 | 对设计的影响 |
|---|---|---|
| 渲染器 | Slint 1.9.2 `renderer-software` | 无背景模糊/亚克力；阴影可用但要克制 → 层级用实色分层 + 描边表达 |
| 后端 | `backend-winit-x11`（WSLg 开发） | 窗口装饰由 WM 提供；自绘标题栏（阶段 B）需要手动处理拖拽/缩放，风险高 |
| 控件风格 | Slint 内置 fluent 风格（WinUI 3 色板的实现） | 直接复用其色值与几何基线，自定义控件与其保持一致；`build.rs` 显式 pin `fluent` 避免平台漂移 |
| 主题 | `Palette`/`FluentPalette` 支持 `color-scheme`（dark/light/跟随系统） | 主题切换机制天然存在；YShell 自己的 token 需同步跟随 |
| 字体 | `.slint` 支持 `import "./x.ttf"` 内嵌 | UI 字体可内嵌；CJK 体积大 → 策略见 §7.5 |
| 翻译 | `@tr()` + `slint-tr-extractor` + 打包 `.po`，`select_bundled_translation()` 运行时切换 | 无需 gettext 运行时依赖，portable 构建友好 |
| 终端 | 自绘像素渲染（`yshell-terminal`，已内嵌 DejaVu Sans Mono） | 终端配色走"配色方案"体系（Campbell / One Half Light），不随 UI 主题强制反色 |

---

## 3. 设计 Token

### 3.1 颜色（Fluent 2 语义命名，深/浅双套）

> 基线来源：Slint fluent 风格的 `styling.slint`（即 WinUI 3 色值）。实施 W1 时从该文件取准值，不允许凭印象填写。

| Token | Dark | Light | 用途 |
|---|---|---|---|
| `window.background` | `#202020` | `#F3F3F3` | 窗口底色（代替 Mica 的实色） |
| `layer.fill` | `#FFFFFF0F` | `#FFFFFFB3` | 导航栏/命令栏等内容层 |
| `layer.stroke` | `#0FFFFFFF` | `#0F000000` | 层分隔线 |
| `card.fill` | `#FFFFFF0D` | `#FFFFFFB3` | 卡片、右侧工具面板、对话框主体 |
| `card.stroke` | `#0FFFFFFF` | `#0F000000` | 卡片描边 |
| `control.fill` | `#FFFFFF0F` | `#B3FFFFFF` | 按钮/输入框/下拉默认底 |
| `control.fill.hover` | `#FFFFFF17` | `#80FFFFFF` | 悬停 |
| `control.fill.pressed` | `#FFFFFF0A` | `#4DFFFFFF` | 按下 |
| `control.fill.disabled` | `#FFFFFF0A` | `#F0FFFFFF` | 禁用底 |
| `control.stroke` | `#FFFFFF14` | `#0F000000` | 控件描边 |
| `subtle.hover` | `#FFFFFF0F` | `#0F000000` | 列表项/导航项悬停 |
| `subtle.pressed` | `#FFFFFF0A` | `#0A000000` | 列表项按下 |
| `subtle.selected` | `#FFFFFF0F` | `#0F000000` | 列表项/导航项选中底（强调条另绘） |
| `text.primary` | `#FFFFFF` | `#E4000000` | 正文 |
| `text.secondary` | `#C5FFFFFF` | `#9E000000` | 次要信息、菜单快捷键 |
| `text.tertiary` | `#87FFFFFF` | `#72000000` | 占位符、提示 |
| `text.disabled` | `#5DFFFFFF` | `#5C000000` | 禁用文字 |
| `accent.default` | `#60CDFF` | `#005FB8` | 主操作、选中指示 |
| `accent.hover` | `#7EDEFF` | `#0067C0` | 强调色悬停（实施时取 WinUI Light2/Dark1 准值） |
| `accent.pressed` | `#4FB8E8` | `#003E92` | 强调色按下 |
| `accent.text` | `#000000` | `#FFFFFF` | 强调色上的文字 |
| `selection.fill` | `#60CDFF33` | `#005FB833` | 文本选区、拖拽高亮 |
| `system.success` | `#6CCB5F` | `#0F7B0F` | 成功 |
| `system.caution` | `#FCE100` | `#9D5D00` | 警告 |
| `system.critical` | `#FF99A4` | `#C42B1C` | 错误/危险 |
| `focus.outer` | `#000000` | `#FFFFFF` | 焦点外环（与 accent 内环配合，保证任何底色上可见） |
| `focus.inner` | `#FFFFFF` | `#000000` | 焦点内环 |
| `terminal.background` | `#0C0C0C` | `#FAFAFA` | 终端底（跟随终端配色方案，见 §5.17） |
| `terminal.foreground` | `#CCCCCC` | `#383A42` | 终端前景 |

InfoBar / 提示类不写死背景色，用规则生成：`背景 = 系统色 12% over layer.fill`、`描边 = 系统色 40%`、`图标与主文字 = 系统色`。

### 3.2 字体排印

| Token | 字号 / 行高 | 字重 | 用途 |
|---|---|---|---|
| `caption` | 12 / 16 | 400 | 状态栏、辅助说明、时间戳 |
| `body` | 14 / 20 | 400 | 默认正文、列表项、菜单、按钮 |
| `body-strong` | 14 / 20 | 600 | 列表选中项、区块小标题、面板标题 |
| `subtitle` | 20 / 28 | 600 | 对话框标题、空态标题 |
| `title` | 28 / 36 | 600 | 欢迎页/关于页主标题 |
| `code` | 13 / 18 | 400 | 主机名、路径、权限、端口（等宽） |

字体族：见 §7.5。**只能写一个字体族**——Slint 1.9 的 `font-family` 不支持逗号字体链，软件渲染器也没有逐字形回退；窗口通过 `default-font-family: Theme.font-ui` 应用 Rust 按平台写入的单值。

等宽：同样只能写单一值（当前 `Theme.font-mono = "monospace"`）；后续可评估内置 `Cascadia Mono`（OFL，终端字体升级备选）。

### 3.3 间距 / 尺寸 / 圆角 / 动效

| 类别 | 值 |
|---|---|
| 间距刻度 | 4 / 8 / 12 / 16 / 20 / 24 / 32（4px 网格） |
| 控件高度 | 标准 32；紧凑 24；主操作 32；图标按钮 32×32（命中区 ≥32，触控目标 ≥40） |
| 列表项高度 | 导航 40；文件列表 32；菜单项 32；树行 28（缩进 16/级） |
| 图标尺寸 | 默认 20；小号 16；大号（空态）28 |
| 圆角 | 控件 4；卡片/面板/浮层/对话框 8；徽章 pill；窗口 8（由 WM 决定） |
| 描边 | 常规 1px；聚焦环 2px（内环 + 外环） |
| 动效时长 | 见 §6.2：`motion.faster` 100 / `motion.fast` 167 / `motion.normal` 250 / `motion.slow` 333 |
| 缓动 | 进入 `ease-out`，退出 `ease-in`，双向 `cubic-bezier(0.33, 0, 0.67, 1)` |
| 阴影 | 只用于浮层（菜单/对话框/工具提示）：`drop-shadow-blur 8–16px`、`#00000033`，最多一层 |

### 3.4 主题模式与强调色
- 模式：`跟随系统`（默认）/ `浅色` / `深色`，设置页即时生效，无需重启；**深色与浅色同期交付**。
- 强调色：从预设色板中选择，并提供 `跟随系统`（仅 Windows 读取系统强调色，Linux/macOS 回退到当前预设）：

| 预设 | Light | Dark |
|---|---|---|
| Windows 蓝（默认） | `#005FB8` | `#60CDFF` |
| 青 | `#038387` | `#4CC2C4` |
| 紫 | `#8764B8` | `#B199D9` |
| 绿 | `#107C10` | `#6CCB5F` |
| 橙 | `#CA5010` | `#F4A06A` |
| 洋红 | `#C239B3` | `#E38FE0` |

- 预设色以 WinUI 3 `SystemAccentColor` 系列取准值；`accent.hover` / `accent.pressed` 由 default 派生（亮度 +12% / −12%），`accent.text` 按对比度自动取黑或白。
- 终端配色独立于 UI 主题：默认深色 `Campbell`、浅色 `One Half Light`，按 session 可覆盖。
- 配置落点：`config.toml` 的 `[appearance]`（`mode`、`accent`、`language`），后续与 §5.10 的继承链（session > folder > global）对齐。

### 3.5 状态层规则（所有可交互控件统一）

| 状态 | 表现 |
|---|---|
| 默认 | `control.fill` + `control.stroke` |
| 悬停 | 前景色 5%–6% 叠加（`control.fill.hover` / `subtle.hover`），描边不变 |
| 按下 | 叠加加深至 8%–10%（`control.fill.pressed`），内容轻微下沉 1px（可选） |
| 选中 | `subtle.selected` + 左侧 3×16 圆角强调条（导航）或 `accent` 底（主按钮） |
| 焦点 | 键盘焦点才显示：2px 双色环（`focus.outer` + `focus.inner`），贴合控件圆角 +2px |
| 禁用 | 底色 `control.fill.disabled`，文字 `text.disabled`，无 hover 反馈，`mouse-cursor: default` |
| 忙碌 | 控件右侧 16px ProgressRing，控件保持宽度不跳动 |

---

## 4. 布局蓝图

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ [Y] YShell · web-01 — root@10.0.0.12                             ─  □  ×   │ 32px 系统标题栏（阶段 A，OS 绘制）
├──────────────────────────────────────────────────────────────────────────────┤
│ 文件  编辑  会话  视图  帮助                                                  │ 32px MenuBar（低频/全局入口）
├──────────────────────────────────────────────────────────────────────────────┤
│ ＋新建 ▾ │ 快速连接 [ ssh://user@host:22        ] [连接] │ SFTP 隧道 命令  🔍 │ 48px 命令栏（高频动作）
├────────────────┬───────────────────────────────────────────┬─────────────────┤
│ 会话            │ [终端 1 ×][sftp-01 ×][+]                  │ SFTP 传输 隧道   │ 32px 标签条（唯一容器）
│ [搜索已保存会话] │                                           ├─────────────────┤
│ ▾ 生产环境 (3)  │  user@web-01:~$ ls -la                    │ /var/www/html    │
│   web-01        │  drwxr-xr-x  4 root root 4096 ...         │ ⟰ ⟳ ＋ ⬆ ⬇      │
│ ▾ 测试环境 (2)  │  -rw-r--r--  1 root root  220 ...         ├─────────────────┤
│   sftp-01       │  user@web-01:~$ █                         │ 名称  大小  修改 │
│ 最近            │                                           │ 📁 ..            │
│  web-01         │                                           │ 📁 assets        │
│ 最近连接 3 条    │                                           │ 📄 index.php  2K │
├────────────────┴───────────────────────────────────────────┴─────────────────┤
│ ● 已连接 user@web-01 · 12ms   │ 日志:关 代理:无 │        后端: native-ssh  │ 26px 状态栏
└──────────────────────────────────────────────────────────────────────────────┘
   240px(200–360 可拖拽)              自适应                              320px(280–480)
```

### 4.1 区域规格

| 区域 | 尺寸 | 实现要点 |
|---|---|---|
| 标题栏 | 32px（阶段 A 由 OS 绘制） | **阶段 A（默认）**：系统标题栏只显示"YShell — <活动会话>"，标签页只在终端工作区。**阶段 B**：自绘标题栏时标签条移入标题栏，并**同时移除工作区标签条**——任何时刻只存在一个标签容器 |
| 菜单栏 MenuBar | 32px | 5 个顶级菜单（File/Edit/Session/View/Help），条目结构沿用 `ui-restructure-iteration-1.md` §4 契约；WinUI 3 MenuBar 形态见 §5.16 |
| 命令栏 | 48px | WinUI CommandBar：主操作带文字，次级操作只留图标（20px），溢出进 `⋯` 菜单 |
| 左导航 | 240px，可拖 200–360 | NavigationView：分区标题 `caption` + `text.secondary`，导航项 40px、图标 20px、选中强调条 |
| 终端区 | 自适应 | TabView 标签条 + 终端表面（8px 圆角、1px 描边）；查找栏默认隐藏，出现时为 40px 悬浮条 |
| 右工具面板 | 320px，可拖 280–480 | 顶部 Pivot（SFTP / 传输 / 隧道 / 命令），内容为卡片列表；折叠时整列隐藏 |
| 状态栏 | 26px | `caption` 字号；左：连接状态（圆点 + 文本），中：日志/代理/速率，右：后端标识；超长省略 |

### 4.2 自适应规则

| 窗口宽度 | 行为 |
|---|---|
| ≥1280 | 三栏完整（默认 1440×900） |
| 1120–1280 | 右工具面板收窄至 280，左导航 200 |
| 960–1120 | 右工具面板自动折叠（可手动展开为浮层） |
| <960（最小 880×600） | 左导航折叠为 48px 图标条或抽屉浮层 |

面板宽度、折叠状态进内存态（后续持久化到 `state.toml`）。

---

## 5. 组件规范

统一约定：高度取自 §3.3；圆角 4；状态层取自 §3.5；禁用用 `text.disabled`；文案一律 `@tr()`。

**光标语义（桌面端）**：按钮 / 菜单 / 列表 / 导航 / 标签 / 开关等交互控件保持默认箭头，禁止设 hand（`pointer`，那是 Web 习惯）；
仅文本输入与终端文本区用 I 形（`MouseCursor.text`），面板 / 分隔条拖拽把手用 `col-resize`。

### 5.1 Button
- 变体：`standard`（默认）、`accent`（主操作，每屏 ≤1）、`subtle`（无底无描边，悬停才出现底色）、`icon`（32×32 方形）、`toggle`（选中 = `accent` 15% 底 + `accent` 文字）。
- 水平内边距 12；图标可选（20px，图标在左，间距 8）。
- 主操作仅用于"连接/保存/确认"这类终结动作；破坏性操作不用红色实心底，用 `critical` 文字 + 确认弹窗。

### 5.2 输入类（LineEdit / ComboBox / SpinBox）
- 高 32，底 `control.fill`，描边 `control.stroke`，聚焦时底变亮 + 底部 2px `accent` 指示条（Fluent 2 的输入框形态）。
- 占位符 `text.tertiary`；错误态描边 `system.critical` + 下方 12px 错误文案。
- ComboBox 展开用菜单规范（§5.11），选中项左侧勾选标记。

### 5.3 选择类（CheckBox / Switch / Radio）
- CheckBox 20×20，圆角 4；Switch 40×20，滑块 12，动画 167ms。
- 标签在右（CJK 场景可上），间距 8；整行可点。

### 5.4 导航项（NavigationViewItem）
- 高 40；左图标 20 + 文本 14；悬停 `subtle.hover`；选中 `subtle.selected` + 左 3×16 圆角 `accent` 条。
- 分组标题：`caption` / `text.secondary`，上间距 12。
- 折叠态只显示图标，悬停出 ToolTip。

### 5.5 树行（会话树）
- 高 28；缩进 16/级；展开箭头 16px；图标按类型（文件夹/会话）；计数徽章 `caption` + `text.tertiary`。
- 双击重命名（就地编辑）、右键出上下文菜单。

### 5.6 标签条（TabView）
- **唯一容器原则**：标签条只出现一次——阶段 A 在终端工作区顶部；阶段 B 移入自绘标题栏并删除工作区标签条。禁止两处同时出现。
- 高 32；活动标签：底 `layer.fill`、文字 `text.primary`、底部 2px `accent` 指示条；标签之间 1px 分隔线（上下各留 7px）。
- 非活动标签：`text.secondary`，悬停 `subtle.hover`；关闭按钮 16px，悬停 `subtle.hover`；`+` 固定在最右。

### 5.7 命令栏（CommandBar）
- 高 48；结构：`[主按钮] [快速连接输入 + 连接] │ [toggle 组] ⋯ [搜索] [设置]`。
- 次级操作图标化并加 ToolTip，溢出到 `⋯`（保留顺序与快捷键提示）。
- 与 §5.16 MenuBar 的分工：MenuBar 承载低频/全局操作与开关，命令栏只保留高频动作；同一动作可两处都有，但**快捷键提示只出现在菜单里**。

### 5.8 卡片（Card）
- `card.fill` + 1px `card.stroke` + 圆角 8 + 内边距 12–16。
- 卡片标题用 `body-strong`；卡片内列表项高 32。
- 右侧面板的三段（SFTP / 传输 / 隧道）拆成独立卡片，间距 12。

### 5.9 InfoBar（安全/状态横幅）
- 取代当前"整条橙色横幅"：圆角 8、图标 20、标题 `body-strong`、正文 `body`、右侧最多 2 个操作按钮。
- 严重级别：`informational` / `success` / `warning`（主机密钥变更→警告）/ `error`。
- 主机密钥确认策略（2026-09-27 产品决定）：`strict` 与 `trust_on_first_use` 的**首次连接都必须弹窗**——
  Trust Once = 仅本次（内存）信任，Trust and Save = 写入 `known_hosts`；**不存在静默 TOFU**。
  唯一静默信任的是显式的调试策略 `accept-any-for-testing`。变更密钥仍必须阻断式呈现
  （弹窗 + 详情入口 + 输入 `REPLACE` 的确认区）。

### 5.10 ContentDialog（模态对话框）
- 宽度 480（表单类 560）；圆角 8；遮罩 = `window.background` 60% 且**不加模糊**（软件渲染器）。
- 结构：标题 `subtitle` → 正文 `body` → 操作区（右对齐，主操作在右）。
- 关闭方式：Esc、遮罩点击（仅非破坏性）、右上 `×`（可选统一）。
- 现有弹窗（About/Settings/Session Editor/Known Hosts/删除确认/退出确认/SFTP 操作）全部按此规范重排。

### 5.11 Flyout / 菜单
- 圆角 8、`card.fill` 提升一级（深色下用 `#2C2C2C`）、1px 描边、单层阴影。
- 行高 32；左侧 16px 勾选列；右侧快捷键 `caption` / `text.secondary`；禁用项 36% 不透明。
- 分隔线上下各 4px；菜单宽度按内容（230–280）。

### 5.12 ToolTip
- 圆角 4、深色底（dark `#2C2C2C` / light `#FFFFFF`）、`caption` 字号、延时 400ms、只读。

### 5.13 ProgressRing / ProgressBar
- Ring：16（控件内）/ 28（空态）；Bar：高 4、圆角 2、`accent` 填充、底 `control.fill`。
- 传输队列：每项 = 文件名（省略中段）+ 方向图标 + 进度条 + 速率/剩余（`caption`）。

### 5.14 StatusBar
- 高 26；三段式；连接状态用 8px 圆点（`success`/`caution`/`critical`/`text.tertiary`）+ 文本。
- 信息不足时省略，不换行。

### 5.15 图标
- 采用 **Fluent System Icons（MIT）** 的 SVG path，内嵌为 Slint `Path`/`Image`，20×20 默认，线性 1.5px 描边风格。
- 首批清单（20 个）：新建、打开、保存、连接、断开、上传、下载、刷新、上一级、文件夹、文件、重命名、删除、权限、终端、隧道、命令、设置、搜索、更多、关闭、复制、粘贴、清屏。
- 状态点、连接态等用 `Rectangle` 绘制，不用图标字体（避免跨平台字体缺失）。

### 5.16 菜单栏（MenuBar）
- 位置与高度：标题栏下方、命令栏上方，独占 32px 一行；背景 `window.background`，底部 1px `layer.stroke`。
- 顶级菜单（5 个，沿用迭代 1 契约，不增不减）：**文件 File / 编辑 Edit / 会话 Session / 视图 View / 帮助 Help**。
- 菜单项：高 24、水平内边距 10、圆角 4；悬停 `subtle.hover`，展开态 `subtle.selected`；文字 `body` / `text.primary`。
- 弹出面板沿用 §5.11：圆角 8、行高 32、左侧 16px 勾选/单选列、右侧快捷键 `caption` / `text.secondary`、禁用项 36% 不透明、分隔线上下各 4px。
- 勾选态（如 View → Session Manager / SFTP Panel 等）：勾选标记 `accent`；单选态（如后端 fake / native-ssh）：圆点 `accent`。
- 键盘：`Alt` 显示助记符下划线，`Alt+F/E/S/V/H` 打开对应菜单，方向键移动、`Enter` 执行、`Esc` 关闭并回到原焦点（W3 补齐）。
- 交互契约映射（迭代 1 已定，实现时逐条核销）：

| 菜单 | 条目（摘要） |
|---|---|
| 文件 | 新建会话 `Ctrl+N` / 快速连接 `Ctrl+L` / 保存当前会话 `Ctrl+S` / ― / 设置… `Ctrl+,` / ― / 退出 |
| 编辑 | 复制 / 粘贴 / 清屏 `Ctrl+L` / 查找… `Ctrl+F` |
| 会话 | 断开 / 重连 / ― / 编辑选中会话… / 用当前会话更新选中项 / 删除选中会话… / ― / 已知主机… |
| 视图 | 会话管理器 ✓ / SFTP 面板 ✓ / 隧道 ✓ / 快捷命令 ✓ / ― / 后端：Fake ◉ · Native SSH ◉ |
| 帮助 | 关于 YShell |

- 不可用条目必须 disabled 并给出原因（如"拆分窗口（暂无会话）"），不允许静默失效。
- 无子菜单（与迭代 1 决策一致）；条目文案全部走 `@tr()`，快捷键字符串不翻译。

### 5.17 终端表面
- 8px 圆角 + 1px 描边（聚焦时描边 `accent`，非聚焦 `card.stroke`）；内边距 12。
- 配色方案（Dark=Campbell / Light=One Half Light）与 UI 主题解耦，默认跟随模式。
- 滚动条：4px 细条、`text.tertiary` 30%、悬停 60%，覆盖在终端右侧不占宽度。

---

## 6. 动效（Motion）

### 6.1 原则
1. **只解释状态变化，不做装饰**：高频操作（Tab 切换、滚动、键盘输入）不加过渡；危险操作不加抖动/闪烁。
2. **短**：状态类 ≤100ms，出现类 ≤167ms，面板/对话框 ≤250ms；超过 250ms 必须有明确理由。
3. **只动廉价属性**：`color` / `opacity` / `length`（位移、宽度）/ `angle`。软件渲染器下不做整窗淡入、背景位移。
4. **同一时刻最多 1 个大面积动画**（面板 / 对话框 / 遮罩），其余只能是局部（按钮、指示条、状态点）。
5. **尊重系统"减少动画"**：开启后非必要动画时长置 0，只保留状态色即时切换与进度反馈。

### 6.2 时长与缓动 token

| Token | 时长 | 缓动（Slint 写法） | 用途 |
|---|---|---|---|
| `motion.faster` | 100ms | `ease-out` | 控件状态色、hover/pressed、焦点环、指示条 |
| `motion.fast` | 167ms | `ease-out` | 菜单/浮层出现、箭头旋转、标签出现/关闭 |
| `motion.normal` | 250ms | `cubic-bezier(0.33, 0, 0.67, 1)` | 对话框出现、遮罩、面板折叠 |
| `motion.slow` | 333ms | `cubic-bezier(0.33, 0, 0.67, 1)` | 空态/页面级切换（少用） |
| `motion.instant` | 0ms | — | 减少动画模式下的兜底 |

规则：**进入用 `ease-out`（减速收尾），退出用 `ease-in`（加速离场），双向对称用 `cubic-bezier(0.33, 0, 0.67, 1)`**。
能力边界（Slint 1.9.2 实测）：`animate` 支持 `duration` / `easing` / `delay` / `iteration-count` / `direction`，缓动支持 `linear、ease、ease-in/out/in-out` 及 `quad、quart、quint、expo、sine、back、circ、elastic、bounce` 变体与 `cubic-bezier(a,b,c,d)`（参数必须为字面量）；配合 `states` + `transitions` 或独立 `animate` 使用。

### 6.3 逐场景清单

| 场景 | 动画属性 | 时长 / 缓动 | 实现要点 |
|---|---|---|---|
| 按钮 / 列表项 / 导航项 hover、pressed | `background`、`border-color`、文字色 | 100ms / ease-out | `states` + `transitions`；拖拽中的项不参与 |
| 选中态（导航项、标签页） | 背景色 + 指示条 `x`/`width` | 100ms / ease-out | 指示条用独立 `Rectangle` + `animate x, width` |
| 焦点环出现/消失 | `opacity` 0→1 | 100ms / ease-out | 键盘焦点才触发；鼠标点击不闪环 |
| MenuBar 菜单、右键菜单、下拉 | `opacity` 0→1 + `y` 位移 4px | 167ms / ease-out | 只做淡入 + 微上移；无缩放、无阴影动画 |
| ContentDialog（含遮罩） | 面板 `opacity` + 4px 位移；遮罩 `background` alpha 0→0.55 | 250ms / ease-out | 关闭 100ms / ease-in；缩放需 `transform` 支持（W3 验证，回退为纯透明度） |
| 面板折叠 / 展开 | 宽度 + 内容 `opacity` | 250ms / ease-out | **拖拽把手实时跟手、无动画**；软件渲染器下若掉帧则关闭宽度动画（只切终态） |
| 切换工作区标签 | 内容 `opacity` 0→1；指示条平移 100ms | 100ms / ease-out | 不做左右滑动过渡（避免大面积重绘） |
| 树 / 分组展开折叠 | 箭头 `rotation-angle` 0→90° | 167ms / ease-out | 行内容不做高度动画，直接显示 |
| 状态栏连接状态点 | `opacity` 0.4↔1 | 1.5s 无限 `alternate` | 连接中呼吸；连接成功/失败闪烁一次（167ms）后静止 |
| 传输进度 | 进度条宽度即时更新 | 无动画 | 速率/剩余时间用等宽数字，避免文本抖动 |
| 工具栏 toggle（SFTP/隧道/命令） | 背景色 + 文字色 | 100ms / ease-out | 与面板折叠动画并行时，面板动画优先 |
| Toast / 临时提示 | `opacity` + `y` | 167ms in / 100ms out | 自动消失时间由业务定（≥3s） |
| 终端光标闪烁 | 方波 | 530ms | 由终端渲染器负责，可配置关闭 |
| 终端滚动 | 即时 | — | 暂不做平滑滚动（后续可选） |
| 危险操作提示 | 无动画 | — | 只做确认弹窗 |

### 6.4 性能预算（软件渲染器）

1. 动画期间禁止触发布局重排：只动 `color` / `opacity` / 位移 / 宽度，不动字号、间距、换行。
2. `drop-shadow` 不参与动画（浮层阴影在出现动画开始前就已生效）。
3. 同一帧内最多一个大面积动画；面板折叠与对话框同时发生时，对话框优先。
4. 提供统一开关 `Theme.animations-enabled`（来自"减少动画"设置），所有 `animate` 的 `duration` 绑定到 `Theme.motion-*` token，便于一处关闭。

### 6.5 验证方式

1. **确定性单元测试**：用 `slint_testing::mock_elapsed_time()` 断言关键动画中间态（如导航指示条 100ms 时位于两值之间、对话框 250ms 后到达终态）。
2. **人工体验清单**：hover 跟手无延迟；菜单出现不闪烁；面板折叠无撕裂；减少动画模式下无过渡；连续快速切换 10 次标签无残影。
3. **截图**只验证终态（动画不参与截图对比）。

---

## 7. 多语言（i18n）设计

> 本轮新增的产品级需求。目标：中英首发、可运行时切换、后续加语言不改代码结构。

### 7.1 语言清单与策略
| 语言 | 定位 | 备注 |
|---|---|---|
| `zh-CN` | 主语言 | 与文档/目标用户一致，首发必须完成 |
| `en-US` | 源语言 | `.slint` 中的字面量即英文源串 |
| `ja-JP` / `ko-KR` | 预留 | 只加 `.po`，不改代码 |

- 默认 `跟随系统`（读取系统 locale，失败回退 `en-US`）。
- 设置页提供语言下拉，切换后**立即生效**（`select_bundled_translation` + 重新投影 Rust 侧字符串），无需重启。

### 7.2 字符串来源单一化（硬性规则）
1. `.slint` 内所有用户可见文本 → `@tr("...")`；占位符用 `{0}`/`{1}` 或具名语义参数，**允许译者调整语序**。
2. 复数形态用 `@tr("..." | "..." % count)`，禁止用 `+` 拼 `"s"`。
3. **禁止在 Slint 里拼句子**，例如现状 `text: "Backend: " + root.transport_backend_text`、`"scrolled back " + n + " line(s)"`，一律改为 `@tr("Backend: {0}", ...)` 或整句模板。
4. Rust 侧用户可见句子（状态/摘要/错误提示）逐步迁到 `.slint` 侧的 `@tr` 模板，Rust 只传**值**（主机名、路径、计数）。迁移清单在 W4 建立并逐条核销；技术数据（IP、路径、权限串、日志原文）不翻译。
5. `@tr` 的 context 写法是 **context 在前**：`@tr("Context" => "Message")`。
   源码依据：Slint parser 把 `=>` **之前**的字面量包成 `TrContext`，其后才是 msgid（实测 `@tr("File" => "MenuBar")` 会产出 `msgctxt "File"` + `msgid "MenuBar"`，完全反了）。
   用于区分同词不同义（`@tr("Session" => "Open")` = 会话列表里的"打开"）。
6. 文案中不出现拼接式标点：中文用 `，。：`，英文用 `, . :`，由译文决定，不写死在代码里。
7. 文案与快捷键分离：菜单/按钮文案走 `@tr()`，快捷键（`Ctrl+S`、`Alt+F4`）单独字段、不翻译；菜单助记符（`Alt+F`）只在英文下提供。

### 7.3 工程链路
```text
ui/**/*.slint  ──scripts/i18n/extract.sh──▶  translations/yshell-app.pot
                                            ├─ translations/zh-CN/LC_MESSAGES/yshell-app.po
                                            └─ translations/en-US/LC_MESSAGES/yshell-app.po
slint-build(with_bundled_translations)  ──▶  二进制内置（domain = crate 名 yshell-app）
slint::select_bundled_translation(lang) ──▶  运行时切换（立即重绘所有 @tr 文本）
```
- 提取脚本：`bash scripts/i18n/extract.sh`（幂等；`slint-tr-extractor` **不跟进 import**，所以脚本显式传入全部 `.slint`）。
- `.po` 头部必须是完整 gettext 头（缺字段会让 slint-build 的 `polib` panic）。
- 翻译文件随仓库提交，CI 校验：提取后 `.pot` 无未提交差异（防止漏标 `@tr`）。
- 缺失翻译回退到 msgid（英文），不显示 key。
- 日志、CLI 输出、错误码不翻译；CLI 可后续跟随 `LC_ALL`。

### 7.4 布局与排版规则（i18n 安全）
1. 文本容器默认 `overflow: elide` 或 `wrap: word-wrap`，**禁止**给文本控件写死宽度（现状多处 `width: 280px` 类的固定布局需改为弹性）。
2. 按钮宽度按内容自适应，`min-width` 只做下限（≥32 或按 WinUI 的 `max(32, content+24)`）。
3. 关键动作短文案（Connect/Save/Delete）优先；长句放正文而非按钮。
4. 伪本地化（`en-XA`：+40% 长度、加括号）作为 W4 验收项：不允许出现截断、重叠、换行错位。
5. CJK：行高按 §3.2（14/20 起步），中英混排不做字距调整；中文不适用斜体。
6. 数字/单位：文件大小用 `KB/MB/GB`（SI，1 位小数）；时间用 `YYYY-MM-DD HH:mm`（本地时区）；端口/权限/路径用 `code` 样式，禁止连字。

### 7.5 字体策略（含 CJK）

**Slint 1.9 限制（重要）**：`font-family` / `default-font-family` **只接受单一字体族**（逗号字体链无效），
软件渲染器也**没有逐字形回退**。因此字体族本身必须同时覆盖拉丁与 CJK，否则缺失字形渲染为空白
（后端默认的 DejaVu Sans 就缺 CJK 字形，中文界面曾整片空白）。

`Theme.font-ui`（`in-out`）由 Rust 启动时按平台写入**一个**字体族（`bootstrap.rs::resolve_ui_font`），
窗口通过 `default-font-family: Theme.font-ui` 应用；验收/调试可用 `YSHELL_UI_FONT` 覆盖。

| 平台 | UI 字体（单一值） | CJK |
|---|---|---|
| Windows | `Microsoft YaHei UI` | 系统自带 |
| macOS | `PingFang SC` | 系统自带 |
| Linux / WSLg（其它） | `Noto Sans SC` | 依赖系统安装（`fc-list :lang=zh` 可核查） |

- **跟踪项**：portable Linux 若目标机无 CJK 字体，需后续内置子集字体（常用 3500 字，约 1–2MB/字重）保证观感一致；在此之前只能依赖系统字体或提示安装。
- 终端字体沿用内置 `DejaVu Sans Mono`（单值），可升级为 `Cascadia Mono`（OFL）。

### 7.6 无障碍与 i18n 交叉项
- 所有图标按钮必须有 `accessible-label`（同样走 `@tr`）。
- 焦点顺序在语言切换后不变化。
- 对比度按 §8 校验（中英文一致）。

---

## 8. 无障碍

| 项 | 要求 |
|---|---|
| 对比度 | 正文 ≥4.5:1，大字号/图标 ≥3:1（深色 `#FFFFFF`/`#202020` = 16:1；浅色 `#E4000000`/`#F3F3F3` ≈ 13:1；accent 文字按 §3.1 组合校验） |
| 焦点可见 | 全部键盘可达控件显示 §3.5 焦点环，不可用 `focus: none` 移除 |
| 键盘 | Tab 顺序与视觉顺序一致；菜单支持 Esc 关闭、方向键移动（现状缺失，W3 补齐） |
| 命中区 | ≥32×32；相邻目标间距 ≥4 |
| 语义 | 关键控件补 `accessible-role` / `accessible-label` / `accessible-enabled` |
| 状态不靠颜色 | 连接态/错误态同时提供文本或图标（不只靠红绿） |

---

## 9. 实施路线（每轮独立可运行、可截图验收）

本轮交付：**W1 + W2**（决策见 §12）；W1 完成后先出深/浅截图评审，再进入 W2。

| 轮次 | 内容 | 交付 |
|---|---|---|
| **W1 主题地基** | 新建 `ui/theme.slint`（§3 全部 token）+ `build.rs` pin `fluent` 风格；窗口底色/字体/字号；替换 main_window 与各弹窗的硬编码颜色 | 颜色与字号全部 token 化，外观与现状同构但换色板；测试全绿 |
| **W2 主框架** | 菜单栏（MenuBar）、命令栏、导航左栏（NavigationView 风格）、TabView 标签条、右侧 Pivot + 卡片、InfoBar、状态栏 | 主窗口结构 WinUI 化 |
| **W3 控件库** | `ui/components/basic/`：Button/IconButton、LineEdit、ComboBox、CheckBox、Switch、ListItem、Flyout/Menu、Dialog、ProgressRing/Bar、ToolTip、ScrollBar；替换 std-widgets 用法 | 组件画廊 + 全量替换 |
| **W4 i18n 基建** | `@tr` 全量标注（312 处文本 + Rust 句式迁移）、`.po` 两种语言、语言设置项、伪本地化验收、CI 校验 | 中英切换即时生效，无截断 |
| **W5 打磨** | 图标集、动效、面板拖拽/自适应、快捷键、焦点环、高 DPI 检查、（可选）阶段 B 自绘标题栏 | 发布级观感 |

依赖关系：W1 → W2/W3 可并行 → W4 依赖 W2/W3 稳定（避免文案返工）→ W5。
每轮验收：`cargo test --workspace` 全绿、clippy `-D warnings`、`scripts/wsl-dev.sh dist` 出包、`xwd` 截图（深+浅）人评。

---

## 10. 风险与备选

| 风险 | 影响 | 对策 |
|---|---|---|
| 软件渲染器阴影/动画成本 | 掉帧 | 阴影只用于浮层；动画只做颜色与透明度；大面板动画可关闭 |
| 自绘标题栏（阶段 B）在 X11 不稳定 | 无法拖拽/缩放 | 阶段 A 保留系统标题栏（本规范默认）；阶段 B 单独一轮并保留开关 |
| std-widgets 与自绘控件混用产生两套观感 | 视觉不统一 | W1 起自绘控件逐步替换 std-widgets；剩余部分用 `Palette` 对齐 |
| `Palette.color-scheme` 与自定义主题不同步 | 明暗不一致 | 由 `Theme` 全局统一驱动（绑定 `Palette.color-scheme`），W1 先做验证 spike |
| CJK 字体缺失（Linux） | 方框字 | §7.5 字体策略 + 启动时检测并提示 |
| 一次性铺开导致回归 | 入口丢失 | 按 W1–W5 分批；每轮跑产品 smoke 测试 |

---

## 11. 验收标准（设计定稿即本节生效）

1. 深/浅两套主题下，主窗口 + 全部弹窗无硬编码色值（`grep '#[0-9a-fA-F]{6}' ui/` 仅命中 `theme.slint`）。
2. 主窗口在三档窗口宽度下无重叠/截断（占位测试 + 截图）。
3. `zh-CN` / `en-US` 切换即时生效；伪本地化无截断。
4. 所有可交互控件具备 hover/pressed/focus/disabled 四态（组件画廊逐项核对）。
5. `cargo test --workspace`、`cargo clippy -D warnings`、`dist` 构建全绿。
6. 对比度自动检查脚本通过（正文 ≥4.5:1）。
7. 动效：§6.3 场景清单逐项可用，关键动画有 `mock_elapsed_time()` 断言；开启"减少动画"后无非必要过渡。

---

## 12. 已确认决策（2026-09-26）

| # | 议题 | 结论 |
|---|---|---|
| 1 | 主题范围 | 默认跟随系统；**深色 + 浅色同期交付** |
| 2 | 实施节奏 | 先交付 **W1 + W2**，看效果验收后再排 W3–W5 |
| 3 | 强调色 | 预设色板选择；**Windows 支持"跟随系统强调色"**，Linux/macOS 回退到默认预设 |
| 4 | 标题栏 | **阶段 A 系统标题栏 → 阶段 B 自绘**（含标签页），自绘单独一轮并保留开关 |
| 5 | 首发语言 | **中文 + 英文**（en 为源语言，zh 为主语言），架构按 §7 单目录 + 运行时切换 |

默认沿用（如有异议请在 W1 评审时提出）：
- 三栏布局（左会话 / 中终端 / 右工具）不变，按 CommandBar + NavigationView 风格改造；
- 终端配色默认 Campbell（深）/ One Half Light（浅），与 UI 主题解耦；
- 图标采用 Fluent System Icons（MIT），首批 20 个见 §5.15；
- 最小窗口 880×600，默认 1440×900，面板宽度可拖拽（内存态）。
