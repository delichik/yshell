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
    /// 兼容保留：产品决定（2026-09-27）后不再有静默 pin 新密钥的路径，
    /// `TrustOnFirstUse` 首次连接也必须由用户确认（见 `KnownHosts::verify`）。
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

    pub fn replace(&mut self, host: &str, port: u16, fingerprint: HostKeyFingerprint) {
        self.pin(host, port, fingerprint);
    }

    pub fn remove(&mut self, host: &str, port: u16) -> Option<HostKeyFingerprint> {
        self.entries.remove(&known_host_key(host, port))
    }

    pub fn snapshot(&self) -> BTreeMap<String, HostKeyFingerprint> {
        self.entries.clone()
    }

    pub fn from_snapshot(entries: BTreeMap<String, HostKeyFingerprint>) -> Self {
        Self { entries }
    }

    pub fn merge(&mut self, other: &KnownHosts) {
        self.entries.extend(other.entries.clone());
    }

    /// 校验呈现的主机密钥。
    ///
    /// 产品决定（2026-09-27）：`TrustOnFirstUse` **不再**在首次连接时静默信任；
    /// 未知主机与 `Strict` 一样返回 `HostKeyRejected`，由 App 层弹信任弹窗
    /// （Trust Once = 仅本次内存信任；Trust and Save = 写入 known_hosts）。
    /// 唯一静默信任的策略是显式的测试策略 `AcceptAnyForTesting`。
    ///
    /// 保留 `&mut self` 签名以免破坏调用方；新语义下校验不再改动 known_hosts。
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(algorithm: &str, fingerprint: &str) -> HostKeyFingerprint {
        HostKeyFingerprint {
            algorithm: algorithm.to_owned(),
            fingerprint: fingerprint.to_owned(),
        }
    }

    #[test]
    fn tofu_unknown_host_is_rejected_and_not_pinned() {
        // 产品决定（2026-09-27）：首次信任必须由用户确认，TOFU 不再静默 pin。
        let mut known_hosts = KnownHosts::new();
        let presented = fingerprint("ssh-ed25519", "aa:bb");
        let error = known_hosts
            .verify(
                "first.example.test",
                22,
                presented.clone(),
                &HostKeyPolicy::TrustOnFirstUse,
            )
            .expect_err("TOFU must ask the user for an unknown host");
        assert_eq!(error.kind, SshErrorKind::HostKeyRejected);
        assert!(error.message.contains("no known host key"));
        assert_eq!(
            known_hosts.get("first.example.test", 22),
            None,
            "verification must not silently pin the presented key"
        );

        // 用户确认后由 App 层显式写入（Trust and Save / Trust Once 的落盘/内存语义）。
        known_hosts.pin("first.example.test", 22, presented.clone());
        assert_eq!(
            known_hosts
                .verify(
                    "first.example.test",
                    22,
                    presented.clone(),
                    &HostKeyPolicy::TrustOnFirstUse
                )
                .expect("pinned key verifies"),
            HostKeyDecision::Trusted
        );
    }

    #[test]
    fn strict_unknown_host_is_rejected_and_changed_key_reports_expected() {
        let mut known_hosts = KnownHosts::new();
        let error = known_hosts
            .verify(
                "strict.example.test",
                22,
                fingerprint("ssh-ed25519", "aa:bb"),
                &HostKeyPolicy::Strict,
            )
            .expect_err("strict must reject unknown hosts");
        assert_eq!(error.kind, SshErrorKind::HostKeyRejected);

        let expected = fingerprint("ssh-ed25519", "aa:bb");
        known_hosts.pin("strict.example.test", 22, expected.clone());
        assert_eq!(
            known_hosts
                .verify(
                    "strict.example.test",
                    22,
                    fingerprint("ssh-ed25519", "cc:dd"),
                    &HostKeyPolicy::Strict
                )
                .expect("changed key is a decision, not an error"),
            HostKeyDecision::Rejected { expected }
        );
    }

    #[test]
    fn accept_any_for_testing_trusts_unknown_hosts_without_pinning() {
        let mut known_hosts = KnownHosts::new();
        assert_eq!(
            known_hosts
                .verify(
                    "test.example.test",
                    22,
                    fingerprint("ssh-ed25519", "aa:bb"),
                    &HostKeyPolicy::AcceptAnyForTesting
                )
                .expect("accept-any is the explicit testing policy"),
            HostKeyDecision::Trusted
        );
        assert_eq!(known_hosts.get("test.example.test", 22), None);
    }
}
