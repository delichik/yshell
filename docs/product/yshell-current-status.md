# YShell 本 build 产品现状

更新时间：2026-10-29  
性质：**唯一现行产品范围文档**（有什么 / 砍了什么 / 还缺什么 / 仍生效的决策）

冲突时：本文 > 实现与单测 > [`yshell-ui-interaction-contract.md`](./yshell-ui-interaction-contract.md) > [`archive/`](./archive/README.md) 历史稿。

---

## 1. 已交付

| 域 | 概要 |
|---|---|
| 标签 / 分屏 / N8 批量关闭 | 多标签、分屏、Close Other/Left/Right/All |
| 快速连接 N2 | QC 页、历史、Ctrl+O；顶部 QC 输入条已移除 |
| 面板停靠 N3 | 左右 Dock、上下分栏、布局持久化 |
| SFTP N1 | 双栏、应用内拖放、队列、冲突、批量删除、rfd；可拖入远端 |
| 密钥 / 认证 N4 | 认证弹窗、私钥页、主机密钥页 |
| 终端主题 N5 / N5a | 继承链、配色、字体；物理像素光栅 / CJK |
| 日志 UI N6 | 右键启停、REC、弹窗、rfd |
| 多窗口 N7 Phase 1 | 菜单迁窗、关窗确认 |
| 同步输入 N9 | All / Visible / 停止；chip；不弹 KEYS 确认（D23） |
| 本地终端 | 本机 shell 标签 |
| 基建 | Slint 1.18、R1/R2 拆分、L2 Esc/遮罩、对比度 xtask、真实 SSH only |
| MCP（GUI 控制） | `yshell --mcp` stdio 桥接运行中的 GUI；`request_control` 用户授权锁；list/input/output/connect/disconnect/duplicate |

---

## 2. 本版明确不做

UI 为 disabled + tooltip、空态，或决策延后。**不是待补闭环。**

| 项 | 说明 |
|---|---|
| Quick Commands 内容 | Dock 仅空态；View 仍可显隐面板 |
| Import / Export Config | File 菜单无入口；空白区 Import disabled |
| Expand All / Collapse All | 空白区 disabled |
| 文件夹：New Folder / Rename / Delete / Connect All | 仅 New Session Here、Edit Folder Defaults |
| Session Editor：SFTP / Advanced 分区 | nav disabled |
| Settings：Shortcuts；wipe secrets / 重置布局 | Advanced 只展示路径等 |
| 会话栏 RECENT/ACTIVE、`+`/`…` | 已删（D25） |
| Fake SSH / 后端切换 | 已删（D26） |
| OS 拖出到资源管理器 | 等 winit 0.31 + Slint（D3） |
| 标签拖出成新窗 | 等上游；现仅菜单迁窗（D5） |
| 主题导入导出 / 光标形状 | 首版不做（D16） |
| 密钥生成；仅路径引用私钥 | 不做（D20 / D19） |

---

## 3. 真缺口（仍露出、未闭环）

| 项 | 缺口 |
|---|---|
| SSH 端口转发 | 可配 + Dock Start/Stop；runtime 不 provision |
| 会话 Proxy 真连 | Manager/探测可用；native SSH/SFTP 拒绝 proxy |
| Secret Store 解锁 UX | 仅 `YSHELL_MASTER_PASSWORD`；无 in-app 主密码向导 |
| N9 Visible | 分屏已有；目标集仍等同 All；文案可能仍写「等分屏」 |
| N6 遗留 | R-107 存为会话默认；日志大小上限 |
| `ui.new_tab_mode` | schema 有；`+` 忽略 session-editor；Settings 无项（相对 D11） |
| `ui.multi_window_enabled` | schema 幽灵字段，未接线 |
| L1 状态栏 i18n 方案 | 方案文档未落盘 |
| L4 fmt + cargo-deny 冻结 | 等独占窗口 |

---

## 4. 仍生效的产品决策（原 requirements §12）

| # | 决策 |
|---|---|
| D1 | Slint 1.18.1；MSRV 1.92 |
| D2 | 应用内拖放用原生 DragArea/DropArea |
| D3 | OS 拖入/拖出等 winit 0.31 + Slint；不自研平台层 |
| D4 | 文件对话框用 rfd，失败降级路径输入 |
| D5–D8 | 多窗：菜单迁窗；关窗确认断开；布局全局一套；不做自动开窗 |
| D9–D10 | SFTP 可折叠本地栏；远端→远端=下载再上传 |
| D11–D13 | `+` 默认可切 QC/编辑器；历史上限 20；快速链接只存目标（链接入口后来下线，以实现为准） |
| D14 | SFTP 页只留导航 + ⋯ |
| D15–D17 | 预设主字体 + 备用链；主题只影响新开终端；不做导入导出/光标 |
| D18–D21 | 私钥与 host key 分栏；加密托管；不做生成；按 auth_methods 弹窗 |
| D22–D24 | 日志默认 sanitized；N9 不弹确认；关标签 flush；IME 提交后同步 |
| D25–D26 | 会话栏收敛；仅真实 SSH |
| D27–D32 | 状态栏 i18n / Esc 遮罩 / 对比度 / 字形 N5a / swash / WSL 构建路径等（多数已落地，L1/L4 见 §3） |

完整原文与 N0–N9 需求叙述见 [`archive/yshell-next-features-requirements.md`](./archive/yshell-next-features-requirements.md)。

---

## 5. 维护约定

- 砍功能 → 改实现 + 更新 §2；契约对应节加「本版不做」。
- 补闭环 → 改代码 + 从 §3 删行或移入 §1。
- 新决策 → 写入 §4（递增 Dn），不必复活 requirements 长文。
- 历史任务设计、spike、早期规格一律只放 [`archive/`](./archive/README.md)，不在本目录平铺。
