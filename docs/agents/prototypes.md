# UI 原型规则（强制）

任何涉及 **页面功能、布局、交互或视觉** 的前端工作，必须先有可打开的原型，再改 `ui/` 实现。

**视觉**：原型是 **WinUI 3 / Fluent 2 设计稿**（尺寸、位置、控件观感可验收），对齐 [`ui-winui3-design-language.md`](../product/ui-winui3-design-language.md)。共享样式源：[`_theme.css`](../prototypes/_theme.css)、[`form.css`](../prototypes/form.css)、[`_chrome.css`](../prototypes/_chrome.css)、[`_proto.css`](../prototypes/_proto.css)；组件真源 [`components/`](../prototypes/components/)（**iframe + query** 复用，见 [`components.html`](../prototypes/components.html)）；样板 [`forms.html`](../prototypes/forms.html)、[`chrome.html`](../prototypes/chrome.html)。

HTML 设计稿可用假桌面 + `backdrop-filter` **近似 Mica**。Slint `renderer-software` **不能**做真实云母；实现侧用实色分层代替。设计稿条带高度为 Fluent **32**（title / menu / tab）；实现 [`Theme.bar-height-menubar = 32`](../../ui/theme.slint) 已对齐。

## 目的

原型回答：

1. **功能**：页面上有什么控件、状态、入口、空态 / 禁用 / 危险操作。
2. **布局**：区域划分、主次层级、面板/弹窗相对关系（含 Fluent 组件模型）。
3. **交互**：点击 / 双击 / 右键 / 拖拽 / 快捷键触发的流程与页面跳转（用简单状态切换即可）。
4. **视觉基线**：token、圆角、accent、五态、壳层组件与 **准确几何**（窗档、条带、控件行高/列宽）。

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

**浏览入口** [`docs/prototypes/index.html`](../prototypes/index.html)：左侧原型列表，右侧 **iframe** 加载选中项。可用顶栏「新标签打开」看完整页。

### 多文件拆分（强制）

- **每个弹窗必须有单独的原型文件**  
  工具窗、确认框、编辑子弹、向导子步等，一律在同功能目录下用**独立 HTML** 展示。  
  **禁止**把完整弹窗布局只嵌在父页里当「附带 div」交差。
- **同一页面若区域明显独立且较复杂，也要拆文件**  
  对齐 [`main-shell/`](../prototypes/main-shell/)：`index.html` = **整壳拼装**；独立区用切片板。
- **可再套一层分组**  
  仅当同一产品壳分区很多时（`main-shell/panels/`、`settings/sections/` 等）。
- **互链必达**：父页按钮 / 列表行 / 右键项用 `href` 跳到对应子文件。

## 工作流

```text
需求 / 缺口澄清
    → 在 docs/prototypes/ 新建或打开对应原型（设计稿）
    → 直接改原型，展示预期布局、交互与 WinUI3 观感
    → 与用户确认原型
    → 再改 ui/*.slint 与相关 Rust 接线
    → 实现若偏离原型，先改原型再改实现（或同步改两边）
```

### 硬性要求

- **UI 设计阶段 = 改原型**：讨论「做成什么样」时，默认打开并修改 `docs/prototypes/`，用浏览器展示预期效果。
- **无原型不开工实现**：新建页面、大改布局、改交互路径前，须已有对应原型。
- **实现跟随原型**：Slint 对齐原型中的结构、交互与 WinUI3 视觉基线；设计语言与原型冲突时**以原型为准**。
- **范围仍受 status 约束**：不得把「本版不做」画成可点通功能而不标注。
- **参考文档不是设计稿**：契约 / archive 仅供参考。发现原型误抄愿景，**先改原型**。

## 原型写法约定

- 引用共享 CSS（`_theme.css` ← `form.css` / `_chrome.css` / `_proto.css`）；页内只留该页布局，**禁止**线框 `#111` 色板或本地 `.btn` / `.nav-item` / `.flyout` / `.titlebar` 分叉。
- **共享面一律组件化（目的）**：凡多页会出现的 UI（壳、条带、按钮、表单项、注释、舞台脚手架）只维护一份 CSS + 一份可 iframe 的 HTML 真源；功能页只留**本页独有**布局与文案。目录 [`components.html`](../prototypes/components.html)。用 query（`data-q` / `data-q-href` + `_embed.js`）改文案/链接，不要复制 DOM。
- **禁止**页内重写：`.btn` / `.input` / `.form-row` / `.titlebar` / `.nav-item` / `.legend` / `.callout` / `.anno-tag` 等已进共享层的类。
- 用文字标签标明区域名（如 `SessionSidebar`、`TabBar`）；title / 文案不要写 `wireframe`。
- 交互用原生控件或极简 JS。
- 多状态建议提供状态切换条。
- 危险操作：工具窗 + critical 文案按钮；禁止打字确认。
- **通用注释**：样式 [`_anno.css`](../prototypes/_anno.css)；切片 [`anno-tag`](../prototypes/components/anno-tag.html) / [`anno-callout`](../prototypes/components/anno-callout.html) / [`anno-legend`](../prototypes/components/anno-legend.html)；规则页 [`annotations.html`](../prototypes/annotations.html)。格式 **`A · Short name`**。结构说明只在窗外 / `.anno-layer`；**不得**进菜单/表单/pane 文档流。
- **表单控件**：[`form.css`](../prototypes/form.css) + [`components/buttons|form-fields|method-seg`](../prototypes/components/) + [`forms.html`](../prototypes/forms.html)。窄标签用 `.form-row.lbl-100` / `.lbl-110`。
- **壳层**：[`_chrome.css`](../prototypes/_chrome.css) + [`components/tool-window|nav-pane|menubar|tabbar|…`](../prototypes/components/)。Settings / Session Editor：NavigationView 高 40 + accent 轨；TabView 底指示；Flyout；InfoBar。
- **一窗一条提交/关闭栏**：`Close` 或 `Cancel | Save`，禁止叠两层关闭。
- **嵌套流程冻结上级**：模态子窗 + scrim。
- **右键菜单必须可演示**。
- **清单型管理器**：左清单极简（名 + 一行标签）；行动作走右键；顶栏只放全局动作。
- **Settings / Session Editor**：分区内容区不要再放大标题；分区内禁止 Save；整窗脚 `Cancel | Save`。
- **按钮/菜单文案不加省略号**（进度/截断除外）。
- **列表添加按钮在列表上方**。
- **所有可点按钮都要有 `title`**；工具条图标钮用 Tabler outline（`_icons.css`），禁止 Unicode。
- **工具窗标题栏**：标题 + **仅关闭**（不要主窗 `_` / `□`）。
- **快捷键必须可见**；跳转路径必须可点。
- **主窗按 N3** 拆在 `main-shell/`。
- **原地重命名** 用输入框钉在画面上。
- 与契约冲突时：**改原型或 status**。

## 尺寸与滚动（强制）

原型必须能验收 **真实占位**，不能用「跟着视口挤扁」的假布局。

1. **对标固定尺寸（设计稿 = Fluent）**  
   条带：title / menu / tab **32**、status **28**。`tool-window-padding` **28**。  
   **弹窗只有三档**：compact `560×340` / standard `720×520` / wide `1100×700`。主窗 `1440×900`。切片尺寸见 README（不是弹窗档）。
2. **禁止用视口百分比压扁对话框**  
   舞台窄时 `overflow-x: auto`，对话框 `flex-shrink: 0`。
3. **整窗不要无故滚动**  
   内容顶对齐 → spacer → 页脚固定；仅清单等明确区域可滚。
4. **控件行高对齐**  
   `--ctrl-h` = 32；行高 = **n × `--ctrl-h`**。
5. **表单控件列宽统一（`--field-w: 420`）**  
   唯一样式源 `form.css`；旁路钮 `.ctl-with-btn` 是唯一可超出 420 的行内例外。布尔用开关 + 上下文文案。
6. **多状态不改变窗体尺寸**。

## 与 ui-sketches 的关系

| | `docs/prototypes/` | `docs/product/ui-sketches/` |
|---|---|---|
| 用途 | **现行 UI 设计稿**（功能 / 布局 / 交互 / WinUI3 观感） | 历史或补充 mockup |
| 是否强制 | **是** | 否 |
| 复杂度 | 高保真几何 + Fluent 视觉 | 可参考 |

新工作以 `docs/prototypes/` 为准；sketches 不能替代本目录。
