//! System credential storage boundary.
//!
//! Passwords, private-key passphrases, and proxy secrets must pass through this
//! module instead of being written to normal session config files.

pub trait CredentialStore {
    fn set_secret(&self, key: &str, secret: &str) -> Result<String, String>;
    fn get_secret(&self, credential_ref: &str) -> Result<String, String>;
    fn delete_secret(&self, credential_ref: &str) -> Result<(), String>;
}
