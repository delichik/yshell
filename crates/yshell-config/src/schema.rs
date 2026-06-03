//! Versioned configuration schema, profile types, migration, and inheritance.

use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Current configuration schema version for newly-created stores.
pub const SCHEMA_VERSION: u32 = 1;

/// Root TOML document persisted by the configuration store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigDocument {
    /// Schema version used to decide whether migrations are needed.
    pub schema_version: u32,
    /// Saved session folders and sessions.
    #[serde(default)]
    pub folders: Vec<FolderProfile>,
    /// Saved auth profiles keyed by id.
    #[serde(default)]
    pub auth_profiles: BTreeMap<String, AuthProfile>,
    /// Saved proxy profiles keyed by id.
    #[serde(default)]
    pub proxy_profiles: BTreeMap<String, ProxyProfile>,
    /// Default SFTP behavior inherited by sessions.
    #[serde(default)]
    pub sftp: SftpProfile,
    /// Default tunnel behavior inherited by sessions.
    #[serde(default)]
    pub tunnel: TunnelProfile,
    /// Default appearance behavior inherited by sessions.
    #[serde(default)]
    pub appearance: AppearanceProfile,
    /// Default logging behavior inherited by sessions.
    #[serde(default)]
    pub logging: LoggingProfile,
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            folders: Vec::new(),
            auth_profiles: BTreeMap::new(),
            proxy_profiles: BTreeMap::new(),
            sftp: SftpProfile::default(),
            tunnel: TunnelProfile::default(),
            appearance: AppearanceProfile::default(),
            logging: LoggingProfile::default(),
        }
    }
}

/// A folder node in the saved-session tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub sessions: Vec<SessionProfile>,
    #[serde(default)]
    pub folders: Vec<FolderProfile>,
}

impl FolderProfile {
    /// Creates an empty folder node.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            sessions: Vec::new(),
            folders: Vec::new(),
        }
    }

    /// Recursively finds an immutable session by id.
    #[must_use]
    pub fn find_session(&self, id: &str) -> Option<&SessionProfile> {
        self.sessions
            .iter()
            .find(|session| session.id == id)
            .or_else(|| {
                self.folders
                    .iter()
                    .find_map(|folder| folder.find_session(id))
            })
    }

    /// Recursively finds a mutable session by id.
    pub fn find_session_mut(&mut self, id: &str) -> Option<&mut SessionProfile> {
        if let Some(index) = self.sessions.iter().position(|session| session.id == id) {
            return self.sessions.get_mut(index);
        }
        self.folders
            .iter_mut()
            .find_map(|folder| folder.find_session_mut(id))
    }
}

/// Saved SSH/SFTP session profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub auth_profile_id: Option<String>,
    #[serde(default)]
    pub proxy_profile_id: Option<String>,
    #[serde(default)]
    pub sftp: Option<SftpProfile>,
    #[serde(default)]
    pub tunnel: Option<TunnelProfile>,
    #[serde(default)]
    pub appearance: Option<AppearanceProfile>,
    #[serde(default)]
    pub logging: Option<LoggingProfile>,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl SessionProfile {
    /// Creates a session with SSH defaults.
    pub fn new(id: impl Into<String>, name: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            host: host.into(),
            port: default_ssh_port(),
            username: None,
            auth_profile_id: None,
            proxy_profile_id: None,
            sftp: None,
            tunnel: None,
            appearance: None,
            logging: None,
            tags: Vec::new(),
        }
    }

    /// Applies a partial batch edit to basic user-facing fields.
    pub fn apply_basic_edit(&mut self, edit: &SessionBasicEdit) {
        if let Some(name) = &edit.name {
            self.name.clone_from(name);
        }
        if let Some(host) = &edit.host {
            self.host.clone_from(host);
        }
        if let Some(port) = edit.port {
            self.port = port;
        }
        if edit.username_set {
            self.username.clone_from(&edit.username);
        }
        if let Some(tags) = &edit.tags {
            self.tags.clone_from(tags);
        }
    }
}

/// Basic fields supported by batch edits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionBasicEdit {
    pub name: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub username_set: bool,
    pub tags: Option<Vec<String>>,
}

/// Auth material reference. Secrets are addressed by key rather than persisted inline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthProfile {
    pub id: String,
    pub name: String,
    pub method: AuthMethod,
}

/// Supported auth methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthMethod {
    Password {
        secret_key: String,
    },
    PrivateKey {
        path: String,
        #[serde(default)]
        passphrase_secret_key: Option<String>,
    },
    Agent,
}

/// Optional SSH proxy/jump profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
}

/// SFTP defaults or session override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftpProfile {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub initial_directory: Option<String>,
}

impl Default for SftpProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            initial_directory: None,
        }
    }
}

/// Local/remote tunnel defaults or session override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TunnelProfile {
    #[serde(default)]
    pub forwards: Vec<TunnelForward>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelForward {
    pub kind: TunnelForwardKind,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelForwardKind {
    Local,
    Remote,
    Dynamic,
}

/// Visual defaults or session override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppearanceProfile {
    pub theme: String,
    pub font_family: String,
    pub font_size: u16,
}

impl Default for AppearanceProfile {
    fn default() -> Self {
        Self {
            theme: "system".to_owned(),
            font_family: "monospace".to_owned(),
            font_size: 13,
        }
    }
}

/// Logging defaults or session override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggingProfile {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default = "default_log_format")]
    pub format: String,
}

impl Default for LoggingProfile {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: None,
            format: default_log_format(),
        }
    }
}

/// A session with all inheritable defaults resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSessionProfile {
    pub session: SessionProfile,
    pub auth: Option<AuthProfile>,
    pub proxy: Option<ProxyProfile>,
    pub sftp: SftpProfile,
    pub tunnel: TunnelProfile,
    pub appearance: AppearanceProfile,
    pub logging: LoggingProfile,
}

impl ConfigDocument {
    /// Serializes the document as TOML.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Parses a document from TOML and runs migrations.
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigSchemaError> {
        let value = toml::Value::from_str(input)?;
        migrate_value(value)
    }

    /// Finds a session anywhere in the folder tree.
    #[must_use]
    pub fn find_session(&self, id: &str) -> Option<&SessionProfile> {
        self.folders
            .iter()
            .find_map(|folder| folder.find_session(id))
    }

    /// Finds a mutable session anywhere in the folder tree.
    pub fn find_session_mut(&mut self, id: &str) -> Option<&mut SessionProfile> {
        self.folders
            .iter_mut()
            .find_map(|folder| folder.find_session_mut(id))
    }

    /// Applies a basic edit to every matching session id and returns the count updated.
    pub fn batch_edit_basic<'a>(
        &mut self,
        ids: impl IntoIterator<Item = &'a str>,
        edit: &SessionBasicEdit,
    ) -> usize {
        ids.into_iter()
            .filter(|id| {
                self.find_session_mut(id)
                    .map(|session| session.apply_basic_edit(edit))
                    .is_some()
            })
            .count()
    }

    /// Resolves a session against document-level defaults and referenced profiles.
    #[must_use]
    pub fn resolve_session(&self, id: &str) -> Option<ResolvedSessionProfile> {
        let session = self.find_session(id)?.clone();
        Some(ResolvedSessionProfile {
            auth: session
                .auth_profile_id
                .as_deref()
                .and_then(|id| self.auth_profiles.get(id))
                .cloned(),
            proxy: session
                .proxy_profile_id
                .as_deref()
                .and_then(|id| self.proxy_profiles.get(id))
                .cloned(),
            sftp: session.sftp.clone().unwrap_or_else(|| self.sftp.clone()),
            tunnel: session
                .tunnel
                .clone()
                .unwrap_or_else(|| self.tunnel.clone()),
            appearance: session
                .appearance
                .clone()
                .unwrap_or_else(|| self.appearance.clone()),
            logging: session
                .logging
                .clone()
                .unwrap_or_else(|| self.logging.clone()),
            session,
        })
    }
}

/// Schema/migration parse errors.
#[derive(Debug)]
pub enum ConfigSchemaError {
    TomlDe(toml::de::Error),
    UnsupportedVersion(u32),
    MissingVersion,
}

impl fmt::Display for ConfigSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TomlDe(error) => write!(f, "invalid TOML configuration: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported configuration schema version {version}")
            }
            Self::MissingVersion => f.write_str("missing configuration schema_version"),
        }
    }
}

impl std::error::Error for ConfigSchemaError {}

impl From<toml::de::Error> for ConfigSchemaError {
    fn from(value: toml::de::Error) -> Self {
        Self::TomlDe(value)
    }
}

/// Migrates a parsed TOML value into the current schema.
pub fn migrate_value(mut value: toml::Value) -> Result<ConfigDocument, ConfigSchemaError> {
    let version = value
        .get("schema_version")
        .and_then(toml::Value::as_integer)
        .ok_or(ConfigSchemaError::MissingVersion)?;
    if version < 0 {
        return Err(ConfigSchemaError::UnsupportedVersion(version as u32));
    }
    let version = version as u32;

    match version {
        SCHEMA_VERSION => toml::Value::try_into(value).map_err(ConfigSchemaError::TomlDe),
        0 => {
            value["schema_version"] = toml::Value::Integer(i64::from(SCHEMA_VERSION));
            toml::Value::try_into(value).map_err(ConfigSchemaError::TomlDe)
        }
        other => Err(ConfigSchemaError::UnsupportedVersion(other)),
    }
}

const fn default_ssh_port() -> u16 {
    22
}

const fn default_true() -> bool {
    true
}

fn default_log_format() -> String {
    "text".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_document() -> ConfigDocument {
        let mut document = ConfigDocument {
            sftp: SftpProfile {
                enabled: true,
                initial_directory: Some("/srv".to_owned()),
            },
            ..ConfigDocument::default()
        };
        document.auth_profiles.insert(
            "auth-1".to_owned(),
            AuthProfile {
                id: "auth-1".to_owned(),
                name: "Key".to_owned(),
                method: AuthMethod::Agent,
            },
        );
        let mut session = SessionProfile::new("session-1", "Prod", "example.com");
        session.auth_profile_id = Some("auth-1".to_owned());
        document.folders.push(FolderProfile {
            id: "folder-1".to_owned(),
            name: "Servers".to_owned(),
            sessions: vec![session],
            folders: Vec::new(),
        });
        document
    }

    #[test]
    fn toml_round_trip_preserves_profiles() {
        let document = sample_document();
        let toml = document.to_toml_string().expect("serialize");
        let parsed = ConfigDocument::from_toml_str(&toml).expect("parse");

        assert_eq!(parsed, document);
    }

    #[test]
    fn resolves_document_defaults_and_references() {
        let resolved = sample_document()
            .resolve_session("session-1")
            .expect("session should resolve");

        assert_eq!(resolved.sftp.initial_directory.as_deref(), Some("/srv"));
        assert_eq!(resolved.auth.expect("auth").name, "Key");
    }

    #[test]
    fn batch_edits_basic_fields() {
        let mut document = sample_document();
        let edit = SessionBasicEdit {
            port: Some(2200),
            username: Some("deploy".to_owned()),
            username_set: true,
            ..SessionBasicEdit::default()
        };

        assert_eq!(
            document.batch_edit_basic(["session-1"].into_iter(), &edit),
            1
        );
        let session = document.find_session("session-1").expect("edited");
        assert_eq!(session.port, 2200);
        assert_eq!(session.username.as_deref(), Some("deploy"));
    }

    #[test]
    fn migrates_schema_zero_to_current() {
        let parsed = ConfigDocument::from_toml_str("schema_version = 0\n").expect("migration");

        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        assert!(parsed.folders.is_empty());
    }
}
