use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionProfile {
    pub id: String,
    pub name: String,
    pub folder_id: Option<String>,
    pub tags: Vec<String>,
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
