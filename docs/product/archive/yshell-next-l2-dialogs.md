# L2 对话框 Esc / 遮罩点击关闭

状态：**已完成（验收通过 2026-09-27）**；owner `ses_f1cc28cd2ffeG6JskdbR1EDRgM`
验收：lint 0 误、**394 测试**；`run-ui-dialogs.sh` **74/74**（dark + light，用冻结二进制 `dist/l2-work/yshell-l2-verified` sha256 `0d215d05…`）。
偏差：① Settings→Known Hosts 改为**嵌套**（一行可回退）；② 编辑器/设置/已知主机关闭时不清"确认词/敏感字段"（与现有 Close 行为一致，列入后续安全项）；③ HostKeyDialog 新增 `open` 属性；④ e2e 空闲 display 范围 99→220。
范围：`ui/components/{confirm_dialog,password_prompt_dialog,host_key_dialog}.slint`、`ui/pages/{settings,session_editor,known_hosts,about}.slint`、`ui/components/session_sidebar.slint`（NewFolderDialog）、`ui/main_window.slint`（弹窗宿主/嵌套顺序）。
并行：与 **N5a-UI** 同时进行（文件零重叠）；**避免改 `bootstrap.rs`**（N5a-UI 正在改）——取消动作全部复用现有 Rust 回调，不再新增接线。

## 规则
1. `Esc` 与遮罩（scrim）点击 = 与 `Cancel` 按钮**完全相同**的路径（含清空敏感输入：密码/口令/确认词）。
2. 危险确认弹窗（删除、Quit、host key 变更、关闭连接标签）同样允许取消；默认焦点保持安全按钮（现有 E-064 语义不变）。
3. 嵌套弹窗：`Esc` 只关闭**最上层**（按现有 z 顺序），下层保持打开。
4. 弹窗打开期间全局快捷键继续让位（现有 `global-shortcuts-blocked` 语义不变）。
5. 覆盖面：ConfirmDialog、PasswordPrompt、HostKey、Settings、SessionEditor、KnownHosts、About、Quit/Delete 确认、SFTP 各操作弹窗、关闭标签确认、NewFolderDialog。

## 实现要点
- 每个模态组件内加根级 `FocusScope`：`key-pressed(event)` 判定 Esc（`event.text == "\u{001b}"`，与现有 `session_tree` 用 `\u{000a}` 判 Enter 的做法一致）→ 调 `canceled()`。
- 打开时聚焦：组件在 `open` 变为 true 时调用 `self.focus()`（或提供 `public function focus-dialog()` 由宿主在打开时调用；二选一，优先"组件内 watch open"）。
- 遮罩：现有拦截用 `TouchArea` 改为可点击取消（触发 `canceled()`）；取消后由宿主统一隐藏，避免二次关闭。
- 各页面（Settings/SessionEditor/KnownHosts/About）的关闭按钮路径复用其现有 close 回调。
- 若发现某弹窗的取消需要清理 Rust 侧状态而现有回调没做：**先报告**，不要改 `bootstrap.rs`。

## 验收
1. 逐个弹窗：`Esc`、遮罩点击 → 关闭并回到原界面；焦点恢复（`restore_shortcut_focus_after_modal` 路径）。
2. 敏感字段清空：密码弹窗取消后 `password_prompt_value_text` 为空；HostKey 替换确认词清空；删除确认词清空（以现有回调行为为准，逐项记录）。
3. 嵌套：在上层弹窗按 `Esc` 只关上层；下层仍可继续操作。
4. e2e/截图：`dist/ui-checks/l2-*`（至少：密码弹窗 Esc、删除确认遮罩点击、SessionEditor Esc、嵌套场景）；`Enter` 不会误确认破坏性操作（沿用 N0 的断言方式）。
5. `cargo xtask lint` + `cargo xtask test` 全绿（≥392）；告警数 ≤21。

## 风险
- 根 FocusScope 抢焦点影响输入框：逐弹窗验证 Tab 顺序与输入正常（打开后光标在首个输入框）。
- 报警告式的"关闭时二次触发"：取消回调必须幂等/由宿主统一隐藏。
