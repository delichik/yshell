//! TAB-4/TAB-5：本地 PTY 终端标签。
//!
//! `+`（[`AppRuntime::handle_new_tab_default`]）和冷启动都走
//! [`AppRuntime::open_local_terminal`]：用 `yshell-local` 的
//! `LocalPty::spawn_boxed` 启动平台默认 shell（D3；`YSHELL_LOCAL_SHELL` 可覆盖），
//! 再通过既有的 `ShellSession` 管线接入终端网格/输入/尺寸同步。
//!
//! 本地标签与 SSH 标签共享标签/会话登记，但**没有** SSH 连接：`SessionSource::Local`
//! 的 `ssh_config` 只是占位配置，重连/主机密钥/认证/SFTP 路径都会在进入网络层之前
//! 被守卫（见 `connection.rs` / `sftp.rs` / `auth.rs`）。

use yshell_core::{CommandDispatcher, SessionCommand, SessionState};
use yshell_local::{LocalPty, LocalPtyConfig};
use yshell_ssh::{PtySize, ShellSession};

use crate::{error::AppError, error::AppResult, session_runtime::SessionRuntime};

use super::*;

impl AppRuntime {
    /// TAB-4：新建一个本地终端标签（`+` 的默认行为）。
    ///
    /// 使用平台默认 shell（Windows 按 D3 探测 `pwsh` → `powershell` → `cmd`，
    /// Unix 走 `$SHELL` / `/bin/sh`），PTY 初始尺寸取最近一次窗口视口尺寸；
    /// 窗口尚未创建时退回 `120x32`。spawn 失败时只写状态栏，不创建标签。
    pub fn open_local_terminal(&mut self) -> AppProjection {
        let (columns, rows) = self.local_terminal_initial_size();
        let config = LocalPtyConfig::for_default_shell().with_size(columns, rows);
        self.open_local_terminal_with_config(config)
    }

    /// 同 [`Self::open_local_terminal`]，但允许调用方显式指定 shell/尺寸
    /// （测试与未来设置项覆盖使用）。
    pub(crate) fn open_local_terminal_with_config(
        &mut self,
        config: LocalPtyConfig,
    ) -> AppProjection {
        let display_name = config.shell_name();
        let size = config.size;
        match LocalPty::spawn_boxed(config) {
            Ok(shell_session) => {
                self.open_local_terminal_with_shell(display_name, size, shell_session)
            }
            Err(error) => {
                tracing::warn!(
                    target: "yshell::runtime",
                    shell = %display_name,
                    %error,
                    "failed to start the local shell"
                );
                // 不创建标签、不插入会话：失败不会留下抢输入的坏标签。
                self.set_status_kind(
                    "local-terminal-start-failed",
                    format!("Could not start the local shell `{display_name}`: {error}"),
                    display_name.to_string(),
                    error.to_string(),
                );
                self.projection()
            }
        }
    }

    /// 用已建立的 shell 会话完成本地标签接线（测试注入 `TestShellSession` 的入口）。
    pub(crate) fn open_local_terminal_with_shell(
        &mut self,
        display_name: impl Into<String>,
        size: PtySize,
        shell_session: Box<dyn ShellSession>,
    ) -> AppProjection {
        let runtime = SessionRuntime::from_local(
            self.allocate_runtime_ordinal(),
            display_name.into(),
            shell_session,
        );
        match self.attach_local_runtime(runtime, size) {
            Ok(projection) => projection,
            Err(error) => {
                tracing::warn!(
                    target: "yshell::runtime",
                    %error,
                    "failed to attach the local terminal to the runtime"
                );
                self.set_status_kind(
                    "local-terminal-attach-failed",
                    format!("Could not open the local terminal: {error}"),
                    error.to_string(),
                    String::new(),
                );
                self.projection()
            }
        }
    }

    /// 把一个已连接的本地运行时接入会话表/标签条（成功后才插入）。
    fn attach_local_runtime(
        &mut self,
        mut runtime: SessionRuntime,
        size: PtySize,
    ) -> AppResult<AppProjection> {
        let session_id = runtime.session_id().clone();
        let tab_id = runtime.tab_id().clone();
        let tab_id_text = tab_id.as_str().to_owned();
        let session_key = session_id.as_str().to_owned();
        let display_name = runtime.display_name.clone();

        // 本地 PTY 已按 spawn 配置创建；这里同步渲染网格与 runtime 期望尺寸。
        let _ = runtime.resize_shell_pty(size.columns, size.rows);
        // 立即消费 shell 的首屏输出（提示符/banner），第一帧就有内容。
        let _ = runtime.poll_shell_output();

        // N5/D17：创建时冻结外观；日志/回滚上限与 SSH 会话同一路径。
        self.capture_runtime_appearance(&mut runtime);
        self.configure_runtime_logging(&mut runtime);
        self.configure_runtime_terminal_limits(&mut runtime);
        let logging_notice = runtime.take_logging_notice();

        let open_events = self
            .dispatcher
            .dispatch(SessionCommand::OpenSession {
                tab_id,
                session_id: session_id.clone(),
            })
            .map_err(AppError::from_error)?;
        Self::apply_session_events(&session_id, &mut runtime, open_events);
        let state_events = self
            .dispatcher
            .dispatch(SessionCommand::SetSessionState {
                session_id: session_id.clone(),
                state: SessionState::Connected,
            })
            .map_err(AppError::from_error)?;
        Self::apply_session_events(&session_id, &mut runtime, state_events);
        runtime.log_runtime_event("Local terminal tab opened. PTY shell is attached.");

        self.sessions.insert(session_key.clone(), runtime);
        self.attach_tab(&tab_id_text, &session_key);
        self.active_tab_id = Some(tab_id_text);
        self.active_session_id = Some(session_key.clone());
        // 本地标签没有 SSH/SFTP：生命周期固定为 disconnected（不触发远端连接）。
        let _ = self.sync_sftp_lifecycle_for_session(&session_key, false);
        self.set_status_kind(
            "session-connected",
            format!("Connected to {display_name}."),
            display_name,
            String::new(),
        );
        if let Some(notice) = logging_notice {
            self.fold_logging_notice_into_status(notice);
        }
        Ok(self.projection())
    }

    /// TAB-5：冷启动工作区——无标签时创建恰好一个本地终端，并把 Quick Connect
    /// 覆盖层重新打开到它上面（`attach_tab` 会收起覆盖层）。
    ///
    /// 已有标签（会话恢复/后续调用）时只返回投影，不重复创建。
    pub fn prepare_desktop_startup_workspace(&mut self) -> AppProjection {
        if self.tabs.is_empty() {
            self.open_local_terminal();
            self.quick_connect_overlay_open = true;
        }
        self.projection()
    }

    /// 测试注入版本：[`Self::prepare_desktop_startup_workspace`] 不依赖用户 `$SHELL`。
    #[cfg(test)]
    pub(crate) fn prepare_desktop_startup_workspace_with_shell(
        &mut self,
        display_name: impl Into<String>,
        size: PtySize,
        shell_session: Box<dyn ShellSession>,
    ) -> AppProjection {
        if self.tabs.is_empty() {
            self.open_local_terminal_with_shell(display_name, size, shell_session);
            self.quick_connect_overlay_open = true;
        }
        self.projection()
    }
}
