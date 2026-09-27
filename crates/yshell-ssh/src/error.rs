//! SSH error types.

use std::fmt;

use crate::auth::{AuthMethods, AuthProblemKind};
use crate::host_key::HostKeyFingerprint;

pub type SshResult<T> = Result<T, SshError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshError {
    pub kind: SshErrorKind,
    pub message: String,
    pub host_key_problem: Option<Box<HostKeyProblem>>,
    /// Server advertised authentication methods observed when this error was
    /// produced (`None` = not queried yet or the server did not return a list).
    ///
    /// Populated on authentication errors so the runtime can open / update the
    /// auth window without a second connection attempt.
    pub auth_methods: Option<AuthMethods>,
    /// Fine grained authentication classification (only meaningful when
    /// `kind == SshErrorKind::Authentication`).
    pub auth_problem: Option<AuthProblemKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyProblem {
    Unknown {
        host: String,
        port: u16,
        presented: HostKeyFingerprint,
    },
    Changed {
        host: String,
        port: u16,
        presented: HostKeyFingerprint,
        expected: HostKeyFingerprint,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshErrorKind {
    Configuration,
    Dns,
    TcpConnect,
    Proxy,
    Handshake,
    Authentication,
    HostKeyRejected,
    Channel,
    Timeout,
    Io,
    Unsupported,
}

impl SshError {
    pub fn new(kind: SshErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            host_key_problem: None,
            auth_methods: None,
            auth_problem: None,
        }
    }

    pub fn configuration(message: impl Into<String>) -> Self {
        Self::new(SshErrorKind::Configuration, message)
    }

    pub fn with_host_key_problem(mut self, problem: HostKeyProblem) -> Self {
        self.host_key_problem = Some(Box::new(problem));
        self
    }

    /// Attach the server advertised auth methods observed for this error.
    pub fn with_auth_methods(mut self, methods: Option<AuthMethods>) -> Self {
        self.auth_methods = methods;
        self
    }

    /// Attach a fine grained authentication classification.
    pub fn with_auth_problem(mut self, problem: AuthProblemKind) -> Self {
        self.auth_problem = Some(problem);
        self
    }

    /// Attach both the observed auth methods and the classification.
    pub fn with_auth_context(
        mut self,
        methods: Option<AuthMethods>,
        problem: AuthProblemKind,
    ) -> Self {
        self.auth_methods = methods;
        self.auth_problem = Some(problem);
        self
    }
}

impl fmt::Display for SshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)?;
        if let Some(problem) = self.auth_problem {
            write!(formatter, " (auth-problem: {})", problem.label())?;
        }
        if let Some(methods) = self.auth_methods {
            write!(formatter, " (server-auth-methods: {})", methods.describe())?;
        }
        Ok(())
    }
}

impl std::error::Error for SshError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_context_is_optional_and_composable() {
        let error = SshError::new(SshErrorKind::Authentication, "boom");
        assert_eq!(error.auth_methods, None);
        assert_eq!(error.auth_problem, None);

        let methods = AuthMethods::parse("publickey").expect("list");
        let error = SshError::configuration("bad config")
            .with_auth_context(Some(methods), AuthProblemKind::MethodNotAllowed);
        assert_eq!(error.auth_methods, Some(methods));
        assert_eq!(error.auth_problem, Some(AuthProblemKind::MethodNotAllowed));
        let display = error.to_string();
        assert!(display.contains("method-not-allowed"));
        assert!(display.contains("server-auth-methods: publickey,agent"));
    }
}
