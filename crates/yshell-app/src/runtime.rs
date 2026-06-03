//! Runtime composition layer that bridges config, core commands, and UI callbacks.

use std::{collections::BTreeMap, path::PathBuf};

use yshell_config::{parse_quick_connect, ConfigDocument, ConfigStore, LoadOutcome, SessionProfile};
use yshell_core::{CommandDispatcher, CoreCommandDispatcher, SessionCommand, SessionEvent};
use yshell_ssh::{ShellClient, TransportBackend};

use crate::{
    error::{AppError, AppResult},
    session_runtime::SessionRuntime,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppProjection {
    pub config_dir_text: String,
    pub active_session_text: String,
    pub session_summary_text: String,
    pub tab_text: String,
    pub terminal_title_text: String,
    pub terminal_body_text: String,
    pub status_text: String,
    pub transport_backend_text: String,
    pub sftp_visible: bool,
    pub tunnels_visible: bool,
    pub commands_visible: bool,
}

#[derive(Debug)]
pub struct AppRuntime {
    config_dir: PathBuf,
    config_store: ConfigStore,
    config_document: ConfigDocument,
    dispatcher: CoreCommandDispatcher,
    sessions: BTreeMap<String, SessionRuntime>,
    active_session_id: Option<String>,
    sftp_visible: bool,
    tunnels_visible: bool,
    commands_visible: bool,
    status_text: String,
    transport_backend: TransportBackend,
    recovered_from_backup: Option<PathBuf>,
    next_runtime_ordinal: usize,
}

impl AppRuntime {
    pub fn new(config_dir: PathBuf) -> AppResult<Self> {
        let config_store = ConfigStore::new(config_dir.clone());
        let LoadOutcome {
            document,
            recovered_from_backup,
        } = config_store.load_or_recover().map_err(AppError::from_error)?;
        let mut runtime = Self {
            config_dir,
            config_store,
            config_document: document,
            dispatcher: CoreCommandDispatcher::new(),
            sessions: BTreeMap::new(),
            active_session_id: None,
            sftp_visible: true,
            tunnels_visible: true,
            commands_visible: true,
            status_text: String::new(),
            transport_backend: TransportBackend::Fake,
            recovered_from_backup,
            next_runtime_ordinal: 1,
        };
        runtime.status_text = runtime.startup_status();
        runtime.hydrate_saved_sessions();
        Ok(runtime)
    }

    pub fn projection(&self) -> AppProjection {
        AppProjection {
            config_dir_text: self.config_dir.display().to_string(),
            active_session_text: self.active_session_label(),
            session_summary_text: self.session_summary_text(),
            tab_text: self.tab_text(),
            terminal_title_text: self.terminal_title_text(),
            terminal_body_text: self.terminal_body_text(),
            status_text: self.status_text.clone(),
            transport_backend_text: self.transport_backend.label().to_owned(),
            sftp_visible: self.sftp_visible,
            tunnels_visible: self.tunnels_visible,
            commands_visible: self.commands_visible,
        }
    }

    pub fn handle_quick_connect(&mut self, input: &str) -> AppResult<AppProjection> {
        let target = parse_quick_connect(input).map_err(AppError::from_error)?;
        let runtime = SessionRuntime::from_quick_connect(target, self.allocate_runtime_ordinal());
        self.activate_runtime_session(runtime)
    }

    pub fn handle_new_session(&mut self) -> AppProjection {
        let mut runtime = SessionRuntime::draft(self.allocate_runtime_ordinal());
        let session_id = runtime.session_id().as_str().to_owned();
        runtime.append_status_line("This draft is runtime-owned but not yet connected.");
        self.active_session_id = Some(session_id.clone());
        self.status_text =
            "Created a runtime-backed session draft. Next step is wiring this draft into saved-session editing and real connect.".to_owned();
        self.sessions.insert(session_id, runtime);
        self.projection()
    }

    pub fn select_fake_transport_backend(&mut self) -> AppProjection {
        self.select_transport_backend(TransportBackend::Fake)
    }

    pub fn select_real_transport_backend(&mut self) -> AppProjection {
        self.select_transport_backend(TransportBackend::Real)
    }

    pub fn save_active_session(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let active_runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let profile_id = self.next_saved_profile_id(&active_runtime.ssh_config.host);
        let profile = active_runtime.to_session_profile(profile_id.clone());

        if let Some(existing) = self
            .config_document
            .sessions
            .iter_mut()
            .find(|existing| existing.id == profile_id)
        {
            *existing = profile;
        } else {
            self.config_document.sessions.push(profile);
        }
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        self.status_text = format!(
            "Saved active runtime session as profile `{}`. Config now contains {} saved session(s).",
            profile_id,
            self.config_document.sessions.len()
        );
        Ok(self.projection())
    }

    pub fn open_saved_session(&mut self, profile_id: &str) -> AppResult<AppProjection> {
        let profile = self
            .config_document
            .find_session(profile_id)
            .cloned()
            .ok_or_else(|| AppError::new(format!("saved session `{profile_id}` was not found")))?;
        let runtime = SessionRuntime::from_profile(&profile, self.allocate_runtime_ordinal());
        self.activate_runtime_session(runtime)
    }

    pub fn open_first_saved_session(&mut self) -> AppResult<AppProjection> {
        let first_profile_id = self
            .config_document
            .sessions
            .first()
            .map(|profile| profile.id.clone())
            .ok_or_else(|| AppError::new("no saved sessions are available"))?;
        self.open_saved_session(&first_profile_id)
    }

    pub fn send_active_terminal_input(&mut self, input: &str) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        self.dispatcher
            .dispatch(SessionCommand::SendTerminalInput {
                session_id,
                bytes: input.as_bytes().to_vec(),
            })
            .map_err(AppError::from_error)?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let _ = runtime
            .write_terminal_input(input.as_bytes())
            .map_err(AppError::from_error)?;
        self.status_text = format!(
            "Sent {} bytes through the runtime terminal pipeline.",
            input.len()
        );
        Ok(self.projection())
    }

    pub fn resize_active_terminal(&mut self, columns: u16, rows: u16) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        self.dispatcher
            .dispatch(SessionCommand::ResizeTerminal {
                session_id,
                columns,
                rows,
            })
            .map_err(AppError::from_error)?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let _ = runtime
            .resize_shell_pty(columns, rows)
            .map_err(AppError::from_error)?;
        self.status_text = format!(
            "Resized runtime terminal to {}x{}.",
            columns, rows
        );
        Ok(self.projection())
    }

    pub fn disconnect_active_session(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            runtime.disconnect_shell().map_err(AppError::from_error)?;
        }
        let events = self
            .dispatcher
            .dispatch(SessionCommand::SetSessionState {
                session_id: session_id.clone(),
                state: yshell_core::SessionState::Disconnected,
            })
            .map_err(AppError::from_error)?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        Self::apply_session_events(&session_id, runtime, events);
        self.status_text = "Disconnected the active runtime session.".to_owned();
        Ok(self.projection())
    }

    pub fn reconnect_active_session(&mut self) -> AppResult<AppProjection> {
        let session_key = self.active_session_key()?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let ssh_config = self
            .sessions
            .get(&session_key)
            .map(|session| session.ssh_config.clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;

        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            runtime.append_status_line("Reconnecting runtime shell session.");
        }
        let connecting_events = self
            .dispatcher
            .dispatch(SessionCommand::SetSessionState {
                session_id: session_id.clone(),
                state: yshell_core::SessionState::Connecting,
            })
            .map_err(AppError::from_error)?;
        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            Self::apply_session_events(&session_id, runtime, connecting_events);
        }

        let shell_session = self
            .open_shell_for_runtime(&ssh_config)
            .map_err(AppError::from_error)?;
        let shell_connected = shell_session.is_connected();
        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            runtime.attach_shell_session(shell_session);
            let _ = runtime.poll_shell_output().map_err(AppError::from_error)?;
            if !shell_connected {
                runtime.append_status_line(
                    "Shell runtime reopened a staged backend scaffold. Connection is not live yet.",
                );
            }
        }
        if shell_connected {
            let connected_events = self
                .dispatcher
                .dispatch(SessionCommand::SetSessionState {
                    session_id: session_id.clone(),
                    state: yshell_core::SessionState::Connected,
                })
                .map_err(AppError::from_error)?;
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            Self::apply_session_events(&session_id, runtime, connected_events);
        }
        self.status_text = format!(
            "{} the active runtime session through the `{}` shell backend.",
            if shell_connected {
                "Reconnected"
            } else {
                "Reopened"
            },
            self.transport_backend.label()
        );
        Ok(self.projection())
    }

    pub fn toggle_sftp(&mut self) -> AppProjection {
        self.sftp_visible = !self.sftp_visible;
        self.status_text = if self.sftp_visible {
            "SFTP panel shown".to_owned()
        } else {
            "SFTP panel hidden".to_owned()
        };
        self.projection()
    }

    pub fn toggle_tunnels(&mut self) -> AppProjection {
        self.tunnels_visible = !self.tunnels_visible;
        self.status_text = if self.tunnels_visible {
            "Tunnels panel shown".to_owned()
        } else {
            "Tunnels panel hidden".to_owned()
        };
        self.projection()
    }

    pub fn toggle_commands(&mut self) -> AppProjection {
        self.commands_visible = !self.commands_visible;
        self.status_text = if self.commands_visible {
            "Quick Commands panel shown".to_owned()
        } else {
            "Quick Commands panel hidden".to_owned()
        };
        self.projection()
    }

    pub fn saved_session_profiles(&self) -> &[SessionProfile] {
        &self.config_document.sessions
    }

    pub fn emitted_events(&self) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        for session in self.sessions.values() {
            events.push(SessionEvent::StateChanged {
                session_id: session.session_id().clone(),
                state: session.state,
            });
        }
        events
    }

    fn startup_status(&self) -> String {
        let saved_count = self.config_document.sessions.len();
        match &self.recovered_from_backup {
            Some(path) => format!(
                "Recovered configuration from backup. Saved sessions: {}. Backup: {}",
                saved_count,
                path.display()
            ),
            None => format!(
                "Runtime initialized. Config: {}. Saved sessions discovered: {}.",
                self.config_dir.display(),
                saved_count
            ),
        }
    }

    fn select_transport_backend(&mut self, backend: TransportBackend) -> AppProjection {
        self.transport_backend = backend;
        self.status_text = match backend {
            TransportBackend::Fake => {
                "Transport backend set to `fake`. Quick Connect will use the deterministic shell adapter.".to_owned()
            }
            TransportBackend::Real => {
                "Transport backend set to `real`. Quick Connect will enter the staged real-transport scaffold until TCP/SSH wiring exists.".to_owned()
            }
        };
        self.projection()
    }

    fn active_session_label(&self) -> String {
        self.active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
            .map(|session| format!("{}  {}", session.display_name, session.state_label()))
            .unwrap_or_else(|| {
                if self.config_document.sessions.is_empty() {
                    "Welcome".to_owned()
                } else {
                    format!("Saved Sessions ({})", self.config_document.sessions.len())
                }
            })
    }

    fn session_summary_text(&self) -> String {
        if self.sessions.is_empty() {
            return "No runtime sessions yet.".to_owned();
        }

        let mut lines = self
            .sessions
            .values()
            .take(6)
            .map(SessionRuntime::sidebar_summary)
            .collect::<Vec<_>>();
        if self.sessions.len() > lines.len() {
            lines.push(format!("... and {} more", self.sessions.len() - lines.len()));
        }
        lines.join("\n")
    }

    fn tab_text(&self) -> String {
        self.active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
            .map(|session| format!("{}  {}", session.display_name, session.state_label()))
            .unwrap_or_else(|| "Welcome  idle".to_owned())
    }

    fn terminal_title_text(&self) -> String {
        self.active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
            .map(|session| format!("Terminal: {}", session.display_name))
            .unwrap_or_else(|| "YShell terminal".to_owned())
    }

    fn terminal_body_text(&self) -> String {
        self.active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
            .map(|session| {
                let visible = session.visible_text();
                if visible.trim().is_empty() {
                    "Terminal runtime is initialized but no output is available yet.".to_owned()
                } else {
                    visible
                }
            })
            .unwrap_or_else(|| {
                "Runtime is ready. Open a Quick Connect target or create a draft session.".to_owned()
            })
    }

    fn hydrate_saved_sessions(&mut self) {
        let profiles = self.config_document.sessions.clone();
        for (index, profile) in profiles.iter().enumerate() {
            let runtime = SessionRuntime::from_profile(profile, index + 1);
            self.sessions
                .insert(runtime.session_id().as_str().to_owned(), runtime);
        }
        self.next_runtime_ordinal = self
            .next_runtime_ordinal
            .max(self.config_document.sessions.len() + 1);
    }

    fn allocate_runtime_ordinal(&mut self) -> usize {
        let ordinal = self.next_runtime_ordinal;
        self.next_runtime_ordinal += 1;
        ordinal
    }

    fn active_session_key(&self) -> AppResult<String> {
        self.active_session_id
            .clone()
            .ok_or_else(|| AppError::new("no active runtime session"))
    }

    fn open_shell_for_runtime(
        &self,
        ssh_config: &yshell_ssh::SshConnectionConfig,
    ) -> Result<Box<dyn yshell_ssh::ShellSession>, yshell_ssh::SshError> {
        match self.transport_backend {
            TransportBackend::Fake => ShellClient::with_fake_backend().open_shell_boxed(ssh_config),
            TransportBackend::Real => ShellClient::with_real_backend().open_shell_boxed(ssh_config),
        }
    }

    fn activate_runtime_session(&mut self, mut runtime: SessionRuntime) -> AppResult<AppProjection> {
        let session_id = runtime.session_id().clone();
        let tab_id = runtime.tab_id().clone();
        let open_events = self
            .dispatcher
            .dispatch(SessionCommand::OpenSession {
                tab_id,
                session_id: session_id.clone(),
            })
            .map_err(AppError::from_error)?;
        Self::apply_session_events(&session_id, &mut runtime, open_events);
        let state_events = self
            .dispatcher
            .dispatch(SessionCommand::SetSessionState {
                session_id: session_id.clone(),
                state: runtime.state,
            })
            .map_err(AppError::from_error)?;
        Self::apply_session_events(&session_id, &mut runtime, state_events);
        runtime.append_status_line("Core session entry created. Opening shell runtime boundary.");
        match self.open_shell_for_runtime(&runtime.ssh_config) {
            Ok(shell_session) => {
                let shell_connected = shell_session.is_connected();
                runtime.attach_shell_session(shell_session);
                let _ = runtime.poll_shell_output().map_err(AppError::from_error)?;
                if shell_connected {
                    let connected_events = self
                        .dispatcher
                        .dispatch(SessionCommand::SetSessionState {
                            session_id: session_id.clone(),
                            state: yshell_core::SessionState::Connected,
                        })
                        .map_err(AppError::from_error)?;
                    Self::apply_session_events(&session_id, &mut runtime, connected_events);
                } else {
                    let connecting_events = self
                        .dispatcher
                        .dispatch(SessionCommand::SetSessionState {
                            session_id: session_id.clone(),
                            state: yshell_core::SessionState::Connecting,
                        })
                        .map_err(AppError::from_error)?;
                    Self::apply_session_events(&session_id, &mut runtime, connecting_events);
                    runtime.append_status_line(
                        "Shell runtime opened a staged backend scaffold. The connection has not reached a live shell yet.",
                    );
                }
            }
            Err(error) => {
                runtime.append_status_line(&format!(
                    "Shell runtime failed before real transport wiring: {error}"
                ));
                let failed_events = self
                    .dispatcher
                    .dispatch(SessionCommand::SetSessionState {
                        session_id: session_id.clone(),
                        state: yshell_core::SessionState::Failed,
                    })
                    .map_err(AppError::from_error)?;
                Self::apply_session_events(&session_id, &mut runtime, failed_events);
            }
        }

        let session_key = session_id.as_str().to_owned();
        self.active_session_id = Some(session_key.clone());
        self.status_text = format!(
            "Runtime session created for {} (user={}) using the `{}` shell backend. {}",
            runtime.host_label(),
            runtime.username_label(),
            self.transport_backend.label(),
            if runtime.state == yshell_core::SessionState::Connected {
                "The shell boundary is live."
            } else {
                "The backend scaffold is present, but real SSH transport is still being wired."
            }
        );
        self.sessions.insert(session_key, runtime);
        Ok(self.projection())
    }

    fn next_saved_profile_id(&self, host: &str) -> String {
        let mut stem = host
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_owned();
        if stem.is_empty() {
            stem = "session".to_owned();
        }
        let base = format!("saved-{stem}");
        if !self
            .config_document
            .sessions
            .iter()
            .any(|session| session.id == base)
        {
            return base;
        }
        let mut ordinal = 2usize;
        loop {
            let candidate = format!("{base}-{ordinal}");
            if !self
                .config_document
                .sessions
                .iter()
                .any(|session| session.id == candidate)
            {
                return candidate;
            }
            ordinal += 1;
        }
    }

    fn apply_session_events(
        expected_session_id: &yshell_core::SessionId,
        runtime: &mut SessionRuntime,
        events: Vec<SessionEvent>,
    ) {
        for event in events {
            match event {
                SessionEvent::TabOpened { session_id, .. } if &session_id == expected_session_id => {
                    runtime.append_status_line("Core tab opened for session.");
                }
                SessionEvent::StateChanged { session_id, state }
                    if &session_id == expected_session_id =>
                {
                    runtime.set_state(state);
                    runtime.append_status_line(&format!(
                        "Core state updated: {}",
                        runtime.state_label()
                    ));
                }
                SessionEvent::Connecting { session_id } if &session_id == expected_session_id => {
                    runtime.set_state(yshell_core::SessionState::Connecting);
                    runtime.append_status_line("Connection entering connecting state.");
                }
                SessionEvent::Connected { session_id } if &session_id == expected_session_id => {
                    runtime.set_state(yshell_core::SessionState::Connected);
                    runtime.append_status_line("Connection established.");
                }
                SessionEvent::Disconnected { session_id, reason }
                    if &session_id == expected_session_id =>
                {
                    runtime.set_state(yshell_core::SessionState::Disconnected);
                    runtime.append_status_line(&format!("Disconnected: {reason}"));
                }
                SessionEvent::Error { session_id, error } if &session_id == expected_session_id => {
                    runtime.set_state(yshell_core::SessionState::Failed);
                    runtime.append_status_line(&format!("Session error: {error}"));
                }
                SessionEvent::TerminalOutput { session_id, bytes }
                    if &session_id == expected_session_id =>
                {
                    runtime.terminal_parser.advance(&mut runtime.terminal_grid, &bytes);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use yshell_config::QuickConnectTarget;

    #[test]
    fn runtime_boots_with_startup_projection() {
        let temp = tempdir().expect("tempdir");
        let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let projection = runtime.projection();

        assert!(projection.status_text.contains("Saved sessions discovered"));
        assert_eq!(projection.active_session_text, "Welcome");
        assert!(projection.sftp_visible);
    }

    #[test]
    fn quick_connect_enters_runtime_pipeline() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let projection = runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        assert!(projection.active_session_text.contains("alice@example.com:2200"));
        assert!(projection.status_text.contains("The shell boundary is live"));
        assert!(projection.tab_text.contains("alice@example.com:2200"));
        assert!(projection.terminal_body_text.contains("Fake shell established"));
        assert_eq!(runtime.emitted_events().len(), 1);
    }

    #[test]
    fn hydrates_saved_sessions_from_config() {
        let temp = tempdir().expect("tempdir");
        let store = ConfigStore::new(temp.path());
        let mut document = ConfigDocument::default();
        document
            .sessions
            .push(QuickConnectTarget {
                username: Some("ops".to_owned()),
                host: "saved.example.test".to_owned(),
                port: 22,
            }
            .into_session_profile("saved-session-1"));
        store.save(&document).expect("save config");

        let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        assert_eq!(runtime.saved_session_profiles().len(), 1);
        assert!(runtime.projection().active_session_text.contains("Saved Sessions (1)"));
    }

    #[test]
    fn active_terminal_input_flows_through_shell_boundary() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let projection = runtime
            .send_active_terminal_input("pwd\n")
            .expect("send input");

        assert!(projection.status_text.contains("Sent"));
        assert!(projection.terminal_body_text.contains("fake-shell received input"));
    }

    #[test]
    fn active_terminal_resize_flows_through_shell_boundary() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let projection = runtime
            .resize_active_terminal(100, 40)
            .expect("resize terminal");

        assert!(projection.status_text.contains("100x40"));
        assert!(projection.terminal_body_text.contains("fake-shell resized"));
    }

    #[test]
    fn disconnect_and_reconnect_update_runtime_state() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let disconnected = runtime
            .disconnect_active_session()
            .expect("disconnect active session");
        assert!(disconnected.tab_text.contains("disconnected"));

        let reconnected = runtime
            .reconnect_active_session()
            .expect("reconnect active session");
        assert!(reconnected.tab_text.contains("connected"));
        assert!(reconnected.terminal_body_text.contains("Fake shell established"));
    }

    #[test]
    fn real_backend_enters_connecting_scaffold_instead_of_false_connected_state() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let projection = runtime.select_real_transport_backend();
        assert_eq!(projection.transport_backend_text, "real");

        let projection = runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        assert!(projection.tab_text.contains("connecting"));
        assert!(projection.status_text.contains("backend scaffold"));
        assert!(projection.terminal_body_text.contains("Real SSH backend selected"));
        assert!(projection.terminal_body_text.contains("tcp-connect"));
    }

    #[test]
    fn transport_backend_selection_is_runtime_visible() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let fake_projection = runtime.select_fake_transport_backend();
        assert_eq!(fake_projection.transport_backend_text, "fake");
        assert!(fake_projection.status_text.contains("deterministic shell adapter"));

        let real_projection = runtime.select_real_transport_backend();
        assert_eq!(real_projection.transport_backend_text, "real");
        assert!(real_projection.status_text.contains("staged real-transport scaffold"));
    }

    #[test]
    fn save_active_session_persists_minimal_profile() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("alice@example.com:2200")
            .expect("quick connect");

        let projection = runtime.save_active_session().expect("save active session");

        assert!(projection.status_text.contains("Saved active runtime session"));
        assert_eq!(runtime.saved_session_profiles().len(), 1);
        assert_eq!(runtime.saved_session_profiles()[0].host, "example.com");
        assert_eq!(runtime.saved_session_profiles()[0].port, 2200);
    }

    #[test]
    fn open_saved_session_reuses_saved_profile_path() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("ops@saved.example.test:2222")
            .expect("quick connect");
        runtime.save_active_session().expect("save active session");

        let profile_id = runtime.saved_session_profiles()[0].id.clone();
        let projection = runtime
            .open_saved_session(&profile_id)
            .expect("open saved session");

        assert!(projection.active_session_text.contains("saved.example.test"));
        assert!(projection.terminal_body_text.contains("Fake shell established"));
    }

    #[test]
    fn open_first_saved_session_uses_minimal_ui_path() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        runtime
            .handle_quick_connect("ops@saved.example.test:2222")
            .expect("quick connect");
        runtime.save_active_session().expect("save active session");

        let projection = runtime
            .open_first_saved_session()
            .expect("open first saved session");

        assert!(projection.active_session_text.contains("saved.example.test"));
        assert!(projection.tab_text.contains("connected"));
    }
}
