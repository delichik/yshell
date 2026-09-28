//! SSH connection lifecycle: transport backend selection, connect, reconnect,
//! disconnect, host-key and password prompt state.

use crate::{
    error::AppError, error::AppResult, session_runtime::SessionRuntime,
    session_runtime::SessionSource,
};
use std::path::PathBuf;
use yshell_config::{parse_quick_connect, AuthMethod as ConfigAuthMethod};
use yshell_core::{CommandDispatcher, SessionCommand, SessionState};
use yshell_ssh::{
    AuthAttemptKind, AuthMethod, AuthMethods, AuthProblemKind, HostKeyFingerprint, HostKeyProblem,
    KeyboardInteractiveChallenge, KeyboardInteractiveResponse, Prompt, ShellClient,
    SshConnectionConfig, SshError, SshErrorKind, TransportBackend,
};

use super::*;

impl AppRuntime {
    pub fn handle_quick_connect(&mut self, input: &str) -> AppResult<AppProjection> {
        let target = parse_quick_connect(input).map_err(AppError::from_error)?;
        let runtime = SessionRuntime::from_quick_connect(target, self.allocate_runtime_ordinal());
        self.activate_runtime_session(runtime)
    }

    /// 旧 `+` 行为：新建一个草稿终端标签（`SessionRuntime::draft`）。
    ///
    /// N2 起 `+` 走 `handle_new_tab_default()`（Quick Connect 页 / Session Editor），
    /// 本方法暂无可达 UI 入口；保留实现与测试覆盖（runtime 测试仍覆盖草稿路径）。
    #[allow(dead_code)]
    pub fn handle_new_session(&mut self) -> AppProjection {
        let mut runtime = SessionRuntime::draft(self.allocate_runtime_ordinal());
        let session_id = runtime.session_id().as_str().to_owned();
        let tab_id = runtime.tab_id().as_str().to_owned();
        self.configure_runtime_logging(&mut runtime);
        self.configure_runtime_terminal_limits(&mut runtime);
        // N0：draft 也是一个正式标签，需要像其它会话一样在 core dispatcher 里登记，
        // 否则活动标签落在 draft 上时 resize/断流等命令会报 "session ... is not open"。
        // dispatcher 对全新 id 的登记不会失败；真失败时保留 draft 可用（best effort）。
        let core_session_id = runtime.session_id().clone();
        if let Ok(open_events) = self.dispatcher.dispatch(SessionCommand::OpenSession {
            tab_id: runtime.tab_id().clone(),
            session_id: core_session_id.clone(),
        }) {
            Self::apply_session_events(&core_session_id, &mut runtime, open_events);
        }
        if let Ok(state_events) = self.dispatcher.dispatch(SessionCommand::SetSessionState {
            session_id: core_session_id.clone(),
            state: SessionState::Idle,
        }) {
            Self::apply_session_events(&core_session_id, &mut runtime, state_events);
        }
        runtime.log_runtime_event("This draft is runtime-owned but not yet connected.");
        self.sftp_session = SftpSessionLifecycle::Disconnected {
            session_key: Some(session_id.clone()),
        };
        self.sftp_listing = SftpListingState::DraftDisconnected;
        self.status_text =
            "Created a runtime-backed session draft. Next step is wiring this draft into saved-session editing and real connect.".to_owned();
        if let Some(notice) = runtime.take_logging_notice() {
            self.fold_logging_notice_into_status(notice);
        }
        self.sessions.insert(session_id.clone(), runtime);
        self.attach_tab(&tab_id, &session_id);
        self.projection()
    }

    pub fn select_fake_transport_backend(&mut self) -> AppProjection {
        self.select_transport_backend(TransportBackend::Fake)
    }

    pub fn select_native_ssh_transport_backend(&mut self) -> AppProjection {
        self.select_transport_backend(TransportBackend::Real)
    }

    pub fn prepare_desktop_startup_projection(&mut self) -> AppProjection {
        self.transport_backend = TransportBackend::Real;
        self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
        self.sftp_listing = SftpListingState::NativeSshSelected;
        self.projection()
    }

    pub fn update_host_key_replace_confirmation(&mut self, confirmation: &str) -> AppProjection {
        self.host_key_replace_confirmation = confirmation.to_owned();
        self.projection()
    }

    pub fn cancel_host_key_prompt(&mut self) -> AppProjection {
        self.pending_host_key_prompt = None;
        self.host_key_replace_confirmation.clear();
        self.status_text = "Canceled host key confirmation. Connection remains blocked.".to_owned();
        self.projection()
    }

    pub fn submit_password(&mut self, password: &str) -> AppResult<AppProjection> {
        let prompt = self
            .pending_password_prompt
            .clone()
            .ok_or_else(|| AppError::new("no password prompt is pending"))?;
        if password.trim().is_empty() {
            return Err(AppError::new("password must not be empty"));
        }
        let result = self.retry_pending_password_connection(&prompt, password);
        self.pending_password_prompt = None;
        match result {
            Ok(projection) => Ok(projection),
            Err(error) => {
                tracing::warn!(
                    target: "yshell::runtime",
                    "password connection for {} failed: {error}",
                    prompt.host_text()
                );
                self.set_status_kind(
                    "session-password-failed",
                    format!(
                        "Could not connect to {} with the supplied password.",
                        prompt.host_text()
                    ),
                    prompt.host_text(),
                    String::new(),
                );
                Ok(self.projection())
            }
        }
    }

    pub fn cancel_password_prompt(&mut self) -> AppProjection {
        self.pending_password_prompt = None;
        self.status_text =
            "Canceled the password prompt. The connection was not started.".to_owned();
        self.projection()
    }

    pub fn trust_host_key_once(&mut self) -> AppResult<AppProjection> {
        let prompt = self
            .pending_host_key_prompt
            .clone()
            .ok_or_else(|| AppError::new("no host key confirmation is pending"))?;
        if prompt.mode() != HostKeyPromptMode::FirstTrust {
            return Err(AppError::new(
                "trust once is only available for a first-seen host key",
            ));
        }
        self.temporary_known_hosts
            .pin(&prompt.host, prompt.port, prompt.presented.clone());
        self.pending_host_key_prompt = None;
        self.host_key_replace_confirmation.clear();
        if let Some(resumed) = self.resume_password_prompt_after_host_key(&prompt) {
            return resumed;
        }
        self.status_text = format!(
            "Trusted the presented host key for {}:{} once. Reconnecting.",
            prompt.host, prompt.port
        );
        self.reconnect_session_by_key(&prompt.session_key)
    }

    pub fn trust_host_key_and_save(&mut self) -> AppResult<AppProjection> {
        let prompt = self
            .pending_host_key_prompt
            .clone()
            .ok_or_else(|| AppError::new("no host key confirmation is pending"))?;
        if prompt.mode() != HostKeyPromptMode::FirstTrust {
            return Err(AppError::new(
                "trust and save is only available for a first-seen host key",
            ));
        }
        self.persistent_known_hosts
            .pin(&prompt.host, prompt.port, prompt.presented.clone());
        self.config_store
            .save_known_hosts(&self.persistent_known_hosts)
            .map_err(AppError::from_error)?;
        self.pending_host_key_prompt = None;
        self.host_key_replace_confirmation.clear();
        if let Some(resumed) = self.resume_password_prompt_after_host_key(&prompt) {
            return resumed;
        }
        self.status_text = format!(
            "Saved the presented host key for {}:{} to `{}` and reconnecting.",
            prompt.host,
            prompt.port,
            prompt.known_hosts_path.display()
        );
        self.reconnect_session_by_key(&prompt.session_key)
    }

    pub fn replace_host_key_and_connect(&mut self) -> AppResult<AppProjection> {
        let prompt = self
            .pending_host_key_prompt
            .clone()
            .ok_or_else(|| AppError::new("no host key confirmation is pending"))?;
        if prompt.mode() != HostKeyPromptMode::Changed {
            return Err(AppError::new(
                "replace key is only available for a changed host key",
            ));
        }
        if self.host_key_replace_confirmation.trim() != "REPLACE" {
            return Err(AppError::new(
                "type REPLACE before replacing the stored host key",
            ));
        }
        self.persistent_known_hosts
            .replace(&prompt.host, prompt.port, prompt.presented.clone());
        self.config_store
            .save_known_hosts(&self.persistent_known_hosts)
            .map_err(AppError::from_error)?;
        self.pending_host_key_prompt = None;
        self.host_key_replace_confirmation.clear();
        if let Some(resumed) = self.resume_password_prompt_after_host_key(&prompt) {
            return resumed;
        }
        self.status_text = format!(
            "Replaced the stored host key for {}:{} in `{}` and reconnecting.",
            prompt.host,
            prompt.port,
            prompt.known_hosts_path.display()
        );
        self.reconnect_session_by_key(&prompt.session_key)
    }

    pub fn disconnect_active_session(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        self.disconnect_session_by_key(&session_key)
    }

    /// D18：按会话 key 断开（活动会话菜单与标签右键菜单复用同一动作）。
    pub(crate) fn disconnect_session_by_key(
        &mut self,
        session_key: &str,
    ) -> AppResult<AppProjection> {
        let session_id = self
            .sessions
            .get(session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        {
            let runtime = self
                .sessions
                .get_mut(session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            runtime.disconnect_shell().map_err(AppError::from_error)?;
        }
        let events = self
            .dispatcher
            .dispatch(SessionCommand::SetSessionState {
                session_id: session_id.clone(),
                state: yshell_core::SessionState::Disconnected,
            })
            .map_err(AppError::from_error)?;
        let runtime = self
            .sessions
            .get_mut(session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        Self::apply_session_events(&session_id, runtime, events);
        let sftp_status = self.sync_sftp_lifecycle_for_session(session_key, false);
        self.status_text = format!("Disconnected the runtime session. {sftp_status}");
        Ok(self.projection())
    }

    pub fn reconnect_active_session(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let ssh_config = self
            .sessions
            .get(&session_key)
            .map(|session| session.ssh_config.clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;

        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            runtime.log_runtime_event("Reconnecting runtime shell session.");
            runtime.transport_backend = self.transport_backend;
        }
        let connecting_events = self
            .dispatcher
            .dispatch(SessionCommand::SetSessionState {
                session_id: session_id.clone(),
                state: yshell_core::SessionState::Connecting,
            })
            .map_err(AppError::from_error)?;
        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            Self::apply_session_events(&session_id, runtime, connecting_events);
        }

        let shell_session = match self.open_shell_for_runtime(&ssh_config) {
            Ok(shell_session) => shell_session,
            Err(error) => {
                return self.handle_runtime_shell_open_error(&session_key, &session_id, error);
            }
        };
        let shell_connected = shell_session.is_connected();
        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            runtime.attach_shell_session(shell_session);
            let _ = runtime.poll_shell_output().map_err(AppError::from_error)?;
            if !shell_connected {
                runtime.log_runtime_event(
                        "Shell runtime reopened a non-live backend path. Either later SSH stages are still scaffolded, or the live shell has already exited.",
                    );
            }
            if let Some(notice) = runtime.take_logging_notice() {
                self.fold_logging_notice_into_status(notice);
            }
        }
        if shell_connected {
            let connected_events = self
                .dispatcher
                .dispatch(SessionCommand::SetSessionState {
                    session_id: session_id.clone(),
                    state: yshell_core::SessionState::Connected,
                })
                .map_err(AppError::from_error)?;
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            Self::apply_session_events(&session_id, runtime, connected_events);
        }
        if shell_connected {
            // N4：认证重试成功后关闭弹窗；失败时由 `apply_auth_prompt_from_error`
            // 原地更新同一个弹窗（保留已填内容）。
            if self
                .pending_auth_prompt
                .as_ref()
                .is_some_and(|prompt| prompt.session_key == session_key)
            {
                self.pending_auth_prompt = None;
            }
        }
        let sftp_status = self.sync_sftp_lifecycle_for_session(&session_key, shell_connected);
        // D15：用户向文案；后端细节（fake/native-ssh）不再进状态栏。
        let host_text = self
            .sessions
            .get(&session_key)
            .map(|session| {
                format!(
                    "{}@{}:{}",
                    session.username_label(),
                    session.ssh_config.host,
                    session.ssh_config.port
                )
            })
            .unwrap_or_default();
        let (kind, base_text) = if shell_connected {
            (
                "session-reconnected",
                format!("Reconnected to {host_text}."),
            )
        } else {
            (
                "session-reopened",
                format!("Reopened the session for {host_text}, but the shell is not live."),
            )
        };
        self.set_status_kind(
            kind,
            format!("{base_text} {sftp_status}"),
            host_text,
            String::new(),
        );
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub(crate) fn select_transport_backend(&mut self, backend: TransportBackend) -> AppProjection {
        self.transport_backend = backend;
        self.status_text = match backend {
            TransportBackend::Fake => {
                self.sftp_session = SftpSessionLifecycle::Unavailable {
                    reason: SftpUnavailableReason::FakeBackendSelected,
                };
                self.sftp_listing = SftpListingState::FakeBackendSelected;
                "Transport backend set to `fake`. Quick Connect will use the deterministic in-process shell adapter.".to_owned()
            }
            TransportBackend::Real => {
                if let Some(session_key) = self.active_session_id.clone() {
                    let _ = self.sync_sftp_lifecycle_for_session(&session_key, false);
                } else {
                    self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
                    self.sftp_listing = SftpListingState::NativeSshSelected;
                }
                "Transport backend set to `native-ssh`. Quick Connect will use the embedded ssh2 shell path, and the SFTP panel will use the embedded ssh2 SFTP path.".to_owned()
            }
        };
        self.status_text = format!(
            "{} {}",
            self.status_text,
            self.sftp_session.legacy_status_text()
        );
        self.projection()
    }

    pub(crate) fn open_shell_for_runtime(
        &mut self,
        ssh_config: &SshConnectionConfig,
    ) -> Result<Box<dyn yshell_ssh::ShellSession>, yshell_ssh::SshError> {
        let effective_config = self.effective_ssh_config(ssh_config);
        match self.transport_backend {
            TransportBackend::Fake => {
                // e2e/dev：`YSHELL_FAKE_AUTH_FAILURE=<methods>` 时首次开 shell 注入
                // 认证失败（fake 后端不做真实认证，用于驱动认证弹窗三态）。
                if let Some(scenario) = self.fake_auth_scenario.clone() {
                    if !self.fake_auth_failure_injected {
                        self.fake_auth_failure_injected = true;
                        return Err(fake_auth_failure_error(&scenario));
                    }
                }
                ShellClient::with_fake_backend().open_shell_boxed(&effective_config)
            }
            TransportBackend::Real => {
                ShellClient::with_real_backend().open_shell_boxed(&effective_config)
            }
        }
    }

    pub(crate) fn reconnect_session_by_key(
        &mut self,
        session_key: &str,
    ) -> AppResult<AppProjection> {
        self.active_session_id = Some(session_key.to_owned());
        if let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.session_id() == Some(session_key))
        {
            tab.unread = 0;
            let tab_id = tab.tab_id.clone();
            self.active_tab_id = Some(tab_id);
        }
        self.reconnect_active_session()
    }

    pub(crate) fn handle_runtime_shell_open_error(
        &mut self,
        session_key: &str,
        session_id: &yshell_core::SessionId,
        error: yshell_ssh::SshError,
    ) -> AppResult<AppProjection> {
        let handled = self.apply_host_key_prompt_from_error(
            session_key,
            self.sessions
                .get(session_key)
                .map(|session| session.username_label().to_owned())
                .unwrap_or_else(|| "user".to_owned()),
            &error,
        );
        // N4：host key 之后才是认证弹窗（host key 仍在认证之前）。
        let auth_context = self
            .sessions
            .get(session_key)
            .map(|session| AuthPromptContext::for_runtime(session_key, session));
        let handled_auth = !handled
            && auth_context
                .is_some_and(|context| self.apply_auth_prompt_from_error(context, &error));
        // N4：非认证错误（网络/配置等）不会更新弹窗；此时关闭本会话的陈旧认证弹窗，
        // 避免用户对着不再有效的凭据窗口操作。
        if !handled
            && !handled_auth
            && self
                .pending_auth_prompt
                .as_ref()
                .is_some_and(|prompt| prompt.session_key == session_key)
        {
            self.pending_auth_prompt = None;
        }
        if let Some(runtime) = self.sessions.get_mut(session_key) {
            runtime.log_runtime_event(&format!("Shell runtime failed while reconnecting: {error}"));
        }
        let failed_events = self
            .dispatcher
            .dispatch(SessionCommand::SetSessionState {
                session_id: session_id.clone(),
                state: yshell_core::SessionState::Failed,
            })
            .map_err(AppError::from_error)?;
        if let Some(runtime) = self.sessions.get_mut(session_key) {
            Self::apply_session_events(session_id, runtime, failed_events);
        }
        self.sftp_session = SftpSessionLifecycle::Failed {
            session_key: Some(session_key.to_owned()),
            reason: "shell reconnect failed before SFTP could attach".to_owned(),
        };
        self.sftp_listing = SftpListingState::ReconnectFailed;
        // D15：用户向文案；错误细节已进日志/会话日志。
        let (kind, base_text) = if handled {
            (
                "session-hostkey-required",
                "Host key confirmation is required before reconnecting.".to_owned(),
            )
        } else if handled_auth {
            (
                "session-auth-required",
                "Authentication requires input before reconnecting.".to_owned(),
            )
        } else {
            tracing::warn!(
                target: "yshell::runtime",
                "shell reconnect for {session_key} failed: {error}"
            );
            (
                "session-reconnect-failed",
                "Could not reconnect the session.".to_owned(),
            )
        };
        self.set_status_kind(
            kind,
            format!("{base_text} {}", self.sftp_session.legacy_status_text()),
            String::new(),
            String::new(),
        );
        Ok(self.projection())
    }

    pub(crate) fn apply_host_key_prompt_from_error(
        &mut self,
        session_key: &str,
        username: String,
        error: &yshell_ssh::SshError,
    ) -> bool {
        let Some(problem) = &error.host_key_problem else {
            return false;
        };
        let prompt = match problem.as_ref() {
            HostKeyProblem::Unknown {
                host,
                port,
                presented,
            } => PendingHostKeyPrompt {
                session_key: session_key.to_owned(),
                host: host.clone(),
                port: *port,
                username,
                presented: presented.clone(),
                expected: None,
                known_hosts_path: self.config_store.known_hosts_file(),
                resume_password: None,
            },
            HostKeyProblem::Changed {
                host,
                port,
                presented,
                expected,
            } => PendingHostKeyPrompt {
                session_key: session_key.to_owned(),
                host: host.clone(),
                port: *port,
                username,
                presented: presented.clone(),
                expected: Some(expected.clone()),
                known_hosts_path: self.config_store.known_hosts_file(),
                resume_password: None,
            },
        };
        self.pending_host_key_prompt = Some(prompt);
        self.host_key_replace_confirmation.clear();
        true
    }

    pub(crate) fn pending_password_prompt_for_runtime(
        &self,
        runtime: &SessionRuntime,
    ) -> Option<PendingPasswordPrompt> {
        let SessionSource::SavedSession { profile_id } = &runtime.source else {
            return None;
        };
        let resolved = self.config_document.resolve_session(profile_id)?;
        let auth = resolved.auth.as_ref()?;
        let (secret_key, auth_method) = password_prompt_auth_method(auth)?;
        // 密钥库里有可用密码（或没有密钥库）都返回 None；后者只有真的取不到密钥时才挂起。
        if self.resolve_secret_value(secret_key).is_ok() {
            return None;
        }
        Some(PendingPasswordPrompt {
            profile_id: profile_id.clone(),
            host: resolved.session.host.clone(),
            port: resolved.session.port,
            username: resolved
                .session
                .username
                .clone()
                .unwrap_or_else(|| "user".to_owned()),
            auth_method,
        })
    }

    pub(crate) fn retry_pending_password_connection(
        &mut self,
        prompt: &PendingPasswordPrompt,
        password: &str,
    ) -> AppResult<AppProjection> {
        let profile = self
            .config_document
            .find_session(&prompt.profile_id)
            .cloned()
            .ok_or_else(|| {
                AppError::new(format!(
                    "saved session `{}` was not found",
                    prompt.profile_id
                ))
            })?;
        let resolved = self
            .config_document
            .resolve_session(&prompt.profile_id)
            .ok_or_else(|| {
                AppError::new(format!(
                    "saved session `{}` could not be resolved from config",
                    prompt.profile_id
                ))
            })?;
        // 挂起后配置可能被改动：认证方式不一致就不要再把密码发给新目标。
        let current_method = resolved
            .auth
            .as_ref()
            .and_then(password_prompt_auth_method)
            .map(|(_, method)| method)
            .ok_or_else(|| {
                AppError::new(format!(
                    "saved session `{}` no longer uses password authentication; connect again",
                    prompt.profile_id
                ))
            })?;
        if current_method != prompt.auth_method {
            return Err(AppError::new(format!(
                "saved session `{}` changed its authentication method; connect again",
                prompt.profile_id
            )));
        }
        let ssh_config =
            self.build_ssh_config_from_resolved_session_with_password(&resolved, password)?;
        let runtime = SessionRuntime::from_profile(&profile, self.allocate_runtime_ordinal());
        // 先清挂起状态再连接：连接路径返回的投影必须显示弹窗已关闭。
        self.pending_password_prompt = None;
        self.activate_runtime_session_after_password(runtime, ssh_config)
    }

    pub(crate) fn activate_runtime_session(
        &mut self,
        runtime: SessionRuntime,
    ) -> AppResult<AppProjection> {
        if let SessionSource::SavedSession { profile_id } = &runtime.source {
            self.selected_saved_session_id = Some(profile_id.clone());
        }
        // W5：密码型认证在密钥库里找不到密码时不报错，改为挂起并让 UI 弹输入框；
        // 用户 `submit_password` 后走 `activate_runtime_session_with_ssh_config` 重试。
        if let Some(prompt) = self.pending_password_prompt_for_runtime(&runtime) {
            // D7：TOFU 顺序——密码之前先完成主机密钥确认。只有原生后端才会
            // 探测（fake 后端没有握手，无从取得指纹）。
            if self.probe_host_key_before_password(&runtime, &prompt)? {
                return Ok(self.projection());
            }
            let host_text = prompt.host_text();
            self.pending_password_prompt = Some(prompt);
            self.set_status_kind(
                "session-password-required",
                format!("Password required for {host_text}."),
                host_text,
                String::new(),
            );
            return Ok(self.projection());
        }
        let ssh_config = self.resolve_runtime_ssh_config(&runtime)?;
        self.activate_runtime_session_with_ssh_config(runtime, ssh_config)
    }

    /// D7：主机密钥确认完成后的续接。
    ///
    /// 探针路径里运行时实例尚未建立（也不需要重连），确认完成后回到密码弹窗；
    /// 返回 `Some` 表示已接管本次调用的返回投影。
    fn resume_password_prompt_after_host_key(
        &mut self,
        prompt: &PendingHostKeyPrompt,
    ) -> Option<AppResult<AppProjection>> {
        let resume = prompt.resume_password.clone()?;
        let host_text = resume.host_text();
        self.pending_password_prompt = Some(resume);
        // D37：状态栏走 kind 模板（`TextFormats.status-message`），host_text 作为参数。
        self.set_status_kind(
            "session-hostkey-confirmed-password",
            format!("Host key confirmed for {host_text}. Enter the password to continue."),
            host_text,
            String::new(),
        );
        Some(Ok(self.projection()))
    }

    /// D7：密码挂起前的主机密钥探针。
    ///
    /// 用"不存在的私钥"配置打开一次 shell：SSH 握手与主机密钥校验发生在认证
    /// 之前，因此未知/变更的主机密钥会先以 [`HostKeyProblem`] 返回；认证阶段
    /// 在本地读不到密钥文件即失败，不会把任何凭据送到服务端。返回 `true` 表示
    /// 已经弹出主机密钥确认（密码框等确认完成后再弹），`false` 表示主机密钥已
    /// 可信/无法探测，继续走原有密码挂起路径。
    fn probe_host_key_before_password(
        &mut self,
        runtime: &SessionRuntime,
        prompt: &PendingPasswordPrompt,
    ) -> AppResult<bool> {
        if self.transport_backend != TransportBackend::Real {
            return Ok(false);
        }
        let mut probe_config = runtime.ssh_config.clone();
        probe_config.auth = AuthMethod::PrivateKey {
            username: prompt.username.clone(),
            // 空路径：libssh2 在本地读取失败，探针绝不提交凭据。
            key_path: String::new(),
            passphrase: None,
        };
        match self.open_shell_for_runtime(&probe_config) {
            Ok(_) => Ok(false),
            Err(error) => {
                if error.host_key_problem.is_none() {
                    return Ok(false);
                }
                let handled = self.apply_host_key_prompt_from_error(
                    runtime.session_id().as_str(),
                    prompt.username.clone(),
                    &error,
                );
                if !handled {
                    return Ok(false);
                }
                if let Some(pending) = self.pending_host_key_prompt.as_mut() {
                    pending.resume_password = Some(prompt.clone());
                }
                // D37：状态栏走 kind 模板（`TextFormats.status-message`）。
                let host_text = prompt.host_text();
                self.set_status_kind(
                    "session-hostkey-required-password",
                    format!(
                        "Host key confirmation is required for {host_text} before the password prompt."
                    ),
                    host_text,
                    String::new(),
                );
                Ok(true)
            }
        }
    }

    pub(crate) fn activate_runtime_session_with_ssh_config(
        &mut self,
        runtime: SessionRuntime,
        ssh_config: SshConnectionConfig,
    ) -> AppResult<AppProjection> {
        self.activate_runtime_session_inner(runtime, ssh_config, false)
    }

    pub(crate) fn activate_runtime_session_after_password(
        &mut self,
        runtime: SessionRuntime,
        ssh_config: SshConnectionConfig,
    ) -> AppResult<AppProjection> {
        self.activate_runtime_session_inner(runtime, ssh_config, true)
    }

    pub(crate) fn activate_runtime_session_inner(
        &mut self,
        mut runtime: SessionRuntime,
        ssh_config: SshConnectionConfig,
        password_retry: bool,
    ) -> AppResult<AppProjection> {
        let session_id = runtime.session_id().clone();
        // N5/D17：创建时冻结外观（之后的主题改动不影响本会话）。
        self.capture_runtime_appearance(&mut runtime);
        if let SessionSource::SavedSession { profile_id } = &runtime.source {
            self.selected_saved_session_id = Some(profile_id.clone());
        }
        let tab_id = runtime.tab_id().clone();
        let tab_id_text = tab_id.as_str().to_owned();
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
                state: runtime.state,
            })
            .map_err(AppError::from_error)?;
        Self::apply_session_events(&session_id, &mut runtime, state_events);
        runtime.ssh_config = ssh_config;
        runtime.transport_backend = self.transport_backend;
        self.configure_runtime_logging(&mut runtime);
        self.configure_runtime_terminal_limits(&mut runtime);
        runtime.log_runtime_event("Core session entry created. Opening shell runtime boundary.");
        // 密码重试失败时把 shell 的真实错误带进状态栏（普通路径保持原中性文案）。
        let mut shell_open_error: Option<String> = None;
        // D15：连接挂起在主机密钥/认证确认时，状态栏给用户向提示。
        let mut prompt_pending = false;
        match self.open_shell_for_runtime(&runtime.ssh_config) {
            Ok(shell_session) => {
                let shell_connected = shell_session.is_connected();
                runtime.attach_shell_session(shell_session);
                let _ = runtime.poll_shell_output().map_err(AppError::from_error)?;
                if shell_connected {
                    let connected_events = self
                        .dispatcher
                        .dispatch(SessionCommand::SetSessionState {
                            session_id: session_id.clone(),
                            state: yshell_core::SessionState::Connected,
                        })
                        .map_err(AppError::from_error)?;
                    Self::apply_session_events(&session_id, &mut runtime, connected_events);
                } else {
                    let connecting_events = self
                        .dispatcher
                        .dispatch(SessionCommand::SetSessionState {
                            session_id: session_id.clone(),
                            state: yshell_core::SessionState::Connecting,
                        })
                        .map_err(AppError::from_error)?;
                    Self::apply_session_events(&session_id, &mut runtime, connecting_events);
                    runtime.log_runtime_event(
                        "Shell runtime opened a non-live backend path. Either later SSH stages remain scaffolded, or the spawned shell is not yet connected.",
                    );
                }
            }
            Err(error) => {
                let handled = self.apply_host_key_prompt_from_error(
                    session_id.as_str(),
                    runtime.username_label().to_owned(),
                    &error,
                );
                // N4：host key 之后才是认证弹窗（host key 仍在认证之前）。
                let handled_auth = !handled
                    && self.apply_auth_prompt_from_error(
                        AuthPromptContext::for_runtime(session_id.as_str(), &runtime),
                        &error,
                    );
                runtime.log_runtime_event(&format!(
                    "Shell runtime failed during real shell startup: {error}"
                ));
                let failed_events = self
                    .dispatcher
                    .dispatch(SessionCommand::SetSessionState {
                        session_id: session_id.clone(),
                        state: yshell_core::SessionState::Failed,
                    })
                    .map_err(AppError::from_error)?;
                Self::apply_session_events(&session_id, &mut runtime, failed_events);
                if handled {
                    prompt_pending = true;
                    runtime.log_runtime_event(
                        "Host key confirmation is required before the native SSH session can continue.",
                    );
                } else if handled_auth {
                    prompt_pending = true;
                    runtime.log_runtime_event(
                        "Authentication requires input before the native SSH session can continue.",
                    );
                } else {
                    // 主机密钥问题会走上面的 prompt，不算密码失败。
                    shell_open_error = Some(error.to_string());
                }
            }
        }

        let session_key = session_id.as_str().to_owned();
        // N2：shell 是否在本函数内连上（用于插入会话后触发连接成功钩子）。
        let connected_now = runtime.state == yshell_core::SessionState::Connected;
        self.record_recent_session(&session_key);
        let host_text = format!(
            "{}@{}:{}",
            runtime.username_label(),
            runtime.ssh_config.host,
            runtime.ssh_config.port
        );
        if let Some(error) = &shell_open_error {
            tracing::warn!(
                target: "yshell::runtime",
                "connection to {host_text} failed: {error}"
            );
        }
        let logging_notice = runtime.take_logging_notice();
        self.sessions.insert(session_key.clone(), runtime);
        self.attach_tab(&tab_id_text, &session_key);
        let sftp_status = self.sync_sftp_lifecycle_for_session(&session_key, true);
        // D15：用户向的连接状态文案（kind 供 Slint 侧 @tr 模板映射）。
        let (kind, base_text) = if shell_open_error.is_some() {
            if password_retry {
                (
                    "session-password-failed",
                    format!("Could not connect to {host_text} with the supplied password."),
                )
            } else {
                (
                    "session-connect-failed",
                    format!("Could not connect to {host_text}."),
                )
            }
        } else if connected_now {
            ("session-connected", format!("Connected to {host_text}."))
        } else if prompt_pending {
            (
                "session-pending-confirmation",
                format!("Waiting for confirmation to continue the connection to {host_text}."),
            )
        } else {
            (
                "session-connecting",
                format!("Connecting to {host_text} ..."),
            )
        };
        self.set_status_kind(
            kind,
            format!("{base_text} {sftp_status}"),
            host_text,
            String::new(),
        );
        if let Some(notice) = logging_notice {
            self.fold_logging_notice_into_status(notice);
        }
        // N2：连接成功钩子（QC 历史 / 已保存会话最近使用）；失败/挂起不触发。
        // W5 密码重试路径（`password_retry`）明确不写任何配置文件（密码绝不落盘，
        // 见 `session_auth::submit_password_retries_saved_session_and_connects`），
        // 因此该路径跳过历史记录；正常连接路径照常记录。
        if connected_now && !password_retry {
            self.record_connect_success(&session_key);
        }
        Ok(self.projection())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingHostKeyPrompt {
    pub(crate) session_key: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) username: String,
    pub(crate) presented: HostKeyFingerprint,
    pub(crate) expected: Option<HostKeyFingerprint>,
    pub(crate) known_hosts_path: PathBuf,
    /// D7：TOFU 顺序——密码型会话先做主机密钥确认，确认完成后再弹密码框。
    /// 该字段保存"确认完成后继续的密码挂起目标"；`None` = 常规 host key 流程。
    pub(crate) resume_password: Option<PendingPasswordPrompt>,
}

/// W5：密码弹窗挂起的目标（仅内存，重试连接时据此重建运行时会话）。
///
/// 保存的是"最小上下文"：已保存会话 id + host/port/user + 认证方式；密码本身
/// 只在 `submit_password` 的一次调用里流转，不进入任何持久化结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingPasswordPrompt {
    pub(crate) profile_id: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) username: String,
    pub(crate) auth_method: PendingPasswordAuthMethod,
}

impl PendingPasswordPrompt {
    /// 弹窗文案里的 `user@host:port`（Slint 侧套 `@tr("Enter the password for {0} ...")`）。
    pub(crate) fn host_text(&self) -> String {
        format!("{}@{}:{}", self.username, self.host, self.port)
    }
}

/// 需要用户输入密码型密钥的认证方式（决定重试时构造哪种 `AuthMethod`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingPasswordAuthMethod {
    Password,
    KeyboardInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostKeyPromptMode {
    FirstTrust,
    Changed,
}

impl PendingHostKeyPrompt {
    pub(crate) fn mode(&self) -> HostKeyPromptMode {
        if self.expected.is_some() {
            HostKeyPromptMode::Changed
        } else {
            HostKeyPromptMode::FirstTrust
        }
    }
}

/// W5：认证档案里"需要用户输入的密码型密钥"（secret key + 认证方式）。
///
/// 私钥口令与代理密码不在此列：弹窗文案与重试路径只处理 `Password` /
/// `KeyboardInteractive` 这两种认证密钥。
pub(crate) fn password_prompt_auth_method(
    auth: &yshell_config::AuthProfile,
) -> Option<(&str, PendingPasswordAuthMethod)> {
    match &auth.method {
        ConfigAuthMethod::Password { secret_key } => {
            Some((secret_key.as_str(), PendingPasswordAuthMethod::Password))
        }
        ConfigAuthMethod::KeyboardInteractive { secret_key } => Some((
            secret_key.as_str(),
            PendingPasswordAuthMethod::KeyboardInteractive,
        )),
        ConfigAuthMethod::PrivateKey { .. } | ConfigAuthMethod::Agent => None,
    }
}

// ---------------------------------------------------------------------------
// N4：认证请求状态机（设计 §2）
//
// `PendingAuthPrompt` 是"按服务端能力驱动的一次认证窗口"：默认选中会话配置的
// 方式（服务端允许时），否则第一个可用方式；一次弹窗内多方式重试且不清空已
// 填内容；keyboard-interactive 支持多轮 prompt。Phase 1 只提供纯状态（可单测），
// Phase 2 把它挂到 `AppRuntime` 并接到 `auth_prompt_dialog.slint`。
// ---------------------------------------------------------------------------

/// 弹窗里可选的认证方式（agent 并入 publickey 的选项，见设计 §2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AuthPromptMethod {
    Password,
    PublicKey,
    KeyboardInteractive,
}

impl AuthPromptMethod {
    /// 展示名（弹窗文案走 Slint 侧 `@tr`；这里只用于状态/错误文案）。
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Password => "Password",
            Self::PublicKey => "Public key",
            Self::KeyboardInteractive => "Keyboard interactive",
        }
    }

    /// Slint 侧的方式索引（0/1/2，与可见性属性一一对应）。
    #[must_use]
    pub(crate) const fn index(self) -> i32 {
        match self {
            Self::Password => 0,
            Self::PublicKey => 1,
            Self::KeyboardInteractive => 2,
        }
    }

    #[must_use]
    pub(crate) const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Password),
            1 => Some(Self::PublicKey),
            2 => Some(Self::KeyboardInteractive),
            _ => None,
        }
    }

    /// 会话配置的认证方式 → 弹窗方式（Agent 走 PublicKey + "使用 ssh-agent"）。
    #[must_use]
    pub(crate) fn from_config_method(method: &ConfigAuthMethod) -> Self {
        match method {
            ConfigAuthMethod::Password { .. } => Self::Password,
            ConfigAuthMethod::PrivateKey { .. } | ConfigAuthMethod::Agent => Self::PublicKey,
            ConfigAuthMethod::KeyboardInteractive { .. } => Self::KeyboardInteractive,
        }
    }
}

/// 一轮 keyboard-interactive 里的单个 prompt（echo=false 时 UI 用密码框）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingKeyboardPrompt {
    pub(crate) text: String,
    pub(crate) echo: bool,
    pub(crate) answer: String,
}

/// 一轮 keyboard-interactive 挑战（服务端一次 info-request）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingKeyboardRound {
    pub(crate) name: String,
    pub(crate) instruction: String,
    pub(crate) prompts: Vec<PendingKeyboardPrompt>,
    /// 本轮是否已提交（历史轮次用于"不清空已填内容"的回填）。
    pub(crate) submitted: bool,
}

impl PendingKeyboardRound {
    #[must_use]
    fn from_challenge(challenge: &KeyboardInteractiveChallenge) -> Self {
        Self {
            name: challenge.name.clone(),
            instruction: challenge.instruction.clone(),
            prompts: challenge
                .prompts
                .iter()
                .map(|prompt| PendingKeyboardPrompt {
                    text: prompt.text.clone(),
                    echo: prompt.echo,
                    answer: String::new(),
                })
                .collect(),
            submitted: false,
        }
    }

    #[must_use]
    pub(crate) fn answers(&self) -> Vec<String> {
        self.prompts
            .iter()
            .map(|prompt| prompt.answer.clone())
            .collect()
    }
}

/// 认证弹窗状态机的非法操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthPromptError {
    /// 服务端明确不允许该方式。
    MethodNotAvailable(AuthPromptMethod),
    /// 没有进行中的 keyboard-interactive 轮次。
    NoKeyboardChallenge,
    /// 当前轮次已提交。
    KeyboardRoundAlreadySubmitted,
}

impl AuthPromptError {
    #[must_use]
    pub(crate) fn message(&self) -> String {
        match self {
            Self::MethodNotAvailable(method) => format!(
                "the server does not accept {} authentication",
                method.label().to_lowercase()
            ),
            Self::NoKeyboardChallenge => {
                "no keyboard-interactive challenge is in progress".to_owned()
            }
            Self::KeyboardRoundAlreadySubmitted => {
                "the keyboard-interactive round was already submitted".to_owned()
            }
        }
    }
}

/// 一次认证窗口的挂起状态（从 [`PendingPasswordPrompt`] 扩展）。
///
/// 生命周期：连接失败（`SshError.auth_problem`/`auth_methods`）时挂起 → 用户选择
/// 方式/填写凭据 → 提交重试 → 失败时 `apply_problem` 更新可用方式与错误提示，
/// 已填内容保留；取消/关闭时 `cancel` 清空敏感值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingAuthPrompt {
    /// 触发挂起的运行时会话 key（重试时据此更新 ssh 配置并重连）。
    pub(crate) session_key: String,
    pub(crate) profile_id: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) username: String,
    /// 会话配置里指定的方式（默认选中的第一优先）。
    pub(crate) configured_method: Option<AuthPromptMethod>,
    /// 服务端返回的可用方式；`None` = 未返回列表（全部可见）。
    pub(crate) server_methods: Option<AuthMethods>,
    /// 上一次尝试的结果分类（弹窗错误条）。
    pub(crate) last_problem: Option<AuthProblemKind>,
    /// 已尝试次数（含挂起前的那一次）。
    pub(crate) attempts: u32,
    /// 当前选中的方式（用 `select` 修改，保证服务端允许）。
    selected: AuthPromptMethod,
    /// 密码输入（重试不清空；取消/关闭清空）。
    pub(crate) password_text: String,
    /// "记住密码"（写 secret store，设计 §2）。
    pub(crate) remember_password: bool,
    /// 私钥口令输入（重试不清空；取消/关闭清空）。
    pub(crate) passphrase_text: String,
    /// 私钥下拉的选中 key id（托管清单）。
    pub(crate) selected_key_id: Option<String>,
    /// "浏览文件"选中的私钥路径（托管 key 之外的本地文件）。
    pub(crate) browsed_key_path: String,
    /// "使用 ssh-agent"（publickey 的选项）。
    pub(crate) use_agent: bool,
    /// 已提交的 keyboard-interactive 轮次（多轮历史）。
    pub(crate) keyboard_rounds: Vec<PendingKeyboardRound>,
    /// 进行中的 keyboard-interactive 轮次。
    pub(crate) active_keyboard_round: Option<PendingKeyboardRound>,
    cancelled: bool,
}

impl PendingAuthPrompt {
    /// 挂起一个新的认证窗口（`server_methods` 来自失败响应；`None` = 未知）。
    #[must_use]
    pub(crate) fn new(
        profile_id: impl Into<String>,
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
        configured_method: Option<AuthPromptMethod>,
        server_methods: Option<AuthMethods>,
    ) -> Self {
        Self {
            session_key: String::new(),
            profile_id: profile_id.into(),
            host: host.into(),
            port,
            username: username.into(),
            configured_method,
            server_methods,
            last_problem: None,
            attempts: 1,
            selected: default_auth_prompt_selection(configured_method, server_methods),
            password_text: String::new(),
            remember_password: false,
            passphrase_text: String::new(),
            selected_key_id: None,
            browsed_key_path: String::new(),
            use_agent: false,
            keyboard_rounds: Vec::new(),
            active_keyboard_round: None,
            cancelled: false,
        }
    }

    /// 弹窗文案里的 `user@host:port`。
    #[must_use]
    pub(crate) fn host_text(&self) -> String {
        format!("{}@{}:{}", self.username, self.host, self.port)
    }

    #[must_use]
    pub(crate) fn selected(&self) -> AuthPromptMethod {
        self.selected
    }

    /// 服务端是否允许该方式（`None` = 未返回列表 → 允许，回退到手动选择）。
    #[must_use]
    pub(crate) fn method_allowed(&self, method: AuthPromptMethod) -> bool {
        match self.server_methods {
            Some(methods) => methods.allows(attempt_kind_of(method)),
            None => true,
        }
    }

    /// 弹窗上真实可见的方式（按服务端能力过滤）。
    #[must_use]
    pub(crate) fn visible_methods(&self) -> Vec<AuthPromptMethod> {
        [
            AuthPromptMethod::Password,
            AuthPromptMethod::PublicKey,
            AuthPromptMethod::KeyboardInteractive,
        ]
        .into_iter()
        .filter(|method| self.method_allowed(*method))
        .collect()
    }

    /// 是否有任何可用方式（false = 服务端不接受我们支持的任何方式）。
    #[must_use]
    pub(crate) fn has_usable_method(&self) -> bool {
        !self.visible_methods().is_empty()
    }

    /// 切换方式；服务端不允许/没有可用方式时拒绝（保持原选择）。
    pub(crate) fn select(&mut self, method: AuthPromptMethod) -> Result<(), AuthPromptError> {
        if !self.method_allowed(method) {
            return Err(AuthPromptError::MethodNotAvailable(method));
        }
        self.selected = method;
        Ok(())
    }

    /// 一次尝试失败后的状态更新：刷新服务端方式列表、记录错误分类；当前方式被
    /// 禁用时自动切到第一个可用方式。已填内容保留（设计 §2）。
    pub(crate) fn apply_problem(&mut self, methods: Option<AuthMethods>, problem: AuthProblemKind) {
        self.attempts = self.attempts.saturating_add(1);
        self.last_problem = Some(problem);
        if let Some(methods) = methods {
            self.server_methods = Some(methods);
        }
        if !self.method_allowed(self.selected) {
            if let Some(next) = self.visible_methods().first().copied() {
                self.selected = next;
            }
        }
        // 失败后上一轮挑战作废；历史轮次保留用于回填（不清空已填内容）。
        self.active_keyboard_round = None;
    }

    /// 把一次 A0 认证错误并入弹窗；非认证错误返回 `false`（宿主继续走错误路径）。
    pub(crate) fn apply_auth_error(&mut self, error: &SshError) -> bool {
        let Some(problem) = error.auth_problem else {
            return false;
        };
        self.apply_problem(error.auth_methods, problem);
        true
    }

    /// 错误条文案（无错误时 `None`）。
    #[must_use]
    pub(crate) fn problem_text(&self) -> Option<String> {
        self.last_problem.map(|problem| match problem {
            AuthProblemKind::InvalidCredentials => {
                "The server rejected these credentials. Check them and retry.".to_owned()
            }
            AuthProblemKind::MethodNotAllowed => format!(
                "The server no longer accepts {} authentication. Pick another method.",
                self.selected.label().to_lowercase()
            ),
            AuthProblemKind::OtherMethodRequired => {
                "The server requires a different authentication method.".to_owned()
            }
            AuthProblemKind::Cancelled => "Authentication was cancelled.".to_owned(),
        })
    }

    /// 取消/关闭弹窗：清空所有敏感输入（设计 §2："关闭/取消清空；不落日志"）。
    pub(crate) fn cancel(&mut self) {
        self.cancelled = true;
        self.password_text.clear();
        self.passphrase_text.clear();
        self.active_keyboard_round = None;
        for round in &mut self.keyboard_rounds {
            for prompt in &mut round.prompts {
                prompt.answer.clear();
            }
        }
    }

    /// 开始一轮 keyboard-interactive 挑战（多轮时服务端会再次触发）。
    pub(crate) fn begin_keyboard_round(
        &mut self,
        challenge: &KeyboardInteractiveChallenge,
    ) -> PendingKeyboardRound {
        let round = PendingKeyboardRound::from_challenge(challenge);
        self.active_keyboard_round = Some(round.clone());
        round
    }

    /// 填写进行中轮次的一个作答（越界返回 false）。
    pub(crate) fn set_keyboard_answer(&mut self, index: usize, answer: &str) -> bool {
        let Some(round) = self.active_keyboard_round.as_mut() else {
            return false;
        };
        match round.prompts.get_mut(index) {
            Some(prompt) => {
                prompt.answer = answer.to_owned();
                true
            }
            None => false,
        }
    }

    /// 提交进行中的轮次：返回给 A0 prompter 的答案（按 prompt 顺序）。
    pub(crate) fn submit_keyboard_round(
        &mut self,
    ) -> Result<KeyboardInteractiveResponse, AuthPromptError> {
        let Some(round) = self.active_keyboard_round.as_mut() else {
            return Err(AuthPromptError::NoKeyboardChallenge);
        };
        if round.submitted {
            return Err(AuthPromptError::KeyboardRoundAlreadySubmitted);
        }
        let answers = round.answers();
        round.submitted = true;
        let submitted = round.clone();
        self.keyboard_rounds.push(submitted);
        self.active_keyboard_round = None;
        Ok(KeyboardInteractiveResponse::Answers(answers))
    }
}

/// 默认选中：会话配置的方式（服务端允许时）→ 第一个可见方式 → 配置方式/Password。
#[must_use]
fn default_auth_prompt_selection(
    configured: Option<AuthPromptMethod>,
    server_methods: Option<AuthMethods>,
) -> AuthPromptMethod {
    let allowed = |method: AuthPromptMethod| match server_methods {
        Some(methods) => methods.allows(attempt_kind_of(method)),
        None => true,
    };
    if let Some(configured) = configured.filter(|method| allowed(*method)) {
        return configured;
    }
    [
        AuthPromptMethod::Password,
        AuthPromptMethod::PublicKey,
        AuthPromptMethod::KeyboardInteractive,
    ]
    .into_iter()
    .find(|method| allowed(*method))
    .or(configured)
    .unwrap_or(AuthPromptMethod::Password)
}

const fn attempt_kind_of(method: AuthPromptMethod) -> AuthAttemptKind {
    match method {
        AuthPromptMethod::Password => AuthAttemptKind::Password,
        AuthPromptMethod::PublicKey => AuthAttemptKind::PublicKey,
        AuthPromptMethod::KeyboardInteractive => AuthAttemptKind::KeyboardInteractive,
    }
}

// ---------------------------------------------------------------------------
// N4 Phase 2：认证弹窗 ↔ 宿主（服务端驱动的认证失败 → 挂起 → 选择/输入 → 重试）
// ---------------------------------------------------------------------------

/// 认证弹窗的私钥选项（Rust → Slint `AuthKeyOption` 的镜像）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthKeyOptionData {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) detail: String,
}

/// 认证弹窗的一个 keyboard-interactive prompt（Rust → Slint `AuthKeyboardPrompt`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthPromptQuestionData {
    pub(crate) text: String,
    pub(crate) echo: bool,
    pub(crate) answer: String,
}

impl PendingAuthPrompt {
    /// 绑定触发挂起的运行时会话 key（Phase 2 重试路径使用）。
    #[must_use]
    pub(crate) fn with_session_key(mut self, session_key: &str) -> Self {
        self.session_key = session_key.to_owned();
        self
    }

    /// 当前私钥选项在托管清单里的索引（未选中返回 -1）。
    #[must_use]
    pub(crate) fn selected_key_index(&self, entries: &[PrivateKeyEntryInfo]) -> i32 {
        self.selected_key_id
            .as_deref()
            .and_then(|selected| entries.iter().position(|entry| entry.id == selected))
            .map(|index| i32::try_from(index).unwrap_or(i32::MAX))
            .unwrap_or(-1)
    }

    /// publickey 主操作是否可用（agent / 托管 key / 浏览文件三选一）。
    #[must_use]
    pub(crate) fn publickey_submit_enabled(&self) -> bool {
        self.use_agent || self.selected_key_id.is_some() || !self.browsed_key_path.is_empty()
    }

    /// keyboard-interactive 主操作是否可用（有进行中的轮次）。
    #[must_use]
    pub(crate) fn keyboard_submit_enabled(&self) -> bool {
        self.active_keyboard_round.is_some()
    }

    /// 当前轮是否为最后一轮（宿主判断；fake 场景固定两轮）。
    #[must_use]
    pub(crate) fn keyboard_final_round(&self, total_rounds: usize) -> bool {
        self.active_keyboard_round.is_some() && self.keyboard_rounds.len() + 1 >= total_rounds
    }

    /// 进行中轮次的 prompt 列表（投影用）。
    #[must_use]
    pub(crate) fn active_keyboard_prompts(&self) -> Vec<AuthPromptQuestionData> {
        self.active_keyboard_round
            .as_ref()
            .map(|round| {
                round
                    .prompts
                    .iter()
                    .map(|prompt| AuthPromptQuestionData {
                        text: prompt.text.clone(),
                        echo: prompt.echo,
                        answer: prompt.answer.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// 认证挂起所需的会话上下文（连接路径里会话可能还没进 `sessions` 表）。
pub(crate) struct AuthPromptContext {
    pub(crate) session_key: String,
    pub(crate) profile_id: Option<String>,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) username: String,
}

impl AuthPromptContext {
    /// D6：从运行时会话构建弹窗上下文（`host` 只放裸主机；端口由
    /// `PendingAuthPrompt::host_text()` 拼一次，避免 `host:port:port`）。
    pub(crate) fn for_runtime(session_key: &str, session: &SessionRuntime) -> Self {
        Self {
            session_key: session_key.to_owned(),
            profile_id: match &session.source {
                SessionSource::SavedSession { profile_id } => Some(profile_id.clone()),
                _ => None,
            },
            host: session.ssh_config.host.clone(),
            port: session.ssh_config.port,
            username: session.username_label().to_owned(),
        }
    }
}

impl AppRuntime {
    /// 认证错误 → 挂起认证弹窗（host key 已在认证之前处理；非认证错误返回 `false`）。
    pub(crate) fn apply_auth_prompt_from_error(
        &mut self,
        context: AuthPromptContext,
        error: &SshError,
    ) -> bool {
        let Some(problem) = error.auth_problem else {
            return false;
        };
        // N4：同会话重试再次失败 → 原地更新同一个弹窗（刷新服务端方式 + 错误条，
        // 保留已填内容；设计 §2）。错误里必带 `auth_problem`，分支必为 true。
        if let Some(existing) = self.pending_auth_prompt.as_mut() {
            if existing.session_key == context.session_key && existing.apply_auth_error(error) {
                return true;
            }
        }
        let configured_method = context.profile_id.as_deref().and_then(|profile_id| {
            self.config_document
                .resolve_session(profile_id)
                .and_then(|resolved| resolved.auth)
                .map(|auth| AuthPromptMethod::from_config_method(&auth.method))
        });
        let mut prompt = PendingAuthPrompt::new(
            context.profile_id.unwrap_or_default(),
            context.host,
            context.port,
            context.username,
            configured_method,
            error.auth_methods,
        )
        .with_session_key(&context.session_key);
        prompt.last_problem = Some(problem);
        self.seed_fake_keyboard_challenge(&mut prompt);
        self.pending_auth_prompt = Some(prompt);
        // 认证弹窗取代 W5 的密码挂起（同一窗口的两种形态）。
        self.pending_password_prompt = None;
        true
    }

    /// e2e/dev：`YSHELL_FAKE_AUTH_FAILURE` 场景下给 keyboard-interactive 预置挑战
    /// （fake 后端不做真实认证，挑战由运行时合成，用于验证多轮渲染与状态机）。
    fn seed_fake_keyboard_challenge(&self, prompt: &mut PendingAuthPrompt) {
        if !self.fake_keyboard_scenario() {
            return;
        }
        if prompt.selected() != AuthPromptMethod::KeyboardInteractive {
            return;
        }
        prompt.begin_keyboard_round(&fake_keyboard_challenge(1));
    }

    pub(crate) fn fake_keyboard_scenario(&self) -> bool {
        self.fake_auth_scenario
            .as_deref()
            .is_some_and(|scenario| scenario.contains("keyboard-interactive"))
    }

    pub fn select_auth_prompt_method(&mut self, index: i32) -> AppProjection {
        if let Some(method) = AuthPromptMethod::from_index(index) {
            if let Some(prompt) = self.pending_auth_prompt.as_mut() {
                let _ = prompt.select(method);
            }
        }
        self.projection()
    }

    pub fn update_auth_prompt_password(&mut self, value: &str) -> AppProjection {
        if let Some(prompt) = self.pending_auth_prompt.as_mut() {
            prompt.password_text = value.to_owned();
        }
        self.projection()
    }

    pub fn toggle_auth_prompt_remember_password(&mut self, checked: bool) -> AppProjection {
        if let Some(prompt) = self.pending_auth_prompt.as_mut() {
            prompt.remember_password = checked;
        }
        self.projection()
    }

    /// 私钥下拉选择（index 来自 `auth_prompt_key_options`）。
    pub fn select_auth_prompt_key(&mut self, index: i32) -> AppProjection {
        let entries = self.private_key_entries();
        let Some(entry) = usize::try_from(index)
            .ok()
            .and_then(|index| entries.get(index))
        else {
            return self.projection();
        };
        if let Some(prompt) = self.pending_auth_prompt.as_mut() {
            prompt.selected_key_id = Some(entry.id.clone());
            prompt.browsed_key_path.clear();
        }
        self.projection()
    }

    /// "浏览文件"选中的私钥路径（rfd 由宿主填入）。
    pub fn set_auth_prompt_key_path(&mut self, path: &str) -> AppProjection {
        if let Some(prompt) = self.pending_auth_prompt.as_mut() {
            prompt.browsed_key_path = path.to_owned();
            if !path.is_empty() {
                prompt.selected_key_id = None;
            }
        }
        self.projection()
    }

    pub fn update_auth_prompt_passphrase(&mut self, value: &str) -> AppProjection {
        if let Some(prompt) = self.pending_auth_prompt.as_mut() {
            prompt.passphrase_text = value.to_owned();
        }
        self.projection()
    }

    pub fn toggle_auth_prompt_use_agent(&mut self, checked: bool) -> AppProjection {
        if let Some(prompt) = self.pending_auth_prompt.as_mut() {
            prompt.use_agent = checked;
        }
        self.projection()
    }

    pub fn update_auth_prompt_keyboard_answer(&mut self, index: i32, value: &str) -> AppProjection {
        if let Some(prompt) = self.pending_auth_prompt.as_mut() {
            if let Ok(index) = usize::try_from(index) {
                prompt.set_keyboard_answer(index, value);
            }
        }
        self.projection()
    }

    /// 认证弹窗主操作（password / publickey；keyboard-interactive 走轮次提交）。
    pub fn submit_auth_prompt(&mut self) -> AppProjection {
        let Some(prompt) = self.pending_auth_prompt.clone() else {
            return self.projection();
        };
        let method = match prompt.selected() {
            AuthPromptMethod::Password => {
                let password = prompt.password_text.trim().to_owned();
                if password.is_empty() {
                    self.status_text = "Password must not be empty.".to_owned();
                    return self.projection();
                }
                AuthMethod::Password {
                    username: prompt.username.clone(),
                    password: password.clone(),
                }
            }
            AuthPromptMethod::PublicKey if prompt.use_agent => AuthMethod::Agent {
                username: prompt.username.clone(),
            },
            AuthPromptMethod::PublicKey => match self.auth_prompt_private_key_method(&prompt) {
                Ok(method) => method,
                Err(message) => {
                    self.status_text = message;
                    return self.projection();
                }
            },
            AuthPromptMethod::KeyboardInteractive => {
                self.status_text = "Submit the keyboard-interactive round first.".to_owned();
                return self.projection();
            }
        };
        let remember_password = prompt.selected() == AuthPromptMethod::Password
            && prompt.remember_password
            && !prompt.profile_id.is_empty();
        if remember_password {
            self.remember_prompt_password(&prompt);
        }
        match self.retry_auth_prompt(&prompt, method) {
            Ok(projection) => projection,
            Err(message) => {
                self.status_text = message;
                self.projection()
            }
        }
    }

    /// keyboard-interactive 轮次提交：记录本轮作答；fake 场景第一轮后进入第二轮，
    /// 否则用本轮首个非 echo 作答作为 secret 走 A0 单秘密 prompter 重试。
    pub fn submit_auth_prompt_keyboard_round(&mut self) -> AppProjection {
        let Some(mut prompt) = self.pending_auth_prompt.clone() else {
            return self.projection();
        };
        if let Err(error) = prompt.submit_keyboard_round() {
            self.status_text = error.message();
            return self.projection();
        }
        if self.fake_keyboard_scenario() && prompt.keyboard_rounds.len() == 1 {
            prompt.begin_keyboard_round(&fake_keyboard_challenge(2));
            self.pending_auth_prompt = Some(prompt);
            self.status_text = "Round 1 accepted; answer the next challenge.".to_owned();
            return self.projection();
        }
        let secret = prompt
            .keyboard_rounds
            .last()
            .and_then(|round| {
                round
                    .prompts
                    .iter()
                    .find(|prompt| !prompt.echo)
                    .map(|prompt| prompt.answer.clone())
            })
            .unwrap_or_default();
        let method = AuthMethod::KeyboardInteractive {
            username: prompt.username.clone(),
            secret,
        };
        match self.retry_auth_prompt(&prompt, method) {
            Ok(projection) => projection,
            Err(message) => {
                self.status_text = message;
                self.projection()
            }
        }
    }

    pub fn cancel_auth_prompt(&mut self) -> AppProjection {
        if let Some(prompt) = self.pending_auth_prompt.as_mut() {
            prompt.cancel();
        }
        self.pending_auth_prompt = None;
        self.status_text =
            "Canceled the authentication prompt. The connection was not started.".to_owned();
        self.projection()
    }

    /// 组装 publickey 的 A0 认证方法（托管 key 先物化为 0600 文件）。
    fn auth_prompt_private_key_method(
        &self,
        prompt: &PendingAuthPrompt,
    ) -> Result<AuthMethod, String> {
        let (key_path, stored_passphrase) = if let Some(key_id) = prompt.selected_key_id.as_deref()
        {
            let path = self
                .materialize_private_key_file(key_id)
                .map_err(|error| format!("Cannot prepare the managed key: {error}"))?;
            (
                path.display().to_string(),
                self.private_key_passphrase(key_id),
            )
        } else if !prompt.browsed_key_path.trim().is_empty() {
            (prompt.browsed_key_path.trim().to_owned(), None)
        } else {
            return Err("Select a managed private key or browse for a key file.".to_owned());
        };
        let passphrase = if prompt.passphrase_text.is_empty() {
            stored_passphrase
        } else {
            Some(prompt.passphrase_text.clone())
        };
        Ok(AuthMethod::PrivateKey {
            username: prompt.username.clone(),
            key_path,
            passphrase,
        })
    }

    /// "记住密码"：写回已保存会话密码型 auth profile 的 secret store。
    fn remember_prompt_password(&mut self, prompt: &PendingAuthPrompt) {
        let Some(secret_key) = self
            .config_document
            .auth_profiles
            .get(&prompt.profile_id)
            .and_then(|auth| match &auth.method {
                ConfigAuthMethod::Password { secret_key } => Some(secret_key.clone()),
                _ => None,
            })
        else {
            return;
        };
        let password = prompt.password_text.clone();
        if let Err(error) = self.store_secret_value(&secret_key, &password) {
            self.status_text = format!("Could not remember the password: {error}");
        }
    }

    /// 用选定的认证方法更新会话 ssh 配置并重连（失败会再次挂起认证弹窗）。
    fn retry_auth_prompt(
        &mut self,
        prompt: &PendingAuthPrompt,
        auth: AuthMethod,
    ) -> Result<AppProjection, String> {
        let session_key = prompt.session_key.clone();
        if session_key.is_empty() || !self.sessions.contains_key(&session_key) {
            return Err("The connection target is gone; open the session again.".to_owned());
        }
        {
            let session = self.sessions.get_mut(&session_key).ok_or_else(|| {
                "The connection target is gone; open the session again.".to_owned()
            })?;
            session.ssh_config.auth = auth;
        }
        // N4：重试期间保持弹窗挂载 —— 连接成功在 `reconnect_active_session` 里关闭，
        // 认证再次失败则由 `apply_auth_prompt_from_error` 原地更新（保留已填内容）。
        self.reconnect_session_by_key(&session_key)
            .map_err(|error| error.message)
    }
}

/// fake 场景的 keyboard-interactive 挑战（第 1/2 轮）。
fn fake_keyboard_challenge(round: usize) -> KeyboardInteractiveChallenge {
    match round {
        1 => KeyboardInteractiveChallenge::new(
            "SSH Server",
            "Please authenticate to continue.",
            vec![
                Prompt::new("Password: ", false),
                Prompt::new("Verification code: ", false),
            ],
        ),
        _ => KeyboardInteractiveChallenge::new(
            "SSH Server",
            "Additional verification required.",
            vec![Prompt::new("One-time password: ", false)],
        ),
    }
}

/// fake 后端注入的认证失败（`YSHELL_FAKE_AUTH_FAILURE=<methods>`）。
fn fake_auth_failure_error(scenario: &str) -> SshError {
    let methods = AuthMethods::parse(scenario);
    let problem = if scenario.contains("keyboard-interactive")
        && !scenario.contains("password")
        && !scenario.contains("publickey")
    {
        AuthProblemKind::OtherMethodRequired
    } else {
        AuthProblemKind::InvalidCredentials
    };
    SshError::new(
        SshErrorKind::Authentication,
        format!("fake auth scenario `{scenario}` rejected the configured method"),
    )
    .with_auth_context(methods, problem)
}

#[cfg(test)]
mod auth_prompt_tests {
    use yshell_ssh::Prompt;

    use super::*;

    fn challenge(
        name: &str,
        instruction: &str,
        prompts: &[(&str, bool)],
    ) -> KeyboardInteractiveChallenge {
        KeyboardInteractiveChallenge::new(
            name,
            instruction,
            prompts
                .iter()
                .map(|(text, echo)| Prompt::new(*text, *echo))
                .collect(),
        )
    }

    #[test]
    fn defaults_to_the_configured_method_when_the_server_allows_it() {
        let methods = AuthMethods::parse("password,publickey").expect("list");
        let prompt = PendingAuthPrompt::new(
            "saved-1",
            "example.test",
            22,
            "alice",
            Some(AuthPromptMethod::PublicKey),
            Some(methods),
        );

        assert_eq!(prompt.selected(), AuthPromptMethod::PublicKey);
        assert!(prompt.has_usable_method());
        assert_eq!(
            prompt.visible_methods(),
            vec![AuthPromptMethod::Password, AuthPromptMethod::PublicKey]
        );
    }

    #[test]
    fn server_only_publickey_hides_the_password_tab() {
        let methods = AuthMethods::parse("publickey").expect("list");
        let prompt = PendingAuthPrompt::new(
            "saved-1",
            "example.test",
            22,
            "alice",
            Some(AuthPromptMethod::Password),
            Some(methods),
        );

        assert_eq!(prompt.selected(), AuthPromptMethod::PublicKey);
        assert!(!prompt.method_allowed(AuthPromptMethod::Password));
        assert!(prompt.method_allowed(AuthPromptMethod::PublicKey));
        // agent 与 publickey 共用同一方法位（A0：agent 骑在 publickey 上）。
        assert_eq!(prompt.visible_methods(), vec![AuthPromptMethod::PublicKey]);
    }

    #[test]
    fn unknown_server_methods_keep_every_tab_visible() {
        let prompt = PendingAuthPrompt::new("saved-1", "example.test", 22, "alice", None, None);

        assert_eq!(prompt.selected(), AuthPromptMethod::Password);
        assert_eq!(prompt.visible_methods().len(), 3);
        assert!(prompt.server_methods.is_none());
    }

    #[test]
    fn problem_updates_visibility_and_switches_away_from_a_disabled_method() {
        let mut prompt = PendingAuthPrompt::new(
            "saved-1",
            "example.test",
            22,
            "alice",
            Some(AuthPromptMethod::PublicKey),
            Some(AuthMethods::parse("publickey").expect("list")),
        );
        assert_eq!(prompt.selected(), AuthPromptMethod::PublicKey);
        prompt.password_text = "kept-input".to_owned();

        prompt.apply_problem(
            AuthMethods::parse("password"),
            AuthProblemKind::MethodNotAllowed,
        );

        assert_eq!(prompt.selected(), AuthPromptMethod::Password);
        assert!(!prompt.method_allowed(AuthPromptMethod::PublicKey));
        assert_eq!(prompt.attempts, 2);
        assert!(prompt
            .problem_text()
            .expect("problem")
            .contains("no longer accepts"));
        // 已填内容保留（设计 §2：多方式重试不清空）。
        assert_eq!(prompt.password_text, "kept-input");
    }

    #[test]
    fn apply_auth_error_only_consumes_authentication_problems() {
        let mut prompt = PendingAuthPrompt::new(
            "saved-1",
            "example.test",
            22,
            "alice",
            Some(AuthPromptMethod::Password),
            None,
        );

        let host_key_error = SshError::new(yshell_ssh::SshErrorKind::HostKeyRejected, "host key");
        assert!(!prompt.apply_auth_error(&host_key_error));
        assert_eq!(prompt.attempts, 1);

        let auth_error = SshError::new(yshell_ssh::SshErrorKind::Authentication, "denied")
            .with_auth_context(
                AuthMethods::parse("keyboard-interactive"),
                AuthProblemKind::InvalidCredentials,
            );
        assert!(prompt.apply_auth_error(&auth_error));
        assert_eq!(prompt.attempts, 2);
        assert_eq!(prompt.selected(), AuthPromptMethod::KeyboardInteractive);
        assert!(prompt
            .problem_text()
            .expect("problem")
            .contains("rejected these credentials"));
    }

    #[test]
    fn host_text_formats_user_host_and_port() {
        let prompt = PendingAuthPrompt::new(
            "saved-1",
            "example.test",
            22,
            "alice",
            Some(AuthPromptMethod::Password),
            None,
        );
        assert_eq!(prompt.configured_method, Some(AuthPromptMethod::Password));
        assert_eq!(prompt.selected(), AuthPromptMethod::Password);
        assert_eq!(prompt.host_text(), "alice@example.test:22");
    }

    #[test]
    fn keyboard_interactive_rounds_submit_answers_in_prompt_order() {
        let mut prompt = PendingAuthPrompt::new(
            "saved-1",
            "example.test",
            22,
            "alice",
            Some(AuthPromptMethod::KeyboardInteractive),
            Some(AuthMethods::parse("keyboard-interactive").expect("list")),
        );

        let round = prompt.begin_keyboard_round(&challenge(
            "SSH Server",
            "Please authenticate",
            &[("Password: ", false), ("Verification code: ", false)],
        ));
        assert_eq!(round.prompts.len(), 2);

        // 逐个写入作答（UI 每个输入框一次回调）；越界返回 false。
        assert!(!prompt.set_keyboard_answer(9, "out-of-range"));
        assert!(prompt.set_keyboard_answer(0, "secret"));
        assert!(prompt.set_keyboard_answer(1, "123456"));

        let response = prompt.submit_keyboard_round().expect("submit");
        assert_eq!(
            response,
            KeyboardInteractiveResponse::Answers(vec!["secret".to_owned(), "123456".to_owned(),])
        );

        // 第二轮（服务端一次认证里可以有多轮 challenge）。
        prompt.begin_keyboard_round(&challenge("", "", &[("One-time password: ", false)]));
        assert!(prompt.set_keyboard_answer(0, "otp-2"));
        let response = prompt.submit_keyboard_round().expect("second submit");
        assert_eq!(
            response,
            KeyboardInteractiveResponse::Answers(vec!["otp-2".to_owned()])
        );

        // 没有进行中的轮次时提交报错。
        assert_eq!(
            prompt.submit_keyboard_round().expect_err("no round"),
            AuthPromptError::NoKeyboardChallenge
        );
    }

    #[test]
    fn auth_prompt_method_indices_and_labels_are_stable() {
        for method in [
            AuthPromptMethod::Password,
            AuthPromptMethod::PublicKey,
            AuthPromptMethod::KeyboardInteractive,
        ] {
            assert_eq!(AuthPromptMethod::from_index(method.index()), Some(method));
            assert!(!method.label().is_empty());
        }
        assert_eq!(AuthPromptMethod::from_index(9), None);
    }

    #[test]
    fn configured_method_maps_from_session_auth_profiles() {
        let cases = [
            (
                ConfigAuthMethod::Password {
                    secret_key: "s".to_owned(),
                },
                AuthPromptMethod::Password,
            ),
            (
                ConfigAuthMethod::KeyboardInteractive {
                    secret_key: "s".to_owned(),
                },
                AuthPromptMethod::KeyboardInteractive,
            ),
            (
                ConfigAuthMethod::PrivateKey {
                    key_id: Some("key-1".to_owned()),
                    path: String::new(),
                    passphrase_secret_key: None,
                },
                AuthPromptMethod::PublicKey,
            ),
            (ConfigAuthMethod::Agent, AuthPromptMethod::PublicKey),
        ];
        for (method, expected) in cases {
            assert_eq!(AuthPromptMethod::from_config_method(&method), expected);
        }
    }

    #[test]
    fn successful_keyboard_rounds_keep_answers_for_refill_but_new_attempt_clears_the_round() {
        let mut prompt = PendingAuthPrompt::new(
            "saved-1",
            "example.test",
            22,
            "alice",
            Some(AuthPromptMethod::KeyboardInteractive),
            Some(AuthMethods::parse("keyboard-interactive").expect("list")),
        );
        prompt.begin_keyboard_round(&challenge("", "", &[("Password: ", false)]));
        prompt.set_keyboard_answer(0, "round-1");
        prompt.submit_keyboard_round().expect("submit");

        prompt.apply_problem(
            AuthMethods::parse("keyboard-interactive"),
            AuthProblemKind::InvalidCredentials,
        );

        // 失败后：没有进行中的轮次，但已提交的历史轮次仍保留（不清空已填内容）。
        assert!(prompt.active_keyboard_round.is_none());
        assert_eq!(prompt.keyboard_rounds.len(), 1);
        assert_eq!(
            prompt.keyboard_rounds[0].answers(),
            vec!["round-1".to_owned()]
        );
        assert_eq!(
            prompt.submit_keyboard_round().expect_err("cleared"),
            AuthPromptError::NoKeyboardChallenge
        );
    }

    #[test]
    fn select_rejects_methods_the_server_does_not_offer() {
        let mut prompt = PendingAuthPrompt::new(
            "saved-1",
            "example.test",
            22,
            "alice",
            None,
            Some(AuthMethods::parse("publickey").expect("list")),
        );

        assert_eq!(
            prompt
                .select(AuthPromptMethod::Password)
                .expect_err("hidden"),
            AuthPromptError::MethodNotAvailable(AuthPromptMethod::Password)
        );
        assert!(prompt
            .select(AuthPromptMethod::Password)
            .expect_err("hidden")
            .message()
            .contains("does not accept"));
        assert_eq!(prompt.selected(), AuthPromptMethod::PublicKey);
        prompt
            .select(AuthPromptMethod::KeyboardInteractive)
            .expect_err("hidden");
    }

    #[test]
    fn cancel_clears_sensitive_values_and_marks_the_window_cancelled() {
        let mut prompt = PendingAuthPrompt::new(
            "saved-1",
            "example.test",
            22,
            "alice",
            Some(AuthPromptMethod::KeyboardInteractive),
            None,
        );
        prompt.password_text = "hunter2".to_owned();
        prompt.passphrase_text = "key-pass".to_owned();
        prompt.begin_keyboard_round(&challenge("", "", &[("Password: ", false)]));
        prompt.set_keyboard_answer(0, "kbd-secret");

        prompt.cancel();

        assert!(prompt.cancelled);
        assert!(prompt.password_text.is_empty());
        assert!(prompt.passphrase_text.is_empty());
        assert!(prompt.active_keyboard_round.is_none());
    }
}
