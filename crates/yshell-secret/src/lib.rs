//! Secret-management boundary for YShell.
//!
//! Plaintext secrets are accepted only at the edge of this crate. Stored values
//! are encrypted in memory, resolved through opaque [`SecretRef`] identifiers,
//! and never exposed through `Display`/`Debug` formatting.

use std::{
    collections::HashMap,
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use keyring::Entry;
use serde::{Deserialize, Serialize};

/// Opaque identifier for a credential stored outside normal configuration.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SecretRef(String);

impl SecretRef {
    /// Creates a new opaque secret reference.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the non-secret reference identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("SecretRef").field(&self.0).finish()
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Plaintext secret wrapper that redacts formatting output.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(REDACTED)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Errors returned by secret stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    NotFound(SecretRef),
    Locked,
    InvalidMasterPassword,
    Backend(String),
}

impl fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(secret_ref) => write!(formatter, "secret not found: {secret_ref}"),
            Self::Locked => formatter.write_str("secret store is locked"),
            Self::InvalidMasterPassword => formatter.write_str("invalid master password"),
            Self::Backend(message) => write!(formatter, "secret backend error: {message}"),
        }
    }
}

impl Error for SecretError {}

impl From<io::Error> for SecretError {
    fn from(value: io::Error) -> Self {
        Self::Backend(format!("secret I/O error: {value}"))
    }
}

/// Boundary implemented by OS keychains and test doubles.
pub trait Keychain: Send + Sync {
    fn put(&self, secret_ref: SecretRef, secret: SecretString) -> Result<(), SecretError>;
    fn get(&self, secret_ref: &SecretRef) -> Result<SecretString, SecretError>;
    fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretError>;
}

/// In-memory encrypted store keyed by a master password.
#[derive(Debug)]
pub struct InMemoryEncryptedStore {
    master_fingerprint: u64,
    encrypted: Mutex<HashMap<SecretRef, Vec<u8>>>,
}

impl InMemoryEncryptedStore {
    #[must_use]
    pub fn new(master_password: impl AsRef<str>) -> Self {
        Self::from_encrypted(master_password, HashMap::new())
    }

    #[must_use]
    pub fn from_encrypted(
        master_password: impl AsRef<str>,
        encrypted: HashMap<SecretRef, Vec<u8>>,
    ) -> Self {
        Self {
            master_fingerprint: fingerprint(master_password.as_ref().as_bytes()),
            encrypted: Mutex::new(encrypted),
        }
    }

    pub fn put_with_master(
        &self,
        master_password: impl AsRef<str>,
        secret_ref: SecretRef,
        secret: impl Into<SecretString>,
    ) -> Result<(), SecretError> {
        self.ensure_master(master_password.as_ref())?;
        let ciphertext = xor_keystream(
            secret.into().expose_secret().as_bytes(),
            master_password.as_ref().as_bytes(),
            secret_ref.as_str().as_bytes(),
        );
        self.encrypted
            .lock()
            .map_err(|_| SecretError::Backend("secret mutex poisoned".to_owned()))?
            .insert(secret_ref, ciphertext);
        Ok(())
    }

    pub fn get_with_master(
        &self,
        master_password: impl AsRef<str>,
        secret_ref: &SecretRef,
    ) -> Result<SecretString, SecretError> {
        self.ensure_master(master_password.as_ref())?;
        let ciphertext = self
            .encrypted
            .lock()
            .map_err(|_| SecretError::Backend("secret mutex poisoned".to_owned()))?
            .get(secret_ref)
            .cloned()
            .ok_or_else(|| SecretError::NotFound(secret_ref.clone()))?;
        let plaintext = xor_keystream(
            &ciphertext,
            master_password.as_ref().as_bytes(),
            secret_ref.as_str().as_bytes(),
        );
        String::from_utf8(plaintext)
            .map(SecretString)
            .map_err(|_| SecretError::Backend("decrypted secret was not utf-8".to_owned()))
    }

    pub fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretError> {
        self.encrypted
            .lock()
            .map_err(|_| SecretError::Backend("secret mutex poisoned".to_owned()))?
            .remove(secret_ref);
        Ok(())
    }

    #[must_use]
    pub fn encrypted_snapshot(&self, secret_ref: &SecretRef) -> Option<Vec<u8>> {
        self.encrypted.lock().ok()?.get(secret_ref).cloned()
    }

    #[must_use]
    pub fn encrypted_entries_snapshot(&self) -> HashMap<SecretRef, Vec<u8>> {
        self.encrypted.lock().map(|entries| entries.clone()).unwrap_or_default()
    }

    fn ensure_master(&self, master_password: &str) -> Result<(), SecretError> {
        if fingerprint(master_password.as_bytes()) == self.master_fingerprint {
            Ok(())
        } else {
            Err(SecretError::InvalidMasterPassword)
        }
    }
}

/// Keychain fake backed by [`InMemoryEncryptedStore`].
#[derive(Debug)]
pub struct FakeKeychain {
    master_password: SecretString,
    store: InMemoryEncryptedStore,
}

impl FakeKeychain {
    #[must_use]
    pub fn new(master_password: impl Into<SecretString>) -> Self {
        let master_password = master_password.into();
        Self {
            store: InMemoryEncryptedStore::new(master_password.expose_secret()),
            master_password,
        }
    }

    #[must_use]
    pub const fn store(&self) -> &InMemoryEncryptedStore {
        &self.store
    }
}

impl Keychain for FakeKeychain {
    fn put(&self, secret_ref: SecretRef, secret: SecretString) -> Result<(), SecretError> {
        self.store
            .put_with_master(self.master_password.expose_secret(), secret_ref, secret)
    }

    fn get(&self, secret_ref: &SecretRef) -> Result<SecretString, SecretError> {
        self.store
            .get_with_master(self.master_password.expose_secret(), secret_ref)
    }

    fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretError> {
        self.store.delete(secret_ref)
    }
}

pub struct OsKeychain {
    service_name: String,
    entries: Mutex<HashMap<SecretRef, Entry>>,
}

impl OsKeychain {
    #[must_use]
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn with_entry<T>(
        &self,
        secret_ref: &SecretRef,
        op: impl FnOnce(&Entry) -> Result<T, SecretError>,
    ) -> Result<T, SecretError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| SecretError::Backend("os keychain mutex poisoned".to_owned()))?;
        let entry = if let Some(entry) = entries.get(secret_ref) {
            entry
        } else {
            let entry = Entry::new(&self.service_name, secret_ref.as_str())
                .map_err(|error| map_keyring_error("build entry", error))?;
            entries.entry(secret_ref.clone()).or_insert(entry)
        };
        op(entry)
    }
}

impl fmt::Debug for OsKeychain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OsKeychain")
            .field("service_name", &self.service_name)
            .finish_non_exhaustive()
    }
}

impl Keychain for OsKeychain {
    fn put(&self, secret_ref: SecretRef, secret: SecretString) -> Result<(), SecretError> {
        self.with_entry(&secret_ref, |entry| {
            entry
                .set_password(secret.expose_secret())
                .map_err(|error| map_keyring_error("store secret", error))
        })
    }

    fn get(&self, secret_ref: &SecretRef) -> Result<SecretString, SecretError> {
        self.with_entry(secret_ref, |entry| {
            entry
                .get_password()
                .map(SecretString::from)
                .map_err(|error| map_keyring_error("load secret", error))
        })
    }

    fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretError> {
        self.with_entry(secret_ref, |entry| {
            entry
                .delete_credential()
                .map_err(|error| map_keyring_error("delete secret", error))
        })
    }
}

#[derive(Debug)]
pub struct FileKeychain {
    path: PathBuf,
    master_password: SecretString,
    store: Mutex<InMemoryEncryptedStore>,
}

impl FileKeychain {
    pub fn open_or_create(
        path: impl Into<PathBuf>,
        master_password: impl Into<SecretString>,
    ) -> Result<Self, SecretError> {
        let path = path.into();
        let master_password = master_password.into();
        let document = load_secret_document(&path)?;
        let encrypted = document
            .entries
            .into_iter()
            .map(|entry| (SecretRef::new(entry.secret_ref), entry.ciphertext))
            .collect();
        Ok(Self {
            path,
            store: Mutex::new(InMemoryEncryptedStore::from_encrypted(
                master_password.expose_secret(),
                encrypted,
            )),
            master_password,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn persist(&self) -> Result<(), SecretError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let entries = self
            .store
            .lock()
            .map_err(|_| SecretError::Backend("secret mutex poisoned".to_owned()))?
            .encrypted_entries_snapshot()
            .into_iter()
            .map(|(secret_ref, ciphertext)| SecretDocumentEntry {
                secret_ref: secret_ref.as_str().to_owned(),
                ciphertext,
            })
            .collect();
        let document = SecretDocument { entries };
        let serialized = toml::to_string_pretty(&document)
            .map_err(|error| SecretError::Backend(format!("serialize secret store: {error}")))?;
        fs::write(&self.path, serialized)?;
        Ok(())
    }
}

impl Keychain for FileKeychain {
    fn put(&self, secret_ref: SecretRef, secret: SecretString) -> Result<(), SecretError> {
        self.store
            .lock()
            .map_err(|_| SecretError::Backend("secret mutex poisoned".to_owned()))?
            .put_with_master(self.master_password.expose_secret(), secret_ref, secret)?;
        self.persist()
    }

    fn get(&self, secret_ref: &SecretRef) -> Result<SecretString, SecretError> {
        self.store
            .lock()
            .map_err(|_| SecretError::Backend("secret mutex poisoned".to_owned()))?
            .get_with_master(self.master_password.expose_secret(), secret_ref)
    }

    fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretError> {
        self.store
            .lock()
            .map_err(|_| SecretError::Backend("secret mutex poisoned".to_owned()))?
            .delete(secret_ref)?;
        self.persist()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct SecretDocument {
    #[serde(default)]
    entries: Vec<SecretDocumentEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SecretDocumentEntry {
    secret_ref: String,
    #[serde(default)]
    ciphertext: Vec<u8>,
}

fn load_secret_document(path: &Path) -> Result<SecretDocument, SecretError> {
    if !path.exists() {
        return Ok(SecretDocument::default());
    }
    let input = fs::read_to_string(path)?;
    toml::from_str(&input)
        .map_err(|error| SecretError::Backend(format!("parse secret store: {error}")))
}

fn map_keyring_error(action: &str, error: keyring::Error) -> SecretError {
    match error {
        keyring::Error::NoEntry => {
            SecretError::NotFound(SecretRef::new(format!("keyring://{action}")))
        }
        other => SecretError::Backend(format!("{action}: {other}")),
    }
}

fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn xor_keystream(input: &[u8], master: &[u8], context: &[u8]) -> Vec<u8> {
    let seed = fingerprint(master) ^ fingerprint(context).rotate_left(17);
    input
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            let stream = seed
                .rotate_left((index % 63) as u32)
                .wrapping_add((index as u64).wrapping_mul(0x9e3779b97f4a7c15));
            byte ^ stream.to_le_bytes()[index % 8]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex as StdMutex, OnceLock};

    fn keyring_test_lock() -> &'static StdMutex<()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
    }

    #[test]
    fn secret_ref_is_opaque_identifier() {
        let secret_ref = SecretRef::new("keychain://yshell/session/demo");
        assert_eq!(secret_ref.as_str(), "keychain://yshell/session/demo");
        assert_eq!(secret_ref.to_string(), "keychain://yshell/session/demo");
    }

    #[test]
    fn secret_string_does_not_leak_display_or_debug() {
        let secret = SecretString::from("correct horse battery staple");

        assert_eq!(secret.to_string(), "[REDACTED]");
        assert!(!format!("{secret:?}").contains("correct horse"));
    }

    #[test]
    fn encrypted_store_round_trips_without_plaintext_snapshot() {
        let store = InMemoryEncryptedStore::new("master");
        let secret_ref = SecretRef::new("mem://demo");

        store
            .put_with_master("master", secret_ref.clone(), "hunter2")
            .expect("secret stored");

        let snapshot = store.encrypted_snapshot(&secret_ref).expect("ciphertext");
        assert_ne!(snapshot, b"hunter2");
        assert_eq!(
            store
                .get_with_master("master", &secret_ref)
                .expect("secret loaded")
                .expose_secret(),
            "hunter2"
        );
        assert_eq!(
            store.get_with_master("wrong", &secret_ref).unwrap_err(),
            SecretError::InvalidMasterPassword
        );
    }

    #[test]
    fn fake_keychain_implements_boundary() {
        let keychain = FakeKeychain::new("master");
        let secret_ref = SecretRef::new("fake://session/password");

        keychain
            .put(secret_ref.clone(), SecretString::from("top-secret"))
            .expect("put");

        assert_eq!(
            keychain.get(&secret_ref).expect("get").expose_secret(),
            "top-secret"
        );
        keychain.delete(&secret_ref).expect("delete");
        assert!(matches!(
            keychain.get(&secret_ref),
            Err(SecretError::NotFound(_))
        ));
    }

    #[test]
    fn file_keychain_persists_ciphertext_without_plaintext() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("secret-store.toml");
        let keychain = FileKeychain::open_or_create(&path, "master").expect("keychain");
        let secret_ref = SecretRef::new("file://session/password");

        keychain
            .put(secret_ref.clone(), SecretString::from("top-secret"))
            .expect("put");

        let on_disk = fs::read_to_string(&path).expect("read store");
        assert!(on_disk.contains(secret_ref.as_str()));
        assert!(!on_disk.contains("top-secret"));

        let reopened = FileKeychain::open_or_create(&path, "master").expect("reopen");
        assert_eq!(
            reopened.get(&secret_ref).expect("get").expose_secret(),
            "top-secret"
        );
    }

    #[test]
    fn os_keychain_implements_real_keyring_boundary_via_mock_store() {
        let _guard = keyring_test_lock().lock().expect("test lock");
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());

        let keychain = OsKeychain::new("yshell-test");
        let secret_ref = SecretRef::new("keychain://yshell/session/demo");
        keychain
            .put(secret_ref.clone(), SecretString::from("platform-secret"))
            .expect("put");

        assert_eq!(
            keychain.get(&secret_ref).expect("get").expose_secret(),
            "platform-secret"
        );
        keychain.delete(&secret_ref).expect("delete");
        assert!(matches!(
            keychain.get(&secret_ref),
            Err(SecretError::NotFound(_))
        ));
    }
}
