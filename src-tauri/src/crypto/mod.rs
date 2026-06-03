//! System credential storage boundary.
//!
//! Passwords, private-key passphrases, and proxy secrets must pass through this
//! module instead of being written to normal session config files.

pub trait CredentialStore {
    fn set_secret(&self, key: &str, secret: &str) -> Result<String, String>;
    fn get_secret(&self, credential_ref: &str) -> Result<String, String>;
    fn delete_secret(&self, credential_ref: &str) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct UnavailableCredentialStore;

impl CredentialStore for UnavailableCredentialStore {
    fn set_secret(&self, _key: &str, _secret: &str) -> Result<String, String> {
        Err("secure credential storage is not available in this build".to_string())
    }

    fn get_secret(&self, _credential_ref: &str) -> Result<String, String> {
        Err("secure credential storage is not available in this build".to_string())
    }

    fn delete_secret(&self, _credential_ref: &str) -> Result<(), String> {
        Err("secure credential storage is not available in this build".to_string())
    }
}
