//! Host-key verification and known-hosts management.

use std::collections::BTreeMap;

use crate::error::{SshError, SshErrorKind, SshResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyFingerprint {
    pub algorithm: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyPolicy {
    Strict,
    TrustOnFirstUse,
    AcceptAnyForTesting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyDecision {
    Trusted,
    PinNewKey,
    Rejected { expected: HostKeyFingerprint },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownHosts {
    entries: BTreeMap<String, HostKeyFingerprint>,
}

impl KnownHosts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, host: &str, port: u16) -> Option<&HostKeyFingerprint> {
        self.entries.get(&known_host_key(host, port))
    }

    pub fn pin(&mut self, host: &str, port: u16, fingerprint: HostKeyFingerprint) {
        self.entries.insert(known_host_key(host, port), fingerprint);
    }

    pub fn remove(&mut self, host: &str, port: u16) -> Option<HostKeyFingerprint> {
        self.entries.remove(&known_host_key(host, port))
    }

    pub fn verify(
        &mut self,
        host: &str,
        port: u16,
        presented: HostKeyFingerprint,
        policy: &HostKeyPolicy,
    ) -> SshResult<HostKeyDecision> {
        if matches!(policy, HostKeyPolicy::AcceptAnyForTesting) {
            return Ok(HostKeyDecision::Trusted);
        }

        match self.get(host, port) {
            Some(expected) if expected == &presented => Ok(HostKeyDecision::Trusted),
            Some(expected) => Ok(HostKeyDecision::Rejected {
                expected: expected.clone(),
            }),
            None if matches!(policy, HostKeyPolicy::TrustOnFirstUse) => {
                self.pin(host, port, presented);
                Ok(HostKeyDecision::PinNewKey)
            }
            None => Err(SshError::new(
                SshErrorKind::HostKeyRejected,
                format!("no known host key for {host}:{port}"),
            )),
        }
    }
}

fn known_host_key(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}
