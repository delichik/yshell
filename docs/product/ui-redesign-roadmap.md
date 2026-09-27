# UI 重设计路线图

状态：迭代 1–3 已完成（基线见 `ui-redesign-remaining-work.md`）；本文件规划后续迭代。

并行说明：WinUI3 设计语言重构（`ui-winui3-design-language.md` /
`ui-winui3-implementation-notes.md`）与本路线图并行推进；剩余事项、协作规则与交付检查表
统一登记在 **`ui-redesign-remaining-work.md`**，新任务请先读那份文档。

## 1. 用户反馈与总体目标

反馈：

- 已知遗留（菜单状态感知、终端右键项不全、快捷键、文件选择器）都要解决
- SFTP 不是正常 SFTP 工具的设计（现在是文本 + 输入框，没有文件列表/列/选择/导航）
- 终端不是真 TTY（输入是"输一行反应一下"，没有按键级交互、颜色、光标、滚动）
- 布局不能调整（左右面板固定宽度）、不能适应窗口缩放

原则：

1. **真实控件**：列表就是列表、终端就是终端，不再用"字符串投影"糊功能
2. **层级入口**：高频在工具栏，中频在菜单，区域操作在右键菜单，破坏性操作走确认弹窗
3. **状态感知**：不可用的菜单项/按钮必须 disabled 并给出原因
4. **可调整布局**：面板可拖拽、可折叠，窗口缩放时正确重排
5. **可达性不回归**：任何改动后既有能力必须有入口，测试不回归

## 2. 迭代计划

### 迭代 2：真终端（本次）
- 按键级输入（含 Ctrl/Alt/方向键/F 键/Shift+Tab，UTF-8/CJK）
- 像素级渲染器（ANSI 颜色、粗体、下划线、反色光标、宽字符），字体内置
- scrollback 滚动（滚轮/Shift+PgUp/PgDn/Home/End）与回到底部
- 鼠标选择 + 复制（Ctrl+Shift+C / 右键 Copy），选择高亮渲染
- 终端右键菜单补全（Copy/Paste/Select All/Clear/Find）
- 移除行输入框，终端表面成为焦点控件（点击聚焦、Ctrl+C 无选区时发 SIGINT）

### 迭代 3：真 SFTP
- 结构化目录列表模型（名称/大小/修改时间/权限/类型）
- 列头排序、隐藏文件切换、面包屑 + 可编辑路径
- 行选择（单击/方向键/多选）、双击进目录或下载、右键菜单
- 工具栏：Up / Refresh / Home / New Folder / Upload / Download / Rename / Delete / Chmod / Edit
- 操作弹窗（New Folder / Rename / Chmod / Delete 确认 / 上传下载目标）
- 与传输队列联动

### 迭代 4：布局、快捷键与状态感知
- 左右面板宽度拖拽（200–420 / 280–560）、折叠、最小窗口 1120×720、默认 1440×900
- 窗口缩放自适应与面板比例记忆（内存态即可，后续持久化）
- 全局快捷键：Ctrl+N/O/S/F/L/B/Q、F5、Ctrl+,
- 菜单/按钮状态感知（连接中/已连接/无会话/无选区/无 SFTP 等）
- Settings 页补全（外观/日志/布局重置），对话框统一视觉

## 3. 验收方式（每轮相同）

1. `bash scripts/wsl-dev.sh test` 全绿，既有测试不回归
2. `bash scripts/wsl-dev.sh check` 的 clippy 通过
3. `bash scripts/wsl-dev.sh dist` 产出 `dist/linux-x86_64/yshell`
4. `xwd` 截取真实窗口存 `dist/linux-x86_64/ui-check.png`，附上验证说明
5. 每个迭代独立可运行，用户试用后再进入下一轮
