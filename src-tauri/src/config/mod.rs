use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionProfile {
    pub id: String,
    pub name: String,
    pub folder_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub favorite: bool,
    pub protocol: SessionProtocol,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub auth: AuthConfig,
    pub proxy: Option<ProxyConfig>,
    pub terminal: TerminalConfig,
    pub appearance: Option<AppearanceConfig>,
    pub logging: Option<LoggingConfig>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_connected_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionProtocol {
    Local,
    Ssh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthConfig {
    pub method: AuthMethod,
    pub username: Option<String>,
    pub private_key_path: Option<String>,
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    Password,
    PrivateKey,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyKind {
    Http,
    Socks5,
    JumpHost,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalConfig {
    pub shell: Option<String>,
    pub working_directory: Option<String>,
    pub font_family: String,
    pub font_size: u8,
    pub line_height: f32,
    pub color_scheme: String,
    pub scrollback: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceConfig {
    pub app_theme: AppTheme,
    pub terminal_opacity: f32,
    pub reduce_motion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppTheme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoggingConfig {
    pub enabled: bool,
    pub directory: Option<String>,
    pub naming_template: String,
    pub redact_sensitive_input: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub appearance: AppearanceConfig,
    pub terminal: TerminalConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExportBundle {
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub sessions: Vec<SessionProfile>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            appearance: AppearanceConfig {
                app_theme: AppTheme::Dark,
                terminal_opacity: 1.0,
                reduce_motion: false,
            },
            terminal: TerminalConfig {
                shell: None,
                working_directory: None,
                font_family: "Cascadia Mono, JetBrains Mono, SFMono-Regular, Consolas, monospace"
                    .to_string(),
                font_size: 14,
                line_height: 1.25,
                color_scheme: "One Dark".to_string(),
                scrollback: 10_000,
            },
            logging: LoggingConfig {
                enabled: false,
                directory: None,
                naming_template: "{session}-{host}-{date}.log".to_string(),
                redact_sensitive_input: true,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedConfig {
    version: u32,
    sessions: Vec<SessionProfile>,
    settings: AppSettings,
}

impl Default for PersistedConfig {
    fn default() -> Self {
        Self {
            version: 1,
            sessions: Vec::new(),
            settings: AppSettings::default(),
        }
    }
}

#[derive(Debug)]
pub struct ConfigStore {
    path: std::path::PathBuf,
    state: std::sync::Mutex<PersistedConfig>,
}

impl ConfigStore {
    pub fn load_default() -> Self {
        let path = default_config_path();
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str::<PersistedConfig>(&content).ok())
            .unwrap_or_default();
        Self {
            path,
            state: std::sync::Mutex::new(state),
        }
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionProfile>, String> {
        let state = self.lock_state()?;
        Ok(state.sessions.clone())
    }

    pub fn save_session(&self, profile: SessionProfile) -> Result<SessionProfile, String> {
        Self::validate_session(&profile)?;
        let mut state = self.lock_state()?;
        if let Some(existing) = state
            .sessions
            .iter_mut()
            .find(|session| session.id == profile.id)
        {
            *existing = profile.clone();
        } else {
            state.sessions.insert(0, profile.clone());
        }
        Self::persist(&self.path, &state)?;
        Ok(profile)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<Vec<SessionProfile>, String> {
        let mut state = self.lock_state()?;
        let original_len = state.sessions.len();
        state.sessions.retain(|session| session.id != session_id);
        if state.sessions.len() == original_len {
            return Err(format!("session {session_id} not found"));
        }
        Self::persist(&self.path, &state)?;
        Ok(state.sessions.clone())
    }

    pub fn duplicate_session(&self, session_id: &str) -> Result<SessionProfile, String> {
        let mut state = self.lock_state()?;
        let Some(source) = state
            .sessions
            .iter()
            .find(|session| session.id == session_id)
        else {
            return Err(format!("session {session_id} not found"));
        };
        let now = Utc::now();
        let mut cloned = source.clone();
        cloned.id = Uuid::new_v4().to_string();
        cloned.name = format!("{} 副本", source.name);
        cloned.created_at = now;
        cloned.updated_at = now;
        cloned.last_connected_at = None;
        state.sessions.insert(0, cloned.clone());
        Self::persist(&self.path, &state)?;
        Ok(cloned)
    }

    pub fn export_sessions(&self) -> Result<SessionExportBundle, String> {
        let state = self.lock_state()?;
        let mut sessions = state.sessions.clone();
        for session in &mut sessions {
            session.auth.credential_ref = None;
        }
        Ok(SessionExportBundle {
            version: state.version,
            exported_at: Utc::now(),
            sessions,
        })
    }

    pub fn import_sessions(
        &self,
        bundle: SessionExportBundle,
    ) -> Result<Vec<SessionProfile>, String> {
        if bundle.version != 1 {
            return Err(format!(
                "unsupported session export version: {}",
                bundle.version
            ));
        }
        let mut state = self.lock_state()?;
        for mut imported in bundle.sessions {
            Self::validate_session(&imported)?;
            imported.auth.credential_ref = None;
            imported.updated_at = Utc::now();
            if imported.id.trim().is_empty() {
                imported.id = Uuid::new_v4().to_string();
            }
            if let Some(existing) = state
                .sessions
                .iter_mut()
                .find(|session| session.id == imported.id)
            {
                *existing = imported;
            } else {
                state.sessions.insert(0, imported);
            }
        }
        Self::persist(&self.path, &state)?;
        Ok(state.sessions.clone())
    }

    pub fn load_settings(&self) -> Result<AppSettings, String> {
        let state = self.lock_state()?;
        Ok(state.settings.clone())
    }

    pub fn save_settings(&self, settings: AppSettings) -> Result<AppSettings, String> {
        let mut state = self.lock_state()?;
        state.settings = settings.clone();
        Self::persist(&self.path, &state)?;
        Ok(settings)
    }

    fn validate_session(profile: &SessionProfile) -> Result<(), String> {
        if profile.name.trim().is_empty() {
            return Err("session name is required".to_string());
        }
        if matches!(profile.protocol, SessionProtocol::Ssh) {
            if profile
                .host
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
            {
                return Err(format!("SSH session {} is missing host", profile.name));
            }
            if profile.port.is_none() {
                return Err(format!("SSH session {} is missing port", profile.name));
            }
        }
        Ok(())
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, PersistedConfig>, String> {
        self.state
            .lock()
            .map_err(|_| "config store lock poisoned".to_string())
    }

    fn persist(path: &std::path::Path, state: &PersistedConfig) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create config directory: {error}"))?;
        }
        let content = serde_json::to_string_pretty(state)
            .map_err(|error| format!("failed to serialize config: {error}"))?;
        std::fs::write(path, content).map_err(|error| format!("failed to write config: {error}"))
    }
}

fn default_config_path() -> std::path::PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .or_else(home_config_dir)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            std::path::PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(home_config_dir)
    }
    .unwrap_or_else(|| std::path::PathBuf::from("."));

    base.join("yshell").join("config.json")
}

fn home_config_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
}
