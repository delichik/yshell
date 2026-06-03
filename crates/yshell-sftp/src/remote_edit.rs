//! Remote file temporary-edit placeholder.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEditSession {
    pub remote_path: String,
    pub local_temp_path: String,
}
