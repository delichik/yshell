//! TOML configuration persistence with damaged-file backup recovery.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::schema::{ConfigDocument, ConfigSchemaError, ConfigWarning};
use serde::{Deserialize, Serialize};
use yshell_ssh::{HostKeyFingerprint, KnownHosts};

const CONFIG_FILE_NAME: &str = "config.toml";
const KNOWN_HOSTS_FILE_NAME: &str = "known_hosts.toml";

/// File-backed configuration store rooted at the YShell config directory.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    root: PathBuf,
}

/// Result of loading a configuration document.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadOutcome {
    pub document: ConfigDocument,
    pub recovered_from_backup: Option<PathBuf>,
}

impl ConfigStore {
    /// Creates a store rooted at a configuration directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the root configuration directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the canonical configuration file path.
    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.root.join(CONFIG_FILE_NAME)
    }

    #[must_use]
    pub fn known_hosts_file(&self) -> PathBuf {
        self.root.join(KNOWN_HOSTS_FILE_NAME)
    }

    /// Loads the document, creating a default file when missing.
    ///
    /// If the existing TOML is unreadable or invalid, it is renamed to a
    /// timestamped `.bak` file and replaced with a default document.
    pub fn load_or_recover(&self) -> Result<LoadOutcome, ConfigStoreError> {
        self.load_or_recover_with_warnings()
            .map(|(outcome, _)| outcome)
    }

    /// Loads the document and returns non-fatal warnings found while loading.
    ///
    /// Warnings cover invalid Quick Connect/quick-link targets and invalid theme
    /// values that were dropped or cleared by [`ConfigDocument::sanitize`].
    pub fn load_or_recover_with_warnings(
        &self,
    ) -> Result<(LoadOutcome, Vec<ConfigWarning>), ConfigStoreError> {
        fs::create_dir_all(&self.root)?;
        let path = self.config_file();
        if !path.exists() {
            let document = ConfigDocument::default();
            self.save(&document)?;
            let outcome = LoadOutcome {
                document,
                recovered_from_backup: None,
            };
            return Ok((outcome, Vec::new()));
        }

        let input = fs::read_to_string(&path);
        match input {
            Ok(input) => match ConfigDocument::from_toml_str_with_warnings(&input) {
                Ok((document, warnings)) => {
                    let outcome = LoadOutcome {
                        document,
                        recovered_from_backup: None,
                    };
                    Ok((outcome, warnings))
                }
                Err(error) => {
                    let outcome = self.backup_and_reset(path, ConfigStoreError::Schema(error))?;
                    Ok((outcome, Vec::new()))
                }
            },
            Err(error) => {
                let outcome = self.backup_and_reset(path, ConfigStoreError::Io(error))?;
                Ok((outcome, Vec::new()))
            }
        }
    }

    /// Saves the document atomically enough for local configuration usage.
    pub fn save(&self, document: &ConfigDocument) -> Result<(), ConfigStoreError> {
        fs::create_dir_all(&self.root)?;
        let path = self.config_file();
        let tmp_path = path.with_extension("toml.tmp");
        let toml = document.to_toml_string()?;
        fs::write(&tmp_path, toml)?;
        fs::rename(tmp_path, path)?;
        Ok(())
    }

    pub fn load_known_hosts(&self) -> Result<KnownHosts, ConfigStoreError> {
        fs::create_dir_all(&self.root)?;
        let path = self.known_hosts_file();
        if !path.exists() {
            self.save_known_hosts(&KnownHosts::new())?;
            return Ok(KnownHosts::new());
        }
        let input = fs::read_to_string(&path)?;
        let document: KnownHostsDocument = toml::from_str(&input)
            .map_err(|error| ConfigStoreError::KnownHostsSchema(error.to_string()))?;
        Ok(document.into_known_hosts())
    }

    pub fn save_known_hosts(&self, known_hosts: &KnownHosts) -> Result<(), ConfigStoreError> {
        fs::create_dir_all(&self.root)?;
        let path = self.known_hosts_file();
        let tmp_path = path.with_extension("toml.tmp");
        let toml = toml::to_string_pretty(&KnownHostsDocument::from_known_hosts(known_hosts))
            .map_err(ConfigStoreError::Serialize)?;
        fs::write(&tmp_path, toml)?;
        fs::rename(tmp_path, path)?;
        Ok(())
    }

    fn backup_and_reset(
        &self,
        path: PathBuf,
        _cause: ConfigStoreError,
    ) -> Result<LoadOutcome, ConfigStoreError> {
        let backup_path = self.backup_path();
        fs::rename(&path, &backup_path)?;
        let document = ConfigDocument::default();
        self.save(&document)?;
        Ok(LoadOutcome {
            document,
            recovered_from_backup: Some(backup_path),
        })
    }

    fn backup_path(&self) -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        self.root.join(format!("config.toml.{millis}.bak"))
    }
}

/// Persistence errors.
#[derive(Debug)]
pub enum ConfigStoreError {
    Io(io::Error),
    Schema(ConfigSchemaError),
    KnownHostsSchema(String),
    Serialize(toml::ser::Error),
}

impl std::fmt::Display for ConfigStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "configuration I/O error: {error}"),
            Self::Schema(error) => write!(f, "configuration schema error: {error}"),
            Self::KnownHostsSchema(error) => write!(f, "known_hosts schema error: {error}"),
            Self::Serialize(error) => write!(f, "configuration serialization error: {error}"),
        }
    }
}

impl std::error::Error for ConfigStoreError {}

impl From<io::Error> for ConfigStoreError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<toml::ser::Error> for ConfigStoreError {
    fn from(value: toml::ser::Error) -> Self {
        Self::Serialize(value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct KnownHostsDocument {
    #[serde(default)]
    entries: BTreeMap<String, KnownHostFingerprintRecord>,
}

impl KnownHostsDocument {
    fn from_known_hosts(known_hosts: &KnownHosts) -> Self {
        Self {
            entries: known_hosts
                .snapshot()
                .into_iter()
                .map(|(key, value)| {
                    (
                        key,
                        KnownHostFingerprintRecord {
                            algorithm: value.algorithm,
                            fingerprint: value.fingerprint,
                        },
                    )
                })
                .collect(),
        }
    }

    fn into_known_hosts(self) -> KnownHosts {
        KnownHosts::from_snapshot(
            self.entries
                .into_iter()
                .map(|(key, value)| {
                    (
                        key,
                        HostKeyFingerprint {
                            algorithm: value.algorithm,
                            fingerprint: value.fingerprint,
                        },
                    )
                })
                .collect(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct KnownHostFingerprintRecord {
    algorithm: String,
    fingerprint: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_loads_toml() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let document = ConfigDocument::default();

        store.save(&document).expect("save");
        let outcome = store.load_or_recover().expect("load");

        assert_eq!(outcome.document, document);
        assert!(outcome.recovered_from_backup.is_none());
    }

    #[test]
    fn load_reports_sanitize_warnings_and_drops_invalid_targets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        fs::create_dir_all(temp.path()).expect("mkdir");
        fs::write(
            store.config_file(),
            r#"
schema_version = 2

[[quick_connect.history]]
target = "not a host"
"#,
        )
        .expect("write config");

        let (outcome, warnings) = store
            .load_or_recover_with_warnings()
            .expect("load with warnings");

        assert!(outcome.recovered_from_backup.is_none());
        assert!(outcome.document.quick_connect.history.is_empty());
        assert!(matches!(
            warnings.as_slice(),
            [ConfigWarning::InvalidQuickConnectTarget { target, .. }] if target == "not a host"
        ));
    }

    #[test]
    fn backs_up_damaged_toml_and_resets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        fs::create_dir_all(temp.path()).expect("mkdir");
        fs::write(store.config_file(), "not = [valid").expect("write bad config");

        let outcome = store.load_or_recover().expect("recover");

        assert_eq!(outcome.document, ConfigDocument::default());
        let backup = outcome.recovered_from_backup.expect("backup");
        assert!(backup.exists());
        assert!(store.config_file().exists());
    }

    #[test]
    fn saves_and_loads_known_hosts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut known_hosts = KnownHosts::new();
        known_hosts.pin(
            "example.test",
            22,
            HostKeyFingerprint {
                algorithm: "ssh-ed25519".to_owned(),
                fingerprint: "abc123".to_owned(),
            },
        );

        store
            .save_known_hosts(&known_hosts)
            .expect("save known_hosts");
        let loaded = store.load_known_hosts().expect("load known_hosts");

        assert_eq!(loaded, known_hosts);
    }
}
