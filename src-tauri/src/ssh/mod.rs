//! SSH protocol integration boundary.
//!
//! Authentication, host-key verification, shell channels, SFTP, keepalive, and
//! future jump-host support live behind this module. Stage 2 currently exposes
//! the data contracts and explicit placeholder failure path while the real SSH
//! transport is wired in.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKeyPolicy {
    Strict,
    AcceptNew,
    Prompt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyRecord {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    pub public_key: String,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

pub trait HostKeyStore {
    fn lookup(&self, host: &str, port: u16) -> Result<Option<HostKeyRecord>, String>;
    fn trust(&self, record: HostKeyRecord) -> Result<(), String>;
}
