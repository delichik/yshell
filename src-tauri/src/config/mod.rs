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
