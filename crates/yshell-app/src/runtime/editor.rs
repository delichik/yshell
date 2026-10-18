//! Session editor draft state: field updates, section selection, folder choice
//! and persistence.

use crate::{error::AppError, error::AppResult};
use yshell_config::{
    FolderProfile, LoggingProfile, ProxyProtocol, SessionProfile, TerminalProfile, ThemeCatalog,
    TunnelForward, TunnelForwardKind, TunnelProfile,
};

use super::panels::{
    SETTINGS_SCROLLBACK_LINES_MAX, SETTINGS_SCROLLBACK_LINES_MIN,
    SETTINGS_SCROLLBACK_MAX_CELLS_MAX, SETTINGS_SCROLLBACK_MAX_CELLS_MIN,
};
use super::theme::{
    appearance_font_preset_labels, color_scheme_id_at, parse_scrollback, resolve_palette,
    scheme_display_name, theme_field_rows, ThemeDraft, ThemeFieldAction, ThemeFieldDraft,
    ThemeFieldError, ThemeSources, TriStateFieldData, KEY_SCROLLBACK_LINES,
    KEY_SCROLLBACK_MAX_CELLS,
};

use super::*;

impl AppRuntime {
    pub fn start_new_saved_session_editor(&mut self) -> AppProjection {
        self.editor = SessionEditorDraft::default();
        self.new_folder_name.clear();
        self.editor_discard_dialog_visible = false;
        self.editor_modal_visible = true;
        self.editor_section = EditorSection::General;
        self.editor_auth_test_status = EditorAuthTestStatus::Hint;
        self.editor_set_theme_baseline(None);
        self.editor_baseline = Some(self.editor.clone());
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
        self.editor_baseline = Some(self.editor.clone());
        self.editor_discard_dialog_visible = false;
        self.editor_modal_visible = true;
        self.editor_section = EditorSection::General;
        self.editor_auth_test_status = EditorAuthTestStatus::LoadedHint;
        self.status_text = format!("Loaded saved session `{profile_id}` into the editor.");
        Ok(self.projection())
    }

    /// Cancel / Esc / scrim: prompt Discard Changes when the draft is dirty.
    pub fn request_close_session_editor_modal(&mut self) -> AppProjection {
        if self.editor_is_dirty() {
            self.editor_discard_dialog_visible = true;
            self.status_text = "Discard unsaved session editor changes?".to_owned();
            return self.projection();
        }
        self.close_session_editor_modal()
    }

    pub fn confirm_discard_session_editor(&mut self) -> AppProjection {
        self.editor_discard_dialog_visible = false;
        self.close_session_editor_modal()
    }

    pub fn cancel_discard_session_editor(&mut self) -> AppProjection {
        self.editor_discard_dialog_visible = false;
        self.status_text = "Continued editing the session.".to_owned();
        self.projection()
    }

    pub fn close_session_editor_modal(&mut self) -> AppProjection {
        self.editor_modal_visible = false;
        self.editor_discard_dialog_visible = false;
        self.editor_baseline = None;
        self.status_text = "Closed Session Editor.".to_owned();
        self.projection()
    }

    pub(crate) fn editor_is_dirty(&self) -> bool {
        match &self.editor_baseline {
            Some(baseline) => &self.editor != baseline,
            None => false,
        }
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
        // Scrollback / terminal limits live in global Settings; session editor
        // no longer exposes a Terminal section — keep the callback but land on General.
        self.editor_section = EditorSection::General;
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
        // Logging is started from the session/terminal context menu, not the editor.
        self.editor_section = EditorSection::General;
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
        self.editor_refresh_theme_for_folder();
        self.status_text = format!("Editor target folder: {}", self.editor_folder_legacy_text());
        self.projection()
    }

    pub fn select_previous_editor_folder(&mut self) -> AppProjection {
        self.rotate_editor_folder(false);
        self.editor_refresh_theme_for_folder();
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

    pub fn update_editor_password(&mut self, value: &str) -> AppProjection {
        self.editor.password = value.to_owned();
        self.projection()
    }

    /// SSH-1：Key 认证改为从托管密钥清单选择（`label + algorithm/fingerprint`）。
    ///
    /// 选择即视为迁移：草稿里的 legacy path 标记被清除，保存时只写 `key_id`。
    pub fn select_editor_key(&mut self, index: i32) -> AppProjection {
        let entries = self.private_key_entries();
        let Some(entry) = usize::try_from(index)
            .ok()
            .and_then(|index| entries.get(index))
        else {
            return self.projection();
        };
        self.editor.key_id = Some(entry.id.clone());
        self.editor.legacy_key_path.clear();
        self.editor.legacy_key_has_passphrase = false;
        self.status_text = format!("Selected managed key `{}` for this session.", entry.label);
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
        let auth_profile_id = self
            .editor_auth_profile_id()
            .unwrap_or_else(|| format!("{session_id}-auth"));
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
        // 主机密钥策略不属于编辑器状态：保存时保持 `SessionProfile::host_key_policy`
        // 为 None，运行时解析回退到 Strict（known_hosts 严格校验）。
        // 兼容性：旧配置/导入的会话仍可带 `host_key_policy`（见 `yshell_config`），
        // 但用编辑器再保存会清掉该字段——这是有意的，Accept Any 不再是用户可选项；
        // 测试/自动化需要 accept-any 时走 `host_key_policy_override`。
        profile.auth_profile_id = Some(auth_profile_id.clone());
        profile.proxy_profile_id = proxy_profile.as_ref().map(|_| proxy_profile_id.clone());
        profile.tunnel = if self.editor.tunnels.is_empty() {
            None
        } else {
            Some(TunnelProfile {
                forwards: self.editor.tunnels.clone(),
            })
        };
        profile.terminal = match self.editor_terminal_override() {
            Ok(terminal) => terminal,
            Err(error) => {
                return Err(AppError::new(format!("{}: {}", error.field, error.message)));
            }
        };

        self.config_document
            .auth_profiles
            .insert(auth_profile_id.clone(), auth_profile);
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
        self.editor.auth_profile_id = Some(auth_profile_id);
        self.editor.target_folder_id = target_folder_id;
        Ok(session_id)
    }

    pub(crate) fn editor_target_parts(&self) -> (bool, String) {
        match &self.editor.target_session_id {
            Some(id) => (true, id.clone()),
            None => (false, String::new()),
        }
    }

    /// 被编辑会话的 auth profile id：优先沿用加载时记录的 id（兼容自定义 id 的
    /// 旧配置），新会话回退到 `{session_id}-auth` 约定。
    pub(crate) fn editor_auth_profile_id(&self) -> Option<String> {
        self.editor.auth_profile_id.clone().or_else(|| {
            self.editor
                .target_session_id
                .as_deref()
                .map(|session_id| format!("{session_id}-auth"))
        })
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

/// N5：Session Editor 的 Appearance/Terminal 覆盖草稿（含继承来源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionThemeDraft {
    pub(crate) theme: ThemeDraft,
    pub(crate) scrollback_lines: ThemeFieldDraft,
    pub(crate) scrollback_max_cells: ThemeFieldDraft,
    /// Parent resolution (target folder chain + global) for inherited fields.
    pub(crate) parent: TerminalProfile,
    pub(crate) sources: ThemeSources,
}

impl Default for SessionThemeDraft {
    fn default() -> Self {
        let parent = TerminalProfile::default();
        Self {
            theme: ThemeDraft::from_resolved(&parent, None, &ThemeCatalog::default()),
            scrollback_lines: ThemeFieldDraft {
                text: parent.scrollback_lines.to_string(),
                explicit: false,
            },
            scrollback_max_cells: ThemeFieldDraft {
                text: parent.scrollback_max_cells.to_string(),
                explicit: false,
            },
            sources: ThemeSources::resolve(&[], &parent, &LoggingProfile::default()),
            parent,
        }
    }
}

impl SessionThemeDraft {
    pub(crate) fn new(
        parent: TerminalProfile,
        sources: ThemeSources,
        own: Option<&TerminalProfile>,
        catalog: &ThemeCatalog,
    ) -> Self {
        let current = own.unwrap_or(&parent);
        Self {
            theme: ThemeDraft::from_resolved(&parent, own, catalog),
            scrollback_lines: ThemeFieldDraft {
                text: current.scrollback_lines.to_string(),
                explicit: own.is_some(),
            },
            scrollback_max_cells: ThemeFieldDraft {
                text: current.scrollback_max_cells.to_string(),
                explicit: own.is_some(),
            },
            sources,
            parent,
        }
    }

    fn has_explicit_field(&self) -> bool {
        self.theme.has_explicit_field()
            || self.scrollback_lines.explicit
            || self.scrollback_max_cells.explicit
    }

    fn refresh_sources(
        &mut self,
        parent: TerminalProfile,
        sources: ThemeSources,
        catalog: &ThemeCatalog,
    ) {
        self.theme.refresh_inherited(&parent, catalog);
        if !self.scrollback_lines.explicit {
            self.scrollback_lines.text = parent.scrollback_lines.to_string();
        }
        if !self.scrollback_max_cells.explicit {
            self.scrollback_max_cells.text = parent.scrollback_max_cells.to_string();
        }
        self.parent = parent;
        self.sources = sources;
    }
}

impl AppRuntime {
    /// Recomputes the editor theme baseline for its target folder.
    ///
    /// `own` is the edited session's own terminal override (None for a new
    /// session); explicit flags are taken from it.
    pub(crate) fn editor_set_theme_baseline(&mut self, own: Option<&TerminalProfile>) {
        let folder_id = self.editor.target_folder_id.clone();
        let parent = self.terminal_profile_for_draft(Some(&folder_id));
        let chain = self.config_document.folder_chain_to(&folder_id);
        let sources = ThemeSources::resolve(
            &chain,
            &self.config_document.terminal,
            &self.config_document.logging,
        );
        self.editor.theme = SessionThemeDraft::new(parent, sources, own, &self.theme_catalog);
    }

    /// Recomputes inherit sources after the target folder changed, keeping the
    /// explicit values the user already typed.
    pub(crate) fn editor_refresh_theme_for_folder(&mut self) {
        let folder_id = self.editor.target_folder_id.clone();
        let parent = self.terminal_profile_for_draft(Some(&folder_id));
        let chain = self.config_document.folder_chain_to(&folder_id);
        let sources = ThemeSources::resolve(
            &chain,
            &self.config_document.terminal,
            &self.config_document.logging,
        );
        self.editor
            .theme
            .refresh_sources(parent, sources, &self.theme_catalog);
    }

    /// Applies a three-state action to one editor theme field.
    pub fn editor_theme_field_action(
        &mut self,
        key: &str,
        action: ThemeFieldAction,
    ) -> AppProjection {
        let explicit = matches!(action, ThemeFieldAction::SetExplicit);
        let parent = self.editor.theme.parent.clone();
        if let Some(field) = self.editor.theme.theme.field_mut(key) {
            field.explicit = explicit;
        }
        match key {
            KEY_SCROLLBACK_LINES => {
                let field = &mut self.editor.theme.scrollback_lines;
                field.explicit = explicit;
                if !explicit {
                    field.text = parent.scrollback_lines.to_string();
                }
            }
            KEY_SCROLLBACK_MAX_CELLS => {
                let field = &mut self.editor.theme.scrollback_max_cells;
                field.explicit = explicit;
                if !explicit {
                    field.text = parent.scrollback_max_cells.to_string();
                }
            }
            _ => {}
        }
        self.projection()
    }

    /// Updates the draft text of one editor theme field (typing implies an
    /// explicit override at the session level).
    pub fn update_editor_theme_field(&mut self, key: &str, value: &str) -> AppProjection {
        if let Some(field) = self.editor.theme.theme.field_mut(key) {
            field.text = value.to_owned();
            field.explicit = true;
        } else {
            match key {
                KEY_SCROLLBACK_LINES => {
                    self.editor.theme.scrollback_lines.text = value.to_owned();
                    self.editor.theme.scrollback_lines.explicit = true;
                }
                KEY_SCROLLBACK_MAX_CELLS => {
                    self.editor.theme.scrollback_max_cells.text = value.to_owned();
                    self.editor.theme.scrollback_max_cells.explicit = true;
                }
                _ => {}
            }
        }
        self.projection()
    }

    /// Picks a color scheme for the edited session (built-in + user themes).
    pub fn select_editor_scheme_index(&mut self, index: i32) -> AppProjection {
        if let Some(id) = color_scheme_id_at(&self.theme_catalog, index) {
            let field = &mut self.editor.theme.theme.color_scheme;
            field.text = id;
            field.explicit = true;
        }
        self.projection()
    }

    /// Picks a font family preset for the edited session.
    pub fn select_editor_font_index(&mut self, index: i32) -> AppProjection {
        let labels = appearance_font_preset_labels();
        if let Ok(index) = usize::try_from(index) {
            if let Some(label) = labels.get(index) {
                let field = &mut self.editor.theme.theme.font_family;
                field.text = label.clone();
                field.explicit = true;
            }
        }
        self.projection()
    }

    /// Drops every theme override of the edited session.
    pub fn reset_all_editor_theme(&mut self) -> AppProjection {
        let parent = self.editor.theme.parent.clone();
        let sources = self.editor.theme.sources.clone();
        self.editor.theme = SessionThemeDraft {
            theme: ThemeDraft::from_resolved(&parent, None, &self.theme_catalog),
            scrollback_lines: ThemeFieldDraft {
                text: parent.scrollback_lines.to_string(),
                explicit: false,
            },
            scrollback_max_cells: ThemeFieldDraft {
                text: parent.scrollback_max_cells.to_string(),
                explicit: false,
            },
            sources,
            parent,
        };
        self.projection()
    }

    /// Appearance rows of the Session Editor.
    pub(crate) fn editor_appearance_fields(&self) -> Vec<TriStateFieldData> {
        let draft = &self.editor.theme;
        let resolved = self.editor_theme_resolved_terminal();
        let palette = resolve_palette(&resolved, &self.theme_catalog);
        let scheme_label = resolved
            .color_scheme
            .as_deref()
            .map(|id| scheme_display_name(id, &self.theme_catalog))
            .unwrap_or_else(|| "YShell Default".to_owned());
        theme_field_rows(
            &draft.theme,
            &draft.sources,
            "local-session",
            palette,
            scheme_label,
            &self.theme_catalog,
        )
    }

    /// Terminal rows of the Session Editor (scrollback).
    pub(crate) fn editor_terminal_fields(&self) -> Vec<TriStateFieldData> {
        let draft = &self.editor.theme;
        vec![
            TriStateFieldData::new(
                KEY_SCROLLBACK_LINES,
                "Scrollback lines",
                "Per session",
                "number",
                &draft.scrollback_lines,
                &draft.sources.scrollback_lines,
                "local-session",
            ),
            TriStateFieldData::new(
                KEY_SCROLLBACK_MAX_CELLS,
                "Scrollback memory cap",
                "Cells kept per session",
                "number",
                &draft.scrollback_max_cells,
                &draft.sources.scrollback_max_cells,
                "local-session",
            ),
        ]
    }

    /// Terminal profile the editor is currently showing (draft applied over
    /// the parent resolution).
    pub(crate) fn editor_theme_resolved_terminal(&self) -> TerminalProfile {
        let draft = &self.editor.theme;
        let mut terminal = draft.parent.clone();
        let _ = draft
            .theme
            .write_into(&mut terminal, &draft.parent, &self.theme_catalog);
        terminal
    }

    /// Validates the theme draft and returns the session terminal override.
    pub(crate) fn editor_terminal_override(
        &self,
    ) -> Result<Option<TerminalProfile>, ThemeFieldError> {
        let draft = &self.editor.theme;
        if !draft.has_explicit_field() {
            return Ok(None);
        }
        let mut terminal = draft.parent.clone();
        draft
            .theme
            .write_into(&mut terminal, &draft.parent, &self.theme_catalog)?;
        terminal.scrollback_lines = parse_scrollback(
            &draft.scrollback_lines,
            draft.parent.scrollback_lines,
            SETTINGS_SCROLLBACK_LINES_MIN,
            SETTINGS_SCROLLBACK_LINES_MAX,
            KEY_SCROLLBACK_LINES,
        )?;
        terminal.scrollback_max_cells = parse_scrollback(
            &draft.scrollback_max_cells,
            draft.parent.scrollback_max_cells,
            SETTINGS_SCROLLBACK_MAX_CELLS_MIN,
            SETTINGS_SCROLLBACK_MAX_CELLS_MAX,
            KEY_SCROLLBACK_MAX_CELLS,
        )?;
        Ok(Some(terminal))
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
                startup_snippet,
            } => {
                if startup_snippet.is_empty() {
                    format!("Auth test succeeded for {host_label}.")
                } else {
                    format!(
                        "Auth test succeeded for {host_label}. Startup: {startup_snippet}"
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
    Sftp,
    Tunnels,
    Proxy,
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
            Self::Sftp => "sftp",
            Self::Tunnels => "tunnels",
            Self::Proxy => "proxy",
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
    /// 被编辑会话当前引用的 auth profile id（保留自定义 id，保存时原地更新）。
    pub(crate) auth_profile_id: Option<String>,
    pub(crate) target_folder_id: String,
    pub(crate) name: String,
    pub(crate) host: String,
    pub(crate) port_text: String,
    pub(crate) username: String,
    pub(crate) auth_method: EditorAuthMethod,
    pub(crate) password: String,
    /// 选中的托管私钥 id（`AuthMethod::PrivateKey::key_id`）；保存时优先写它。
    pub(crate) key_id: Option<String>,
    /// 旧配置里 path-only 的私钥路径：只读展示，保存时原样保留以便旧会话仍可连接。
    /// 用户从选择器里选中托管密钥后该字段清空（迁移）。
    pub(crate) legacy_key_path: String,
    /// 旧 path-only 私钥在 secret store 里是否有已保存口令（只读提示/连接时复用）。
    pub(crate) legacy_key_has_passphrase: bool,
    /// Edit mode: a password / keyboard-interactive secret is already stored.
    /// Empty password field means "keep existing" rather than "missing".
    pub(crate) has_stored_password: bool,
    pub(crate) proxy_mode: EditorProxyMode,
    pub(crate) proxy_protocol: ProxyProtocol,
    pub(crate) proxy_host: String,
    pub(crate) proxy_port_text: String,
    pub(crate) proxy_username: String,
    pub(crate) proxy_password: String,
    /// Edit mode: a proxy password is already stored.
    pub(crate) has_stored_proxy_password: bool,
    pub(crate) proxy_dns_by_proxy: bool,
    pub(crate) tunnel_kind: TunnelForwardKind,
    pub(crate) tunnel_bind_host: String,
    pub(crate) tunnel_bind_port_text: String,
    pub(crate) tunnel_target_host: String,
    pub(crate) tunnel_target_port_text: String,
    pub(crate) tunnels: Vec<TunnelForward>,
    /// N5：Appearance/Terminal 覆盖草稿（三态）。
    pub(crate) theme: SessionThemeDraft,
}

impl Default for SessionEditorDraft {
    fn default() -> Self {
        Self {
            target_session_id: None,
            auth_profile_id: None,
            target_folder_id: SAVED_SESSIONS_FOLDER_ID.to_owned(),
            name: String::new(),
            host: String::new(),
            port_text: "22".to_owned(),
            username: String::new(),
            auth_method: EditorAuthMethod::Agent,
            password: String::new(),
            key_id: None,
            legacy_key_path: String::new(),
            legacy_key_has_passphrase: false,
            has_stored_password: false,
            proxy_mode: EditorProxyMode::None,
            proxy_protocol: ProxyProtocol::Socks5,
            proxy_host: String::new(),
            proxy_port_text: String::new(),
            proxy_username: String::new(),
            proxy_password: String::new(),
            has_stored_proxy_password: false,
            proxy_dns_by_proxy: true,
            tunnel_kind: TunnelForwardKind::Local,
            tunnel_bind_host: "127.0.0.1".to_owned(),
            tunnel_bind_port_text: String::new(),
            tunnel_target_host: String::new(),
            tunnel_target_port_text: String::new(),
            tunnels: Vec::new(),
            theme: SessionThemeDraft::default(),
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
