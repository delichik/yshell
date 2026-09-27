# N1 SFTP 紧凑化 + 双栏 + 拖放（设计）

状态：**待确认**；依赖：F0（后端能力）✅、R2、rfd（spike 后入库）、Slint 1.18 应用内 DragArea/DropArea（已具备）
关联决策：D9（面板内可折叠本地栏）、D10（远端→远端=下载再上传）、D14（页面只留导航 + ⋯）、D3（OS 拖放等 winit，不在本任务）
文件落位：重写 `ui/components/sftp_panel.slint`；新 `ui/components/local_pane.slint`；新 `crates/yshell-app/src/local_fs.rs`；`runtime/sftp.rs`；`bootstrap.rs`；`ui/text_formats.slint`（文案）

## 1. 紧凑单栏（先做）
- 头部：面包屑（窄宽折叠）+ `↑` + 刷新 + `⋯`；行操作全部进右键菜单（文件/目录/空白/多选四套，见 `context-menu-design.md` §4）；输入类操作走弹窗；删除/覆盖有确认。
- 列自适应：窄宽隐藏 Permissions/Modified（进 Properties 弹窗）；Size 降级 `-`；条目计数底部细行；错误内联 + 重试。
- 高度弹性（取消 420px 固定），内部滚动；传输队列改为可折叠抽屉（摘要一行 + 展开列表）。
- 多选（Ctrl/Shift）与批量下载/删除/chmod。

## 2. 本地栏与搬运（后做）
- 可折叠本地栏（D9）：本地目录列表（`local_fs.rs`：列目录/排序/导航/选择，只读元数据）；与远端栏**并排**在同一面板内，窄宽时可上下叠放。
- 搬运手势：
  - 系统文件对话框（rfd，降级路径输入）：上传到当前远端目录 / 下载到选定本地目录；
  - **应用内拖动**（Slint `DragArea`/`DropArea`，窗口内）：本地↔远端拖动 = 复制（默认）/ Ctrl 拖动 = 移动；多选拖动；远端内拖动 = 移动到目录；
  - 应用内剪贴板：Ctrl+C/Ctrl+V 在两侧之间复制（内部剪贴板，不承诺与系统资源管理器互通）。
- 远端→远端复制（D10）：右键"复制到…"→ 输入目标路径 → 下载临时文件再上传（走传输队列，进度可见）。
- 冲突策略：Ask/覆盖/重命名/跳过（F0 `TreeTransferOptions`）；递归目录传输用 F0 `upload_tree/download_tree`（进度/取消/部分失败报告接入队列 UI）。
- OS 拖入/拖出（D3）：本任务在远端栏挂一个 `DropArea` 占位（接线到上传流程），**真正 OS 拖放等 winit 0.31**；不做自研平台层。

## 3. 验收
1. 280px 宽/600px 高可用：无截断、无按钮换行；主要操作 ≤2 次点击可达。
2. 上传/下载（对话框与拖动两条路径）、多选批量、递归目录、冲突四策略、取消与重试均有可见反馈。
3. 远端→远端复制成功（中转）且进度正确。
4. 队列抽屉：pause/resume/retry/cancel/remove/clear completed。
5. `cargo xtask lint/test` 全绿；e2e 双主题；截图 `dist/ui-checks/n1-*`（紧凑头部/四套右键/多选/本地栏/拖动中/冲突弹窗/队列抽屉）。
