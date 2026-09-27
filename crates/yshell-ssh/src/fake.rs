//! Fake SSH transport used to exercise the app/runtime pipeline before real transport exists.

use std::collections::VecDeque;

use crate::{
    auth::{
        AuthAttempt, AuthAttemptKind, AuthMethods, AuthProblemKind, KeyboardInteractiveChallenge,
        KeyboardInteractivePrompter, KeyboardInteractiveResponse,
    },
    channel::ShellSession,
    client::{ExecOutput, ShellAdapter, SshAdapter, SshConnectionConfig},
    error::{SshError, SshErrorKind, SshResult},
    proxy::ProxyState,
    pty::PtySize,
};

/// Configurable fake SSH server behavior.
///
/// The default adapter accepts every connection and authentication attempt; set
/// [`FakeSshAdapter::auth_methods`], [`FakeSshAdapter::auth_expectation`],
/// [`FakeSshAdapter::rejection`] and
/// [`FakeSshAdapter::keyboard_interactive_rounds`] to simulate server driven
/// authentication flows (N4 auth window development).
///
/// # Simulating an auth flow (N4)
///
/// ```
/// use yshell_ssh::{
///     AuthMethod, AuthMethods, FakeSshAdapter, KeyboardInteractiveChallenge, Prompt,
///     ScriptedPrompter, SshClient, SshConnectionConfig,
/// };
///
/// let adapter = FakeSshAdapter {
///     auth_methods: AuthMethods::parse("keyboard-interactive"),
///     keyboard_interactive_rounds: vec![
///         KeyboardInteractiveChallenge::new(
///             "SSH Server",
///             "Password authentication",
///             vec![Prompt::new("Password: ", false)],
///         ),
///         KeyboardInteractiveChallenge::new(
///             "SSH Server",
///             "Two factor",
///             vec![Prompt::new("Verification code: ", false)],
///         ),
///     ],
///     ..FakeSshAdapter::default()
/// };
/// let client = SshClient::with_adapter(adapter);
/// let config = SshConnectionConfig::new(
///     "example.test",
///     22,
///     AuthMethod::Agent {
///         username: "alice".to_owned(),
///     },
/// );
/// let mut session = client.connect(&config).expect("connect");
/// assert_eq!(
///     session.auth_methods,
///     AuthMethods::parse("keyboard-interactive")
/// );
///
/// // A scripted prompter answers each round in order; a real UI would show a
/// // dialog per challenge and call `respond` from the UI thread.
/// let mut prompter = ScriptedPrompter::new(vec!["hunter2", "123456"]);
/// client
///     .authenticate_keyboard_interactive(&mut session, "alice", &mut prompter)
///     .expect("two rounds");
/// assert_eq!(prompter.challenges().len(), 2);
/// assert!(session.authenticated);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeSshAdapter {
    /// Simulated `Session::auth_methods` result (`None` = server returned no list).
    pub auth_methods: Option<AuthMethods>,
    /// Expected credential payload per method; unset fields are not validated.
    pub auth_expectation: FakeAuthExpectation,
    /// Forced failure for every authentication attempt (takes precedence over
    /// the expectation).
    pub rejection: Option<FakeAuthRejection>,
    /// Scripted keyboard-interactive rounds served by
    /// [`SshAdapter::authenticate_keyboard_interactive`]; empty = the server
    /// sends no challenge and accepts immediately.
    pub keyboard_interactive_rounds: Vec<KeyboardInteractiveChallenge>,
}

/// Expected credential payloads for [`FakeSshAdapter::auth_expectation`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeAuthExpectation {
    pub password: Option<String>,
    pub key_path: Option<String>,
    pub key_passphrase: Option<String>,
    /// Expected answers per keyboard-interactive round.
    pub keyboard_interactive: Vec<Vec<String>>,
}

/// Simulated server side authentication failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeAuthRejection {
    InvalidCredentials,
    MethodNotAllowed,
    OtherMethodRequired,
}

impl FakeAuthRejection {
    const fn problem(self) -> AuthProblemKind {
        match self {
            Self::InvalidCredentials => AuthProblemKind::InvalidCredentials,
            Self::MethodNotAllowed => AuthProblemKind::MethodNotAllowed,
            Self::OtherMethodRequired => AuthProblemKind::OtherMethodRequired,
        }
    }

    const fn label(self) -> &'static str {
        self.problem().label()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeSshSession {
    pub host: String,
    pub username: String,
    pub proxy_state: ProxyState,
    pub executed_commands: Vec<String>,
    /// Simulated server advertised auth methods.
    pub auth_methods: Option<AuthMethods>,
    pub authenticated: bool,
    /// Authentication attempts seen by the fake server, in order.
    pub auth_attempts: Vec<AuthAttempt>,
    /// Keyboard-interactive rounds exchanged with a prompter.
    pub keyboard_interactive_log: Vec<FakeKeyboardInteractiveExchange>,
    connected: bool,
}

/// One recorded keyboard-interactive round of a fake session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeKeyboardInteractiveExchange {
    pub username: String,
    pub challenge: KeyboardInteractiveChallenge,
    pub answers: Vec<String>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeShellSession {
    pub host: String,
    pub username: String,
    pub proxy_state: ProxyState,
    pub pty_size: PtySize,
    pub written_inputs: Vec<Vec<u8>>,
    /// Simulated server advertised auth methods.
    pub auth_methods: Option<AuthMethods>,
    connected: bool,
    pending_output: VecDeque<Vec<u8>>,
}

impl SshAdapter for FakeSshAdapter {
    type Session = FakeSshSession;

    fn connect(&self, config: &SshConnectionConfig) -> SshResult<Self::Session> {
        config.validate()?;
        Ok(FakeSshSession {
            host: config.host.clone(),
            username: config.username().to_owned(),
            proxy_state: match config.proxy.address() {
                Some(address) => ProxyState::Connected {
                    address: address.to_owned(),
                },
                None => ProxyState::Disabled,
            },
            executed_commands: Vec::new(),
            auth_methods: self.auth_methods,
            authenticated: false,
            auth_attempts: Vec::new(),
            keyboard_interactive_log: Vec::new(),
            connected: true,
        })
    }

    fn exec(&self, session: &mut Self::Session, command: &str) -> SshResult<ExecOutput> {
        if !session.connected {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "cannot execute command on disconnected session",
            ));
        }
        session.executed_commands.push(command.to_owned());
        Ok(ExecOutput {
            stdout: format!("fake ssh executed: {command}\n").into_bytes(),
            stderr: Vec::new(),
            exit_status: 0,
        })
    }

    fn disconnect(&self, mut session: Self::Session) -> SshResult<()> {
        session.connected = false;
        Ok(())
    }

    fn auth_methods(&self, session: &Self::Session) -> SshResult<Option<AuthMethods>> {
        Ok(session.auth_methods)
    }

    fn authenticate_with(
        &self,
        session: &mut Self::Session,
        attempt: AuthAttempt,
    ) -> SshResult<()> {
        self.apply_auth_attempt(session, attempt)
    }

    fn authenticate_keyboard_interactive(
        &self,
        session: &mut Self::Session,
        username: &str,
        prompter: &mut dyn KeyboardInteractivePrompter,
    ) -> SshResult<()> {
        self.apply_keyboard_interactive(session, username, prompter)
    }
}

impl FakeSshAdapter {
    fn apply_auth_attempt(
        &self,
        session: &mut FakeSshSession,
        attempt: AuthAttempt,
    ) -> SshResult<()> {
        ensure_connected(session)?;
        session.auth_attempts.push(attempt.clone());
        let kind = attempt.kind();
        self.check_method_allowed(session, kind)?;
        if let Some(rejection) = self.rejection {
            // MethodNotAllowed already resolved above; keep the remaining
            // forced classifications available for tests/UI development.
            return Err(self.rejection_error(session, kind, rejection));
        }
        if !self.expectation_matches(&attempt) {
            return Err(self.credential_error(session, kind));
        }
        session.authenticated = true;
        Ok(())
    }

    fn apply_keyboard_interactive(
        &self,
        session: &mut FakeSshSession,
        username: &str,
        prompter: &mut dyn KeyboardInteractivePrompter,
    ) -> SshResult<()> {
        ensure_connected(session)?;
        self.check_method_allowed(session, AuthAttemptKind::KeyboardInteractive)?;
        if let Some(rejection) = self.rejection {
            return Err(self.rejection_error(
                session,
                AuthAttemptKind::KeyboardInteractive,
                rejection,
            ));
        }
        for (index, challenge) in self.keyboard_interactive_rounds.iter().enumerate() {
            match prompter.respond(challenge) {
                KeyboardInteractiveResponse::Cancelled => {
                    session
                        .keyboard_interactive_log
                        .push(FakeKeyboardInteractiveExchange {
                            username: username.to_owned(),
                            challenge: challenge.clone(),
                            answers: Vec::new(),
                            cancelled: true,
                        });
                    return Err(SshError::new(
                        SshErrorKind::Authentication,
                        "fake keyboard-interactive authentication was cancelled by the user",
                    )
                    .with_auth_context(session.auth_methods, AuthProblemKind::Cancelled));
                }
                KeyboardInteractiveResponse::Answers(answers) => {
                    let answers = normalize_answers(answers, challenge.prompt_count());
                    session
                        .keyboard_interactive_log
                        .push(FakeKeyboardInteractiveExchange {
                            username: username.to_owned(),
                            challenge: challenge.clone(),
                            answers: answers.clone(),
                            cancelled: false,
                        });
                    if let Some(expected) = self.auth_expectation.keyboard_interactive.get(index) {
                        let expected =
                            normalize_answers(expected.clone(), challenge.prompt_count());
                        if expected != answers {
                            return Err(self
                                .credential_error(session, AuthAttemptKind::KeyboardInteractive));
                        }
                    }
                }
            }
        }
        session.authenticated = true;
        Ok(())
    }

    fn check_method_allowed(
        &self,
        session: &FakeSshSession,
        kind: AuthAttemptKind,
    ) -> SshResult<()> {
        let allowed = match session.auth_methods {
            Some(methods) => methods.allows(kind),
            None => true,
        };
        if allowed {
            return Ok(());
        }
        Err(SshError::new(
            SshErrorKind::Authentication,
            format!(
                "fake ssh server does not offer `{}` authentication",
                kind.label()
            ),
        )
        .with_auth_context(session.auth_methods, AuthProblemKind::MethodNotAllowed))
    }

    fn rejection_error(
        &self,
        session: &FakeSshSession,
        kind: AuthAttemptKind,
        rejection: FakeAuthRejection,
    ) -> SshError {
        SshError::new(
            SshErrorKind::Authentication,
            format!(
                "fake ssh server rejected `{}` authentication: {}",
                kind.label(),
                rejection.label()
            ),
        )
        .with_auth_context(session.auth_methods, rejection.problem())
    }

    fn credential_error(&self, session: &FakeSshSession, kind: AuthAttemptKind) -> SshError {
        SshError::new(
            SshErrorKind::Authentication,
            format!(
                "fake ssh server rejected the supplied credentials for `{}` authentication",
                kind.label()
            ),
        )
        .with_auth_context(session.auth_methods, AuthProblemKind::InvalidCredentials)
    }

    fn expectation_matches(&self, attempt: &AuthAttempt) -> bool {
        match attempt {
            AuthAttempt::Password { password, .. } => self
                .auth_expectation
                .password
                .as_deref()
                .is_none_or(|expected| expected == password),
            AuthAttempt::PublicKey {
                key_path,
                passphrase,
                ..
            } => {
                let key_matches = self
                    .auth_expectation
                    .key_path
                    .as_deref()
                    .is_none_or(|expected| expected == key_path);
                let passphrase_matches = match &self.auth_expectation.key_passphrase {
                    Some(expected) => passphrase.as_deref() == Some(expected.as_str()),
                    None => true,
                };
                key_matches && passphrase_matches
            }
            AuthAttempt::Agent { .. } => true,
            AuthAttempt::KeyboardInteractive { responses, .. } => {
                if self.auth_expectation.keyboard_interactive.is_empty() {
                    true
                } else {
                    &self.auth_expectation.keyboard_interactive.concat() == responses
                }
            }
        }
    }
}

fn ensure_connected(session: &FakeSshSession) -> SshResult<()> {
    if session.connected {
        Ok(())
    } else {
        Err(SshError::new(
            SshErrorKind::Channel,
            "cannot authenticate on disconnected session",
        ))
    }
}

fn normalize_answers(answers: Vec<String>, prompt_count: usize) -> Vec<String> {
    let mut answers = answers;
    answers.truncate(prompt_count);
    answers.resize(prompt_count, String::new());
    answers
}

impl ShellAdapter for FakeSshAdapter {
    type Shell = FakeShellSession;

    fn open_shell(&self, config: &SshConnectionConfig) -> SshResult<Self::Shell> {
        config.validate()?;
        let mut pending_output = VecDeque::new();
        pending_output.push_back(
            format!(
                "Connecting to {}:{} as {}\n",
                config.host,
                config.port,
                config.username()
            )
            .into_bytes(),
        );
        pending_output.push_back(
            format!(
                "Fake shell established with TERM={} and size={}x{}\n",
                config.pty.term, config.pty.size.columns, config.pty.size.rows
            )
            .into_bytes(),
        );
        pending_output.push_back(b"$ ".to_vec());
        Ok(FakeShellSession {
            host: config.host.clone(),
            username: config.username().to_owned(),
            proxy_state: match config.proxy.address() {
                Some(address) => ProxyState::Connected {
                    address: address.to_owned(),
                },
                None => ProxyState::Disabled,
            },
            pty_size: config.pty.size,
            written_inputs: Vec::new(),
            auth_methods: self.auth_methods,
            connected: true,
            pending_output,
        })
    }
}

impl ShellSession for FakeShellSession {
    fn is_connected(&self) -> bool {
        self.connected
    }

    fn poll_output(&mut self) -> SshResult<Vec<u8>> {
        if !self.connected {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "cannot poll output from disconnected shell session",
            ));
        }
        Ok(self.pending_output.pop_front().unwrap_or_default())
    }

    fn write_input(&mut self, bytes: &[u8]) -> SshResult<()> {
        if !self.connected {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "cannot write input into disconnected shell session",
            ));
        }
        self.written_inputs.push(bytes.to_vec());
        let printable = String::from_utf8_lossy(bytes).replace('\r', "\\r");
        self.pending_output
            .push_back(format!("fake-shell received input: {printable}\n$ ").into_bytes());
        Ok(())
    }

    fn resize_pty(&mut self, size: PtySize) -> SshResult<()> {
        if !self.connected {
            return Err(SshError::new(
                SshErrorKind::Channel,
                "cannot resize disconnected shell session",
            ));
        }
        self.pty_size = size;
        self.pending_output.push_back(
            format!("fake-shell resized to {}x{}\n$ ", size.columns, size.rows).into_bytes(),
        );
        Ok(())
    }

    fn disconnect(&mut self) -> SshResult<()> {
        self.connected = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        auth::{
            AuthAttempt, AuthMethods, AuthProblemKind, KeyboardInteractiveChallenge, Prompt,
            ScriptedPrompter, SingleSecretPrompter,
        },
        AuthMethod, ShellClient, ShellSession, SshClient, SshConnectionConfig,
    };

    use super::{FakeAuthExpectation, FakeAuthRejection, FakeSshAdapter};

    fn agent_config() -> SshConnectionConfig {
        SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        )
    }

    #[test]
    fn fake_adapter_connects_and_executes_deterministically() {
        let client = SshClient::new();
        let config = agent_config();
        let mut session = client.connect(&config).expect("connect");
        let output = client.exec(&mut session, "uptime").expect("exec");
        assert_eq!(session.host, "example.test");
        assert_eq!(output.stdout, b"fake ssh executed: uptime\n");
    }

    #[test]
    fn fake_shell_session_streams_output_and_accepts_input() {
        let client = ShellClient::new();
        let config = agent_config();
        let mut shell = client.open_shell(&config).expect("open shell");

        assert!(
            String::from_utf8_lossy(&shell.poll_output().expect("banner")).contains("Connecting")
        );
        assert!(String::from_utf8_lossy(&shell.poll_output().expect("term"))
            .contains("Fake shell established"));

        shell.write_input(b"pwd\n").expect("write input");
        assert!(String::from_utf8_lossy(&shell.poll_output().expect("prompt")).contains('$'));
        assert!(String::from_utf8_lossy(&shell.poll_output().expect("echo"))
            .contains("fake-shell received input"));
    }

    #[test]
    fn fake_reports_configured_auth_methods() {
        let adapter = FakeSshAdapter {
            auth_methods: AuthMethods::parse("password,keyboard-interactive"),
            ..FakeSshAdapter::default()
        };
        let client = SshClient::with_adapter(adapter.clone());
        let session = client.connect(&agent_config()).expect("connect");

        let expected = AuthMethods::parse("password,keyboard-interactive");
        assert_eq!(session.auth_methods, expected);
        assert_eq!(
            client.auth_methods(&session).expect("query methods"),
            expected
        );

        let shell = ShellClient::with_adapter(adapter)
            .open_shell(&agent_config())
            .expect("open shell");
        assert_eq!(
            shell.auth_methods, expected,
            "shell sessions expose the list too"
        );
    }

    #[test]
    fn fake_rejects_method_missing_from_server_list() {
        let adapter = FakeSshAdapter {
            auth_methods: AuthMethods::parse("publickey"),
            ..FakeSshAdapter::default()
        };
        let client = SshClient::with_adapter(adapter);
        let mut session = client.connect(&agent_config()).expect("connect");

        let error = client
            .authenticate_with(
                &mut session,
                AuthAttempt::Password {
                    username: "alice".to_owned(),
                    password: "secret".to_owned(),
                },
            )
            .expect_err("password is not offered");

        assert_eq!(error.kind, crate::SshErrorKind::Authentication);
        assert_eq!(error.auth_problem, Some(AuthProblemKind::MethodNotAllowed));
        assert_eq!(error.auth_methods, AuthMethods::parse("publickey"));
        assert!(!session.authenticated);
    }

    #[test]
    fn fake_classifies_wrong_password_as_invalid_credentials() {
        let adapter = FakeSshAdapter {
            auth_methods: AuthMethods::parse("password"),
            auth_expectation: FakeAuthExpectation {
                password: Some("correct".to_owned()),
                ..FakeAuthExpectation::default()
            },
            ..FakeSshAdapter::default()
        };
        let client = SshClient::with_adapter(adapter);
        let mut session = client.connect(&agent_config()).expect("connect");

        let error = client
            .authenticate_with(
                &mut session,
                AuthAttempt::Password {
                    username: "alice".to_owned(),
                    password: "wrong".to_owned(),
                },
            )
            .expect_err("wrong password is rejected");
        assert_eq!(
            error.auth_problem,
            Some(AuthProblemKind::InvalidCredentials)
        );
        assert!(error.message.contains("credentials"));
        assert_eq!(session.auth_attempts.len(), 1);

        client
            .authenticate_with(
                &mut session,
                AuthAttempt::Password {
                    username: "alice".to_owned(),
                    password: "correct".to_owned(),
                },
            )
            .expect("retry with the right password succeeds");
        assert!(session.authenticated);
        assert_eq!(session.auth_attempts.len(), 2);
    }

    #[test]
    fn fake_forced_rejection_maps_to_other_method_required() {
        let adapter = FakeSshAdapter {
            rejection: Some(FakeAuthRejection::OtherMethodRequired),
            ..FakeSshAdapter::default()
        };
        let client = SshClient::with_adapter(adapter);
        let mut session = client.connect(&agent_config()).expect("connect");

        let error = client
            .authenticate_with(
                &mut session,
                AuthAttempt::Agent {
                    username: "alice".to_owned(),
                },
            )
            .expect_err("forced rejection");
        assert_eq!(error.kind, crate::SshErrorKind::Authentication);
        assert_eq!(
            error.auth_problem,
            Some(AuthProblemKind::OtherMethodRequired)
        );
        assert_eq!(error.auth_methods, None, "server returned no method list");
    }

    #[test]
    fn fake_keyboard_interactive_serves_multi_round_prompts() {
        let adapter = FakeSshAdapter {
            auth_methods: AuthMethods::parse("keyboard-interactive"),
            auth_expectation: FakeAuthExpectation {
                keyboard_interactive: vec![
                    vec!["otp-round-1".to_owned()],
                    vec!["otp-round-2".to_owned(), "yes".to_owned()],
                ],
                ..FakeAuthExpectation::default()
            },
            keyboard_interactive_rounds: vec![
                KeyboardInteractiveChallenge::new(
                    "SSH Server",
                    "Password authentication",
                    vec![Prompt::new("Password: ", false)],
                ),
                KeyboardInteractiveChallenge::new(
                    "SSH Server",
                    "Two factor",
                    vec![
                        Prompt::new("Verification code: ", false),
                        Prompt::new("Trust this device? ", true),
                    ],
                ),
            ],
            ..FakeSshAdapter::default()
        };
        let client = SshClient::with_adapter(adapter);
        let mut session = client.connect(&agent_config()).expect("connect");
        let mut prompter = ScriptedPrompter::new(vec!["otp-round-1", "otp-round-2", "yes"]);

        client
            .authenticate_keyboard_interactive(&mut session, "alice", &mut prompter)
            .expect("two rounds answered");

        assert!(session.authenticated);
        assert_eq!(prompter.challenges().len(), 2);
        assert_eq!(session.keyboard_interactive_log.len(), 2);
        assert_eq!(
            session.keyboard_interactive_log[1].answers,
            vec!["otp-round-2".to_owned(), "yes".to_owned()]
        );
        assert!(!session.keyboard_interactive_log[0].cancelled);
    }

    #[test]
    fn fake_keyboard_interactive_mismatch_is_invalid_credentials() {
        let adapter = FakeSshAdapter {
            auth_expectation: FakeAuthExpectation {
                keyboard_interactive: vec![vec!["expected".to_owned()]],
                ..FakeAuthExpectation::default()
            },
            keyboard_interactive_rounds: vec![KeyboardInteractiveChallenge::new(
                "",
                "",
                vec![Prompt::new("Password: ", false)],
            )],
            ..FakeSshAdapter::default()
        };
        let client = SshClient::with_adapter(adapter);
        let mut session = client.connect(&agent_config()).expect("connect");
        let mut prompter = ScriptedPrompter::new(vec!["wrong"]);

        let error = client
            .authenticate_keyboard_interactive(&mut session, "alice", &mut prompter)
            .expect_err("mismatched answer is rejected");
        assert_eq!(
            error.auth_problem,
            Some(AuthProblemKind::InvalidCredentials)
        );
        assert!(!session.authenticated);
    }

    #[test]
    fn fake_keyboard_interactive_cancel_is_reported() {
        let adapter = FakeSshAdapter {
            keyboard_interactive_rounds: vec![KeyboardInteractiveChallenge::new(
                "",
                "",
                vec![Prompt::new("Password: ", false)],
            )],
            ..FakeSshAdapter::default()
        };
        let client = SshClient::with_adapter(adapter);
        let mut session = client.connect(&agent_config()).expect("connect");
        let mut prompter = ScriptedPrompter::cancelling();

        let error = client
            .authenticate_keyboard_interactive(&mut session, "alice", &mut prompter)
            .expect_err("cancel aborts");
        assert_eq!(error.auth_problem, Some(AuthProblemKind::Cancelled));
        assert!(!session.authenticated);
        assert!(session.keyboard_interactive_log[0].cancelled);
    }

    #[test]
    fn fake_keyboard_interactive_works_with_single_secret_prompter() {
        // Mirrors the legacy `AuthMethod::KeyboardInteractive { secret }` flow.
        let adapter = FakeSshAdapter {
            auth_expectation: FakeAuthExpectation {
                keyboard_interactive: vec![vec!["s3cret".to_owned()]],
                ..FakeAuthExpectation::default()
            },
            keyboard_interactive_rounds: vec![KeyboardInteractiveChallenge::new(
                "",
                "",
                vec![Prompt::new("Password: ", false)],
            )],
            ..FakeSshAdapter::default()
        };
        let client = SshClient::with_adapter(adapter);
        let mut session = client.connect(&agent_config()).expect("connect");
        let mut prompter = SingleSecretPrompter::new("s3cret");

        client
            .authenticate_keyboard_interactive(&mut session, "alice", &mut prompter)
            .expect("single secret authenticates");
        assert!(session.authenticated);
    }

    #[test]
    fn fake_authenticate_with_keyboard_interactive_uses_flat_responses() {
        let adapter = FakeSshAdapter {
            auth_expectation: FakeAuthExpectation {
                keyboard_interactive: vec![
                    vec!["one".to_owned()],
                    vec!["two".to_owned(), "three".to_owned()],
                ],
                ..FakeAuthExpectation::default()
            },
            ..FakeSshAdapter::default()
        };
        let client = SshClient::with_adapter(adapter);
        let mut session = client.connect(&agent_config()).expect("connect");

        client
            .authenticate_with(
                &mut session,
                AuthAttempt::KeyboardInteractive {
                    username: "alice".to_owned(),
                    responses: vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
                },
            )
            .expect("flat responses cover all rounds");
        assert!(session.authenticated);
    }
}
