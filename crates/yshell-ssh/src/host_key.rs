//! Host-key verification placeholders.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyFingerprint {
    pub algorithm: String,
    pub fingerprint: String,
}
