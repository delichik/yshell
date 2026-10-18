//! SSH/proxy/tunnel config assembly from resolved profiles and editor drafts,
//! plus secret resolution helpers.

use crate::{
    error::AppError, error::AppResult, session_runtime::SessionRuntime,
    session_runtime::SessionSource,
};
use yshell_config::{
    AuthMethod as ConfigAuthMethod, ResolvedSessionProfile, TunnelForward, TunnelForwardKind,
};
use yshell_ssh::{
    AuthMethod, ForwardingKind, HostKeyPolicy, ProxyConfig, SshConnectionConfig, TunnelConfig,
};

use super::editor::SessionThemeDraft;
use super::theme::ThemeSources;
use super::*;

impl AppRuntime {
    pub(crate) fn editor_from_resolved_session(
        &self,
        resolved: &ResolvedSessionProfile,
    ) -> SessionEditorDraft {
        let target_folder_id =
            find_session_folder_id(&self.config_document.folders, &resolved.session.id)
                .unwrap_or_else(|| SAVED_SESSIONS_FOLDER_ID.to_owned());
        let parent_profile = self.terminal_profile_for_draft(Some(&target_folder_id));
        let chain = self.config_document.folder_chain_to(&target_folder_id);
        let sources = ThemeSources::resolve(
            &chain,
            &self.config_document.terminal,
            &self.config_document.logging,
        );
        let theme = SessionThemeDraft::new(
            parent_profile,
            sources,
            resolved.session.terminal.as_ref(),
            &self.theme_catalog,
        );
        let mut draft = SessionEditorDraft {
            target_session_id: Some(resolved.session.id.clone()),
            auth_profile_id: resolved.auth.as_ref().map(|auth| auth.id.clone()),
            target_folder_id,
            name: resolved.session.name.clone(),
            host: resolved.session.host.clone(),
            port_text: resolved.session.port.to_string(),
            username: resolved.session.username.clone().unwrap_or_default(),
            auth_method: EditorAuthMethod::Agent,
            password: String::new(),
            key_id: None,
            legacy_key_path: String::new(),
            legacy_key_has_passphrase: false,
            has_stored_password: false,
            proxy_mode: EditorProxyMode::None,
            proxy_profile_id: None,
            tunnel_kind: TunnelForwardKind::Local,
            tunnel_bind_host: "127.0.0.1".to_owned(),
            tunnel_bind_port_text: String::new(),
            tunnel_target_host: String::new(),
            tunnel_target_port_text: String::new(),
            tunnels: resolved.tunnel.forwards.clone(),
            theme,
        };
        if let Some(auth) = &resolved.auth {
            match &auth.method {
                ConfigAuthMethod::Agent => {
                    draft.auth_method = EditorAuthMethod::Agent;
                }
                ConfigAuthMethod::Password { .. } => {
                    draft.auth_method = EditorAuthMethod::Password;
                    draft.has_stored_password = true;
                }
                ConfigAuthMethod::KeyboardInteractive { .. } => {
                    draft.auth_method = EditorAuthMethod::KeyboardInteractive;
                    draft.has_stored_password = true;
                }
                ConfigAuthMethod::PrivateKey {
                    key_id,
                    path,
                    passphrase_secret_key,
                } => {
                    draft.auth_method = EditorAuthMethod::PrivateKey;
                    // SSH-1：优先按 key_id 呈现托管密钥；缺失的 key_id 回退到旧 path
                    // （与 `ConfigDocument::resolve_private_key_source` 一致），旧 path
                    // 只读展示并标记 legacy。
                    let managed_key_id = key_id
                        .as_deref()
                        .filter(|key_id| self.config_document.find_key(key_id).is_some());
                    if let Some(key_id) = managed_key_id {
                        draft.key_id = Some(key_id.to_owned());
                    } else if !path.is_empty() {
                        draft.legacy_key_path = path.clone();
                        draft.legacy_key_has_passphrase = passphrase_secret_key.is_some();
                    } else if let Some(key_id) = key_id {
                        // 悬空 key_id（没有 legacy path 可回退）：投影为"选中密钥已不可用"。
                        draft.key_id = Some(key_id.clone());
                    }
                }
            }
        }
        // NET-1：托管代理（proxy_profile_id → 管理器条目）与旧 per-session 代理
        // （`<session_id>-proxy` 或悬空引用）分开呈现；legacy 保持只读但可连接。
        if let Some(proxy_id) = resolved.session.proxy_profile_id.as_deref() {
            draft.proxy_profile_id = Some(proxy_id.to_owned());
            if resolved.proxy.is_some() && !is_legacy_proxy_profile_id(&resolved.session.id, proxy_id)
            {
                draft.proxy_mode = EditorProxyMode::Managed;
            } else {
                draft.proxy_mode = EditorProxyMode::Legacy;
            }
        }
        draft
    }

    pub(crate) fn effective_ssh_config(
        &self,
        ssh_config: &SshConnectionConfig,
    ) -> SshConnectionConfig {
        let mut effective_config = ssh_config.clone();
        let mut known_hosts = self.persistent_known_hosts.clone();
        known_hosts.merge(&self.temporary_known_hosts);
        effective_config.known_hosts = known_hosts;
        if let Some(policy) = &self.host_key_policy_override {
            effective_config.host_key_policy = policy.clone();
        }
        effective_config
    }

    pub(crate) fn resolve_runtime_ssh_config(
        &self,
        runtime: &SessionRuntime,
    ) -> AppResult<SshConnectionConfig> {
        match &runtime.source {
            SessionSource::SavedSession { profile_id } => {
                let resolved = self
                    .config_document
                    .resolve_session(profile_id)
                    .ok_or_else(|| {
                        AppError::new(format!(
                            "saved session `{profile_id}` could not be resolved from config"
                        ))
                    })?;
                self.build_ssh_config_from_resolved_session(&resolved)
            }
            SessionSource::QuickConnect | SessionSource::Draft => Ok(runtime.ssh_config.clone()),
            // TAB-4：本地终端没有 SSH 配置；误入连接路径时直接失败，绝不发网络请求。
            SessionSource::Local => Err(AppError::new(
                "local terminal sessions do not have an SSH configuration",
            )),
        }
    }

    pub(crate) fn build_ssh_config_from_resolved_session(
        &self,
        resolved: &ResolvedSessionProfile,
    ) -> AppResult<SshConnectionConfig> {
        self.build_ssh_config_from_resolved_session_inner(resolved, None)
    }

    pub(crate) fn build_ssh_config_from_resolved_session_with_password(
        &self,
        resolved: &ResolvedSessionProfile,
        password: &str,
    ) -> AppResult<SshConnectionConfig> {
        self.build_ssh_config_from_resolved_session_inner(resolved, Some(password))
    }

    pub(crate) fn build_ssh_config_from_resolved_session_inner(
        &self,
        resolved: &ResolvedSessionProfile,
        password_override: Option<&str>,
    ) -> AppResult<SshConnectionConfig> {
        let username = resolved
            .session
            .username
            .clone()
            .unwrap_or_else(|| "user".to_owned());
        let auth = match resolved.auth.as_ref().map(|profile| &profile.method) {
            Some(ConfigAuthMethod::Password { secret_key }) => AuthMethod::Password {
                username,
                password: self.resolve_secret_value_with_override(secret_key, password_override)?,
            },
            Some(ConfigAuthMethod::KeyboardInteractive { secret_key }) => {
                AuthMethod::KeyboardInteractive {
                    username,
                    secret: self
                        .resolve_secret_value_with_override(secret_key, password_override)?,
                }
            }
            Some(ConfigAuthMethod::PrivateKey {
                key_id,
                path,
                passphrase_secret_key,
            }) => self.resolve_private_key_auth(
                username,
                key_id.as_deref(),
                path,
                passphrase_secret_key.as_deref(),
            )?,
            Some(ConfigAuthMethod::Agent) | None => AuthMethod::Agent { username },
        };
        let mut config =
            SshConnectionConfig::new(resolved.session.host.clone(), resolved.session.port, auth);
        config.host_key_policy = resolved
            .session
            .host_key_policy
            .map(config_host_key_policy_to_runtime)
            .unwrap_or(HostKeyPolicy::Strict);
        config.proxy = self.build_proxy_config_from_resolved_session(resolved)?;
        config.tunnels = resolved
            .tunnel
            .forwards
            .iter()
            .cloned()
            .map(tunnel_forward_to_config)
            .collect();
        Ok(config)
    }

    /// SSH-1：把保存的 `AuthMethod::PrivateKey` 解析成运行时可用的私钥认证。
    ///
    /// 托管密钥优先：私钥材料从 secret store 取出并物化为 `0600` 文件，口令使用密钥
    /// 管理器里保存的口令；只有 `key_id` 缺失/悬空时才回退到旧 `path`（legacy）。
    fn resolve_private_key_auth(
        &self,
        username: String,
        key_id: Option<&str>,
        legacy_path: &str,
        passphrase_secret_key: Option<&str>,
    ) -> AppResult<AuthMethod> {
        if let Some(key_id) =
            key_id.filter(|key_id| self.config_document.find_key(key_id).is_some())
        {
            let key_path = self.materialize_private_key_file(key_id)?;
            return Ok(AuthMethod::PrivateKey {
                username,
                key_path: key_path.display().to_string(),
                passphrase: self.private_key_passphrase(key_id),
            });
        }
        if !legacy_path.is_empty() {
            let passphrase = passphrase_secret_key
                .map(|secret_key| self.resolve_secret_value(secret_key))
                .transpose()?;
            return Ok(AuthMethod::PrivateKey {
                username,
                key_path: legacy_path.to_owned(),
                passphrase,
            });
        }
        match key_id {
            Some(key_id) => Err(AppError::new(format!(
                "saved session references managed key `{key_id}`, but it no longer exists; edit the session and choose another key"
            ))),
            None => Err(AppError::new(
                "private key auth has neither a managed key nor a legacy key path",
            )),
        }
    }

    pub(crate) fn build_ssh_config_from_editor(&self) -> AppResult<SshConnectionConfig> {
        let host = self.editor.host.trim();
        if host.is_empty() {
            return Err(AppError::new("editor host must not be empty"));
        }
        let port = self.editor.port_text.trim().parse::<u16>().map_err(|_| {
            AppError::new("editor port must be a valid integer between 1 and 65535")
        })?;
        if port == 0 {
            return Err(AppError::new("editor port must be greater than zero"));
        }
        let username = if self.editor.username.trim().is_empty() {
            "user".to_owned()
        } else {
            self.editor.username.trim().to_owned()
        };
        let auth = match self.editor.auth_method {
            EditorAuthMethod::Agent => AuthMethod::Agent { username },
            EditorAuthMethod::Password => {
                let password = self.resolve_editor_password_secret(false)?;
                AuthMethod::Password { username, password }
            }
            EditorAuthMethod::KeyboardInteractive => {
                let secret = self.resolve_editor_password_secret(true)?;
                AuthMethod::KeyboardInteractive { username, secret }
            }
            EditorAuthMethod::PrivateKey => {
                if let Some(key_id) = self.editor.key_id.as_deref() {
                    if self.config_document.find_key(key_id).is_none() {
                        return Err(AppError::new(format!(
                            "the selected managed key `{key_id}` is no longer available; choose another key"
                        )));
                    }
                    // 与保存的托管密钥会话走同一条解析路径（材料物化 + 已存口令）。
                    AuthMethod::PrivateKey {
                        username,
                        key_path: self
                            .materialize_private_key_file(key_id)?
                            .display()
                            .to_string(),
                        passphrase: self.private_key_passphrase(key_id),
                    }
                } else if !self.editor.legacy_key_path.trim().is_empty() {
                    AuthMethod::PrivateKey {
                        username,
                        key_path: self.editor.legacy_key_path.trim().to_owned(),
                        passphrase: self.resolve_editor_legacy_passphrase_secret()?,
                    }
                } else {
                    return Err(AppError::new(
                        "private key auth requires selecting a managed key",
                    ));
                }
            }
        };
        let mut config = SshConnectionConfig::new(host.to_owned(), port, auth);
        // 会话编辑器不再选择主机密钥策略：`SshConnectionConfig::new` 的默认值即
        // Strict（known_hosts 严格校验）；测试/自动化的 accept-any 覆盖由
        // `effective_ssh_config` 的 `host_key_policy_override` 统一施加。
        // 旧配置里的 `host_key_policy` 仅在按已保存会话解析时生效（见上方
        // `build_ssh_config_from_resolved_session_inner`），编辑器草稿不携带它。
        config.proxy = self.build_proxy_config_from_editor()?;
        config.tunnels = self
            .editor
            .tunnels
            .iter()
            .cloned()
            .map(tunnel_forward_to_config)
            .collect();
        Ok(self.effective_ssh_config(&config))
    }

    pub(crate) fn build_proxy_config_from_resolved_session(
        &self,
        resolved: &ResolvedSessionProfile,
    ) -> AppResult<ProxyConfig> {
        let Some(proxy) = &resolved.proxy else {
            return Ok(ProxyConfig::None);
        };
        self.proxy_config_from_profile(proxy)
    }

    /// NET-1：会话编辑器草稿的代理配置。
    ///
    /// `Managed` 从管理器条目解析；`Legacy` 保留旧 per-session 引用（仍可连接）；
    /// `None` 关闭代理。编辑会话永远不会创建/改写代理条目。
    pub(crate) fn build_proxy_config_from_editor(&self) -> AppResult<ProxyConfig> {
        let proxy_id = match self.editor.proxy_mode {
            EditorProxyMode::None => return Ok(ProxyConfig::None),
            EditorProxyMode::Managed | EditorProxyMode::Legacy => self
                .editor
                .proxy_profile_id
                .as_deref()
                .ok_or_else(|| AppError::new("select a proxy profile or choose No Proxy"))?,
        };
        let Some(profile) = self.config_document.proxy_profiles.get(proxy_id) else {
            return Err(AppError::new(match self.editor.proxy_mode {
                EditorProxyMode::Legacy => format!(
                    "this session references a legacy proxy `{proxy_id}` that no longer exists; choose No Proxy or a managed proxy"
                ),
                _ => format!(
                    "the selected managed proxy `{proxy_id}` is no longer available; choose another proxy"
                ),
            }));
        };
        self.proxy_config_from_profile(profile)
    }

    pub(crate) fn build_auth_profile_for_editor(
        &self,
        auth_profile_id: &str,
    ) -> AppResult<yshell_config::AuthProfile> {
        let existing = self.config_document.auth_profiles.get(auth_profile_id);
        let method = match self.editor.auth_method {
            EditorAuthMethod::Agent => ConfigAuthMethod::Agent,
            EditorAuthMethod::Password => {
                let secret_key = self.resolve_or_store_editor_password_secret(
                    auth_profile_id,
                    "password",
                    false,
                    existing,
                )?;
                ConfigAuthMethod::Password { secret_key }
            }
            EditorAuthMethod::KeyboardInteractive => {
                let secret_key = self.resolve_or_store_editor_password_secret(
                    auth_profile_id,
                    "keyboard-interactive",
                    true,
                    existing,
                )?;
                ConfigAuthMethod::KeyboardInteractive { secret_key }
            }
            EditorAuthMethod::PrivateKey => {
                if let Some(key_id) = self.editor.key_id.as_deref() {
                    if self.config_document.find_key(key_id).is_none() {
                        return Err(AppError::new(format!(
                            "the selected managed key `{key_id}` is no longer available; choose another key"
                        )));
                    }
                    // SSH-1：会话只保存 key_id；私钥材料/口令由密钥管理器与 secret store
                    // 负责，不再写入会话级 path/passphrase。
                    ConfigAuthMethod::PrivateKey {
                        key_id: Some(key_id.to_owned()),
                        path: String::new(),
                        passphrase_secret_key: None,
                    }
                } else if !self.editor.legacy_key_path.trim().is_empty() {
                    // 旧 path-only 会话保持可连接：未选择托管密钥时原样保留 path（以及
                    // 已保存口令的 secret 引用），避免仅改名/改端口就破坏旧配置。
                    let passphrase_secret_key = if self.editor.legacy_key_has_passphrase {
                        existing.and_then(|profile| match &profile.method {
                            ConfigAuthMethod::PrivateKey {
                                passphrase_secret_key,
                                ..
                            } => passphrase_secret_key.clone(),
                            _ => None,
                        })
                    } else {
                        None
                    };
                    ConfigAuthMethod::PrivateKey {
                        key_id: None,
                        path: self.editor.legacy_key_path.trim().to_owned(),
                        passphrase_secret_key,
                    }
                } else {
                    return Err(AppError::new(
                        "private key auth requires selecting a managed key",
                    ));
                }
            }
        };
        Ok(yshell_config::AuthProfile {
            id: auth_profile_id.to_owned(),
            name: format!("Auth for {}", self.editor.name.trim()),
            method,
        })
    }

    /// Resolves the typed password, or the stored secret when editing and the
    /// field was left blank ("keep existing").
    fn resolve_editor_password_secret(&self, keyboard_interactive: bool) -> AppResult<String> {
        if !self.editor.password.trim().is_empty() {
            return Ok(self.editor.password.trim().to_owned());
        }
        if !self.editor.has_stored_password {
            return Err(AppError::new(if keyboard_interactive {
                "keyboard-interactive auth requires a response value"
            } else {
                "password auth requires a password value"
            }));
        }
        let Some(auth_profile_id) = self.editor_auth_profile_id() else {
            return Err(AppError::new(if keyboard_interactive {
                "keyboard-interactive auth requires a response value"
            } else {
                "password auth requires a password value"
            }));
        };
        let secret_key = self
            .config_document
            .auth_profiles
            .get(&auth_profile_id)
            .and_then(|profile| match &profile.method {
                ConfigAuthMethod::Password { secret_key } if !keyboard_interactive => {
                    Some(secret_key.as_str())
                }
                ConfigAuthMethod::KeyboardInteractive { secret_key } if keyboard_interactive => {
                    Some(secret_key.as_str())
                }
                _ => None,
            })
            .ok_or_else(|| {
                AppError::new(if keyboard_interactive {
                    "keyboard-interactive auth requires a response value"
                } else {
                    "password auth requires a password value"
                })
            })?;
        self.resolve_secret_value(secret_key)
    }

    /// SSH-1：legacy path-only 私钥的已存口令（旧会话保持可连接）。
    fn resolve_editor_legacy_passphrase_secret(&self) -> AppResult<Option<String>> {
        if !self.editor.legacy_key_has_passphrase {
            return Ok(None);
        }
        let Some(auth_profile_id) = self.editor_auth_profile_id() else {
            return Ok(None);
        };
        let Some(secret_key) = self
            .config_document
            .auth_profiles
            .get(&auth_profile_id)
            .and_then(|profile| match &profile.method {
                ConfigAuthMethod::PrivateKey {
                    passphrase_secret_key,
                    ..
                } => passphrase_secret_key.clone(),
                _ => None,
            })
        else {
            return Ok(None);
        };
        Ok(Some(self.resolve_secret_value(&secret_key)?))
    }

    fn resolve_or_store_editor_password_secret(
        &self,
        auth_profile_id: &str,
        key_suffix: &str,
        keyboard_interactive: bool,
        existing: Option<&yshell_config::AuthProfile>,
    ) -> AppResult<String> {
        if !self.editor.password.trim().is_empty() {
            let secret_key = format!("local://yshell/{auth_profile_id}/{key_suffix}");
            self.store_secret_value(&secret_key, self.editor.password.trim())?;
            return Ok(secret_key);
        }
        if self.editor.has_stored_password {
            if let Some(profile) = existing {
                match &profile.method {
                    ConfigAuthMethod::Password { secret_key } if !keyboard_interactive => {
                        return Ok(secret_key.clone());
                    }
                    ConfigAuthMethod::KeyboardInteractive { secret_key }
                        if keyboard_interactive =>
                    {
                        return Ok(secret_key.clone());
                    }
                    _ => {}
                }
            }
            // Stable key when the profile id is unchanged but method metadata drifted.
            return Ok(format!("local://yshell/{auth_profile_id}/{key_suffix}"));
        }
        Err(AppError::new(if keyboard_interactive {
            "keyboard-interactive auth requires a response value"
        } else {
            "password auth requires a password value"
        }))
    }

    pub(crate) fn run_editor_auth_test(&mut self) -> AppResult<EditorAuthTestStatus> {
        let ssh_config = self.build_ssh_config_from_editor()?;
        let host_label = format!(
            "{}@{}:{}",
            ssh_config.username(),
            ssh_config.host,
            ssh_config.port
        );
        let mut shell = self
            .open_shell_for_runtime(&ssh_config)
            .map_err(AppError::from_error)?;
        let connected = shell.is_connected();
        let startup_output = shell.poll_output().map_err(AppError::from_error)?;
        shell.disconnect().map_err(AppError::from_error)?;
        let startup_snippet = String::from_utf8_lossy(&startup_output)
            .split_whitespace()
            .take(18)
            .collect::<Vec<_>>()
            .join(" ");
        if connected {
            Ok(EditorAuthTestStatus::Success {
                host_label,
                startup_snippet,
            })
        } else {
            Err(AppError::new(if startup_snippet.is_empty() {
                format!(
                    "auth test for {host_label} did not establish a live shell"
                )
            } else {
                format!(
                    "auth test for {host_label} did not establish a live shell. Startup: {startup_snippet}"
                )
            }))
        }
    }

    pub(crate) fn build_editor_tunnel_forward(&self) -> AppResult<TunnelForward> {
        let bind_host = self.editor.tunnel_bind_host.trim();
        if bind_host.is_empty() {
            return Err(AppError::new("tunnel listen host must not be empty"));
        }
        let bind_port = parse_port_field(
            &self.editor.tunnel_bind_port_text,
            "tunnel listen port must be a valid integer between 1 and 65535",
        )?;
        let (target_host, target_port) = if self.editor.tunnel_kind == TunnelForwardKind::Dynamic {
            (String::new(), 0)
        } else {
            let host = self.editor.tunnel_target_host.trim();
            if host.is_empty() {
                return Err(AppError::new("tunnel target host must not be empty"));
            }
            let port = parse_port_field(
                &self.editor.tunnel_target_port_text,
                "tunnel target port must be a valid integer between 1 and 65535",
            )?;
            (host.to_owned(), port)
        };
        Ok(TunnelForward {
            kind: self.editor.tunnel_kind,
            bind_host: bind_host.to_owned(),
            bind_port,
            target_host,
            target_port,
        })
    }
}

pub(crate) fn tunnel_kind_label(kind: TunnelForwardKind) -> &'static str {
    match kind {
        TunnelForwardKind::Local => "local",
        TunnelForwardKind::Remote => "remote",
        TunnelForwardKind::Dynamic => "dynamic",
    }
}

pub(crate) fn tunnel_forward_summary(forward: &TunnelForward) -> String {
    match forward.kind {
        TunnelForwardKind::Dynamic => format!(
            "{} {}:{}",
            tunnel_kind_label(forward.kind),
            forward.bind_host,
            forward.bind_port
        ),
        TunnelForwardKind::Local | TunnelForwardKind::Remote => format!(
            "{} {}:{} -> {}:{}",
            tunnel_kind_label(forward.kind),
            forward.bind_host,
            forward.bind_port,
            forward.target_host,
            forward.target_port
        ),
    }
}

/// I18N-1：隧道端点摘要（不含 `local/remote/dynamic` 英文前缀）。
///
/// 状态栏模板把类型单独翻译（`TextFormats.tunnel-kind-label`），这里只保留
/// `bind -> target` 这种与语言无关的技术参数。
pub(crate) fn tunnel_forward_summary_body(forward: &TunnelForward) -> String {
    match forward.kind {
        TunnelForwardKind::Dynamic => format!("{}:{}", forward.bind_host, forward.bind_port),
        TunnelForwardKind::Local | TunnelForwardKind::Remote => format!(
            "{}:{} -> {}:{}",
            forward.bind_host,
            forward.bind_port,
            forward.target_host,
            forward.target_port
        ),
    }
}

pub(crate) fn tunnel_forward_to_config(forward: TunnelForward) -> TunnelConfig {
    TunnelConfig {
        id: format!(
            "{}-{}-{}",
            tunnel_kind_label(forward.kind),
            forward.bind_host.replace(':', "-"),
            forward.bind_port
        ),
        kind: match forward.kind {
            TunnelForwardKind::Local => ForwardingKind::Local,
            TunnelForwardKind::Remote => ForwardingKind::Remote,
            TunnelForwardKind::Dynamic => ForwardingKind::Dynamic,
        },
        listen_host: forward.bind_host,
        listen_port: forward.bind_port,
        target_host: forward.target_host,
        target_port: forward.target_port,
    }
}

pub(crate) fn tunnel_config_summary(config: &TunnelConfig) -> String {
    match config.kind {
        ForwardingKind::Dynamic => {
            format!("dynamic {}:{}", config.listen_host, config.listen_port)
        }
        ForwardingKind::Local => format!(
            "local {}:{} -> {}:{}",
            config.listen_host, config.listen_port, config.target_host, config.target_port
        ),
        ForwardingKind::Remote => format!(
            "remote {}:{} -> {}:{}",
            config.listen_host, config.listen_port, config.target_host, config.target_port
        ),
    }
}
