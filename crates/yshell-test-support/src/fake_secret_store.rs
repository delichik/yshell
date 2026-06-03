//! In-memory fake secret store for tests.

use std::collections::HashMap;

/// Simple in-memory secret store keyed by test-controlled identifiers.
#[derive(Debug, Default, Clone)]
pub struct FakeSecretStore {
    secrets: HashMap<String, Vec<u8>>,
}

impl FakeSecretStore {
    /// Creates an empty fake store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or replaces a secret.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.secrets.insert(key.into(), value.into());
    }

    /// Returns a secret by key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.secrets.get(key).map(Vec::as_slice)
    }
}
