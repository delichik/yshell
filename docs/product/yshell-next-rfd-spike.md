# rfd 后端选型 spike（设计）

状态：**待确认**；类型：调研 spike（不落依赖、不改仓库代码）
目的：决定 `rfd` 在 Linux（含 WSLg）走哪个后端，以及降级策略，供 D0b 一次性入库。
关联决策：D2（rfd + 路径输入降级）、D31（rfd 选型并入 N1 前）

## 调研问题
1. **GTK3 后端**：构建需要 `libgtk-3-dev`（CI/本机安装成本）；运行时对 winit/Slint 事件循环的影响（同步对话框是否阻塞渲染）；WSLg 无 xdg-portal 时是否可用（有 GTK 即可）；打包体积/依赖面。
2. **xdg-portal 后端**：需要 `xdg-desktop-portal` 服务（WSLg 通常没有）；异步模型需要执行器（tokio/async-std）——与现有同步 UI 线程如何桥接（block_on? 专用线程?）；无 portal 时的错误路径。
3. **fallback**：无论选谁，都要保留"内置路径输入弹窗"（已有组件模式），在对话框不可用时自动降级（探测/错误捕获后切换）。

## 方法
- 在 `/root/` 建临时工程（不进仓库）：两种 feature 组合各一个最小 main（`pick_file` + `save_file`），分别在本机 `:0`（WSLg）与 Xvfb 下运行。
- 记录：构建前置包、编译时间、运行时是否弹窗、Slint/winit 窗口是否卡死、返回路径正确性、错误信息。
- 不修改仓库任何文件。

## 交付物
- 报告（最终消息 + 可选 `docs/product/yshell-next-rfd-spike-report.md`）：后端结论（含 feature 串）、需要的系统包、降级触发条件（代码侧如何探测）、证据（命令/截图/错误）。
- 建议：D0b 应加入的 `rfd` feature 串与版本；CI/文档需要的系统依赖。

## 验收
- 两条路线都有实跑结果（成功或明确失败证据）；结论可执行（D0b 按此入库即可）；无仓库改动。
