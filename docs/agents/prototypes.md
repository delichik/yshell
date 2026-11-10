# UI 原型规则（强制）

任何涉及 **页面功能、布局或交互逻辑** 的前端工作，必须先有可打开的原型，再改 `ui/` 实现。  
视觉抛光（复杂 CSS、动效、材质）**不是**原型目标；那些属于实现阶段与设计语言文档。

## 目的

原型只回答三件事：

1. **功能**：页面上有什么控件、状态、入口、空态 / 禁用 / 危险操作。
2. **布局**：区域划分、主次层级、面板/弹窗相对关系。
3. **交互**：点击 / 双击 / 右键 / 拖拽 / 快捷键触发的流程与页面跳转（用简单状态切换即可）。

不要求：像素级还原、Fluent 材质、复杂动画、精致阴影、品牌插画。

## 存放位置

所有现行原型放在：

```text
docs/prototypes/
  README.md          # 目录索引
  <feature>/         # 按功能或页面命名
    index.html       # 主窗 / 主流程（可直接打开）
    modals.html      # 例：该功能下的子弹窗（可多个独立文件）
    <region>.html    # 例：同页内明显独立的复杂区域
    <group>/         # 可选再套一层（仅当同壳分区较多时）
      a.html
      b.html
```

命名用短英文或现有产品代号（如 `session-editor`、`known-hosts`）。  
一个功能一个子目录；**默认同目录多 HTML**（`known-hosts/index.html` + `modals.html`），不要每个子页再开空壳目录。  
索引见 [`docs/prototypes/README.md`](../prototypes/README.md)。

**浏览入口** [`docs/prototypes/index.html`](../prototypes/index.html)：左侧原型列表，右侧 **iframe** 加载选中项（含子窗如 `modals.html` / `login-step-keys.html`）。可用顶栏「新标签打开」看完整页。

### 多文件拆分（强制）

- **每个弹窗必须有单独的原型文件**  
  工具窗、确认框、编辑子弹、向导子步等，一律在同功能目录下用**独立 HTML** 展示（如 `modals.html`、`login-step-keys.html`）。  
  **禁止**把完整弹窗布局只嵌在父页里当「附带 div」交差——父页最多保留入口链接 / 极简触发说明，弹窗本体在独立文件里钉尺寸与页脚。
- **同一页面若区域明显独立且较复杂，也要拆文件**  
  对齐 [`main-shell/`](../prototypes/main-shell/)：`index.html` = **整壳拼装**；独立区用切片板（board），**不要**每个分区复制一整窗。例：`session-editor/sections/authentication.html`、`settings/sections/terminal.html`。弹窗仍落在功能根目录（如 `login-step-keys.html`、`known-hosts/modals.html`）。
- **可再套一层分组**  
  仅当同一产品壳分区很多时：如 `main-shell/panels/`、`main-shell/workspace/`、`session-editor/sections/`、`settings/sections/`，组内仍是多 HTML。不要散落在 `docs/prototypes/` 顶层，也不要再深套。
- **互链必达**：父页按钮、列表行、右键项用 `href` 跳到对应子文件（可带 hash）；子文件可链回父页。参考 `known-hosts/`、`main-shell/`。

## 工作流

```text
需求 / 缺口澄清
    → 在 docs/prototypes/ 新建或打开对应原型
    → 直接改原型，展示预期布局与交互（多状态用按钮/锚点切换即可）
    → 与用户确认原型
    → 再改 ui/*.slint 与相关 Rust 接线
    → 实现若偏离原型，先改原型再改实现（或同步改两边）
```

### 硬性要求

- **UI 设计阶段 = 改原型**：讨论「做成什么样」时，默认打开并修改 `docs/prototypes/` 下文件，用浏览器展示预期效果；不要只改口头描述或只画 ASCII。
- **无原型不开工实现**：新建页面、大改布局、改交互路径前，须已有对应原型（或本轮先补齐再写 Slint）。
- **实现跟随原型**：Slint 实现应对齐原型中的结构与交互；token / Fluent 观感可参考设计语言，但**设计对错以原型为准**。
- **范围仍受 status 约束**：原型不得把「本版不做」画成可点通功能而不标注；禁用 / 空态要在原型里标出来。
- **参考文档不是设计稿**：[`yshell-ui-interaction-contract.md`](../product/yshell-ui-interaction-contract.md)、archive、旧规格仅供参考。发现原型误抄了参考愿景（例如从未拍板的「最多 4 pane」），**先改原型**，再改实现 / 计划；不要按契约「补缺口」。

## 原型写法约定

- 单文件或少文件 HTML + **最少** CSS（灰阶块、边框、固定区域即可）。
- 用文字标签标明区域名（如 `SessionSidebar`、`TabBar`、`SFTP Local`）。
- 交互用原生控件或极简 JS：`button` 切换 `hidden` / 换「状态面板」即可。
- 多状态建议在页内提供状态切换条，例如：`默认 | 加载中 | 空 | 错误 | 禁用项可见`。
- 危险操作、二次确认、阻断弹窗：用简单叠层框表达，不必动画。
- 结构说明 / 分区注解写在**窗口外**（侧栏 callout 或页底 `.anno`），不要塞进对话框抢高度。格式统一 **`A · Short name`**；共用样式 [`_anno.css`](../prototypes/_anno.css)。通用 **chrome / spacer / footer** 只在 [`annotations.html`](../prototypes/annotations.html) 说明，各页勿重复占字母。
- **注释不得参与布局**：窗内注释只能放在 `.anno-layer`（`position:absolute; pointer-events:none`）或已有的 `.tag` 叠加层；**禁止**把 `.note` / `.anno` / 说明文字插进菜单、表单、pane、列表或 footer 的文档流——那会改变组件尺寸/位置。窗口外的 legend / callout 不受此限。
- **表单控件**：引用根 [`form.css`](../prototypes/form.css)；对照样板 [`forms.html`](../prototypes/forms.html)。开关 / 行高 / 按钮 / Method / `.ctl-with-btn` 的样式与动画以该页为准，禁止页内再抄一套。
- **一窗一条提交/关闭栏**：页脚要么是 `Close`（浏览型管理器），要么是 `Cancel | Save`（编辑型），**禁止**在 `Cancel|Save` 外再叠一层窗级 `Close`。
- **嵌套流程冻结上级**：Import / Export / Deploy / 二次确认等不是父窗内嵌表单块，而是叠在父窗上的模态子窗（scrim + 子对话框），父窗不可点；子窗自带 Cancel | 主操作。
- **右键菜单必须可演示**：关键区域（会话树、标签、终端、SFTP 行等）要能右键弹出菜单；菜单项写清文案，禁用项标出。
- **清单型管理器（known-hosts / private-keys / proxies / terminal-styles 等）**：左清单极简——**名称 + 一行极短标签**即可（如 Built-in / Custom、host:port、指纹缩写）；**不要**把字体/字号/色块/协议等详情塞进列表（详情只在右栏）。行级动作走**右键菜单**，不要在列表下堆多钮行。顶栏只放全局动作（New / Add / Import / Export）。点选即载入右栏；Import/Export / 步骤编辑等弹窗见上文「多文件拆分」，**不画父窗副本**。左右栏**不要**再加「Managed / Edit / Details」等分区标题（窗标题已够）。
- **Settings / Session Editor 分区**：左导航（或切片 tools）已标明当前页；分区内容区**不要**再放 `General` / `Authentication` 等 section 大标题。可 Inherit 的页只保留顶栏 Inherit 开关。
- **Settings / Session Editor 提交**：分区内**禁止** Save（Reset / 管理 / Browse 等旁路动作可留）；只用整窗页脚 **Cancel | Save**。**Save = 保存并关闭**；Cancel / × / Esc 同弃稿关闭。嵌套子窗（Login Keys/Wait、Import）自带 Cancel|Save，不受此条约束。
- **按钮/菜单文案不加省略号**：可见文案与 `title` 不要尾部 `…` / `...`（不要用省略号表示「会打开对话框」）。进度/截断类文案（Connecting、指纹缩写）除外。
- **列表的添加按钮必须在列表上方**：固定在清单**之上**，不要放在列表下方——否则列表变长会把按钮顶到视口外或位置跳动。清空类次要动作可与 Add 同行（仍在上方），或走右键。  
- **所有可点按钮都要有悬停提示**：每个 `<button>` / 可点 `.btn`（含文字钮、图标钮、主按钮、危险钮）都设 **`title`**（必要时再加 `aria-label`）。图标钮 `title`/`aria-label` 写全称；文字钮 `title` 与可见文案一致或略完整。正面案例：Terminal Styles 字体旁 plus 图标 → 悬停显示 Add Font File。右键菜单项同样建议有 `title`。  
- **工具栏 / 旁路图标钮**：清单与表单旁路的添加、删除、复制、清空、管理、Browse、导入、导出、生成等，用 **Tabler outline**（`<span class="i i-plus|copy|clear|trash|close|manage|browse|import|export|generate">`，样式 `_icons.css`；资源 `_icons/*.svg`），**禁止** Unicode，也**禁止**在工具条写「管理」「Import」「Generate」等可见文案——全称只进 `title` / `aria-label`。删除类图标钮加 **`btn-danger`**（红描边 + 图标随 `currentColor`），与文字 Delete 同套。  
  - 同一工具条要区分**多种**添加时才保留短文案（如 `Keys` / `Wait`）。  
  - **右键菜单**仍用文字项（Edit / Duplicate / Delete），不要改成纯图标。  
  - 对话框**页脚主按钮**（Cancel | Import / Generate / Export）保留文字；工具条与旁路一律图标。
- **工具窗标题栏**：对齐 `ToolWindowChrome`——标题 + **仅关闭**（`i-close`）；**不要**照搬主窗的 `_` / `□`。Esc / 关闭与页脚 Close 或 Cancel 同路径。
- **快捷键必须可见**：菜单项右侧标 `Ctrl+…` / `F5` 等；页外可附键盘速查；关键按键用 `keydown` 演示跳转或状态切换（至少覆盖契约 §2 常用项）。
- **跳转路径必须可点**：打开其它工具窗/确认框的动作，用链接或按钮跳到对应原型文件，并写清路径，例如 `→ session-editor` / `→ session-editor/sections/login-step-keys.html` / `→ compact-confirm (quit)`。
- **主窗按 N3 拆原型（`main-shell/`）**：根上 `index.html`（整壳）+ `tool-dock.html`；`panels/`（sessions / transfers / tunnels / commands）；`workspace/`（tab-bar / terminal / sftp / quick-connect / split / sync-input）。**禁止**再画固定 Left Sidebar / RightDock Tab。N9 用 banner/chip，禁止 KEYS 打字确认（D23）。
- **原地重命名 / 新建名**：标签 Rename Tab、**SFTP** Rename / New Folder、Sessions 树 **Session** 改名均为**原地输入框**（插行再编辑），不是另开对话框；`Enter` 或失焦确认，`Esc` 取消（新建取消则移除该行）。对应分区原型必须用状态钮（或默认演示态）把输入框钉在画面上。Folder 的 New Folder / Rename / Delete 以 status「本版不做」为准（菜单 disabled）。不再提供 `sftp-entry` / `new-folder` 短表单（已删除）。
- **禁止打字确认破坏操作**：删除、清空、覆盖等高危动作**永远不要**让用户手输 `CLEAR` / `DELETE` / 确认词。用清晰文案 + 明确的破坏性按钮（及可选二次确认窗）即可；主按钮可标 critical，安全默认焦点在 Cancel。
- 与交互契约等参考文档冲突时：**改原型（设计）或 status（范围）**；不必先改契约。契约随后可作参考性同步，但不是开工前提。

## 尺寸与滚动（强制）

原型必须能验收 **真实占位**，不能用「跟着视口挤扁」的假布局。

1. **对标 Theme 固定尺寸**  
   工具窗 / 主窗区域用 `ui/theme.slint` 里的 token 写死宽高，例如 standard 工具窗 `720×580`、标题栏 `40`（`bar-height-menubar`）、`tool-window-padding` `28`。在 HTML 顶注释或 CSS 变量里写明对应 token 名。  
   **弹窗只有三档**：compact `560×400` / standard `720×580` / wide `1100×820`；新增弹窗不得自造尺寸。确需特殊尺寸时在 meta 与 README 标注原因。`960×560`、`800×640` 等是工作区 / 内容切片尺寸，**不是弹窗尺寸**。
2. **禁止用视口百分比压扁对话框**  
   不要 `height: min(580px, 78vh)`、`width: min(720px, 100%)` 这类会改变产品几何的写法。舞台比对话框窄时：舞台 `overflow-x: auto`，对话框 `flex-shrink: 0` 保持设计尺寸。
3. **整窗不要无故滚动**  
   对话框 body 用列布局：内容顶对齐 → flex spacer 顶开页脚 → 页脚固定。只有清单、终端输出等**明确可滚区域**设 `overflow: auto` 与 `max-height`；禁止外层再套一层整页滚动导致双滚动条或脚部被挤飞。
4. **控件行高对齐**  
   按钮 / 输入框统一高度 `--ctrl-h`（32）= **最小行高单位**；**表单行高必须是最小行高的整数倍**（1× 默认行，2× 清单 `--row-h`，4×/5× 多行编辑与预览）。禁止 48 / 56 / 120 / 140 等非整倍数。
5. **表单控件列宽统一（`--field-w: 420`）**  
   **唯一样式源**：[`form.css`](../prototypes/form.css)；**验收样板**：[`forms.html`](../prototypes/forms.html)。所有原型的控件 class / token / 开关动画必须与该页一致，禁止本地另写一套 `.form-switch` / 行高 / 按钮。`session-editor/form.css` 仅 re-export 根文件。  
   **编辑表单**与**展示表单**里的普通控件、只读展示块（公钥预览、路径只读框、终端预览等）：`max-width: 420`（管理器右栏同此）。窄字段用专用类（Port `100`、Icon 下拉 `220`）。  
   **唯一允许在表单行上超出 420 的**：旁路动作钮——`.ctl-with-btn`。文案：**字段旁**无歧义时仅「管理」；**独立入口**用全称（如「管理终端样式」）；选文件用 Browse。细则见 [`forms.html`](../prototypes/forms.html)。Notes 不得随意加宽。  
   **展示表单（内嵌 part）**：如 Private Keys / Proxies / Terminal Styles **右栏详情**——字段**不缩进**（不要强行套 `form-row` 标签列）；但宽度规矩同上，只读块也 ≤420，避免与输入混排时一宽一窄。  
   **表单行高（编辑 + 展示）**：`form-row` / `detail-row` 统一 `min-height: --ctrl-h`（1×）；行高 = **n × `--ctrl-h`**。高值列用 `--field-h-md`（4×）/ `--field-h-lg`（5×）；清单 `--row-h`（2×）。始终 `align-items: start`。左 label 固定 `padding-top: calc((ctrl-h − 1.2em) / 2)`——最小高度时与单行控件视觉居中；值列变高时 **上边距不变**（勿再靠 `.top` 改 label pad）。  
   **单字段子窗**（Known Hosts Add / Import / Export 等只有一项输入）：**不适用**表单列——无左侧 label、无缩进；控件顶满内容区（窗标题 + placeholder 即可，勿再堆教学 hint）。  
   **布尔字段**：必须 `form-row`（左短 `lbl` | 右 **开关 + 上下文文案** On/Off、Enabled/Disabled、Use…）；**禁止** checkbox 与游离「☐ + 长句子」。页级 Inherit / pane 接收同用开关。口令类不要 Remember。  
   **用户向 hint**：可以有，但只写**非显而易见的产品行为**（空值语义、边界/副作用）。禁止 SSH/OpenSSH 常识教学、禁止复述标题或开关文案；受众是熟练用户，不是小白。格式/示例优先放 placeholder。  
   **不要套 `--field-w` 的**：  
   - CRUD **清单**（Tunnels / Login Script 步骤列表 + 上方工具条；管理器**左**清单）→ 所在栏全宽；  
   - 上述**单字段子窗**。  
   Login Keys 正文 / Tokens、Import 粘贴 PEM 等大编辑区仍 ≤420（可加高，不可加宽）。
6. **多状态不改变窗体尺寸**  
   切换 Password / Key / Challenge、显隐错误条时，外框宽高保持不变（与产品「不按方法动态改窗」一致）；多出来的空间由 spacer 消化。

## 与 ui-sketches 的关系

| | `docs/prototypes/` | `docs/product/ui-sketches/` |
|---|---|---|
| 用途 | 功能 / 布局 / 交互验收 | 历史或观感向 mockup |
| 是否强制 | **是**（设计与改交互必经） | 否 |
| 复杂度 | 刻意低保真 | 可较完整视觉 |

新工作不要把高保真稿当成原型替代品；需要视觉参考时可链到 sketches，但仍须维护低保真原型。
