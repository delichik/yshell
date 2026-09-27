# N6 终端日志 UI（设计）

状态：**待确认**；依赖：G0/G0-tz ✅（`SessionLogger::open_with` + 本地时间）、R2
关联决策：D22（默认 sanitized + 时间戳开）、D24（单一日志输出、关闭/退出强制 flush）、需求 §7
文件落位：新 `ui/components/logging_dialog.slint`；`runtime/logging.rs`；`ui/main_window.slint`（终端右键菜单 + REC 指示最小接线）；`ui/components/status_bar.slint`（REC 状态）；`bootstrap.rs`；`ui/text_formats.slint`

## 1. 入口与状态
- 终端右键菜单（`context-menu-design.md` §2）：`Start Logging…` / `Stop Logging` / `Open Log File` / `Open Log Folder`；状态栏日志区同一入口（§9.2）。
- REC 指示：标签角标或状态栏（单会话级）；多标签各自独立记录。

## 2. 开启弹窗（`logging_dialog.slint`）
- 字段：保存位置（rfd，降级路径输入；默认上次目录，回退配置 logs）、文件名（默认 `{session}-{timestamp}.log`，非法字符校验，已存在确认覆盖/改名）、格式 raw/sanitized（默认 sanitized）、时间戳开关（默认开）、是否记录本地输入（默认关，开启单独确认）、（可选）大小上限。
- 弹窗只发 `start/stop` 语义回调 + 路径/选项；不内嵌文件逻辑。

## 3. 运行时（`runtime/logging.rs`）
- `start_session_logging(session_id, options)` / `stop_session_logging(session_id)`：调用 `SessionLogger::open_with`（Append/Truncate、timestamps）；同一会话同一时刻**单一日志输出**（手动日志优先，开启时暂停自动日志，关闭后不自动恢复）。
- 记录点：沿用 `poll_shell_output` 的输出路径（只记输出；勾选"记录输入"时对输入按 `TranscriptDirection::Input` 记录）。
- 生命周期：标签关闭/会话断开/应用退出 → 先 `close()` 并提示文件路径；写失败 → 状态栏提示 + 自动停止（保留已写内容）。
- "保存为会话默认策略"：写回 Session Editor 的 Logging 区（现有配置路径）。

## 4. 验收
1. 开启 → `ls --color` → 停止：sanitized 文件无 ESC 字节、行首本地 `[HH:MM:SS]`；raw 模式颜色序列保真。
2. 覆盖确认、非法路径、磁盘写失败（注入）有明确反馈；REC 状态与实际一致。
3. 关闭标签/退出应用自动停止并 flush（文件完整）。
4. `cargo xtask lint/test` 全绿；e2e 双主题；截图 `dist/ui-checks/n6-*`（右键菜单/REC/开启弹窗/停止提示/失败态）。
