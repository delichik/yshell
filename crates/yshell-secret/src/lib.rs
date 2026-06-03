//! Secret-management boundary for YShell.
//!
//! Milestone 0 intentionally exposes only opaque secret references so UI and
//! configuration code can be wired without carrying plaintext credentials.

use std::fmt;

/// Opaque identifier for a credential stored outside normal configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

impl fmt::Display for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::SecretRef;

    #[test]
    fn secret_ref_is_opaque_identifier() {
        let secret_ref = SecretRef::new("keychain://yshell/session/demo");
        assert_eq!(secret_ref.as_str(), "keychain://yshell/session/demo");
        assert_eq!(secret_ref.to_string(), "keychain://yshell/session/demo");
    }
}
