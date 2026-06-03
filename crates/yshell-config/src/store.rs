//! TOML configuration persistence with damaged-file backup recovery.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::schema::{ConfigDocument, ConfigSchemaError};

const CONFIG_FILE_NAME: &str = "config.toml";

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

    /// Loads the document, creating a default file when missing.
    ///
    /// If the existing TOML is unreadable or invalid, it is renamed to a
    /// timestamped `.bak` file and replaced with a default document.
    pub fn load_or_recover(&self) -> Result<LoadOutcome, ConfigStoreError> {
        fs::create_dir_all(&self.root)?;
        let path = self.config_file();
        if !path.exists() {
            let document = ConfigDocument::default();
            self.save(&document)?;
            return Ok(LoadOutcome {
                document,
                recovered_from_backup: None,
            });
        }

        let input = fs::read_to_string(&path);
        match input {
            Ok(input) => match ConfigDocument::from_toml_str(&input) {
                Ok(document) => Ok(LoadOutcome {
                    document,
                    recovered_from_backup: None,
                }),
                Err(error) => self.backup_and_reset(path, ConfigStoreError::Schema(error)),
            },
            Err(error) => self.backup_and_reset(path, ConfigStoreError::Io(error)),
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
    Serialize(toml::ser::Error),
}

impl std::fmt::Display for ConfigStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "configuration I/O error: {error}"),
            Self::Schema(error) => write!(f, "configuration schema error: {error}"),
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
}
