//! SSH authentication configuration, negotiation and prompting types.

use std::collections::VecDeque;

/// Supported SSH authentication mechanisms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    Password {
        username: String,
        password: String,
    },
    PrivateKey {
        username: String,
        key_path: String,
        passphrase: Option<String>,
    },
    Agent {
        username: String,
    },
    KeyboardInteractive {
        username: String,
        secret: String,
    },
}

impl AuthMethod {
    pub fn username(&self) -> &str {
        match self {
            Self::Password { username, .. }
            | Self::PrivateKey { username, .. }
            | Self::Agent { username }
            | Self::KeyboardInteractive { username, .. } => username,
        }
    }

    /// Authentication kind without the credential payload.
    pub fn kind(&self) -> AuthAttemptKind {
        match self {
            Self::Password { .. } => AuthAttemptKind::Password,
            Self::PrivateKey { .. } => AuthAttemptKind::PublicKey,
            Self::Agent { .. } => AuthAttemptKind::Agent,
            Self::KeyboardInteractive { .. } => AuthAttemptKind::KeyboardInteractive,
        }
    }
}

/// Authentication attempt kind, decoupled from its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthAttemptKind {
    Password,
    PublicKey,
    Agent,
    KeyboardInteractive,
}

impl AuthAttemptKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::PublicKey => "publickey",
            Self::Agent => "agent",
            Self::KeyboardInteractive => "keyboard-interactive",
        }
    }
}

/// Authentication methods advertised by the SSH server.
///
/// Mirrors the method list of `SSH_MSG_USERAUTH_FAILURE` (RFC 4252 §5.1).
/// Carriers such as [`crate::error::SshError::auth_methods`] and the session
/// types are `Option<AuthMethods>`: `None` means the server did not return a
/// method list (N4 falls back to "try the configured method + manual choice").
///
/// `agent` reports that agent-backed authentication is usable: SSH has no
/// wire-level `agent` method, agents supply keys for the `publickey` method,
/// so `agent` is true whenever the server allows `publickey` (or explicitly
/// lists `agent`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuthMethods {
    pub password: bool,
    pub publickey: bool,
    pub keyboard_interactive: bool,
    pub agent: bool,
}

impl AuthMethods {
    /// No usable method (server listed only methods we do not implement).
    pub const fn none() -> Self {
        Self {
            password: false,
            publickey: false,
            keyboard_interactive: false,
            agent: false,
        }
    }

    /// Parse the comma separated list returned by `Session::auth_methods`.
    ///
    /// Returns `None` when the server did not return a list (empty string).
    /// Unknown method names are ignored; a non-empty list of unknown methods
    /// still yields `Some(AuthMethods::none())`.
    pub fn parse(list: &str) -> Option<Self> {
        if list.trim().is_empty() {
            return None;
        }
        let mut methods = Self::none();
        for name in list.split(',') {
            match name.trim().to_ascii_lowercase().as_str() {
                "password" => methods.password = true,
                "publickey" => {
                    methods.publickey = true;
                    methods.agent = true;
                }
                "keyboard-interactive" => methods.keyboard_interactive = true,
                "agent" => methods.agent = true,
                _ => {}
            }
        }
        Some(methods)
    }

    /// Whether the server accepts an attempt of this kind.
    pub const fn allows(&self, kind: AuthAttemptKind) -> bool {
        match kind {
            AuthAttemptKind::Password => self.password,
            AuthAttemptKind::PublicKey => self.publickey,
            AuthAttemptKind::Agent => self.agent,
            AuthAttemptKind::KeyboardInteractive => self.keyboard_interactive,
        }
    }

    /// Human readable list for stage notes and error messages.
    pub fn describe(&self) -> String {
        let mut names = Vec::new();
        if self.password {
            names.push("password");
        }
        if self.publickey {
            names.push("publickey");
        }
        if self.keyboard_interactive {
            names.push("keyboard-interactive");
        }
        if self.agent {
            names.push("agent");
        }
        if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(",")
        }
    }
}

/// One authentication attempt handed to `SshAdapter::authenticate_with`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthAttempt {
    Password {
        username: String,
        password: String,
    },
    PublicKey {
        username: String,
        key_path: String,
        passphrase: Option<String>,
    },
    Agent {
        username: String,
    },
    /// Pre-collected answers, consumed in prompt order across rounds
    /// (see [`ScriptedPrompter`]).
    KeyboardInteractive {
        username: String,
        responses: Vec<String>,
    },
}

impl AuthAttempt {
    pub fn username(&self) -> &str {
        match self {
            Self::Password { username, .. }
            | Self::PublicKey { username, .. }
            | Self::Agent { username }
            | Self::KeyboardInteractive { username, .. } => username,
        }
    }

    pub fn kind(&self) -> AuthAttemptKind {
        match self {
            Self::Password { .. } => AuthAttemptKind::Password,
            Self::PublicKey { .. } => AuthAttemptKind::PublicKey,
            Self::Agent { .. } => AuthAttemptKind::Agent,
            Self::KeyboardInteractive { .. } => AuthAttemptKind::KeyboardInteractive,
        }
    }

    /// Build an attempt from the configured single-method [`AuthMethod`].
    ///
    /// The keyboard-interactive variant keeps the "single secret" model: the
    /// secret is answered to prompts in order across rounds.
    pub fn from_method(method: &AuthMethod) -> Self {
        match method {
            AuthMethod::Password { username, password } => Self::Password {
                username: username.clone(),
                password: password.clone(),
            },
            AuthMethod::PrivateKey {
                username,
                key_path,
                passphrase,
            } => Self::PublicKey {
                username: username.clone(),
                key_path: key_path.clone(),
                passphrase: passphrase.clone(),
            },
            AuthMethod::Agent { username } => Self::Agent {
                username: username.clone(),
            },
            AuthMethod::KeyboardInteractive { username, secret } => Self::KeyboardInteractive {
                username: username.clone(),
                responses: vec![secret.clone()],
            },
        }
    }
}

/// Fine grained authentication problem classification.
///
/// Carried by [`crate::error::SshError::auth_problem`] so the runtime can react
/// inside one auth window: re-enter credentials, hide a method the server does
/// not allow, or offer the remaining methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthProblemKind {
    /// The server rejected the presented credentials (wrong password, key not
    /// accepted, expired password).
    InvalidCredentials,
    /// The server does not offer / no longer accepts this method.
    MethodNotAllowed,
    /// The attempt failed without a plain credential rejection (or the server
    /// requires a different method); offer the remaining methods.
    OtherMethodRequired,
    /// The local user cancelled an interactive keyboard-interactive challenge.
    Cancelled,
}

impl AuthProblemKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::InvalidCredentials => "invalid-credentials",
            Self::MethodNotAllowed => "method-not-allowed",
            Self::OtherMethodRequired => "other-method-required",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One keyboard-interactive prompt as presented by the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub text: String,
    /// `true` when the response may be displayed while typing.
    pub echo: bool,
}

impl Prompt {
    pub fn new(text: impl Into<String>, echo: bool) -> Self {
        Self {
            text: text.into(),
            echo,
        }
    }
}

/// A keyboard-interactive challenge (one `SSH_MSG_USERAUTH_INFO_REQUEST`).
///
/// A single authentication call may produce multiple rounds; the prompter is
/// invoked once per round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardInteractiveChallenge {
    /// Server supplied challenge name (often empty, sometimes e.g. "SSH Server").
    pub name: String,
    /// Instructions to display above the prompts.
    pub instruction: String,
    pub prompts: Vec<Prompt>,
}

impl KeyboardInteractiveChallenge {
    pub fn new(
        name: impl Into<String>,
        instruction: impl Into<String>,
        prompts: Vec<Prompt>,
    ) -> Self {
        Self {
            name: name.into(),
            instruction: instruction.into(),
            prompts,
        }
    }

    pub fn prompt_count(&self) -> usize {
        self.prompts.len()
    }
}

/// Prompter answer for one challenge round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyboardInteractiveResponse {
    /// One answer per prompt (same order as `challenge.prompts`).
    Answers(Vec<String>),
    /// Abort the authentication flow.
    Cancelled,
}

impl KeyboardInteractiveResponse {
    pub fn answers(answers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Answers(answers.into_iter().map(Into::into).collect())
    }
}

/// Interactive keyboard-interactive responder.
///
/// Implementations are invoked once per server challenge round and may cancel.
/// An implementation must answer promptly; the connecting thread blocks while
/// `respond` runs.
pub trait KeyboardInteractivePrompter {
    fn respond(&mut self, challenge: &KeyboardInteractiveChallenge) -> KeyboardInteractiveResponse;
}

/// Default prompter for the legacy single-secret model
/// (`AuthMethod::KeyboardInteractive { secret }`).
///
/// The secret answers the first prompt, and any later non-echo prompt whose
/// text looks like a secret (password / passcode / OTP / token / verification
/// code); every other prompt is answered with an empty string. The previous
/// behavior of `real.rs` is preserved exactly.
#[derive(Debug, Clone)]
pub struct SingleSecretPrompter {
    secret: String,
    challenges: Vec<KeyboardInteractiveChallenge>,
}

impl SingleSecretPrompter {
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            challenges: Vec::new(),
        }
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// Challenges seen so far, in order (for diagnostics and tests).
    pub fn challenges(&self) -> &[KeyboardInteractiveChallenge] {
        &self.challenges
    }

    pub fn clear_challenges(&mut self) {
        self.challenges.clear();
    }
}

impl KeyboardInteractivePrompter for SingleSecretPrompter {
    fn respond(&mut self, challenge: &KeyboardInteractiveChallenge) -> KeyboardInteractiveResponse {
        self.challenges.push(challenge.clone());
        let answers = challenge
            .prompts
            .iter()
            .enumerate()
            .map(|(index, prompt)| {
                if index == 0 || (!prompt.echo && looks_like_secret_prompt(&prompt.text)) {
                    self.secret.clone()
                } else {
                    String::new()
                }
            })
            .collect();
        KeyboardInteractiveResponse::Answers(answers)
    }
}

fn looks_like_secret_prompt(text: &str) -> bool {
    let normalized = text.trim().to_ascii_lowercase();
    normalized.contains("password")
        || normalized.contains("passcode")
        || normalized.contains("otp")
        || normalized.contains("token")
        || normalized.contains("verification code")
}

/// Prompter backed by pre-collected answers, consumed in prompt order across
/// rounds (used by `AuthAttempt::KeyboardInteractive { responses }`).
#[derive(Debug, Clone, Default)]
pub struct ScriptedPrompter {
    answers: VecDeque<String>,
    challenges: Vec<KeyboardInteractiveChallenge>,
    cancel: bool,
}

impl ScriptedPrompter {
    pub fn new(answers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            answers: answers.into_iter().map(Into::into).collect(),
            challenges: Vec::new(),
            cancel: false,
        }
    }

    /// Prompter that cancels on the first challenge.
    pub fn cancelling() -> Self {
        Self {
            cancel: true,
            ..Self::default()
        }
    }

    pub fn push_answer(&mut self, answer: impl Into<String>) {
        self.answers.push_back(answer.into());
    }

    /// Remaining answers that have not been consumed yet.
    pub fn pending_answers(&self) -> usize {
        self.answers.len()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel
    }

    pub fn challenges(&self) -> &[KeyboardInteractiveChallenge] {
        &self.challenges
    }
}

impl KeyboardInteractivePrompter for ScriptedPrompter {
    fn respond(&mut self, challenge: &KeyboardInteractiveChallenge) -> KeyboardInteractiveResponse {
        self.challenges.push(challenge.clone());
        if self.cancel {
            return KeyboardInteractiveResponse::Cancelled;
        }
        let answers = challenge
            .prompts
            .iter()
            .map(|_| self.answers.pop_front().unwrap_or_default())
            .collect();
        KeyboardInteractiveResponse::Answers(answers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_method_list() {
        let methods = AuthMethods::parse("publickey,password,keyboard-interactive")
            .expect("non-empty list parses");
        assert!(methods.password);
        assert!(methods.publickey);
        assert!(methods.keyboard_interactive);
        assert!(methods.agent, "agent rides on publickey");
        assert!(methods.allows(AuthAttemptKind::Agent));
        assert!(methods.allows(AuthAttemptKind::PublicKey));

        let methods = AuthMethods::parse(" password , gssapi-with-mic ").expect("list");
        assert!(methods.password);
        assert!(!methods.publickey);
        assert!(!methods.keyboard_interactive);
        assert!(!methods.agent);
        assert!(!methods.allows(AuthAttemptKind::PublicKey));

        let methods = AuthMethods::parse("agent").expect("explicit agent token");
        assert!(methods.agent);
        assert!(!methods.publickey);
    }

    #[test]
    fn empty_server_method_list_is_none() {
        assert_eq!(AuthMethods::parse(""), None);
        assert_eq!(AuthMethods::parse("   "), None);
        assert_eq!(
            AuthMethods::parse("unknown-method"),
            Some(AuthMethods::none()),
            "a non-empty list of unknown methods still reports no usable method"
        );
    }

    #[test]
    fn describes_methods_for_messages() {
        assert_eq!(AuthMethods::none().describe(), "none");
        assert_eq!(
            AuthMethods::parse("keyboard-interactive,publickey")
                .expect("list")
                .describe(),
            "publickey,keyboard-interactive,agent"
        );
    }

    #[test]
    fn builds_attempt_from_configured_method() {
        let attempt = AuthAttempt::from_method(&AuthMethod::KeyboardInteractive {
            username: "alice".to_owned(),
            secret: "otp".to_owned(),
        });
        assert_eq!(
            attempt,
            AuthAttempt::KeyboardInteractive {
                username: "alice".to_owned(),
                responses: vec!["otp".to_owned()],
            }
        );
        assert_eq!(attempt.kind(), AuthAttemptKind::KeyboardInteractive);
        assert_eq!(attempt.username(), "alice");
        assert_eq!(
            AuthMethod::Agent {
                username: "bob".to_owned()
            }
            .kind(),
            AuthAttemptKind::Agent
        );
    }

    #[test]
    fn single_secret_prompter_preserves_legacy_answer_policy() {
        let mut prompter = SingleSecretPrompter::new("s3cret");
        let challenge = KeyboardInteractiveChallenge::new(
            "SSH Server",
            "Please authenticate",
            vec![
                Prompt::new("Password: ", false),
                Prompt::new("Verification code: ", false),
                Prompt::new("Hostname: ", true),
            ],
        );
        assert_eq!(
            prompter.respond(&challenge),
            KeyboardInteractiveResponse::Answers(vec![
                "s3cret".to_owned(),
                "s3cret".to_owned(),
                String::new(),
            ])
        );
        assert_eq!(prompter.challenges().len(), 1);
        assert_eq!(prompter.challenges()[0].name, "SSH Server");
    }

    #[test]
    fn single_secret_prompter_answers_multi_round_challenges() {
        let mut prompter = SingleSecretPrompter::new("otp");
        let first = KeyboardInteractiveChallenge::new(
            "SSH Server",
            "",
            vec![Prompt::new("Password: ", false)],
        );
        let second = KeyboardInteractiveChallenge::new(
            "SSH Server",
            "",
            vec![Prompt::new("One-time password: ", false)],
        );
        assert_eq!(
            prompter.respond(&first),
            KeyboardInteractiveResponse::Answers(vec!["otp".to_owned()])
        );
        assert_eq!(
            prompter.respond(&second),
            KeyboardInteractiveResponse::Answers(vec!["otp".to_owned()])
        );
        assert_eq!(prompter.challenges().len(), 2);
    }

    #[test]
    fn scripted_prompter_consumes_answers_across_rounds() {
        let mut prompter = ScriptedPrompter::new(vec!["first", "second"]);
        let round_one = KeyboardInteractiveChallenge::new(
            "SSH Server",
            "",
            vec![Prompt::new("Password: ", false)],
        );
        let round_two = KeyboardInteractiveChallenge::new(
            "SSH Server",
            "",
            vec![
                Prompt::new("Password: ", false),
                Prompt::new("Verification code: ", false),
            ],
        );
        assert_eq!(
            prompter.respond(&round_one),
            KeyboardInteractiveResponse::Answers(vec!["first".to_owned()])
        );
        assert_eq!(
            prompter.respond(&round_two),
            KeyboardInteractiveResponse::Answers(vec!["second".to_owned(), String::new()])
        );
        assert_eq!(prompter.pending_answers(), 0);
        assert_eq!(prompter.challenges().len(), 2);
    }

    #[test]
    fn scripted_prompter_can_cancel() {
        let mut prompter = ScriptedPrompter::cancelling();
        let challenge =
            KeyboardInteractiveChallenge::new("", "", vec![Prompt::new("Password: ", false)]);
        assert_eq!(
            prompter.respond(&challenge),
            KeyboardInteractiveResponse::Cancelled
        );
        assert!(prompter.is_cancelled());
    }
}
