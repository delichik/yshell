//! Session editor draft state: field updates, section selection, folder choice
//! and persistence.

use crate::{error::AppError, error::AppResult, session_runtime::SessionSource};
use yshell_config::{
    AuthMethod as ConfigAuthMethod, FolderProfile, LoggingProfile, LoginScriptProfile, LoginStep,
    LoginStepAction, ProxyProfile, ResolvedSessionProfile, SessionProfile, TerminalProfile,
    ThemeCatalog, TunnelForward, TunnelForwardKind, TunnelProfile, LOGIN_STEP_MAX_WAIT_MILLIS,
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
        self.set_status_kind(
            "editor-started",
            "Session editor cleared. Fill in the fields to create a saved session.".to_owned(),
            String::new(),
            String::new(),
        );
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
        // --- session-editor：icon / notes / 页级 Inherit / login script 步骤 ----
        // `editor_from_resolved_session` 在 `ssh_config.rs`（不在本波改写范围），
        // 基础层新增的字段在这里补齐，避免两处快照漂移。
        self.editor_apply_resolved_session(&resolved);
        self.editor_baseline = Some(self.editor.clone());
        self.editor_discard_dialog_visible = false;
        self.editor_modal_visible = true;
        self.editor_section = EditorSection::General;
        self.editor_auth_test_status = EditorAuthTestStatus::LoadedHint;
        self.set_status_kind(
            "editor-loaded",
            format!("Loaded saved session `{profile_id}` into the editor."),
            profile_id,
            String::new(),
        );
        Ok(self.projection())
    }

    /// Cancel / Esc / scrim: prompt Discard Changes when the draft is dirty.
    pub fn request_close_session_editor_modal(&mut self) -> AppProjection {
        if self.editor_is_dirty() {
            self.editor_discard_dialog_visible = true;
            self.set_status_kind(
                "editor-discard-confirm",
                "Discard unsaved session editor changes?".to_owned(),
                String::new(),
                String::new(),
            );
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
        self.set_status_kind(
            "editor-continued",
            "Continued editing the session.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    pub fn close_session_editor_modal(&mut self) -> AppProjection {
        self.editor_modal_visible = false;
        self.editor_discard_dialog_visible = false;
        self.editor_baseline = None;
        self.set_status_kind(
            "editor-closed",
            "Closed Session Editor.".to_owned(),
            String::new(),
            String::new(),
        );
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
        let folder_text = self.editor_folder_legacy_text();
        self.set_status_kind(
            "editor-folder-target",
            format!("Editor target folder: {folder_text}"),
            folder_text,
            String::new(),
        );
        self.projection()
    }

    pub fn select_previous_editor_folder(&mut self) -> AppProjection {
        self.rotate_editor_folder(false);
        self.editor_refresh_theme_for_folder();
        let folder_text = self.editor_folder_legacy_text();
        self.set_status_kind(
            "editor-folder-target",
            format!("Editor target folder: {folder_text}"),
            folder_text,
            String::new(),
        );
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
        self.editor.inherit_auth = false;
        self.projection()
    }

    pub fn set_editor_auth_method_password(&mut self) -> AppProjection {
        self.editor.auth_method = EditorAuthMethod::Password;
        self.editor.inherit_auth = false;
        self.projection()
    }

    pub fn set_editor_auth_method_keyboard_interactive(&mut self) -> AppProjection {
        self.editor.auth_method = EditorAuthMethod::KeyboardInteractive;
        self.editor.inherit_auth = false;
        self.projection()
    }

    pub fn set_editor_auth_method_private_key(&mut self) -> AppProjection {
        self.editor.auth_method = EditorAuthMethod::PrivateKey;
        self.editor.inherit_auth = false;
        self.projection()
    }

    pub fn update_editor_password(&mut self, value: &str) -> AppProjection {
        self.editor.password = value.to_owned();
        self.editor.inherit_auth = false;
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
        self.editor.inherit_auth = false;
        self.set_status_kind(
            "editor-key-selected",
            format!("Selected managed key `{}` for this session.", entry.label),
            entry.label.clone(),
            String::new(),
        );
        self.projection()
    }

    pub fn set_editor_proxy_mode_none(&mut self) -> AppProjection {
        self.editor.proxy_mode = EditorProxyMode::None;
        self.editor.proxy_profile_id = None;
        self.editor.inherit_proxy = false;
        self.projection()
    }

    /// NET-1：从托管代理清单选择一条（选择即迁移，保存只写 `proxy_profile_id`）。
    pub fn select_editor_proxy(&mut self, index: i32) -> AppProjection {
        let options = self.managed_proxy_options();
        let Some(option) = usize::try_from(index)
            .ok()
            .and_then(|index| options.get(index))
        else {
            return self.projection();
        };
        self.editor.proxy_mode = EditorProxyMode::Managed;
        self.editor.proxy_profile_id = Some(option.id.clone());
        self.editor.inherit_proxy = false;
        self.set_status_kind(
            "editor-proxy-selected",
            format!(
                "Selected managed proxy `{}` for this session.",
                option.label
            ),
            option.label.clone(),
            String::new(),
        );
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
        self.editor.inherit_tunnels = false;
        self.editor.tunnel_bind_port_text.clear();
        self.editor.tunnel_target_host.clear();
        self.editor.tunnel_target_port_text.clear();
        let kind_text = tunnel_kind_label(forward.kind);
        let summary = super::ssh_config::tunnel_forward_summary_body(&forward);
        self.set_status_kind(
            "editor-tunnel-added",
            format!(
                "Added {} tunnel {}.",
                tunnel_kind_label(forward.kind),
                tunnel_forward_summary(&forward)
            ),
            kind_text.to_owned(),
            summary,
        );
        Ok(self.projection())
    }

    pub fn clear_editor_tunnels(&mut self) -> AppProjection {
        self.editor.tunnels.clear();
        self.editor.inherit_tunnels = false;
        self.editor_selected_tunnel = None;
        self.set_status_kind(
            "editor-tunnels-cleared",
            "Cleared all editor tunnel forwards.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    pub fn test_editor_auth(&mut self) -> AppProjection {
        let status = match self.run_editor_auth_test() {
            Ok(status) => status,
            Err(error) => EditorAuthTestStatus::Failed {
                error: error.to_string(),
            },
        };
        let (kind, param_1, param_2) = match &status {
            EditorAuthTestStatus::Hint => ("editor-auth-test-hint", String::new(), String::new()),
            EditorAuthTestStatus::LoadedHint => {
                ("editor-auth-test-loaded", String::new(), String::new())
            }
            EditorAuthTestStatus::Success {
                host_label,
                startup_snippet,
            } => (
                "editor-auth-test-succeeded",
                host_label.clone(),
                startup_snippet.clone(),
            ),
            EditorAuthTestStatus::Failed { error } => {
                ("editor-auth-test-failed", error.clone(), String::new())
            }
        };
        let text = status.legacy_text();
        self.set_status_kind(kind, text, param_1, param_2);
        self.editor_auth_test_status = status;
        self.projection()
    }

    pub fn save_editor_to_saved_session(&mut self) -> AppResult<AppProjection> {
        let session_id = self.persist_editor_to_saved_session()?;
        self.editor_modal_visible = false;
        self.set_status_kind(
            "editor-saved",
            format!("Saved session editor changes into `{session_id}`."),
            session_id,
            String::new(),
        );
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
        // --- session-editor：页级 Inherit + auth profile 归属 -------------------
        // Inherit = 会话不写本级 auth 引用（父级链路负责解析）；Custom = 写成
        // 会话自己的 auth profile；父级已有 profile 时先复制（口令/密钥引用原样
        // 保留，用户不必重填）。
        let mut auth_profile_id: Option<String> = None;
        if !self.editor.inherit_auth {
            let own_id = self
                .config_document
                .find_session(&session_id)
                .and_then(|profile| profile.auth_profile_id.clone())
                .unwrap_or_else(|| format!("{session_id}-auth"));
            if !self.config_document.auth_profiles.contains_key(&own_id) {
                if let Some(parent_auth) = self.editor_parent_resolved().auth.clone() {
                    let mut copy = parent_auth;
                    copy.id = own_id.clone();
                    copy.name = format!("Auth for {}", self.editor.name.trim());
                    self.config_document
                        .auth_profiles
                        .insert(own_id.clone(), copy);
                }
            }
            self.editor.auth_profile_id = Some(own_id.clone());
            let auth_profile = self.build_auth_profile_for_editor(&own_id)?;
            self.config_document
                .auth_profiles
                .insert(own_id.clone(), auth_profile);
            // Private Key 口令：填写即保存（无 Remember）；留空 = 需要时再问。
            self.editor_store_key_passphrase(&own_id)?;
            auth_profile_id = Some(own_id);
        }
        // NET-1：saving a session only writes `proxy_profile_id`; it never creates
        // or overwrites a proxy profile. Legacy per-session references are kept
        // until the user picks a managed profile (or No Proxy) explicitly.
        let previous_proxy_id = self
            .config_document
            .find_session(&session_id)
            .and_then(|profile| profile.proxy_profile_id.clone());
        let proxy_profile_id = if self.editor.inherit_proxy {
            // 页级 Inherit：不写本级 proxy 引用，解析回落到父级链路。
            None
        } else {
            match self.editor.proxy_mode {
                EditorProxyMode::None => None,
                EditorProxyMode::Managed => {
                    let proxy_id = self.editor.proxy_profile_id.clone().ok_or_else(|| {
                        AppError::new("select a managed proxy profile or choose No Proxy")
                    })?;
                    if !self.config_document.proxy_profiles.contains_key(&proxy_id) {
                        return Err(AppError::new(format!(
                            "the selected managed proxy `{proxy_id}` is no longer available; choose another proxy"
                        )));
                    }
                    Some(proxy_id)
                }
                EditorProxyMode::Legacy => self.editor.proxy_profile_id.clone(),
            }
        };
        let target_folder_id = self.editor.target_folder_id.trim().to_owned();
        if target_folder_id.is_empty() {
            return Err(AppError::new("editor target folder must not be empty"));
        }

        let session_name = self.allocate_unique_session_name(
            &target_folder_id,
            self.editor.name.trim(),
            self.editor.target_session_id.as_deref(),
        )?;
        let mut profile = SessionProfile::new(
            session_id.clone(),
            session_name,
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
        profile.auth_profile_id = auth_profile_id.clone();
        profile.proxy_profile_id = proxy_profile_id.clone();
        // --- session-editor：General / Login Script / Tunnels / Terminal Style --
        profile.icon = self.editor_icon_config_value();
        profile.notes = self.editor_notes_config_value();
        profile.login_script = if self.editor.inherit_login_script {
            None
        } else {
            Some(LoginScriptProfile {
                steps: self.editor.login_steps.clone(),
            })
        };
        profile.tunnel = if self.editor.inherit_tunnels {
            None
        } else {
            // Custom + 空列表 = 明确"本会话无隧道"，覆盖父级继承值。
            Some(TunnelProfile {
                forwards: self.editor.tunnels.clone(),
            })
        };
        profile.terminal = if self.editor.inherit_terminal_style {
            None
        } else {
            match self.editor_terminal_override() {
                Ok(terminal) => terminal,
                Err(error) => {
                    return Err(AppError::new(format!("{}: {}", error.field, error.message)));
                }
            }
        };

        // Ensure the target folder exists in memory and on disk before writing the session file.
        let _ = self.target_saved_sessions_folder_mut(&target_folder_id)?;
        let existing_folder_id = find_session_folder_id(&self.config_document.folders, &session_id);
        let tree = self.config_store.session_tree();
        if existing_folder_id.as_deref() == Some(target_folder_id.as_str()) {
            if self.config_document.find_session(&session_id).is_some() {
                tree.update_session(&mut self.config_document.folders, profile)
                    .map_err(AppError::from_error)?;
            } else {
                tree.add_session(
                    &mut self.config_document.folders,
                    &target_folder_id,
                    profile,
                )
                .map_err(AppError::from_error)?;
            }
        } else {
            if existing_folder_id.is_some() {
                let _ = tree
                    .delete_session(&mut self.config_document.folders, &session_id)
                    .map_err(AppError::from_error)?;
            }
            tree.add_session(
                &mut self.config_document.folders,
                &target_folder_id,
                profile,
            )
            .map_err(AppError::from_error)?;
        }
        // NET-1：迁移后清掉不再被任何会话引用的旧 `<session_id>-proxy`。
        self.cleanup_legacy_proxy(
            &session_id,
            previous_proxy_id.as_deref(),
            proxy_profile_id.as_deref(),
        );
        // Auth/proxy profiles remain in config.toml.
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        self.selected_saved_session_id = Some(session_id.clone());
        self.editor.target_session_id = Some(session_id.clone());
        self.editor.auth_profile_id = auth_profile_id;
        self.editor.target_folder_id = target_folder_id;
        Ok(session_id)
    }

    /// Private Key 口令：非空时写入 secret store 并挂到该 auth profile。
    fn editor_store_key_passphrase(&mut self, auth_profile_id: &str) -> AppResult<()> {
        let passphrase = self.editor.passphrase.trim().to_owned();
        if passphrase.is_empty() || self.editor.auth_method != EditorAuthMethod::PrivateKey {
            return Ok(());
        }
        let secret_key = format!("local://yshell/{auth_profile_id}/key-passphrase");
        self.store_secret_value(&secret_key, &passphrase)?;
        if let Some(profile) = self.config_document.auth_profiles.get_mut(auth_profile_id) {
            if let ConfigAuthMethod::PrivateKey {
                passphrase_secret_key,
                ..
            } = &mut profile.method
            {
                *passphrase_secret_key = Some(secret_key);
            }
        }
        Ok(())
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

    /// NET-1：托管代理下拉的选中下标（无选中/legacy/悬空 → -1）。
    pub(crate) fn editor_proxy_selected_index(&self, options: &[ProxyProfileOptionData]) -> i32 {
        if self.editor.proxy_mode != EditorProxyMode::Managed {
            return -1;
        }
        self.editor
            .proxy_profile_id
            .as_deref()
            .and_then(|proxy_id| options.iter().position(|option| option.id == proxy_id))
            .map(|index| i32::try_from(index).unwrap_or(i32::MAX))
            .unwrap_or(-1)
    }

    /// NET-1：选中托管代理的只读摘要（协议 · 地址 · 用户名 · 口令状态）。
    pub(crate) fn editor_proxy_selected_detail(&self) -> String {
        if self.editor.proxy_mode != EditorProxyMode::Managed {
            return String::new();
        }
        let Some(proxy_id) = self.editor.proxy_profile_id.as_deref() else {
            return String::new();
        };
        self.config_document
            .proxy_profiles
            .get(proxy_id)
            .map(proxy_summary_line)
            .unwrap_or_default()
    }

    /// NET-1：旧 per-session / 悬空代理引用的只读摘要；非 legacy 模式为空。
    pub(crate) fn editor_proxy_legacy_text(&self) -> String {
        if self.editor.proxy_mode != EditorProxyMode::Legacy {
            return String::new();
        }
        let Some(proxy_id) = self.editor.proxy_profile_id.as_deref() else {
            return String::new();
        };
        match self.config_document.proxy_profiles.get(proxy_id) {
            Some(profile) => format!("{} (`{proxy_id}`)", proxy_summary_line(profile)),
            None => {
                format!("Missing proxy profile `{proxy_id}` (legacy per-session proxy reference).")
            }
        }
    }

    /// Selected managed profile disappeared from the config (e.g. deleted while
    /// the editor stayed open); saving would produce a dangling reference.
    pub(crate) fn editor_proxy_selected_missing(&self) -> bool {
        self.editor.proxy_mode == EditorProxyMode::Managed
            && self
                .editor
                .proxy_profile_id
                .as_deref()
                .is_some_and(|proxy_id| !self.config_document.proxy_profiles.contains_key(proxy_id))
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
        self.editor.inherit_terminal_style = false;
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
        self.editor.inherit_terminal_style = false;
        self.projection()
    }

    /// Picks a color scheme for the edited session (built-in + user themes).
    pub fn select_editor_scheme_index(&mut self, index: i32) -> AppProjection {
        if let Some(id) = color_scheme_id_at(&self.theme_catalog, index) {
            let field = &mut self.editor.theme.theme.color_scheme;
            field.text = id;
            field.explicit = true;
            self.editor.inherit_terminal_style = false;
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
                self.editor.inherit_terminal_style = false;
            }
        }
        self.projection()
    }

    /// Drops every theme override of the edited session（= 回到父级 Inherit）。
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
        self.editor.inherit_terminal_style = true;
        self.projection()
    }

    /// Terminal Style 下拉的选项名（内置 + 用户配色）。
    pub(crate) fn editor_scheme_option_names(&self) -> Vec<String> {
        super::theme::color_scheme_option_names(&self.theme_catalog)
    }

    /// Terminal Style 下拉的当前下标（按生效配色解析）。
    pub(crate) fn editor_scheme_selected_index(&self) -> i32 {
        let scheme = self
            .editor_theme_resolved_terminal()
            .color_scheme
            .unwrap_or_else(|| "yshell-default".to_owned());
        super::theme::color_scheme_index(&self.theme_catalog, &scheme)
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
    /// No proxy for this session.
    None,
    /// A managed profile selected from the proxy manager.
    Managed,
    /// Legacy per-session profile (or a dangling reference) kept readable and
    /// connectable until the user picks a managed profile.
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorSection {
    General,
    Authentication,
    // TODO(wave2-editor): Session Editor 的 Login Script 分区接线后删除。
    #[allow(dead_code)]
    LoginScript,
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
            Self::Managed => "managed",
            Self::Legacy => "legacy",
        }
    }
}

impl EditorSection {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Authentication => "authentication",
            Self::LoginScript => "login-script",
            Self::Sftp => "sftp",
            Self::Tunnels => "tunnels",
            Self::Proxy => "proxy",
            Self::Advanced => "advanced",
            // 原 Appearance：本波按原型改名 Terminal Style（内部变体名保留，
            // 因为 `runtime/theme.rs` 引用它；label 才是 UI/接线用的 section 值）。
            Self::Appearance => "terminal-style",
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
    // --- session-editor：General（Icon / Notes）--------------------------------
    /// Icon 引用：空 = Default，`server` / `database` = 预设，其余按自定义图片路径。
    pub(crate) icon: String,
    /// Notes 草稿。
    ///
    /// 注意：`SessionProfile` / `FolderProfile` 目前没有 notes 字段（config 本波冻结），
    /// 所以备注只在本轮编辑会话内保留；接线见
    /// `D:\NewSpace\ui-evidence\wiring\session-editor.md`。
    pub(crate) notes: String,
    // --- session-editor：页级 Inherit -----------------------------------------
    /// Authentication 页级 Inherit：true = 使用父级解析值，保存不写本级 auth。
    pub(crate) inherit_auth: bool,
    /// Login Script 页级 Inherit。
    pub(crate) inherit_login_script: bool,
    /// Proxy 页级 Inherit。
    pub(crate) inherit_proxy: bool,
    /// Tunnels 页级 Inherit。
    pub(crate) inherit_tunnels: bool,
    /// Terminal Style 页级 Inherit。
    pub(crate) inherit_terminal_style: bool,
    // --- session-editor：Login Script / 密钥口令 ------------------------------
    /// Login Script 步骤（生效列表：继承时 = 父级解析值，自定义时 = 本级步骤）。
    pub(crate) login_steps: Vec<LoginStep>,
    /// Private Key 口令草稿：填写即保存，留空 = 需要时再问（无 Remember）。
    pub(crate) passphrase: String,
    /// Icon 下拉是否停在「Custom image」上（尚未 Browse 到文件时 `icon` 仍为空；
    /// config 只存最终引用，不存这个 UI 选择态）。
    pub(crate) icon_selected_custom: bool,
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
    /// NET-1: managed profile id (`Managed`) or legacy per-session id
    /// (`Legacy`). Saving writes it verbatim to `SessionProfile.proxy_profile_id`.
    pub(crate) proxy_profile_id: Option<String>,
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
            icon: String::new(),
            notes: String::new(),
            inherit_auth: true,
            inherit_login_script: true,
            inherit_proxy: true,
            inherit_tunnels: true,
            inherit_terminal_style: true,
            login_steps: Vec::new(),
            passphrase: String::new(),
            icon_selected_custom: false,
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
            proxy_profile_id: None,
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

/// NET-1：代理条目的只读摘要（会话编辑器用；不含口令明文）。
pub(crate) fn proxy_summary_line(profile: &ProxyProfile) -> String {
    let mut line = format!(
        "{} · {}:{}",
        proxy_protocol_display(profile.protocol),
        profile.host.trim(),
        profile.port
    );
    if let Some(username) = profile
        .username
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        line.push_str(&format!(" · user={username}"));
    }
    if profile.password_secret_key.is_some() {
        line.push_str(" · password saved");
    }
    line
}

// ---------------------------------------------------------------------------
// --- session-editor --------------------------------------------------------
// 波 2-B：Session Editor 六分区（General / Authentication / Login Script /
// Proxy / Tunnels / Terminal Style）+ 页级 Inherit + Login Script / Tunnel 子窗。
// 原型：`docs/prototypes/session-editor/`、`docs/prototypes/tunnel-editor/`。
// ---------------------------------------------------------------------------

/// 可页级 Inherit 的分区。顺序 = 投影里 `editor_inherit_sources` 的顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorInheritScope {
    Authentication,
    LoginScript,
    Proxy,
    Tunnels,
    TerminalStyle,
}

impl EditorInheritScope {
    pub(crate) const ALL: [Self; 5] = [
        Self::Authentication,
        Self::LoginScript,
        Self::Proxy,
        Self::Tunnels,
        Self::TerminalStyle,
    ];
}

/// 页级 Inherit 来源：就近父级文件夹名，或全局。
///
/// 文案在 Slint 侧用 `@tr` 组装（Rust 不投英文句子）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorInheritSourceData {
    /// `folder` | `global`
    pub(crate) kind: &'static str,
    /// 就近定义该分区的父级文件夹名（`kind == "global"` 时为空）。
    pub(crate) param: String,
}

/// Login Script 清单行（Slint `LoginStepRow` 的镜像）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorLoginStepRowData {
    /// `keys` | `wait`
    pub(crate) kind: &'static str,
    /// 步骤名（可空）。
    pub(crate) name: String,
    /// 未命名时的摘要：Keys 原文首行，或 `N ms`。
    pub(crate) detail: String,
}

/// Tunnel 清单行（Slint `TunnelRow` 的镜像）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorTunnelRowData {
    /// `local` | `remote` | `dynamic`
    pub(crate) kind: &'static str,
    /// 用户命名（可空；空时 Slint 侧回退到类型标签）。
    pub(crate) name: String,
    /// `bind -> target` 摘要（dynamic 只有 bind）。
    pub(crate) route: String,
}

/// Login Script 步骤子窗种类。
// TODO(wave2-editor): Login Script 步骤子窗接线后删除本行 allow。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorLoginStepKind {
    Keys,
    Wait,
}

impl EditorLoginStepKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Keys => "keys",
            Self::Wait => "wait",
        }
    }

    // TODO(wave2-editor): Login Script 步骤子窗接线后删除。
    #[allow(dead_code)]
    pub(crate) fn from_label(value: &str) -> Self {
        if value == "wait" {
            Self::Wait
        } else {
            Self::Keys
        }
    }
}

/// Login Script 步骤子窗状态（`login_step_keys` / `login_step_wait`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorLoginStepDialog {
    pub(crate) kind: EditorLoginStepKind,
    /// 正在编辑的步骤下标；`None` = 新增。
    pub(crate) index: Option<usize>,
    pub(crate) name: String,
    pub(crate) keys: String,
    /// 是否展开 `{host}` / `{username}` 会话令牌。
    pub(crate) tokens: bool,
    pub(crate) millis_text: String,
}

// TODO(wave2-editor): Login Script 步骤子窗接线后删除本行 allow。
#[allow(dead_code)]
impl EditorLoginStepDialog {
    fn add(kind: EditorLoginStepKind) -> Self {
        Self {
            kind,
            index: None,
            name: String::new(),
            keys: String::new(),
            tokens: false,
            millis_text: "500".to_owned(),
        }
    }

    fn edit(kind: EditorLoginStepKind, index: usize, step: &LoginStep) -> Self {
        let mut dialog = Self {
            kind,
            index: Some(index),
            name: step.name.clone(),
            keys: String::new(),
            tokens: false,
            millis_text: String::new(),
        };
        match &step.action {
            LoginStepAction::Keys { keys, tokens } => {
                dialog.keys = keys.clone();
                dialog.tokens = *tokens;
            }
            LoginStepAction::Wait { millis } => {
                dialog.millis_text = millis.to_string();
            }
        }
        dialog
    }
}

/// Tunnel 子窗状态（`tunnel_editor` 窗）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorTunnelDialog {
    /// 正在编辑的隧道下标；`None` = 新增。
    pub(crate) index: Option<usize>,
    pub(crate) name: String,
    pub(crate) kind: TunnelForwardKind,
    pub(crate) bind_host: String,
    pub(crate) bind_port_text: String,
    pub(crate) target_host: String,
    pub(crate) target_port_text: String,
    /// On connect：连接后自动启动该转发。
    pub(crate) enabled: bool,
}

// TODO(wave2-editor): Tunnel 子窗接线后删除本行 allow。
#[allow(dead_code)]
impl EditorTunnelDialog {
    fn add() -> Self {
        Self {
            index: None,
            name: String::new(),
            kind: TunnelForwardKind::Local,
            bind_host: "127.0.0.1".to_owned(),
            bind_port_text: String::new(),
            target_host: String::new(),
            target_port_text: String::new(),
            enabled: true,
        }
    }

    fn edit(index: usize, forward: &TunnelForward) -> Self {
        Self {
            index: Some(index),
            name: forward.name.clone().unwrap_or_default(),
            kind: forward.kind,
            bind_host: forward.bind_host.clone(),
            bind_port_text: forward.bind_port.to_string(),
            target_host: forward.target_host.clone(),
            target_port_text: forward.target_port.to_string(),
            enabled: forward.enabled,
        }
    }
}

/// `{host}` / `{username}` 会话令牌展开上下文。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LoginScriptTokens {
    pub(crate) host: String,
    pub(crate) username: String,
}

/// Login Script 执行状态：连接成功后按列表顺序推进。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoginScriptRun {
    pub(crate) profile_id: String,
    pub(crate) steps: Vec<LoginStep>,
    pub(crate) next_index: usize,
    pub(crate) wait_until: Option<std::time::Instant>,
    pub(crate) tokens: LoginScriptTokens,
}

// TODO(wave2-editor): Session Editor 的 Icon / 继承开关 / 登录脚本 / 隧道 / Passphrase
// 分区接线后删除本行 allow（当前 Session Editor 子窗尚未接线）。
#[allow(dead_code)]
impl AppRuntime {
    // --- General：Icon / Notes -------------------------------------------
    /// Icon 下拉下标：0 Default / 1 预设 Server / 2 预设 Database / 3 自定义图片。
    pub(crate) fn editor_icon_index(&self) -> i32 {
        match self.editor.icon.trim() {
            "" => {
                if self.editor.icon_selected_custom {
                    3
                } else {
                    0
                }
            }
            "server" => 1,
            "database" => 2,
            _ => 3,
        }
    }

    /// 自定义图片路径（仅自定义项非空）。
    pub(crate) fn editor_icon_custom_text(&self) -> String {
        if self.editor_icon_index() == 3 {
            self.editor.icon.clone()
        } else {
            String::new()
        }
    }

    /// 保存时写入 `SessionProfile.icon` 的值。
    fn editor_icon_config_value(&self) -> Option<String> {
        let value = self.editor.icon.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_owned())
        }
    }

    /// 保存时写入 `SessionProfile.notes` 的值。
    fn editor_notes_config_value(&self) -> Option<String> {
        let value = self.editor.notes.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_owned())
        }
    }

    pub fn update_editor_icon_index(&mut self, index: i32) -> AppProjection {
        let current = self.editor.icon.clone();
        self.editor.icon_selected_custom = index == 3;
        self.editor.icon = match index {
            1 => "server".to_owned(),
            2 => "database".to_owned(),
            // 自定义：保留已经 Browse 到的路径，新选自定义时先留空等 Browse。
            3 if self.editor_icon_index() == 3 => current,
            3 => String::new(),
            _ => String::new(),
        };
        self.projection()
    }

    /// Browse 选中的自定义图标路径（rfd 由宿主触发；见 wiring note）。
    pub fn update_editor_icon_custom(&mut self, value: &str) -> AppProjection {
        self.editor.icon = value.trim().to_owned();
        self.editor.icon_selected_custom = true;
        self.projection()
    }

    pub fn reset_editor_icon(&mut self) -> AppProjection {
        self.editor.icon.clear();
        self.editor.icon_selected_custom = false;
        self.projection()
    }

    pub fn update_editor_notes(&mut self, value: &str) -> AppProjection {
        self.editor.notes = value.to_owned();
        self.projection()
    }

    // --- 页级 Inherit ------------------------------------------------------
    /// 从已解析会话回填草稿里的 icon / notes / Inherit 标记 / login script 步骤。
    pub(crate) fn editor_apply_resolved_session(&mut self, resolved: &ResolvedSessionProfile) {
        self.editor.icon = resolved.session.icon.clone().unwrap_or_default();
        self.editor.notes = resolved.session.notes.clone().unwrap_or_default();
        self.editor.inherit_auth = resolved.session.auth_profile_id.is_none();
        self.editor.inherit_proxy = resolved.session.proxy_profile_id.is_none();
        self.editor.inherit_login_script = resolved.session.login_script.is_none();
        self.editor.inherit_tunnels = resolved.session.tunnel.is_none();
        self.editor.inherit_terminal_style = resolved.session.terminal.is_none();
        self.editor.login_steps = resolved
            .login_script
            .as_ref()
            .map(|script| script.steps.clone())
            .unwrap_or_default();
        self.editor.passphrase.clear();
    }

    /// 只解析父级链路（会话本级覆盖一律清空），用于 Inherit 锁定视图的展示值。
    pub(crate) fn editor_parent_resolved(&self) -> ResolvedSessionProfile {
        let probe = SessionProfile::new("editor-parent-probe", "", "");
        let folder_id = self.editor.target_folder_id.clone();
        self.config_document
            .resolve_session_draft(&probe, &folder_id)
    }

    /// 页级 Inherit 来源（就近父级文件夹 / 全局）。
    pub(crate) fn editor_inherit_sources(&self) -> Vec<EditorInheritSourceData> {
        EditorInheritScope::ALL
            .into_iter()
            .map(|scope| self.editor_inherit_source(scope))
            .collect()
    }

    fn editor_inherit_source(&self, scope: EditorInheritScope) -> EditorInheritSourceData {
        let folder_id = self.editor.target_folder_id.clone();
        let chain = self.config_document.folder_chain_to(&folder_id);
        for folder in chain {
            let defines = match scope {
                EditorInheritScope::Authentication => folder.auth_profile_id.is_some(),
                EditorInheritScope::LoginScript => folder.login_script.is_some(),
                EditorInheritScope::Proxy => folder.proxy_profile_id.is_some(),
                EditorInheritScope::Tunnels => folder.tunnel.is_some(),
                EditorInheritScope::TerminalStyle => folder.terminal.is_some(),
            };
            if defines {
                return EditorInheritSourceData {
                    kind: "folder",
                    param: folder.name.clone(),
                };
            }
        }
        EditorInheritSourceData {
            kind: "global",
            param: String::new(),
        }
    }

    /// Inherit 锁定视图里的父级认证摘要（不含口令明文）。
    pub(crate) fn editor_parent_auth_summary(&self) -> String {
        let Some(auth) = self.editor_parent_resolved().auth else {
            return String::new();
        };
        match &auth.method {
            ConfigAuthMethod::Agent => "Agent".to_owned(),
            ConfigAuthMethod::Password { .. } => "Password · saved".to_owned(),
            ConfigAuthMethod::KeyboardInteractive { .. } => {
                "Keyboard interactive · saved".to_owned()
            }
            ConfigAuthMethod::PrivateKey { key_id, path, .. } => {
                if let Some(label) = key_id
                    .as_deref()
                    .and_then(|key_id| self.config_document.find_key(key_id))
                    .map(|key| key.label.clone())
                {
                    format!("Private key · {label}")
                } else if !path.is_empty() {
                    "Private key · legacy path".to_owned()
                } else {
                    "Private key".to_owned()
                }
            }
        }
    }

    /// Inherit 锁定视图里的父级代理摘要。
    pub(crate) fn editor_parent_proxy_summary(&self) -> String {
        match self.editor_parent_resolved().proxy {
            Some(profile) => proxy_summary_line(&profile),
            None => String::new(),
        }
    }

    /// Inherit 锁定视图里的父级 Terminal Style 名。
    pub(crate) fn editor_parent_terminal_style_summary(&self) -> String {
        let resolved = self.editor_parent_resolved();
        let id = resolved
            .terminal
            .color_scheme
            .as_deref()
            .unwrap_or_default();
        if id.is_empty() {
            String::new()
        } else {
            scheme_display_name(id, &self.theme_catalog)
        }
    }

    /// Inherit 锁定视图里的父级 Login Script 步骤数。
    pub(crate) fn editor_parent_login_step_count(&self) -> i32 {
        let resolved = self.editor_parent_resolved();
        resolved.login_script.as_ref().map_or(0, |script| {
            i32::try_from(script.steps.len()).unwrap_or(i32::MAX)
        })
    }

    /// Inherit 锁定视图里的父级 Tunnel 条数。
    pub(crate) fn editor_parent_tunnel_count(&self) -> i32 {
        let resolved = self.editor_parent_resolved();
        i32::try_from(resolved.tunnel.forwards.len()).unwrap_or(i32::MAX)
    }

    /// 把父级认证镜像进草稿（切 Custom 时"已从父级拷贝"；切 Inherit 时锁定视图同源）。
    fn editor_mirror_parent_auth(&mut self) {
        let parent = self.editor_parent_resolved();
        let Some(auth) = parent.auth.as_ref() else {
            self.editor.auth_method = EditorAuthMethod::Agent;
            self.editor.password.clear();
            self.editor.passphrase.clear();
            self.editor.key_id = None;
            self.editor.legacy_key_path.clear();
            self.editor.legacy_key_has_passphrase = false;
            self.editor.has_stored_password = false;
            return;
        };
        self.editor.auth_profile_id = Some(auth.id.clone());
        self.editor.password.clear();
        self.editor.passphrase.clear();
        self.editor.key_id = None;
        self.editor.legacy_key_path.clear();
        self.editor.legacy_key_has_passphrase = false;
        self.editor.has_stored_password = false;
        match &auth.method {
            ConfigAuthMethod::Agent => self.editor.auth_method = EditorAuthMethod::Agent,
            ConfigAuthMethod::Password { .. } => {
                self.editor.auth_method = EditorAuthMethod::Password;
                self.editor.has_stored_password = true;
            }
            ConfigAuthMethod::KeyboardInteractive { .. } => {
                self.editor.auth_method = EditorAuthMethod::KeyboardInteractive;
                self.editor.has_stored_password = true;
            }
            ConfigAuthMethod::PrivateKey {
                key_id,
                path,
                passphrase_secret_key,
            } => {
                self.editor.auth_method = EditorAuthMethod::PrivateKey;
                if let Some(key_id) = key_id
                    .as_deref()
                    .filter(|key_id| self.config_document.find_key(key_id).is_some())
                {
                    self.editor.key_id = Some(key_id.to_owned());
                } else if !path.is_empty() {
                    self.editor.legacy_key_path = path.clone();
                    self.editor.legacy_key_has_passphrase = passphrase_secret_key.is_some();
                }
            }
        }
    }

    pub fn set_editor_inherit_auth(&mut self, inherit: bool) -> AppProjection {
        if inherit == self.editor.inherit_auth {
            return self.projection();
        }
        self.editor.inherit_auth = inherit;
        self.editor_mirror_parent_auth();
        self.projection()
    }

    pub fn set_editor_inherit_proxy(&mut self, inherit: bool) -> AppProjection {
        if inherit == self.editor.inherit_proxy {
            return self.projection();
        }
        self.editor.inherit_proxy = inherit;
        let parent = self.editor_parent_resolved();
        match parent.proxy.as_ref() {
            Some(proxy) => {
                self.editor.proxy_mode = EditorProxyMode::Managed;
                self.editor.proxy_profile_id = Some(proxy.id.clone());
            }
            None => {
                self.editor.proxy_mode = EditorProxyMode::None;
                self.editor.proxy_profile_id = None;
            }
        }
        self.projection()
    }

    pub fn set_editor_inherit_login_script(&mut self, inherit: bool) -> AppProjection {
        if inherit == self.editor.inherit_login_script {
            return self.projection();
        }
        self.editor.inherit_login_script = inherit;
        let parent = self.editor_parent_resolved();
        self.editor.login_steps = parent
            .login_script
            .as_ref()
            .map(|script| script.steps.clone())
            .unwrap_or_default();
        self.projection()
    }

    pub fn set_editor_inherit_tunnels(&mut self, inherit: bool) -> AppProjection {
        if inherit == self.editor.inherit_tunnels {
            return self.projection();
        }
        self.editor.inherit_tunnels = inherit;
        let parent = self.editor_parent_resolved();
        self.editor.tunnels = parent.tunnel.forwards.clone();
        self.projection()
    }

    pub fn set_editor_inherit_terminal_style(&mut self, inherit: bool) -> AppProjection {
        if inherit == self.editor.inherit_terminal_style {
            return self.projection();
        }
        self.editor.inherit_terminal_style = inherit;
        if inherit {
            // 回到父级：丢掉本级终端覆盖（预览随即回到父级解析值）。
            self.reset_all_editor_theme();
        } else {
            // Custom：把父级配色拷贝成本级显式值（"已从父级拷贝"）。
            let parent_scheme = self
                .editor_parent_resolved()
                .terminal
                .color_scheme
                .clone()
                .unwrap_or_else(|| "yshell-default".to_owned());
            let field = &mut self.editor.theme.theme.color_scheme;
            field.text = parent_scheme;
            field.explicit = true;
        }
        self.projection()
    }

    // --- Authentication：Private Key 口令 ---------------------------------
    pub fn update_editor_passphrase(&mut self, value: &str) -> AppProjection {
        self.editor.passphrase = value.to_owned();
        self.editor.inherit_auth = false;
        self.projection()
    }

    // --- Login Script 分区 -------------------------------------------------
    pub fn select_session_editor_login_script(&mut self) -> AppProjection {
        self.editor_section = EditorSection::LoginScript;
        self.projection()
    }

    /// Login Script 清单行（Rust → Slint `LoginStepRow`）。
    pub(crate) fn editor_login_step_rows(&self) -> Vec<EditorLoginStepRowData> {
        self.editor
            .login_steps
            .iter()
            .map(|step| {
                let (kind, detail) = match &step.action {
                    LoginStepAction::Keys { keys, .. } => {
                        let first_line = keys.lines().next().unwrap_or_default().trim();
                        let detail = if first_line.is_empty() {
                            String::new()
                        } else {
                            first_line.to_owned()
                        };
                        ("keys", detail)
                    }
                    LoginStepAction::Wait { millis } => ("wait", format!("{millis} ms")),
                };
                EditorLoginStepRowData {
                    kind,
                    name: step.name.clone(),
                    detail,
                }
            })
            .collect()
    }

    pub fn select_editor_login_step(&mut self, index: i32) -> AppProjection {
        self.editor_selected_login_step = usize::try_from(index).ok();
        self.projection()
    }

    /// 打开 Keys / Wait 步骤子窗（`kind` = `keys` | `wait`）。
    pub fn add_editor_login_step(&mut self, kind: &str) -> AppProjection {
        let kind = EditorLoginStepKind::from_label(kind);
        self.editor_login_step_dialog = Some(EditorLoginStepDialog::add(kind));
        self.editor.inherit_login_script = false;
        self.projection()
    }

    pub fn edit_editor_login_step(&mut self, index: i32) -> AppProjection {
        let Some(index) = usize::try_from(index).ok() else {
            return self.projection();
        };
        let Some(step) = self.editor.login_steps.get(index) else {
            return self.projection();
        };
        let kind = match step.action {
            LoginStepAction::Keys { .. } => EditorLoginStepKind::Keys,
            LoginStepAction::Wait { .. } => EditorLoginStepKind::Wait,
        };
        self.editor_login_step_dialog = Some(EditorLoginStepDialog::edit(kind, index, step));
        self.editor.inherit_login_script = false;
        self.projection()
    }

    pub fn update_editor_login_step_name(&mut self, value: &str) -> AppProjection {
        if let Some(dialog) = self.editor_login_step_dialog.as_mut() {
            dialog.name = value.to_owned();
        }
        self.projection()
    }

    pub fn update_editor_login_step_keys(&mut self, value: &str) -> AppProjection {
        if let Some(dialog) = self.editor_login_step_dialog.as_mut() {
            dialog.keys = value.to_owned();
        }
        self.projection()
    }

    pub fn set_editor_login_step_tokens(&mut self, tokens: bool) -> AppProjection {
        if let Some(dialog) = self.editor_login_step_dialog.as_mut() {
            dialog.tokens = tokens;
        }
        self.projection()
    }

    pub fn update_editor_login_step_millis(&mut self, value: &str) -> AppProjection {
        if let Some(dialog) = self.editor_login_step_dialog.as_mut() {
            dialog.millis_text = value.to_owned();
        }
        self.projection()
    }

    /// 提交步骤子窗：新增或原地更新。
    pub fn save_editor_login_step(&mut self) -> AppResult<AppProjection> {
        let Some(dialog) = self.editor_login_step_dialog.clone() else {
            return Ok(self.projection());
        };
        let name = dialog.name.trim().to_owned();
        let action = match dialog.kind {
            EditorLoginStepKind::Keys => {
                if dialog.keys.trim().is_empty() {
                    return Err(AppError::new("login step keys must not be empty"));
                }
                LoginStepAction::Keys {
                    keys: dialog.keys.clone(),
                    tokens: dialog.tokens,
                }
            }
            EditorLoginStepKind::Wait => {
                let millis = parse_login_step_millis(&dialog.millis_text)?;
                LoginStepAction::Wait { millis }
            }
        };
        let step = LoginStep { name, action };
        let kind_text = dialog.kind.label();
        match dialog.index {
            Some(index) if index < self.editor.login_steps.len() => {
                self.editor.login_steps[index] = step;
                self.editor_selected_login_step = Some(index);
                self.set_status_kind(
                    "editor-login-step-updated",
                    format!("Updated login script {} step.", kind_text),
                    kind_text.to_owned(),
                    String::new(),
                );
            }
            _ => {
                self.editor.login_steps.push(step);
                self.editor_selected_login_step = Some(self.editor.login_steps.len() - 1);
                self.set_status_kind(
                    "editor-login-step-added",
                    format!("Added login script {} step.", kind_text),
                    kind_text.to_owned(),
                    String::new(),
                );
            }
        }
        self.editor_login_step_dialog = None;
        self.editor.inherit_login_script = false;
        Ok(self.projection())
    }

    pub fn cancel_editor_login_step(&mut self) -> AppProjection {
        self.editor_login_step_dialog = None;
        self.projection()
    }

    pub fn delete_editor_login_step(&mut self, index: i32) -> AppProjection {
        let Some(index) = usize::try_from(index).ok() else {
            return self.projection();
        };
        if index >= self.editor.login_steps.len() {
            return self.projection();
        }
        self.editor.login_steps.remove(index);
        self.editor.inherit_login_script = false;
        self.editor_selected_login_step = if self.editor.login_steps.is_empty() {
            None
        } else {
            Some(index.min(self.editor.login_steps.len() - 1))
        };
        if self
            .editor_login_step_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.index == Some(index))
        {
            self.editor_login_step_dialog = None;
        }
        self.set_status_kind(
            "editor-login-step-deleted",
            "Deleted login script step.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    pub fn duplicate_editor_login_step(&mut self, index: i32) -> AppProjection {
        let Some(index) = usize::try_from(index).ok() else {
            return self.projection();
        };
        let Some(step) = self.editor.login_steps.get(index).cloned() else {
            return self.projection();
        };
        let mut copy = step;
        copy.name = if copy.name.trim().is_empty() {
            String::new()
        } else {
            format!("{} copy", copy.name)
        };
        self.editor.login_steps.insert(index + 1, copy);
        self.editor.inherit_login_script = false;
        self.editor_selected_login_step = Some(index + 1);
        self.set_status_kind(
            "editor-login-step-duplicated",
            "Duplicated login script step.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    /// 拖拽 / 右键调序：把 `from` 移到 `to`（下标越界时忽略）。
    pub fn move_editor_login_step(&mut self, from: i32, to: i32) -> AppProjection {
        let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
            return self.projection();
        };
        let len = self.editor.login_steps.len();
        if from >= len || to >= len || from == to {
            return self.projection();
        }
        let step = self.editor.login_steps.remove(from);
        self.editor.login_steps.insert(to, step);
        self.editor.inherit_login_script = false;
        self.editor_selected_login_step = Some(to);
        self.set_status_kind(
            "editor-login-steps-moved",
            "Reordered login script steps.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    // --- Tunnels 分区 -----------------------------------------------------
    /// Tunnel 清单行（Rust → Slint `TunnelRow`）。
    pub(crate) fn editor_tunnel_rows(&self) -> Vec<EditorTunnelRowData> {
        self.editor
            .tunnels
            .iter()
            .map(|forward| EditorTunnelRowData {
                kind: tunnel_kind_label(forward.kind),
                name: forward.name.clone().unwrap_or_default(),
                route: super::ssh_config::tunnel_forward_summary_body(forward),
            })
            .collect()
    }

    pub fn select_editor_tunnel(&mut self, index: i32) -> AppProjection {
        self.editor_selected_tunnel = usize::try_from(index).ok();
        self.projection()
    }

    /// 打开 tunnel-editor 窗（新增 / 编辑）。
    pub fn begin_add_editor_tunnel(&mut self) -> AppProjection {
        self.editor_tunnel_dialog = Some(EditorTunnelDialog::add());
        self.editor.inherit_tunnels = false;
        self.projection()
    }

    pub fn begin_edit_editor_tunnel(&mut self, index: i32) -> AppProjection {
        let Some(index) = usize::try_from(index).ok() else {
            return self.projection();
        };
        let Some(forward) = self.editor.tunnels.get(index) else {
            return self.projection();
        };
        self.editor_tunnel_dialog = Some(EditorTunnelDialog::edit(index, forward));
        self.editor.inherit_tunnels = false;
        self.projection()
    }

    pub fn update_editor_tunnel_name(&mut self, value: &str) -> AppProjection {
        if let Some(dialog) = self.editor_tunnel_dialog.as_mut() {
            dialog.name = value.to_owned();
        }
        self.projection()
    }

    pub fn set_editor_tunnel_kind(&mut self, kind: &str) -> AppProjection {
        if let Some(dialog) = self.editor_tunnel_dialog.as_mut() {
            dialog.kind = match kind {
                "remote" => TunnelForwardKind::Remote,
                "dynamic" => TunnelForwardKind::Dynamic,
                _ => TunnelForwardKind::Local,
            };
        }
        self.projection()
    }

    pub fn update_editor_tunnel_dialog_bind_host(&mut self, value: &str) -> AppProjection {
        if let Some(dialog) = self.editor_tunnel_dialog.as_mut() {
            dialog.bind_host = value.to_owned();
        }
        self.projection()
    }

    pub fn update_editor_tunnel_dialog_bind_port(&mut self, value: &str) -> AppProjection {
        if let Some(dialog) = self.editor_tunnel_dialog.as_mut() {
            dialog.bind_port_text = value.to_owned();
        }
        self.projection()
    }

    pub fn update_editor_tunnel_dialog_target_host(&mut self, value: &str) -> AppProjection {
        if let Some(dialog) = self.editor_tunnel_dialog.as_mut() {
            dialog.target_host = value.to_owned();
        }
        self.projection()
    }

    pub fn update_editor_tunnel_dialog_target_port(&mut self, value: &str) -> AppProjection {
        if let Some(dialog) = self.editor_tunnel_dialog.as_mut() {
            dialog.target_port_text = value.to_owned();
        }
        self.projection()
    }

    pub fn set_editor_tunnel_enabled(&mut self, enabled: bool) -> AppProjection {
        if let Some(dialog) = self.editor_tunnel_dialog.as_mut() {
            dialog.enabled = enabled;
        }
        self.projection()
    }

    pub fn save_editor_tunnel_dialog(&mut self) -> AppResult<AppProjection> {
        let Some(dialog) = self.editor_tunnel_dialog.clone() else {
            return Ok(self.projection());
        };
        let forward = build_tunnel_forward_from_dialog(&dialog)?;
        let name_text = dialog.name.trim().to_owned();
        let kind_text = tunnel_kind_label(forward.kind);
        let summary = super::ssh_config::tunnel_forward_summary_body(&forward);
        match dialog.index {
            Some(index) if index < self.editor.tunnels.len() => {
                self.editor.tunnels[index] = forward;
                self.editor_selected_tunnel = Some(index);
                self.set_status_kind(
                    "editor-tunnel-updated",
                    format!("Updated tunnel {}.", name_text),
                    kind_text.to_owned(),
                    summary,
                );
            }
            _ => {
                self.editor.tunnels.push(forward);
                self.editor_selected_tunnel = Some(self.editor.tunnels.len() - 1);
                self.set_status_kind(
                    "editor-tunnel-added",
                    format!("Added {kind_text} tunnel {name_text}."),
                    kind_text.to_owned(),
                    summary,
                );
            }
        }
        self.editor_tunnel_dialog = None;
        self.editor.inherit_tunnels = false;
        Ok(self.projection())
    }

    pub fn cancel_editor_tunnel_dialog(&mut self) -> AppProjection {
        self.editor_tunnel_dialog = None;
        self.projection()
    }

    pub fn duplicate_editor_tunnel(&mut self, index: i32) -> AppProjection {
        let Some(index) = usize::try_from(index).ok() else {
            return self.projection();
        };
        let Some(forward) = self.editor.tunnels.get(index).cloned() else {
            return self.projection();
        };
        let mut copy = forward;
        copy.name = match copy.name {
            Some(name) if !name.trim().is_empty() => Some(format!("{name} copy")),
            _ => None,
        };
        self.editor.tunnels.insert(index + 1, copy);
        self.editor.inherit_tunnels = false;
        self.editor_selected_tunnel = Some(index + 1);
        self.set_status_kind(
            "editor-tunnel-duplicated",
            "Duplicated tunnel configuration.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    pub fn delete_editor_tunnel(&mut self, index: i32) -> AppProjection {
        let Some(index) = usize::try_from(index).ok() else {
            return self.projection();
        };
        if index >= self.editor.tunnels.len() {
            return self.projection();
        }
        self.editor.tunnels.remove(index);
        self.editor.inherit_tunnels = false;
        self.editor_selected_tunnel = if self.editor.tunnels.is_empty() {
            None
        } else {
            Some(index.min(self.editor.tunnels.len() - 1))
        };
        if self
            .editor_tunnel_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.index == Some(index))
        {
            self.editor_tunnel_dialog = None;
        }
        self.set_status_kind(
            "editor-tunnel-removed",
            "Deleted tunnel configuration.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }
}

/// Login Script 步骤等待上限校验（配置 sanitize 同一上限）。
// TODO(wave2-editor): Login Script 步骤子窗接线后删除。
#[allow(dead_code)]
fn parse_login_step_millis(value: &str) -> AppResult<u64> {
    let millis = value
        .trim()
        .parse::<u64>()
        .map_err(|_| AppError::new("login step wait must be an integer number of milliseconds"))?;
    Ok(millis.min(LOGIN_STEP_MAX_WAIT_MILLIS))
}

/// 由 Tunnel 子窗草稿构建 `TunnelForward`（校验端口与目标；Dynamic 无目标）。
// TODO(wave2-editor): Tunnel 子窗接线后删除。
#[allow(dead_code)]
fn build_tunnel_forward_from_dialog(dialog: &EditorTunnelDialog) -> AppResult<TunnelForward> {
    let bind_host = dialog.bind_host.trim();
    if bind_host.is_empty() {
        return Err(AppError::new("tunnel listen host must not be empty"));
    }
    let bind_port = parse_port_field(
        &dialog.bind_port_text,
        "tunnel listen port must be a valid integer between 1 and 65535",
    )?;
    let (target_host, target_port) = if dialog.kind == TunnelForwardKind::Dynamic {
        (String::new(), 0)
    } else {
        let host = dialog.target_host.trim();
        if host.is_empty() {
            return Err(AppError::new("tunnel target host must not be empty"));
        }
        let port = parse_port_field(
            &dialog.target_port_text,
            "tunnel target port must be a valid integer between 1 and 65535",
        )?;
        (host.to_owned(), port)
    };
    let name = dialog.name.trim();
    Ok(TunnelForward {
        name: if name.is_empty() {
            None
        } else {
            Some(name.to_owned())
        },
        enabled: dialog.enabled,
        kind: dialog.kind,
        bind_host: bind_host.to_owned(),
        bind_port,
        target_host,
        target_port,
    })
}

/// 把 Login Script 的 Keys 文本展开成写入终端的字节。
///
/// 规则（原型 `sections/login-step-keys.html` 的转换表）：
/// * 普通字符原样发送；`{{` / `}}` 转义为字面量花括号；
/// * 特殊键写成 `{Enter}` / `{Tab}` / `{Ctrl+C}` …（大小写不敏感）；
/// * `{host}` / `{username}` 在 `expand_tokens` 时展开为会话上下文；
/// * 未知 token 原样保留（含花括号），不吞用户输入。
#[allow(dead_code)]
// 接线：连接成功后由 `bootstrap` 调用（见 wiring note）。
// TODO(wave2-editor): 接线后把整段 unescape 循环改成 clippy 建议的 `for … by_ref()`。
#[allow(clippy::while_let_on_iterator)]
pub(crate) fn login_keys_to_bytes(
    keys: &str,
    expand_tokens: bool,
    context: &LoginScriptTokens,
) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut literal = String::new();
    let mut chars = keys.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    literal.push('{');
                    continue;
                }
                let mut token = String::new();
                let mut closed = false;
                while let Some(next) = chars.next() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    token.push(next);
                }
                if !closed {
                    // 没有闭合：原样回填（含起始花括号）。
                    literal.push('{');
                    literal.push_str(&token);
                    continue;
                }
                flush_literal(&mut out, &mut literal);
                match login_key_token(&token, expand_tokens, context) {
                    Some(bytes) => out.extend_from_slice(&bytes),
                    None => {
                        literal.push('{');
                        literal.push_str(&token);
                        literal.push('}');
                    }
                }
            }
            '}' => {
                if chars.peek() == Some(&'}') {
                    chars.next();
                    literal.push('}');
                } else {
                    literal.push('}');
                }
            }
            other => literal.push(other),
        }
    }
    flush_literal(&mut out, &mut literal);
    out
}

fn flush_literal(out: &mut Vec<u8>, literal: &mut String) {
    if !literal.is_empty() {
        out.extend_from_slice(literal.as_bytes());
        literal.clear();
    }
}

/// 单个 `{…}` token → 字节；`None` = 未知 token（保留原文）。
#[allow(dead_code)] // 仅供 `login_keys_to_bytes` 内部使用（接线前不计入 reachable）。
fn login_key_token(
    token: &str,
    expand_tokens: bool,
    context: &LoginScriptTokens,
) -> Option<Vec<u8>> {
    let trimmed = token.trim();
    let lower = trimmed.to_ascii_lowercase();
    if expand_tokens {
        match lower.as_str() {
            "host" => return Some(context.host.as_bytes().to_vec()),
            "username" | "user" => return Some(context.username.as_bytes().to_vec()),
            _ => {}
        }
    }
    // 组合键：`Ctrl+C` / `Alt+x` / `Shift+Tab`（修饰符大小写不敏感）。
    let mut modifiers: Vec<String> = Vec::new();
    let mut base = lower.as_str();
    while let Some((modifier, rest)) = base.split_once('+') {
        modifiers.push(modifier.trim().to_owned());
        base = rest.trim();
    }
    if !modifiers.is_empty() {
        return Some(modified_key_bytes(&modifiers, base));
    }
    simple_key_bytes(&lower)
}

#[allow(dead_code)] // 仅供 `login_key_token` 内部使用。
fn simple_key_bytes(key: &str) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = match key {
        "enter" | "return" => vec![b'\r'],
        "tab" => vec![b'\t'],
        "esc" | "escape" => vec![0x1b],
        "space" => vec![b' '],
        "backspace" => vec![0x7f],
        "delete" | "del" => b"\x1b[3~".to_vec(),
        "insert" | "ins" => b"\x1b[2~".to_vec(),
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "left" => b"\x1b[D".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "pageup" | "pgup" => b"\x1b[5~".to_vec(),
        "pagedown" | "pgdn" => b"\x1b[6~".to_vec(),
        _ => return None,
    };
    Some(bytes)
}

#[allow(dead_code)] // 仅供 `login_key_token` 内部使用。
fn modified_key_bytes(modifiers: &[String], base: &str) -> Vec<u8> {
    let has = |name: &str| modifiers.iter().any(|modifier| modifier == name);
    // 命名基键（Enter / Tab / 方向键…）带修饰符时按基键发送。
    if let Some(mut bytes) = simple_key_bytes(base) {
        if has("alt") {
            let mut prefixed = vec![0x1b];
            prefixed.append(&mut bytes);
            return prefixed;
        }
        return bytes;
    }
    let mut chars = base.chars();
    let Some(ch) = chars.next() else {
        return Vec::new();
    };
    if chars.next().is_some() {
        // 多字符串基键（未知）原样返回。
        return base.as_bytes().to_vec();
    }
    if has("ctrl") {
        // Ctrl+<char>：ASCII 控制码（a-z、@[\]^_ 与空格）。
        let ch = ch.to_ascii_lowercase();
        let code = ch as u32;
        if (0x40..0x80).contains(&code) {
            let control = (code & 0x1f) as u8;
            if has("alt") {
                return vec![0x1b, control];
            }
            return vec![control];
        }
    }
    if has("shift") {
        let mut out: Vec<u8> = Vec::new();
        if has("alt") {
            out.push(0x1b);
        }
        let upper = ch.to_uppercase().collect::<String>();
        out.extend_from_slice(upper.as_bytes());
        return out;
    }
    let mut out: Vec<u8> = Vec::new();
    if has("alt") {
        out.push(0x1b);
    }
    out.extend_from_slice(ch.to_string().as_bytes());
    out
}

impl AppRuntime {
    // --- Login Script 执行挂点（连接成功后调用；见 wiring note）-------------
    /// 连接成功后启动该会话的 Login Script（空列表 = 不启动）。
    #[allow(dead_code)] // 接线：连接成功后调用（见 wiring note）。
    pub fn start_login_script_for_session(&mut self, profile_id: &str) -> AppProjection {
        let Some(resolved) = self.config_document.resolve_session(profile_id) else {
            return self.projection();
        };
        let steps: Vec<LoginStep> = resolved
            .login_script
            .as_ref()
            .map(|script| {
                script
                    .steps
                    .iter()
                    .filter(|step| login_step_has_effect(step))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if steps.is_empty() {
            self.editor_login_script_run = None;
            return self.projection();
        }
        self.editor_login_script_run = Some(LoginScriptRun {
            profile_id: profile_id.to_owned(),
            steps,
            next_index: 0,
            wait_until: None,
            tokens: LoginScriptTokens {
                host: resolved.session.host.clone(),
                username: resolved.session.username.clone().unwrap_or_default(),
            },
        });
        self.set_status_kind(
            "editor-login-script-started",
            "Running the login script for this session.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    /// 推进 Login Script（UI tick 调用；Wait 到期后再继续下一段）。
    #[allow(dead_code)]
    // 接线：UI tick 每帧调用（见 wiring note）。
    // TODO(wave2-editor): 接线后把外层 `loop {}` 改成 clippy 建议的 `while let`。
    #[allow(clippy::while_let_loop)]
    pub fn tick_login_script(&mut self) -> AppProjection {
        loop {
            let Some((next_index, wait_until, total)) = self
                .editor_login_script_run
                .as_ref()
                .map(|run| (run.next_index, run.wait_until, run.steps.len()))
            else {
                break;
            };
            if let Some(deadline) = wait_until {
                if std::time::Instant::now() < deadline {
                    break;
                }
            }
            if next_index >= total {
                self.editor_login_script_run = None;
                self.set_status_kind(
                    "editor-login-script-finished",
                    "Login script finished.".to_owned(),
                    String::new(),
                    String::new(),
                );
                break;
            }
            let Some(run) = self.editor_login_script_run.as_ref() else {
                break;
            };
            let step = run.steps[next_index].clone();
            let tokens = run.tokens.clone();
            let profile_id = run.profile_id.clone();
            match &step.action {
                LoginStepAction::Wait { millis } => {
                    if let Some(run) = self.editor_login_script_run.as_mut() {
                        run.next_index += 1;
                        run.wait_until = if *millis == 0 {
                            None
                        } else {
                            Some(
                                std::time::Instant::now()
                                    + std::time::Duration::from_millis(*millis),
                            )
                        };
                    }
                }
                LoginStepAction::Keys {
                    keys,
                    tokens: expand,
                } => {
                    let bytes = login_keys_to_bytes(keys, *expand, &tokens);
                    let write_result = self.write_login_script_bytes(&profile_id, &bytes);
                    match write_result {
                        Ok(()) => {
                            if let Some(run) = self.editor_login_script_run.as_mut() {
                                run.next_index += 1;
                                run.wait_until = None;
                            }
                        }
                        Err(error) => {
                            self.editor_login_script_run = None;
                            self.set_status_kind(
                                "editor-login-script-failed",
                                format!("Login script stopped: {error}"),
                                error.to_string(),
                                String::new(),
                            );
                            break;
                        }
                    }
                }
            }
        }
        self.projection()
    }

    /// 中止正在执行的 Login Script（断开 / 关标签时调用）。
    #[allow(dead_code)] // 接线：断开 / 关标签时调用（见 wiring note）。
    pub fn abort_login_script(&mut self) {
        self.editor_login_script_run = None;
    }

    #[allow(dead_code)] // 仅供 `tick_login_script` 内部使用。
    fn write_login_script_bytes(&mut self, profile_id: &str, bytes: &[u8]) -> AppResult<()> {
        let session_key = self
            .sessions
            .iter()
            .find(|(_, runtime)| {
                matches!(
                    &runtime.source,
                    SessionSource::SavedSession { profile_id: current } if current == profile_id
                )
            })
            .map(|(key, _)| key.clone())
            .ok_or_else(|| AppError::new("the session is no longer connected"))?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("the session is no longer connected"))?;
        runtime
            .write_terminal_input(bytes)
            .map(|_| ())
            .map_err(AppError::from_error)
    }
}

/// 空 Keys 步骤不产生输入，直接跳过。
#[allow(dead_code)] // 接线：连接成功路径调用（见 wiring note）。
fn login_step_has_effect(step: &LoginStep) -> bool {
    match &step.action {
        LoginStepAction::Keys { keys, .. } => !keys.trim().is_empty(),
        LoginStepAction::Wait { .. } => true,
    }
}

#[cfg(test)]
mod session_editor_tests {
    use super::*;
    use yshell_config::{AuthProfile, ProxyProfile, ProxyProtocol};

    fn test_runtime() -> (tempfile::TempDir, AppRuntime) {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime with temp config dir");
        (temp, runtime)
    }

    fn save_new_session(runtime: &mut AppRuntime, name: &str, host: &str) -> String {
        runtime.start_new_saved_session_editor();
        runtime.update_editor_name(name);
        runtime.update_editor_host(host);
        runtime.update_editor_port("22");
        runtime.update_editor_username("alice");
        runtime
            .save_editor_to_saved_session()
            .expect("save session");
        runtime
            .selected_saved_session_id
            .clone()
            .expect("saved session id")
    }

    fn add_keys_step(runtime: &mut AppRuntime, name: &str, keys: &str) {
        runtime.add_editor_login_step("keys");
        runtime.update_editor_login_step_name(name);
        runtime.update_editor_login_step_keys(keys);
        runtime.save_editor_login_step().expect("save keys step");
    }

    #[test]
    fn login_keys_expand_named_tokens_and_escapes() {
        let tokens = LoginScriptTokens::default();
        assert_eq!(
            login_keys_to_bytes("cd ~{Enter}", false, &tokens),
            b"cd ~\r".to_vec()
        );
        assert_eq!(
            login_keys_to_bytes("a{Tab}b{Esc}{Space}c", false, &tokens),
            b"a\tb\x1b c".to_vec()
        );
        assert_eq!(
            login_keys_to_bytes("{Up}{Down}{Left}{Right}", false, &tokens),
            b"\x1b[A\x1b[B\x1b[D\x1b[C".to_vec()
        );
        // 字面量花括号：`{{` / `}}`。
        assert_eq!(
            login_keys_to_bytes("echo {{a}}{Enter}", false, &tokens),
            b"echo {a}\r".to_vec()
        );
        // 大小写不敏感 + 回车别名。
        assert_eq!(
            login_keys_to_bytes("{ENTER}{return}", false, &tokens),
            b"\r\r".to_vec()
        );
    }

    #[test]
    fn login_keys_expand_control_and_alt_modifiers() {
        let tokens = LoginScriptTokens::default();
        assert_eq!(login_keys_to_bytes("{Ctrl+C}", false, &tokens), vec![0x03]);
        assert_eq!(login_keys_to_bytes("{Ctrl+d}", false, &tokens), vec![0x04]);
        assert_eq!(
            login_keys_to_bytes("{Alt+x}", false, &tokens),
            b"\x1bx".to_vec()
        );
        assert_eq!(
            login_keys_to_bytes("{Shift+Tab}", false, &tokens),
            b"\t".to_vec()
        );
    }

    #[test]
    fn login_keys_expand_session_tokens_only_when_requested() {
        let tokens = LoginScriptTokens {
            host: "edge.example.com".to_owned(),
            username: "alice".to_owned(),
        };
        assert_eq!(
            login_keys_to_bytes("ssh {username}@{host}{Enter}", true, &tokens),
            b"ssh alice@edge.example.com\r".to_vec()
        );
        // tokens=false 时保留原文（由终端看到的字面量）。
        assert_eq!(
            login_keys_to_bytes("{host}", false, &tokens),
            b"{host}".to_vec()
        );
    }

    #[test]
    fn login_keys_keep_unknown_tokens_literal_and_utf8() {
        let tokens = LoginScriptTokens::default();
        assert_eq!(
            login_keys_to_bytes("{Unknown}中文", false, &tokens),
            "{Unknown}中文".as_bytes().to_vec()
        );
        // 没有闭合花括号：原样回填，不吞输入。
        assert_eq!(login_keys_to_bytes("a{b", false, &tokens), b"a{b".to_vec());
    }

    #[test]
    fn login_script_steps_add_edit_reorder_delete_and_persist() {
        let (_temp, mut runtime) = test_runtime();
        let session_id = save_new_session(&mut runtime, "prod-edge", "edge.example.com");

        add_keys_step(&mut runtime, "Go home", "cd ~{Enter}");
        runtime.add_editor_login_step("wait");
        runtime.update_editor_login_step_millis("500");
        runtime.save_editor_login_step().expect("save wait step");
        add_keys_step(&mut runtime, "Load env", "source ./env.sh{Enter}");
        assert_eq!(runtime.editor.login_steps.len(), 3);

        // 调序：第 3 步上移到第 1 位；再删除中间步骤。
        runtime.move_editor_login_step(2, 0);
        assert_eq!(runtime.editor.login_steps[0].name, "Load env");
        runtime.delete_editor_login_step(1);
        assert_eq!(runtime.editor.login_steps.len(), 2);

        // 编辑第 2 步（wait），改等待值。
        runtime.edit_editor_login_step(1);
        runtime.update_editor_login_step_millis("1200");
        runtime.save_editor_login_step().expect("save edited wait");

        // 重复一步。
        runtime.duplicate_editor_login_step(0);
        assert_eq!(runtime.editor.login_steps.len(), 3);

        runtime.save_editor_to_saved_session().expect("persist");
        let session = runtime
            .config_document
            .find_session(&session_id)
            .expect("saved session");
        let script = session.login_script.as_ref().expect("own login script");
        assert_eq!(script.steps.len(), 3);
        assert_eq!(script.steps[0].name, "Load env");
        assert_eq!(script.steps[1].name, "Load env copy");
        match &script.steps[2].action {
            LoginStepAction::Wait { millis } => assert_eq!(*millis, 1200),
            other => panic!("expected wait step, got {other:?}"),
        }
    }

    #[test]
    fn wait_step_millis_is_clamped_to_config_limit() {
        let (_temp, mut runtime) = test_runtime();
        runtime.add_editor_login_step("wait");
        runtime.update_editor_login_step_millis("999999");
        runtime.save_editor_login_step().expect("save wait");
        match &runtime.editor.login_steps[0].action {
            LoginStepAction::Wait { millis } => {
                assert_eq!(*millis, LOGIN_STEP_MAX_WAIT_MILLIS);
            }
            other => panic!("expected wait step, got {other:?}"),
        }
    }

    #[test]
    fn icon_and_notes_persist_with_the_session() {
        let (_temp, mut runtime) = test_runtime();
        runtime.start_new_saved_session_editor();
        runtime.update_editor_name("icon-session");
        runtime.update_editor_host("icon.example.com");
        // 预设图标。
        runtime.update_editor_icon_index(1);
        assert_eq!(runtime.editor_icon_index(), 1);
        runtime.update_editor_notes("Primary edge host.");
        runtime.save_editor_to_saved_session().expect("save");

        let session_id = runtime.selected_saved_session_id.clone().expect("id");
        let session = runtime
            .config_document
            .find_session(&session_id)
            .expect("saved session");
        assert_eq!(session.icon.as_deref(), Some("server"));
        assert_eq!(session.notes.as_deref(), Some("Primary edge host."));

        // 重新载入编辑器：icon / notes 回填。
        runtime
            .load_selected_saved_session_into_editor()
            .expect("load into editor");
        assert_eq!(runtime.editor_icon_index(), 1);
        assert_eq!(runtime.editor.notes, "Primary edge host.");

        // 自定义图片路径 + Reset。
        runtime.update_editor_icon_index(3);
        assert_eq!(runtime.editor_icon_index(), 3);
        runtime.update_editor_icon_custom("  C:/icons/edge.png  ");
        assert_eq!(runtime.editor_icon_custom_text(), "C:/icons/edge.png");
        runtime.reset_editor_icon();
        assert_eq!(runtime.editor_icon_index(), 0);
    }

    #[test]
    fn inherit_toggles_clear_session_level_overrides_on_save() {
        let (_temp, mut runtime) = test_runtime();
        runtime.config_document.proxy_profiles.insert(
            "corp".to_owned(),
            ProxyProfile {
                id: "corp".to_owned(),
                name: "corp-socks5".to_owned(),
                protocol: ProxyProtocol::Socks5,
                host: "proxy.example.com".to_owned(),
                port: 1080,
                username: None,
                resolve_dns_by_proxy: true,
                password_secret_key: None,
            },
        );
        let session_id = save_new_session(&mut runtime, "inherit-me", "edge.example.com");

        // 全部切成 Custom 并写入本级覆盖。
        runtime.set_editor_inherit_auth(false);
        runtime.set_editor_auth_method_agent();
        runtime.set_editor_inherit_proxy(false);
        runtime.select_editor_proxy(0);
        runtime.set_editor_inherit_tunnels(false);
        runtime.begin_add_editor_tunnel();
        runtime.update_editor_tunnel_dialog_bind_port("8080");
        runtime.update_editor_tunnel_dialog_target_host("127.0.0.1");
        runtime.update_editor_tunnel_dialog_target_port("80");
        runtime.save_editor_tunnel_dialog().expect("save tunnel");
        runtime.set_editor_inherit_login_script(false);
        add_keys_step(&mut runtime, "hi", "whoami{Enter}");
        runtime.set_editor_inherit_terminal_style(false);
        runtime.select_editor_scheme_index(1);
        runtime.save_editor_to_saved_session().expect("save custom");

        let session = runtime
            .config_document
            .find_session(&session_id)
            .expect("saved session")
            .clone();
        assert!(session.auth_profile_id.is_some());
        assert!(session.proxy_profile_id.is_some());
        assert!(session.tunnel.is_some());
        assert!(session.login_script.is_some());
        assert!(session.terminal.is_some());

        // 全部回到 Inherit：保存后本级覆盖清空。
        runtime
            .load_selected_saved_session_into_editor()
            .expect("reload");
        assert!(!runtime.editor.inherit_auth);
        assert!(!runtime.editor.inherit_proxy);
        assert!(!runtime.editor.inherit_tunnels);
        assert!(!runtime.editor.inherit_login_script);
        assert!(!runtime.editor.inherit_terminal_style);
        runtime.set_editor_inherit_auth(true);
        runtime.set_editor_inherit_proxy(true);
        runtime.set_editor_inherit_tunnels(true);
        runtime.set_editor_inherit_login_script(true);
        runtime.set_editor_inherit_terminal_style(true);
        runtime
            .save_editor_to_saved_session()
            .expect("save inherit");

        let session = runtime
            .config_document
            .find_session(&session_id)
            .expect("saved session");
        assert!(session.auth_profile_id.is_none());
        assert!(session.proxy_profile_id.is_none());
        assert!(session.tunnel.is_none());
        assert!(session.login_script.is_none());
        assert!(session.terminal.is_none());
    }

    #[test]
    fn inherited_login_script_resolves_from_parent_folder() {
        let (_temp, mut runtime) = test_runtime();
        let mut folder = FolderProfile::new("ops", "Production");
        folder.login_script = Some(LoginScriptProfile {
            steps: vec![LoginStep {
                name: "parent step".to_owned(),
                action: LoginStepAction::Keys {
                    keys: "cd /srv{Enter}".to_owned(),
                    tokens: false,
                },
            }],
        });
        runtime.config_document.folders.push(folder);

        runtime.start_new_saved_session_editor();
        runtime.editor.target_folder_id = "ops".to_owned();
        runtime.update_editor_name("child");
        runtime.update_editor_host("child.example.com");
        // 默认 Inherit 开：会话不写 login_script，解析取父级。
        assert!(runtime.editor.inherit_login_script);
        let sources = runtime.editor_inherit_sources();
        assert_eq!(sources[1].kind, "folder");
        assert_eq!(sources[1].param, "Production");
        assert_eq!(runtime.editor_parent_login_step_count(), 1);

        runtime.save_editor_to_saved_session().expect("save child");
        let session_id = runtime.selected_saved_session_id.clone().expect("id");
        assert!(runtime
            .config_document
            .find_session(&session_id)
            .expect("saved child")
            .login_script
            .is_none());
        let resolved = runtime
            .config_document
            .resolve_session(&session_id)
            .expect("resolve child");
        let script = resolved.login_script.expect("inherited script");
        assert_eq!(script.steps.len(), 1);
        assert_eq!(script.steps[0].name, "parent step");
    }

    #[test]
    fn tunnel_dialog_rows_add_duplicate_delete_and_persist() {
        let (_temp, mut runtime) = test_runtime();
        let session_id = save_new_session(&mut runtime, "tunnels", "tunnel.example.com");

        runtime.begin_add_editor_tunnel();
        runtime.update_editor_tunnel_name("local-postgres");
        runtime.update_editor_tunnel_dialog_bind_host("127.0.0.1");
        runtime.update_editor_tunnel_dialog_bind_port("5432");
        runtime.update_editor_tunnel_dialog_target_host("db.internal");
        runtime.update_editor_tunnel_dialog_target_port("5432");
        runtime.set_editor_tunnel_enabled(true);
        runtime.save_editor_tunnel_dialog().expect("add tunnel");

        runtime.begin_add_editor_tunnel();
        runtime.set_editor_tunnel_kind("dynamic");
        runtime.update_editor_tunnel_dialog_bind_port("1080");
        runtime
            .save_editor_tunnel_dialog()
            .expect("add dynamic tunnel");

        let rows = runtime.editor_tunnel_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "local-postgres");
        assert_eq!(rows[0].kind, "local");
        assert_eq!(rows[1].kind, "dynamic");

        // Duplicate / Delete。
        runtime.duplicate_editor_tunnel(0);
        assert_eq!(runtime.editor_tunnel_rows().len(), 3);
        assert_eq!(runtime.editor_tunnel_rows()[1].name, "local-postgres copy");
        runtime.delete_editor_tunnel(0);
        assert_eq!(runtime.editor_tunnel_rows().len(), 2);
        assert_eq!(runtime.editor_tunnel_rows()[0].name, "local-postgres copy");

        runtime.save_editor_to_saved_session().expect("persist");
        let session = runtime
            .config_document
            .find_session(&session_id)
            .expect("saved session");
        let forwards = &session.tunnel.as_ref().expect("tunnel override").forwards;
        assert_eq!(forwards.len(), 2);
        assert_eq!(forwards[0].name.as_deref(), Some("local-postgres copy"));
        assert!(forwards[0].enabled);
        assert_eq!(forwards[0].bind_port, 5432);
        assert_eq!(forwards[1].kind, TunnelForwardKind::Dynamic);
    }

    #[test]
    fn auth_custom_mode_copies_the_inherited_profile() {
        let (_temp, mut runtime) = test_runtime();
        // 父级文件夹提供一个 agent auth profile，子会话默认继承。
        runtime.config_document.auth_profiles.insert(
            "folder-auth".to_owned(),
            AuthProfile {
                id: "folder-auth".to_owned(),
                name: "Folder auth".to_owned(),
                method: ConfigAuthMethod::Agent,
            },
        );
        let mut folder = FolderProfile::new("secure", "Secure");
        folder.auth_profile_id = Some("folder-auth".to_owned());
        runtime.config_document.folders.push(folder);

        runtime.start_new_saved_session_editor();
        runtime.editor.target_folder_id = "secure".to_owned();
        runtime.update_editor_name("child");
        runtime.update_editor_host("secure.example.com");
        assert_eq!(
            runtime.editor_inherit_sources()[0].param,
            "Secure",
            "auth source should point at the nearest parent folder"
        );
        assert_eq!(runtime.editor_parent_auth_summary(), "Agent");

        // Custom：从父级拷贝（保存时写成自己的 auth profile）。
        runtime.set_editor_inherit_auth(false);
        let own = runtime.editor.auth_profile_id.clone();
        assert_eq!(own.as_deref(), Some("folder-auth"));
        runtime.save_editor_to_saved_session().expect("save");
        let session_id = runtime.selected_saved_session_id.clone().expect("id");
        let session = runtime
            .config_document
            .find_session(&session_id)
            .expect("saved session");
        let own_auth_id = session.auth_profile_id.clone().expect("own auth profile");
        assert_ne!(own_auth_id, "folder-auth");
        assert_eq!(
            runtime
                .config_document
                .auth_profiles
                .get(&own_auth_id)
                .expect("copied profile")
                .method,
            ConfigAuthMethod::Agent
        );
        // 父级 profile 不被覆盖。
        assert!(runtime
            .config_document
            .auth_profiles
            .contains_key("folder-auth"));
    }
}
