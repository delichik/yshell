# N5 终端主题 + Folder Editor（设计）

状态：**待确认**；依赖：N5a-B（渲染链路）✅、C0（TerminalProfile/继承）✅、R2
关联决策：D15（主字体预设列表 + 备用字体自由配置）、D16（不做导入导出/光标形状）、D17（只对新开终端生效）、需求 §6
文件落位：`crates/yshell-terminal/src/render.rs`（调色板/字体选择接口，小改）、新 `ui/pages/folder_editor.slint`；`ui/pages/settings.slint`（Terminal 外观区）、`ui/pages/session_editor.slint`（Appearance/Terminal 覆盖）；`runtime/panels.rs`?→否 `runtime/sessions.rs`+`editor.rs`；`ui/main_window.slint`（文件夹右键入口最小接线）；`bootstrap.rs`

## 1. 配置与解析（C0 已备）
- `TerminalProfile`：`color_scheme` | 内联 `foreground/background/cursor/selection/ansi[16]`、`font_family/font_size/fallback_fonts`；三级：**会话 > 最近祖先文件夹 > 全局 > 内置默认**，逐字段继承。
- 内置配色首版 6–8 套：YShell Default、Dracula、Nord、Gruvbox Dark、Solarized Dark/Light、One Dark、Tokyo Night（清单可调）。

## 2. 渲染器接口（yshell-terminal 小改）
- `TerminalRenderer` 增加 `apply_appearance(palette: TerminalPalette, font: FontSelection)`：
  - 调色板替换固定常量（fg/bg/cursor/selection/ANSI16）；DF 色与 ANSI 索引仍按现有 `TerminalColor` 语义解析；
  - 字体：主字体 = `bundled DejaVu Mono` | `用户导入的字体文件`（路径）；fallback 列表 = 用户导入的文件（CJK 子集始终兜底）。
- 默认值不变；未配置时行为与当前一致。

## 3. UI
- Settings → Terminal 外观：方案选择（色块预览）、单色覆盖（十六进制输入）、字体（主字体下拉 + "添加字体文件…"rfd/路径 + 备用字体列表）、字号；实时预览小窗；"恢复默认"。
- Session Editor → Appearance/Terminal：字段三态（继承/显式）、显示 resolved 来源（"来自：文件夹 X / 全局 / 内置"）、单字段 Reset、整组 Reset All（P-014–P-016）。
- **Folder Editor（最小版）**：文件夹右键 → `Edit Folder Defaults…`（新页面）：Appearance/Terminal/Logging 三块（复用三态控件）。
- 生效时机：所有变更**只对之后新开的终端**生效（D17）；已开终端保持原样；`+`/新标签按当前 resolved 配置创建。

## 4. 验收
1. 同文件夹两个会话继承文件夹配色；其中一个会话覆盖前景后只影响它（重启保持）。
2. 切换配色/字体/字号只影响新开终端；重启后配置保持。
3. Folder Editor 三块可用，解析优先级有单测覆盖（C0 已有解析测试，补 UI 层断言）。
4. `cargo xtask lint/test` 全绿；e2e 双主题；截图 `dist/ui-checks/n5-*`（设置页/Folder Editor/会话覆盖/新旧终端对比/来源提示）。
5. 不做：配色导入导出、光标形状/闪烁、任意系统字体全量扫描（P-003/P-007/P-008 后置）。
