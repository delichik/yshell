//! Managed private-key metadata as independent TOML files.
//!
//! Layout: `{config_dir}/keys/<label>.toml`. Files hold only non-secret
//! [`KeyProfile`] fields (`secret_ref` points at the encrypted keychain).
//! Private key PEM and passphrases must never be written here.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use crate::{
    names::{names_equal_ignore_case, sanitize_entry_name, validate_entry_name, NameError},
    schema::KeyProfile,
};

/// Directory name under the config root that holds key metadata files.
pub const KEYS_DIR_NAME: &str = "keys";

/// In-memory catalog of key metadata plus helpers for disk persistence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyCatalog {
    keys_dir: PathBuf,
    /// Profiles keyed by stable id.
    by_id: BTreeMap<String, KeyProfile>,
}

/// Errors from loading or saving key metadata files.
#[derive(Debug)]
pub enum KeyCatalogError {
    Io(io::Error),
    Name(NameError),
    Parse { path: PathBuf, message: String },
    Serialize(toml::ser::Error),
    NotFound(String),
    DuplicateLabel(String),
    DuplicateId(String),
}

impl std::fmt::Display for KeyCatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "key catalog I/O error: {error}"),
            Self::Name(error) => write!(f, "key catalog name error: {error}"),
            Self::Parse { path, message } => {
                write!(f, "failed to parse key {}: {message}", path.display())
            }
            Self::Serialize(error) => write!(f, "key serialization error: {error}"),
            Self::NotFound(id) => write!(f, "key `{id}` was not found"),
            Self::DuplicateLabel(label) => {
                write!(f, "key label `{label}` is already in use")
            }
            Self::DuplicateId(id) => write!(f, "key id `{id}` is already in use"),
        }
    }
}

impl std::error::Error for KeyCatalogError {}

impl From<io::Error> for KeyCatalogError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<NameError> for KeyCatalogError {
    fn from(value: NameError) -> Self {
        Self::Name(value)
    }
}

impl From<toml::ser::Error> for KeyCatalogError {
    fn from(value: toml::ser::Error) -> Self {
        Self::Serialize(value)
    }
}

impl KeyCatalog {
    /// Empty catalog rooted at `keys_dir`.
    #[must_use]
    pub fn new(keys_dir: impl Into<PathBuf>) -> Self {
        Self {
            keys_dir: keys_dir.into(),
            by_id: BTreeMap::new(),
        }
    }

    /// Loads every `*.toml` file from `{config_root}/keys/`.
    pub fn load(config_root: &Path) -> (Self, Vec<String>) {
        let keys_dir = config_root.join(KEYS_DIR_NAME);
        let mut catalog = Self::new(keys_dir.clone());
        let mut warnings = Vec::new();
        let Ok(entries) = fs::read_dir(&keys_dir) else {
            return (catalog, warnings);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            match load_key_file(&path) {
                Ok(mut profile) => {
                    let stem = path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or_default();
                    if let Ok(label) = validate_entry_name(stem) {
                        profile.label = label;
                    } else if profile.label.trim().is_empty() {
                        warnings.push(format!(
                            "skipped key file {} with invalid label",
                            path.display()
                        ));
                        continue;
                    }
                    if profile.id.trim().is_empty() {
                        warnings.push(format!(
                            "skipped key file {} with empty id",
                            path.display()
                        ));
                        continue;
                    }
                    if looks_like_private_key_pem(&profile) {
                        warnings.push(format!(
                            "skipped key file {} that appears to contain private key material",
                            path.display()
                        ));
                        continue;
                    }
                    if catalog.by_id.contains_key(&profile.id) {
                        warnings.push(format!(
                            "skipped key file {} with duplicate id '{}'",
                            path.display(),
                            profile.id
                        ));
                        continue;
                    }
                    if catalog.label_in_use(&profile.label, None) {
                        warnings.push(format!(
                            "skipped key file {} with duplicate label '{}'",
                            path.display(),
                            profile.label
                        ));
                        continue;
                    }
                    catalog.by_id.insert(profile.id.clone(), profile);
                }
                Err(message) => warnings.push(message),
            }
        }
        (catalog, warnings)
    }

    /// Directory that holds key metadata TOML files.
    #[must_use]
    pub fn keys_dir(&self) -> &Path {
        &self.keys_dir
    }

    /// All key profiles, sorted by id.
    pub fn keys(&self) -> impl Iterator<Item = &KeyProfile> {
        self.by_id.values()
    }

    /// Snapshot as a `Vec` (document-compatible order by id).
    #[must_use]
    pub fn to_vec(&self) -> Vec<KeyProfile> {
        self.by_id.values().cloned().collect()
    }

    /// Replaces the in-memory catalog from a vec (does not touch disk).
    pub fn replace_from_vec(&mut self, keys: Vec<KeyProfile>) {
        self.by_id = keys
            .into_iter()
            .map(|key| (key.id.clone(), key))
            .collect();
    }

    /// Looks up a key by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&KeyProfile> {
        self.by_id.get(id)
    }

    /// Whether a label is already used by another key (case-insensitive).
    #[must_use]
    pub fn label_in_use(&self, label: &str, except_id: Option<&str>) -> bool {
        self.by_id.values().any(|key| {
            except_id.is_none_or(|id| key.id != id)
                && names_equal_ignore_case(&key.label, label)
        })
    }

    /// Writes every in-memory key to disk (legacy migration).
    pub fn write_all(&mut self) -> Result<(), KeyCatalogError> {
        if self.keys_dir.exists() {
            for entry in fs::read_dir(&self.keys_dir)?.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        fs::create_dir_all(&self.keys_dir)?;
        let profiles: Vec<KeyProfile> = self
            .by_id
            .values()
            .map(|profile| {
                let mut profile = profile.clone();
                profile.label = sanitize_entry_name(&profile.label, &profile.id);
                profile
            })
            .collect();
        self.by_id = profiles
            .iter()
            .map(|profile| (profile.id.clone(), profile.clone()))
            .collect();
        for profile in self.by_id.values() {
            self.write_profile_file(profile, None)?;
        }
        Ok(())
    }

    /// Inserts or updates a key profile and writes its metadata file.
    pub fn save_key(&mut self, mut profile: KeyProfile) -> Result<(), KeyCatalogError> {
        let label = validate_entry_name(&profile.label)?;
        profile.label = label.clone();
        if profile.id.trim().is_empty() {
            return Err(KeyCatalogError::NotFound("(empty id)".to_owned()));
        }
        if self.label_in_use(&label, Some(&profile.id)) {
            return Err(KeyCatalogError::DuplicateLabel(label));
        }
        if let Some(existing) = self.by_id.get(&profile.id) {
            let previous_label = existing.label.clone();
            self.write_profile_file(&profile, Some(&previous_label))?;
        } else {
            if self.by_id.values().any(|key| key.id == profile.id) {
                return Err(KeyCatalogError::DuplicateId(profile.id));
            }
            self.write_profile_file(&profile, None)?;
        }
        self.by_id.insert(profile.id.clone(), profile);
        Ok(())
    }

    /// Renames a key's label (and metadata file). Id stays stable.
    pub fn rename_key(&mut self, id: &str, new_label: &str) -> Result<(), KeyCatalogError> {
        let label = validate_entry_name(new_label)?;
        if self.label_in_use(&label, Some(id)) {
            return Err(KeyCatalogError::DuplicateLabel(label));
        }
        let Some(mut profile) = self.by_id.get(id).cloned() else {
            return Err(KeyCatalogError::NotFound(id.to_owned()));
        };
        let previous = profile.label.clone();
        profile.label = label;
        self.write_profile_file(&profile, Some(&previous))?;
        self.by_id.insert(id.to_owned(), profile);
        Ok(())
    }

    /// Deletes a key metadata file and removes it from the catalog.
    pub fn delete_key(&mut self, id: &str) -> Result<KeyProfile, KeyCatalogError> {
        let Some(profile) = self.by_id.remove(id) else {
            return Err(KeyCatalogError::NotFound(id.to_owned()));
        };
        let path = self.key_path(&profile.label);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(profile)
    }

    fn write_profile_file(
        &self,
        profile: &KeyProfile,
        previous_label: Option<&str>,
    ) -> Result<(), KeyCatalogError> {
        if looks_like_private_key_pem(profile) {
            return Err(KeyCatalogError::Parse {
                path: self.key_path(&profile.label),
                message: "refusing to write private key material into key metadata".to_owned(),
            });
        }
        fs::create_dir_all(&self.keys_dir)?;
        if let Some(previous) = previous_label {
            if !names_equal_ignore_case(previous, &profile.label) {
                let old = self.key_path(previous);
                if old.exists() {
                    let _ = fs::remove_file(&old);
                }
            }
        }
        // Collision with an on-disk file owned by a different id.
        let path = self.key_path(&profile.label);
        if path.exists() {
            if let Ok(existing) = load_key_file(&path) {
                if existing.id != profile.id {
                    return Err(KeyCatalogError::DuplicateLabel(profile.label.clone()));
                }
            }
        }
        let tmp = path.with_extension("toml.tmp");
        let body = toml::to_string_pretty(profile)?;
        // Belt-and-suspenders: never persist PEM-looking blobs in any string field.
        if body.contains("BEGIN OPENSSH PRIVATE KEY")
            || body.contains("BEGIN RSA PRIVATE KEY")
            || body.contains("BEGIN PRIVATE KEY")
        {
            return Err(KeyCatalogError::Parse {
                path,
                message: "refusing to write private key material into key metadata".to_owned(),
            });
        }
        fs::write(&tmp, body)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn key_path(&self, label: &str) -> PathBuf {
        self.keys_dir.join(format!("{label}.toml"))
    }
}

fn load_key_file(path: &Path) -> Result<KeyProfile, String> {
    let input = fs::read_to_string(path).map_err(|error| {
        format!("failed to read key {}: {error}", path.display())
    })?;
    if input.contains("BEGIN OPENSSH PRIVATE KEY")
        || input.contains("BEGIN RSA PRIVATE KEY")
        || input.contains("BEGIN PRIVATE KEY")
    {
        return Err(format!(
            "key file {} appears to contain private key material",
            path.display()
        ));
    }
    toml::from_str(&input).map_err(|error| {
        format!("failed to parse key {}: {error}", path.display())
    })
}

fn looks_like_private_key_pem(profile: &KeyProfile) -> bool {
    let fields = [
        profile.public_key.as_str(),
        profile.secret_ref.as_str(),
        profile.comment.as_deref().unwrap_or(""),
        profile.fingerprint.as_str(),
        profile.label.as_str(),
    ];
    fields.iter().any(|value| {
        value.contains("BEGIN OPENSSH PRIVATE KEY")
            || value.contains("BEGIN RSA PRIVATE KEY")
            || value.contains("BEGIN PRIVATE KEY")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample(id: &str, label: &str) -> KeyProfile {
        KeyProfile {
            id: id.to_owned(),
            label: label.to_owned(),
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "SHA256:abc".to_owned(),
            public_key: "ssh-ed25519 AAAA work".to_owned(),
            secret_ref: "local://yshell/keys/key-1/material".to_owned(),
            comment: None,
        }
    }

    #[test]
    fn save_load_round_trip() {
        let temp = TempDir::new().expect("tempdir");
        let mut catalog = KeyCatalog::new(temp.path().join("keys"));
        catalog.save_key(sample("key-1", "Work")).expect("save");
        assert!(temp.path().join("keys/Work.toml").exists());
        let (loaded, warnings) = KeyCatalog::load(temp.path());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(loaded.get("key-1").map(|k| k.label.as_str()), Some("Work"));
        let body = fs::read_to_string(temp.path().join("keys/Work.toml")).expect("read");
        assert!(!body.contains("BEGIN"));
        assert!(body.contains("secret_ref"));
    }

    #[test]
    fn rejects_duplicate_label() {
        let temp = TempDir::new().expect("tempdir");
        let mut catalog = KeyCatalog::new(temp.path().join("keys"));
        catalog.save_key(sample("key-1", "Work")).expect("save");
        let err = catalog
            .save_key(sample("key-2", "work"))
            .expect_err("dup");
        assert!(matches!(err, KeyCatalogError::DuplicateLabel(_)));
    }

    #[test]
    fn rename_rewrites_filename() {
        let temp = TempDir::new().expect("tempdir");
        let mut catalog = KeyCatalog::new(temp.path().join("keys"));
        catalog.save_key(sample("key-1", "Work")).expect("save");
        catalog.rename_key("key-1", "Ops").expect("rename");
        assert!(!temp.path().join("keys/Work.toml").exists());
        assert!(temp.path().join("keys/Ops.toml").exists());
    }

    #[test]
    fn refuses_pem_in_metadata_fields() {
        let temp = TempDir::new().expect("tempdir");
        let mut catalog = KeyCatalog::new(temp.path().join("keys"));
        let mut bad = sample("key-1", "Work");
        bad.comment = Some("-----BEGIN OPENSSH PRIVATE KEY-----\naaaa\n-----END OPENSSH PRIVATE KEY-----".to_owned());
        let err = catalog.save_key(bad).expect_err("pem");
        assert!(matches!(err, KeyCatalogError::Parse { .. }));
    }

}
