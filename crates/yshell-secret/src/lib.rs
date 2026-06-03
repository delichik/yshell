//! Secret-management boundary for YShell.
//!
//! Plaintext secrets are accepted only at the edge of this crate. Stored values
//! are encrypted in memory, resolved through opaque [`SecretRef`] identifiers,
//! and never exposed through `Display`/`Debug` formatting.

use std::{collections::HashMap, error::Error, fmt, sync::Mutex};

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
        Self {
            master_fingerprint: fingerprint(master_password.as_ref().as_bytes()),
            encrypted: Mutex::new(HashMap::new()),
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
}
