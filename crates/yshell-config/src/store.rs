//! TOML configuration persistence with damaged-file backup recovery.
//!
//! Global preferences live in `config.toml`. Saved sessions and key metadata
//! live in sibling directories (`sessions/`, `keys/`) and are merged into the
//! in-memory [`ConfigDocument`] on load.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::keys::KeyCatalog;
use crate::schema::{ConfigDocument, ConfigSchemaError, ConfigWarning};
use crate::sessions::SessionTreeStore;
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

    /// Directory that holds user color-theme TOML files.
    #[must_use]
    pub fn themes_dir(&self) -> PathBuf {
        self.root.join(crate::themes::THEMES_DIR_NAME)
    }

    /// Directory that holds the Xshell-style session tree.
    #[must_use]
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join(crate::sessions::SESSIONS_DIR_NAME)
    }

    /// Directory that holds managed key metadata files.
    #[must_use]
    pub fn keys_dir(&self) -> PathBuf {
        self.root.join(crate::keys::KEYS_DIR_NAME)
    }

    /// Session-tree helper rooted at this config directory.
    #[must_use]
    pub fn session_tree(&self) -> SessionTreeStore {
        SessionTreeStore::new(&self.root)
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
    /// Warnings cover invalid Quick Connect targets and invalid theme
    /// values that were dropped or cleared by [`ConfigDocument::sanitize`].
    /// Session/key tree warnings are recorded as [`ConfigWarning::StorageNotice`].
    pub fn load_or_recover_with_warnings(
        &self,
    ) -> Result<(LoadOutcome, Vec<ConfigWarning>), ConfigStoreError> {
        fs::create_dir_all(&self.root)?;
        let path = self.config_file();
        if !path.exists() {
            let mut document = ConfigDocument::default();
            self.save(&document)?;
            let mut warnings = Vec::new();
            self.attach_external_stores(&mut document, &mut warnings)?;
            let outcome = LoadOutcome {
                document,
                recovered_from_backup: None,
            };
            return Ok((outcome, warnings));
        }

        let input = fs::read_to_string(&path);
        match input {
            Ok(input) => match ConfigDocument::from_toml_str_with_warnings(&input) {
                Ok((mut document, mut warnings)) => {
                    self.attach_external_stores(&mut document, &mut warnings)?;
                    let outcome = LoadOutcome {
                        document,
                        recovered_from_backup: None,
                    };
                    Ok((outcome, warnings))
                }
                Err(error) => {
                    let (mut outcome, mut warnings) =
                        self.backup_and_reset(path, ConfigStoreError::Schema(error))?;
                    self.attach_external_stores(&mut outcome.document, &mut warnings)?;
                    Ok((outcome, warnings))
                }
            },
            Err(error) => {
                let (mut outcome, mut warnings) =
                    self.backup_and_reset(path, ConfigStoreError::Io(error))?;
                self.attach_external_stores(&mut outcome.document, &mut warnings)?;
                Ok((outcome, warnings))
            }
        }
    }

    /// Saves global preferences atomically. Session folders and keys are not
    /// written here — use [`SessionTreeStore`] / [`KeyCatalog`].
    pub fn save(&self, document: &ConfigDocument) -> Result<(), ConfigStoreError> {
        fs::create_dir_all(&self.root)?;
        let path = self.config_file();
        let tmp_path = path.with_extension("toml.tmp");
        let toml = document.to_toml_string()?;
        fs::write(&tmp_path, toml)?;
        fs::rename(tmp_path, path)?;
        Ok(())
    }

    /// Saves globals plus the full session tree and key catalog (tests/fixtures).
    pub fn save_all(&self, document: &ConfigDocument) -> Result<(), ConfigStoreError> {
        self.save(document)?;
        self.session_tree()
            .write_tree(&document.folders)
            .map_err(ConfigStoreError::SessionTree)?;
        let mut catalog = KeyCatalog::new(self.keys_dir());
        catalog.replace_from_vec(document.keys.clone());
        catalog
            .write_all()
            .map_err(ConfigStoreError::KeyCatalog)?;
        Ok(())
    }

    /// Merges `sessions/` and `keys/` into `document`, migrating legacy inline
    /// `folders` / `keys` from `config.toml` when the on-disk trees are empty.
    fn attach_external_stores(
        &self,
        document: &mut ConfigDocument,
        warnings: &mut Vec<ConfigWarning>,
    ) -> Result<(), ConfigStoreError> {
        let session_store = self.session_tree();
        let (disk_folders, session_notices) = session_store.load();
        for notice in session_notices {
            warnings.push(ConfigWarning::StorageNotice { message: notice });
        }

        let legacy_folders = std::mem::take(&mut document.folders);
        let mut migrated = false;
        if disk_folders.is_empty() && !legacy_folders.is_empty() {
            session_store
                .write_tree(&legacy_folders)
                .map_err(ConfigStoreError::SessionTree)?;
            let (rewritten, rewrite_notices) = session_store.load();
            for notice in rewrite_notices {
                warnings.push(ConfigWarning::StorageNotice { message: notice });
            }
            document.folders = rewritten;
            migrated = true;
            warnings.push(ConfigWarning::StorageNotice {
                message: "migrated inline session folders from config.toml into sessions/"
                    .to_owned(),
            });
        } else {
            document.folders = disk_folders;
            if !legacy_folders.is_empty() {
                migrated = true;
                warnings.push(ConfigWarning::StorageNotice {
                    message: "dropped legacy inline session folders from config.toml because sessions/ already exists"
                        .to_owned(),
                });
            }
        }

        let (mut key_catalog, key_notices) = KeyCatalog::load(&self.root);
        for notice in key_notices {
            warnings.push(ConfigWarning::StorageNotice { message: notice });
        }
        let legacy_keys = std::mem::take(&mut document.keys);
        if key_catalog.to_vec().is_empty() && !legacy_keys.is_empty() {
            key_catalog.replace_from_vec(legacy_keys);
            key_catalog
                .write_all()
                .map_err(ConfigStoreError::KeyCatalog)?;
            document.keys = key_catalog.to_vec();
            migrated = true;
            warnings.push(ConfigWarning::StorageNotice {
                message: "migrated inline keys from config.toml into keys/".to_owned(),
            });
        } else {
            document.keys = key_catalog.to_vec();
            if !legacy_keys.is_empty() {
                migrated = true;
                warnings.push(ConfigWarning::StorageNotice {
                    message:
                        "dropped legacy inline keys from config.toml because keys/ already exists"
                            .to_owned(),
                });
            }
        }

        if migrated {
            // Rewrite config.toml without folders/keys.
            self.save(document)?;
        }
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
    ) -> Result<(LoadOutcome, Vec<ConfigWarning>), ConfigStoreError> {
        let backup_path = self.backup_path();
        fs::rename(&path, &backup_path)?;
        let document = ConfigDocument::default();
        self.save(&document)?;
        Ok((
            LoadOutcome {
                document,
                recovered_from_backup: Some(backup_path),
            },
            Vec::new(),
        ))
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
    SessionTree(crate::sessions::SessionTreeError),
    KeyCatalog(crate::keys::KeyCatalogError),
}

impl std::fmt::Display for ConfigStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "configuration I/O error: {error}"),
            Self::Schema(error) => write!(f, "configuration schema error: {error}"),
            Self::KnownHostsSchema(error) => write!(f, "known_hosts schema error: {error}"),
            Self::Serialize(error) => write!(f, "configuration serialization error: {error}"),
            Self::SessionTree(error) => write!(f, "session tree error: {error}"),
            Self::KeyCatalog(error) => write!(f, "key catalog error: {error}"),
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

    #[test]
    fn migrates_legacy_inline_folders_and_keys_to_files() {
        use crate::schema::{FolderProfile, KeyProfile, SessionProfile};

        let temp = tempfile::tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        fs::create_dir_all(temp.path()).expect("mkdir");
        fs::write(
            store.config_file(),
            r#"
schema_version = 2

[[folders]]
id = "folder-1"
name = "Prod"

[[folders.sessions]]
id = "saved-web"
name = "web"
host = "web.example.test"

[[keys]]
id = "key-1"
label = "Work"
algorithm = "ssh-ed25519"
fingerprint = "SHA256:abc"
public_key = "ssh-ed25519 AAAA work"
secret_ref = "local://yshell/keys/key-1/material"
"#,
        )
        .expect("write legacy config");

        let (outcome, warnings) = store
            .load_or_recover_with_warnings()
            .expect("load+migrate");
        assert!(outcome.recovered_from_backup.is_none());
        assert_eq!(outcome.document.folders.len(), 1);
        assert_eq!(outcome.document.folders[0].name, "Prod");
        assert_eq!(outcome.document.folders[0].sessions[0].host, "web.example.test");
        assert_eq!(outcome.document.keys.len(), 1);
        assert_eq!(outcome.document.keys[0].label, "Work");
        assert!(temp.path().join("sessions/Prod/web.toml").exists());
        assert!(temp.path().join("keys/Work.toml").exists());
        assert!(warnings.iter().any(|warning| {
            matches!(warning, ConfigWarning::StorageNotice { message } if message.contains("migrated"))
        }));

        let rewritten = fs::read_to_string(store.config_file()).expect("rewritten");
        assert!(!rewritten.contains("[[folders]]"));
        assert!(!rewritten.contains("[[keys]]"));

        // Second load uses the file trees, not inline TOML.
        let reloaded = store.load_or_recover().expect("reload");
        assert_eq!(reloaded.document.folders[0].sessions[0].id, "saved-web");
        assert_eq!(reloaded.document.keys[0].id, "key-1");

        // Sanity: helpers still construct the expected types.
        let _ = FolderProfile::new("x", "X");
        let _ = SessionProfile::new("y", "y", "h");
        let _ = KeyProfile {
            id: "z".to_owned(),
            label: "Z".to_owned(),
            algorithm: String::new(),
            fingerprint: String::new(),
            public_key: String::new(),
            secret_ref: String::new(),
            comment: None,
        };
    }
}
