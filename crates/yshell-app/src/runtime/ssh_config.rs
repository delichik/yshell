//! SSH/proxy/tunnel config assembly from resolved profiles and editor drafts,
//! plus secret resolution helpers.

use crate::{
    error::AppError, error::AppResult, session_runtime::SessionRuntime,
    session_runtime::SessionSource,
};
use yshell_config::{
    AuthMethod as ConfigAuthMethod, HostKeyPolicy as ConfigHostKeyPolicy, ProxyProfile,
    ProxyProtocol, ResolvedSessionProfile, TunnelForward, TunnelForwardKind,
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
        let theme =
            SessionThemeDraft::new(parent_profile, sources, resolved.session.terminal.as_ref());
        let mut draft = SessionEditorDraft {
            target_session_id: Some(resolved.session.id.clone()),
            target_folder_id,
            name: resolved.session.name.clone(),
            host: resolved.session.host.clone(),
            port_text: resolved.session.port.to_string(),
            username: resolved.session.username.clone().unwrap_or_default(),
            auth_method: EditorAuthMethod::Agent,
            host_key_policy: resolved
                .session
                .host_key_policy
                .unwrap_or(ConfigHostKeyPolicy::Strict),
            password: String::new(),
            key_path: String::new(),
            passphrase: String::new(),
            proxy_mode: EditorProxyMode::None,
            proxy_protocol: ProxyProtocol::Socks5,
            proxy_host: String::new(),
            proxy_port_text: String::new(),
            proxy_username: String::new(),
            proxy_password: String::new(),
            proxy_dns_by_proxy: true,
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
                }
                ConfigAuthMethod::KeyboardInteractive { .. } => {
                    draft.auth_method = EditorAuthMethod::KeyboardInteractive;
                }
                ConfigAuthMethod::PrivateKey { path, .. } => {
                    draft.auth_method = EditorAuthMethod::PrivateKey;
                    draft.key_path = path.clone();
                }
            }
        }
        if let Some(proxy) = &resolved.proxy {
            draft.proxy_mode = EditorProxyMode::Custom;
            draft.proxy_protocol = proxy.protocol;
            draft.proxy_host = proxy.host.clone();
            draft.proxy_port_text = proxy.port.to_string();
            draft.proxy_username = proxy.username.clone().unwrap_or_default();
            draft.proxy_dns_by_proxy = proxy.resolve_dns_by_proxy;
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
                path,
                passphrase_secret_key,
                ..
            }) => AuthMethod::PrivateKey {
                username,
                key_path: path.clone(),
                passphrase: passphrase_secret_key
                    .as_ref()
                    .map(|secret_key| self.resolve_secret_value(secret_key))
                    .transpose()?,
            },
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
                if self.editor.password.trim().is_empty() {
                    return Err(AppError::new("password auth requires a password value"));
                }
                AuthMethod::Password {
                    username,
                    password: self.editor.password.trim().to_owned(),
                }
            }
            EditorAuthMethod::KeyboardInteractive => {
                if self.editor.password.trim().is_empty() {
                    return Err(AppError::new(
                        "keyboard-interactive auth requires a response value",
                    ));
                }
                AuthMethod::KeyboardInteractive {
                    username,
                    secret: self.editor.password.trim().to_owned(),
                }
            }
            EditorAuthMethod::PrivateKey => {
                if self.editor.key_path.trim().is_empty() {
                    return Err(AppError::new("private key auth requires a key path"));
                }
                AuthMethod::PrivateKey {
                    username,
                    key_path: self.editor.key_path.trim().to_owned(),
                    passphrase: if self.editor.passphrase.trim().is_empty() {
                        None
                    } else {
                        Some(self.editor.passphrase.trim().to_owned())
                    },
                }
            }
        };
        let mut config = SshConnectionConfig::new(host.to_owned(), port, auth);
        config.host_key_policy = config_host_key_policy_to_runtime(self.editor.host_key_policy);
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
        let address = format!("{}:{}", proxy.host, proxy.port);
        let username = proxy.username.clone();
        let password = proxy
            .password_secret_key
            .as_ref()
            .map(|secret_key| self.resolve_secret_value(secret_key))
            .transpose()?;
        Ok(match proxy.protocol {
            ProxyProtocol::Socks4 => ProxyConfig::Socks4 { address, username },
            ProxyProtocol::Socks4a => ProxyConfig::Socks4a { address, username },
            ProxyProtocol::Socks5 => ProxyConfig::Socks5 {
                address,
                username,
                password,
                resolve_dns_by_proxy: proxy.resolve_dns_by_proxy,
            },
            ProxyProtocol::HttpConnect => ProxyConfig::HttpConnect {
                address,
                username,
                password,
            },
        })
    }

    pub(crate) fn build_proxy_config_from_editor(&self) -> AppResult<ProxyConfig> {
        if self.editor.proxy_mode == EditorProxyMode::None {
            return Ok(ProxyConfig::None);
        }
        let host = self.editor.proxy_host.trim();
        if host.is_empty() {
            return Err(AppError::new("proxy host must not be empty"));
        }
        let port = parse_port_field(
            &self.editor.proxy_port_text,
            "proxy port must be a valid integer between 1 and 65535",
        )?;
        let address = format!("{host}:{port}");
        let username = if self.editor.proxy_username.trim().is_empty() {
            None
        } else {
            Some(self.editor.proxy_username.trim().to_owned())
        };
        let password = if self.editor.proxy_password.trim().is_empty() {
            None
        } else {
            Some(self.editor.proxy_password.trim().to_owned())
        };
        Ok(match self.editor.proxy_protocol {
            ProxyProtocol::Socks4 => ProxyConfig::Socks4 { address, username },
            ProxyProtocol::Socks4a => ProxyConfig::Socks4a { address, username },
            ProxyProtocol::Socks5 => ProxyConfig::Socks5 {
                address,
                username,
                password,
                resolve_dns_by_proxy: self.editor.proxy_dns_by_proxy,
            },
            ProxyProtocol::HttpConnect => ProxyConfig::HttpConnect {
                address,
                username,
                password,
            },
        })
    }

    pub(crate) fn build_auth_profile_for_editor(
        &self,
        auth_profile_id: &str,
    ) -> AppResult<yshell_config::AuthProfile> {
        let method = match self.editor.auth_method {
            EditorAuthMethod::Agent => ConfigAuthMethod::Agent,
            EditorAuthMethod::Password => {
                if self.editor.password.trim().is_empty() {
                    return Err(AppError::new("password auth requires a password value"));
                }
                let secret_key = format!("local://yshell/{auth_profile_id}/password");
                self.store_secret_value(&secret_key, self.editor.password.trim())?;
                ConfigAuthMethod::Password { secret_key }
            }
            EditorAuthMethod::KeyboardInteractive => {
                if self.editor.password.trim().is_empty() {
                    return Err(AppError::new(
                        "keyboard-interactive auth requires a response value",
                    ));
                }
                let secret_key = format!("local://yshell/{auth_profile_id}/keyboard-interactive");
                self.store_secret_value(&secret_key, self.editor.password.trim())?;
                ConfigAuthMethod::KeyboardInteractive { secret_key }
            }
            EditorAuthMethod::PrivateKey => {
                if self.editor.key_path.trim().is_empty() {
                    return Err(AppError::new("private key auth requires a key path"));
                }
                let passphrase_secret_key = if self.editor.passphrase.trim().is_empty() {
                    None
                } else {
                    let secret_key = format!("local://yshell/{auth_profile_id}/passphrase");
                    self.store_secret_value(&secret_key, self.editor.passphrase.trim())?;
                    Some(secret_key)
                };
                ConfigAuthMethod::PrivateKey {
                    key_id: None,
                    path: self.editor.key_path.trim().to_owned(),
                    passphrase_secret_key,
                }
            }
        };
        Ok(yshell_config::AuthProfile {
            id: auth_profile_id.to_owned(),
            name: format!("Auth for {}", self.editor.name.trim()),
            method,
        })
    }

    pub(crate) fn build_proxy_profile_for_editor(
        &self,
        proxy_profile_id: &str,
    ) -> AppResult<Option<ProxyProfile>> {
        if self.editor.proxy_mode == EditorProxyMode::None {
            return Ok(None);
        }
        let host = self.editor.proxy_host.trim();
        if host.is_empty() {
            return Err(AppError::new("proxy host must not be empty"));
        }
        let port = parse_port_field(
            &self.editor.proxy_port_text,
            "proxy port must be a valid integer between 1 and 65535",
        )?;
        let password_secret_key = if self.editor.proxy_password.trim().is_empty() {
            None
        } else {
            match self.editor.proxy_protocol {
                ProxyProtocol::Socks4 | ProxyProtocol::Socks4a => None,
                ProxyProtocol::Socks5 | ProxyProtocol::HttpConnect => {
                    let secret_key = format!("local://yshell/{proxy_profile_id}/password");
                    self.store_secret_value(&secret_key, self.editor.proxy_password.trim())?;
                    Some(secret_key)
                }
            }
        };
        Ok(Some(ProxyProfile {
            id: proxy_profile_id.to_owned(),
            name: format!("Proxy for {}", self.editor.name.trim()),
            protocol: self.editor.proxy_protocol,
            host: host.to_owned(),
            port,
            username: if self.editor.proxy_username.trim().is_empty() {
                None
            } else {
                Some(self.editor.proxy_username.trim().to_owned())
            },
            resolve_dns_by_proxy: self.editor.proxy_dns_by_proxy,
            password_secret_key,
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
                backend: self.transport_backend.label().to_owned(),
                startup_snippet,
            })
        } else {
            Err(AppError::new(if startup_snippet.is_empty() {
                format!(
                    "auth test reached the `{}` backend path for {host_label}, but a live shell was not established",
                    self.transport_backend.label()
                )
            } else {
                format!(
                    "auth test reached the `{}` backend path for {host_label}, but a live shell was not established. Startup: {}",
                    self.transport_backend.label(),
                    startup_snippet
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

pub(crate) fn proxy_protocol_label(protocol: ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Socks4 => "socks4",
        ProxyProtocol::Socks4a => "socks4a",
        ProxyProtocol::Socks5 => "socks5",
        ProxyProtocol::HttpConnect => "http-connect",
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
