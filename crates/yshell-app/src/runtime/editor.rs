//! Session editor draft state: field updates, section selection, folder choice
//! and persistence.

use crate::{error::AppError, error::AppResult};
use yshell_config::{
    FolderProfile, HostKeyPolicy as ConfigHostKeyPolicy, ProxyProtocol, SessionProfile,
    TunnelForward, TunnelForwardKind, TunnelProfile,
};

use super::*;

impl AppRuntime {
    pub fn start_new_saved_session_editor(&mut self) -> AppProjection {
        self.editor = SessionEditorDraft::default();
        self.new_folder_name.clear();
        self.editor_modal_visible = true;
        self.editor_section = EditorSection::General;
        self.editor_auth_test_status = EditorAuthTestStatus::Hint;
        self.status_text =
            "Session editor cleared. Fill in the fields to create a saved session.".to_owned();
        self.projection()
    }

    pub fn load_selected_saved_session_into_editor(&mut self) -> AppResult<AppProjection> {
        let profile_id = self
            .selected_saved_session_id
            .clone()
            .ok_or_else(|| AppError::new("no saved session is currently selected"))?;
        let resolved = self
            .config_document
            .resolve_session(&profile_id)
            .ok_or_else(|| AppError::new(format!("saved session `{profile_id}` was not found")))?;
        self.editor = self.editor_from_resolved_session(&resolved);
        self.editor_modal_visible = true;
        self.editor_section = EditorSection::General;
        self.editor_auth_test_status = EditorAuthTestStatus::LoadedHint;
        self.status_text = format!("Loaded saved session `{profile_id}` into the editor.");
        Ok(self.projection())
    }

    pub fn close_session_editor_modal(&mut self) -> AppProjection {
        self.editor_modal_visible = false;
        self.status_text = "Closed Session Editor.".to_owned();
        self.projection()
    }

    pub fn select_session_editor_general(&mut self) -> AppProjection {
        self.editor_section = EditorSection::General;
        self.projection()
    }

    pub fn select_session_editor_authentication(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Authentication;
        self.projection()
    }

    pub fn select_session_editor_terminal(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Terminal;
        self.projection()
    }

    pub fn select_session_editor_sftp(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Sftp;
        self.projection()
    }

    pub fn select_session_editor_tunnels(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Tunnels;
        self.projection()
    }

    pub fn select_session_editor_proxy(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Proxy;
        self.projection()
    }

    pub fn select_session_editor_logging(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Logging;
        self.projection()
    }

    pub fn select_session_editor_advanced(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Advanced;
        self.projection()
    }

    pub fn select_session_editor_appearance(&mut self) -> AppProjection {
        self.editor_section = EditorSection::Appearance;
        self.projection()
    }

    pub fn update_new_folder_name(&mut self, value: &str) -> AppProjection {
        self.new_folder_name = value.to_owned();
        self.projection()
    }

    pub fn select_next_editor_folder(&mut self) -> AppProjection {
        self.rotate_editor_folder(true);
        self.status_text = format!("Editor target folder: {}", self.editor_folder_legacy_text());
        self.projection()
    }

    pub fn select_previous_editor_folder(&mut self) -> AppProjection {
        self.rotate_editor_folder(false);
        self.status_text = format!("Editor target folder: {}", self.editor_folder_legacy_text());
        self.projection()
    }

    pub fn update_editor_name(&mut self, value: &str) -> AppProjection {
        self.editor.name = value.to_owned();
        self.projection()
    }

    pub fn update_editor_host(&mut self, value: &str) -> AppProjection {
        self.editor.host = value.to_owned();
        self.projection()
    }

    pub fn update_editor_port(&mut self, value: &str) -> AppProjection {
        self.editor.port_text = value.to_owned();
        self.projection()
    }

    pub fn update_editor_username(&mut self, value: &str) -> AppProjection {
        self.editor.username = value.to_owned();
        self.projection()
    }

    pub fn set_editor_auth_method_agent(&mut self) -> AppProjection {
        self.editor.auth_method = EditorAuthMethod::Agent;
        self.projection()
    }

    pub fn set_editor_auth_method_password(&mut self) -> AppProjection {
        self.editor.auth_method = EditorAuthMethod::Password;
        self.projection()
    }

    pub fn set_editor_auth_method_keyboard_interactive(&mut self) -> AppProjection {
        self.editor.auth_method = EditorAuthMethod::KeyboardInteractive;
        self.projection()
    }

    pub fn set_editor_auth_method_private_key(&mut self) -> AppProjection {
        self.editor.auth_method = EditorAuthMethod::PrivateKey;
        self.projection()
    }

    pub fn set_editor_host_key_policy_strict(&mut self) -> AppProjection {
        self.editor.host_key_policy = ConfigHostKeyPolicy::Strict;
        self.projection()
    }

    pub fn set_editor_host_key_policy_trust_on_first_use(&mut self) -> AppProjection {
        self.editor.host_key_policy = ConfigHostKeyPolicy::TrustOnFirstUse;
        self.projection()
    }

    pub fn set_editor_host_key_policy_accept_any_for_testing(&mut self) -> AppProjection {
        self.editor.host_key_policy = ConfigHostKeyPolicy::AcceptAnyForTesting;
        self.projection()
    }

    pub fn update_editor_password(&mut self, value: &str) -> AppProjection {
        self.editor.password = value.to_owned();
        self.projection()
    }

    pub fn update_editor_key_path(&mut self, value: &str) -> AppProjection {
        self.editor.key_path = value.to_owned();
        self.projection()
    }

    pub fn update_editor_passphrase(&mut self, value: &str) -> AppProjection {
        self.editor.passphrase = value.to_owned();
        self.projection()
    }

    pub fn set_editor_proxy_mode_none(&mut self) -> AppProjection {
        self.editor.proxy_mode = EditorProxyMode::None;
        self.projection()
    }

    pub fn set_editor_proxy_mode_custom(&mut self) -> AppProjection {
        self.editor.proxy_mode = EditorProxyMode::Custom;
        self.projection()
    }

    pub fn set_editor_proxy_protocol_socks4(&mut self) -> AppProjection {
        self.editor.proxy_protocol = ProxyProtocol::Socks4;
        self.projection()
    }

    pub fn set_editor_proxy_protocol_socks4a(&mut self) -> AppProjection {
        self.editor.proxy_protocol = ProxyProtocol::Socks4a;
        self.projection()
    }

    pub fn set_editor_proxy_protocol_socks5(&mut self) -> AppProjection {
        self.editor.proxy_protocol = ProxyProtocol::Socks5;
        self.projection()
    }

    pub fn set_editor_proxy_protocol_http_connect(&mut self) -> AppProjection {
        self.editor.proxy_protocol = ProxyProtocol::HttpConnect;
        self.projection()
    }

    pub fn update_editor_proxy_host(&mut self, value: &str) -> AppProjection {
        self.editor.proxy_host = value.to_owned();
        self.projection()
    }

    pub fn update_editor_proxy_port(&mut self, value: &str) -> AppProjection {
        self.editor.proxy_port_text = value.to_owned();
        self.projection()
    }

    pub fn update_editor_proxy_username(&mut self, value: &str) -> AppProjection {
        self.editor.proxy_username = value.to_owned();
        self.projection()
    }

    pub fn update_editor_proxy_password(&mut self, value: &str) -> AppProjection {
        self.editor.proxy_password = value.to_owned();
        self.projection()
    }

    pub fn toggle_editor_proxy_dns_by_proxy(&mut self) -> AppProjection {
        self.editor.proxy_dns_by_proxy = !self.editor.proxy_dns_by_proxy;
        self.projection()
    }

    pub fn set_editor_tunnel_kind_local(&mut self) -> AppProjection {
        self.editor.tunnel_kind = TunnelForwardKind::Local;
        self.projection()
    }

    pub fn set_editor_tunnel_kind_remote(&mut self) -> AppProjection {
        self.editor.tunnel_kind = TunnelForwardKind::Remote;
        self.projection()
    }

    pub fn set_editor_tunnel_kind_dynamic(&mut self) -> AppProjection {
        self.editor.tunnel_kind = TunnelForwardKind::Dynamic;
        self.projection()
    }

    pub fn update_editor_tunnel_bind_host(&mut self, value: &str) -> AppProjection {
        self.editor.tunnel_bind_host = value.to_owned();
        self.projection()
    }

    pub fn update_editor_tunnel_bind_port(&mut self, value: &str) -> AppProjection {
        self.editor.tunnel_bind_port_text = value.to_owned();
        self.projection()
    }

    pub fn update_editor_tunnel_target_host(&mut self, value: &str) -> AppProjection {
        self.editor.tunnel_target_host = value.to_owned();
        self.projection()
    }

    pub fn update_editor_tunnel_target_port(&mut self, value: &str) -> AppProjection {
        self.editor.tunnel_target_port_text = value.to_owned();
        self.projection()
    }

    pub fn add_editor_tunnel_forward(&mut self) -> AppResult<AppProjection> {
        let forward = self.build_editor_tunnel_forward()?;
        self.editor.tunnels.push(forward.clone());
        self.editor.tunnel_bind_port_text.clear();
        self.editor.tunnel_target_host.clear();
        self.editor.tunnel_target_port_text.clear();
        self.status_text = format!(
            "Added {} tunnel {}.",
            tunnel_kind_label(forward.kind),
            tunnel_forward_summary(&forward)
        );
        Ok(self.projection())
    }

    pub fn clear_editor_tunnels(&mut self) -> AppProjection {
        self.editor.tunnels.clear();
        self.status_text = "Cleared all editor tunnel forwards.".to_owned();
        self.projection()
    }

    pub fn test_editor_auth(&mut self) -> AppProjection {
        let status = match self.run_editor_auth_test() {
            Ok(status) => status,
            Err(error) => EditorAuthTestStatus::Failed {
                error: error.to_string(),
            },
        };
        self.status_text = status.legacy_text();
        self.editor_auth_test_status = status;
        self.projection()
    }

    pub fn save_editor_to_saved_session(&mut self) -> AppResult<AppProjection> {
        let session_id = self.persist_editor_to_saved_session()?;
        self.editor_modal_visible = false;
        self.status_text = format!("Saved session editor changes into `{session_id}`.");
        Ok(self.projection())
    }

    pub fn save_editor_and_connect(&mut self) -> AppResult<AppProjection> {
        let session_id = self.persist_editor_to_saved_session()?;
        self.editor_modal_visible = false;
        let projection = self.open_saved_session(&session_id)?;
        Ok(projection)
    }

    pub(crate) fn persist_editor_to_saved_session(&mut self) -> AppResult<String> {
        let port = self.editor.port_text.trim().parse::<u16>().map_err(|_| {
            AppError::new("editor port must be a valid integer between 1 and 65535")
        })?;
        if port == 0 {
            return Err(AppError::new("editor port must be greater than zero"));
        }
        if self.editor.name.trim().is_empty() {
            return Err(AppError::new("editor session name must not be empty"));
        }
        if self.editor.host.trim().is_empty() {
            return Err(AppError::new("editor host must not be empty"));
        }

        let session_id = self
            .editor
            .target_session_id
            .clone()
            .unwrap_or_else(|| self.next_saved_profile_id(&self.editor.host));
        let auth_profile_id = format!("{session_id}-auth");
        let auth_profile = self.build_auth_profile_for_editor(&auth_profile_id)?;
        let proxy_profile_id = format!("{session_id}-proxy");
        let proxy_profile = self.build_proxy_profile_for_editor(&proxy_profile_id)?;
        let target_folder_id = self.editor.target_folder_id.trim().to_owned();
        if target_folder_id.is_empty() {
            return Err(AppError::new("editor target folder must not be empty"));
        }

        let mut profile = SessionProfile::new(
            session_id.clone(),
            self.editor.name.trim().to_owned(),
            self.editor.host.trim().to_owned(),
        );
        profile.port = port;
        profile.username = if self.editor.username.trim().is_empty() {
            None
        } else {
            Some(self.editor.username.trim().to_owned())
        };
        profile.host_key_policy = Some(self.editor.host_key_policy);
        profile.auth_profile_id = Some(auth_profile_id.clone());
        profile.proxy_profile_id = proxy_profile.as_ref().map(|_| proxy_profile_id.clone());
        profile.tunnel = if self.editor.tunnels.is_empty() {
            None
        } else {
            Some(TunnelProfile {
                forwards: self.editor.tunnels.clone(),
            })
        };

        self.config_document
            .auth_profiles
            .insert(auth_profile_id, auth_profile);
        match proxy_profile {
            Some(profile) => {
                self.config_document
                    .proxy_profiles
                    .insert(proxy_profile_id, profile);
            }
            None => {
                self.config_document
                    .proxy_profiles
                    .remove(&proxy_profile_id);
            }
        }
        let existing_folder_id = find_session_folder_id(&self.config_document.folders, &session_id);
        if existing_folder_id.as_deref() == Some(target_folder_id.as_str()) {
            if let Some(existing) = self.config_document.find_session_mut(&session_id) {
                *existing = profile;
            } else {
                self.target_saved_sessions_folder_mut(&target_folder_id)?
                    .sessions
                    .push(profile);
            }
        } else {
            let _ = self.config_document.remove_session(&session_id);
            self.target_saved_sessions_folder_mut(&target_folder_id)?
                .sessions
                .push(profile);
        }
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        self.selected_saved_session_id = Some(session_id.clone());
        self.editor.target_session_id = Some(session_id.clone());
        self.editor.target_folder_id = target_folder_id;
        Ok(session_id)
    }

    pub(crate) fn editor_target_parts(&self) -> (bool, String) {
        match &self.editor.target_session_id {
            Some(id) => (true, id.clone()),
            None => (false, String::new()),
        }
    }

    pub(crate) fn editor_tunnel_summary_rows_text(&self) -> String {
        self.editor
            .tunnels
            .iter()
            .take(4)
            .map(tunnel_forward_summary)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn editor_proxy_summary_parts(&self) -> (String, String, String) {
        match self.editor.proxy_mode {
            EditorProxyMode::None => ("none".to_owned(), String::new(), String::new()),
            EditorProxyMode::Custom => {
                let user = if self.editor.proxy_username.trim().is_empty() {
                    "<none>".to_owned()
                } else {
                    self.editor.proxy_username.trim().to_owned()
                };
                if self.editor.proxy_host.trim().is_empty()
                    || self.editor.proxy_port_text.trim().is_empty()
                {
                    ("incomplete".to_owned(), String::new(), user)
                } else {
                    (
                        "custom".to_owned(),
                        format!(
                            "{}:{}",
                            self.editor.proxy_host.trim(),
                            self.editor.proxy_port_text.trim()
                        ),
                        user,
                    )
                }
            }
        }
    }

    pub(crate) fn editor_folder_parts(&self) -> (String, String, bool) {
        self.editor_folder_choices()
            .into_iter()
            .find(|choice| choice.id == self.editor.target_folder_id)
            .map(|choice| (choice.path_label, choice.id, true))
            .unwrap_or_else(|| (String::new(), self.editor.target_folder_id.clone(), false))
    }

    pub(crate) fn editor_folder_legacy_text(&self) -> String {
        let (label, id, known) = self.editor_folder_parts();
        if known {
            format!("{label} ({id})")
        } else {
            format!("Unknown folder ({id})")
        }
    }

    pub(crate) fn editor_folder_choices(&self) -> Vec<EditorFolderChoice> {
        let mut choices = Vec::new();
        collect_editor_folder_choices(&self.config_document.folders, &mut Vec::new(), &mut choices);
        if !choices
            .iter()
            .any(|choice| choice.id == SAVED_SESSIONS_FOLDER_ID)
        {
            choices.insert(
                0,
                EditorFolderChoice {
                    id: SAVED_SESSIONS_FOLDER_ID.to_owned(),
                    path_label: SAVED_SESSIONS_FOLDER_NAME.to_owned(),
                },
            );
        }
        if choices.is_empty() {
            choices.push(EditorFolderChoice {
                id: SAVED_SESSIONS_FOLDER_ID.to_owned(),
                path_label: SAVED_SESSIONS_FOLDER_NAME.to_owned(),
            });
        }
        choices
    }

    pub(crate) fn rotate_editor_folder(&mut self, forward: bool) {
        let choices = self.editor_folder_choices();
        if choices.is_empty() {
            self.editor.target_folder_id = SAVED_SESSIONS_FOLDER_ID.to_owned();
            return;
        }
        let current_index = choices
            .iter()
            .position(|choice| choice.id == self.editor.target_folder_id)
            .unwrap_or(0);
        let len = choices.len();
        let next_index = if forward {
            (current_index + 1) % len
        } else if current_index == 0 {
            len - 1
        } else {
            current_index - 1
        };
        self.editor.target_folder_id = choices[next_index].id.clone();
    }
}

/// Value-only auth-test result for the session editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditorAuthTestStatus {
    /// Nothing tested yet (fresh editor).
    Hint,
    /// A saved session was loaded into the editor.
    LoadedHint,
    Success {
        host_label: String,
        backend: String,
        startup_snippet: String,
    },
    Failed {
        error: String,
    },
}

impl EditorAuthTestStatus {
    pub(crate) const fn kind_id(&self) -> &'static str {
        match self {
            Self::Hint => "hint",
            Self::LoadedHint => "loaded",
            Self::Success { .. } => "success",
            Self::Failed { .. } => "failed",
        }
    }

    pub(crate) fn host_label(&self) -> &str {
        match self {
            Self::Success { host_label, .. } => host_label,
            _ => "",
        }
    }

    pub(crate) fn backend(&self) -> &str {
        match self {
            Self::Success { backend, .. } => backend,
            _ => "",
        }
    }

    pub(crate) fn startup_snippet(&self) -> &str {
        match self {
            Self::Success {
                startup_snippet, ..
            } => startup_snippet,
            _ => "",
        }
    }

    pub(crate) fn error(&self) -> &str {
        match self {
            Self::Failed { error } => error,
            _ => "",
        }
    }

    /// English sentence kept only for the not-yet-migrated status bar channel.
    pub(crate) fn legacy_text(&self) -> String {
        match self {
            Self::Hint => {
                "Run Auth Test to validate the current editor fields without saving.".to_owned()
            }
            Self::LoadedHint => {
                "Loaded saved session into the editor. Run Auth Test before saving if you want to validate the current form."
                    .to_owned()
            }
            Self::Success {
                host_label,
                backend,
                startup_snippet,
            } => {
                if startup_snippet.is_empty() {
                    format!("Auth test succeeded for {host_label} using the `{backend}` backend.")
                } else {
                    format!(
                        "Auth test succeeded for {host_label} using the `{backend}` backend. Startup: {startup_snippet}"
                    )
                }
            }
            Self::Failed { error } => format!("Auth test failed: {error}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorAuthMethod {
    Agent,
    Password,
    KeyboardInteractive,
    PrivateKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorProxyMode {
    None,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorSection {
    General,
    Authentication,
    Terminal,
    Sftp,
    Tunnels,
    Proxy,
    Logging,
    Advanced,
    Appearance,
}

impl EditorProxyMode {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Custom => "custom",
        }
    }
}

impl EditorSection {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Authentication => "authentication",
            Self::Terminal => "terminal",
            Self::Sftp => "sftp",
            Self::Tunnels => "tunnels",
            Self::Proxy => "proxy",
            Self::Logging => "logging",
            Self::Advanced => "advanced",
            Self::Appearance => "appearance",
        }
    }
}

impl EditorAuthMethod {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Password => "password",
            Self::KeyboardInteractive => "keyboard_interactive",
            Self::PrivateKey => "private_key",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionEditorDraft {
    pub(crate) target_session_id: Option<String>,
    pub(crate) target_folder_id: String,
    pub(crate) name: String,
    pub(crate) host: String,
    pub(crate) port_text: String,
    pub(crate) username: String,
    pub(crate) auth_method: EditorAuthMethod,
    pub(crate) host_key_policy: ConfigHostKeyPolicy,
    pub(crate) password: String,
    pub(crate) key_path: String,
    pub(crate) passphrase: String,
    pub(crate) proxy_mode: EditorProxyMode,
    pub(crate) proxy_protocol: ProxyProtocol,
    pub(crate) proxy_host: String,
    pub(crate) proxy_port_text: String,
    pub(crate) proxy_username: String,
    pub(crate) proxy_password: String,
    pub(crate) proxy_dns_by_proxy: bool,
    pub(crate) tunnel_kind: TunnelForwardKind,
    pub(crate) tunnel_bind_host: String,
    pub(crate) tunnel_bind_port_text: String,
    pub(crate) tunnel_target_host: String,
    pub(crate) tunnel_target_port_text: String,
    pub(crate) tunnels: Vec<TunnelForward>,
}

impl Default for SessionEditorDraft {
    fn default() -> Self {
        Self {
            target_session_id: None,
            target_folder_id: SAVED_SESSIONS_FOLDER_ID.to_owned(),
            name: String::new(),
            host: String::new(),
            port_text: "22".to_owned(),
            username: String::new(),
            auth_method: EditorAuthMethod::Agent,
            host_key_policy: ConfigHostKeyPolicy::Strict,
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
            tunnels: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorFolderChoice {
    pub(crate) id: String,
    pub(crate) path_label: String,
}

pub(crate) fn collect_editor_folder_choices(
    folders: &[FolderProfile],
    ancestors: &mut Vec<String>,
    out: &mut Vec<EditorFolderChoice>,
) {
    for folder in folders {
        ancestors.push(folder.name.clone());
        out.push(EditorFolderChoice {
            id: folder.id.clone(),
            path_label: ancestors.join(" / "),
        });
        collect_editor_folder_choices(&folder.folders, ancestors, out);
        let _ = ancestors.pop();
    }
}

pub(crate) fn parse_port_field(value: &str, invalid_message: &str) -> AppResult<u16> {
    let port = value
        .trim()
        .parse::<u16>()
        .map_err(|_| AppError::new(invalid_message))?;
    if port == 0 {
        return Err(AppError::new("port must be greater than zero"));
    }
    Ok(port)
}
