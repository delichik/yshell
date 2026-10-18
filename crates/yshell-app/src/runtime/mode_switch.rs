//! In-place conversion between Terminal and SFTP tab modes.
//!
//! Both modes share one authenticated [`yshell_ssh::SshTransport`]. Converting
//! closes the old channel (shell or SFTP) but keeps the transport alive:
//!
//! * Terminal → SFTP: close the shell channel (the remote shell process ends),
//!   keep TCP + SSH auth, open the SFTP subsystem on the same transport.
//! * SFTP → Terminal: drop the SFTP channel, open a fresh shell/PTY channel on
//!   the same transport. The terminal screen is intentionally new.

use crate::error::{AppError, AppResult};
use crate::runtime::tabs::TabKind;
use crate::session_runtime::SessionViewMode;

use super::*;

impl AppRuntime {
    /// Terminal content/tab menu: convert the focused tab to SFTP.
    pub fn switch_active_tab_to_sftp(&mut self) -> AppResult<AppProjection> {
        let tab_id = self
            .active_tab_id
            .clone()
            .ok_or_else(|| AppError::new("no active tab to convert"))?;
        self.switch_tab_to_sftp(&tab_id)
    }

    /// SFTP content/tab menu: convert the focused tab to Terminal.
    pub fn switch_active_tab_to_terminal(&mut self) -> AppResult<AppProjection> {
        let tab_id = self
            .active_tab_id
            .clone()
            .ok_or_else(|| AppError::new("no active tab to convert"))?;
        self.switch_tab_to_terminal(&tab_id)
    }

    /// Whether a tab can switch to SFTP right now.
    pub(crate) fn tab_can_switch_to_sftp(&self, tab_id: &str) -> bool {
        let Some(index) = self.tab_index(tab_id) else {
            return false;
        };
        if self.tabs[index].is_sftp() {
            return false;
        }
        let Some(session_key) = self.tabs[index].session_id() else {
            return false;
        };
        if !self.session_has_active_connection(session_key) {
            return false;
        }
        self.sessions
            .get(session_key)
            .is_some_and(|runtime| !runtime.is_local() && runtime.transport.is_some())
    }

    /// Whether a tab can switch to Terminal right now.
    pub(crate) fn tab_can_switch_to_terminal(&self, tab_id: &str) -> bool {
        let Some(index) = self.tab_index(tab_id) else {
            return false;
        };
        if !self.tabs[index].is_sftp() {
            return false;
        }
        let Some(session_key) = self.tabs[index].session_id() else {
            return false;
        };
        self.session_has_active_connection(session_key)
    }

    /// Converts one terminal tab into an SFTP tab in place (same tab id and
    /// session id). Returns an error when the tab is not a connected terminal.
    pub fn switch_tab_to_sftp(&mut self, tab_id: &str) -> AppResult<AppProjection> {
        let index = self
            .tab_index(tab_id)
            .ok_or_else(|| AppError::new(format!("tab `{tab_id}` was not found")))?;
        if self.tabs[index].is_sftp() {
            return Err(AppError::new("tab is already an SFTP tab"));
        }
        let session_key = self.tabs[index]
            .session_id()
            .map(str::to_owned)
            .ok_or_else(|| AppError::new("New Tab pages cannot switch to SFTP"))?;
        if !self.session_has_active_connection(&session_key) {
            return Err(AppError::new(
                "connect the terminal before switching to SFTP",
            ));
        }
        // Production requires the shared transport: reconnecting instead would
        // not be "reuse the terminal's underlying connection".
        #[cfg(not(test))]
        if !self
            .sessions
            .get(&session_key)
            .is_some_and(|runtime| runtime.transport.is_some())
        {
            return Err(AppError::new(
                "this session has no reusable SSH transport; reconnect the terminal first",
            ));
        }

        // The terminal tab is being ended: stop input sync / logging first.
        self.detach_mode_from_terminal_support(tab_id, &session_key);

        // End the shell channel, keep the transport.
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        runtime
            .close_shell_channel()
            .map_err(AppError::from_error)?;
        runtime.set_view_mode(SessionViewMode::Sftp);
        runtime.log_runtime_event("Terminal session ended; reusing SSH transport for SFTP.");

        self.tabs[index].kind = TabKind::Sftp {
            session_id: session_key.clone(),
        };
        self.tabs[index].unread = 0;
        self.ensure_sftp_tab_state(tab_id);

        if self.active_tab_id.as_deref() == Some(tab_id) {
            self.restore_sftp_tab(tab_id);
            self.sftp_listing_pending = matches!(self.sftp_listing, SftpListingState::ConnectReal);
        }
        self.set_status_kind(
            "tab-switched-sftp",
            "Switched the tab to SFTP.".to_owned(),
            self.tab_display_title(tab_id),
            String::new(),
        );
        Ok(self.projection())
    }

    /// Converts one SFTP tab back into a terminal tab in place.
    pub fn switch_tab_to_terminal(&mut self, tab_id: &str) -> AppResult<AppProjection> {
        let index = self
            .tab_index(tab_id)
            .ok_or_else(|| AppError::new(format!("tab `{tab_id}` was not found")))?;
        if !self.tabs[index].is_sftp() {
            return Err(AppError::new("tab is not an SFTP tab"));
        }
        let session_key = self.tabs[index]
            .session_id()
            .map(str::to_owned)
            .ok_or_else(|| AppError::new("SFTP tab is missing its runtime session"))?;
        if !self.session_has_active_connection(&session_key) {
            return Err(AppError::new(
                "reconnect the SFTP session before switching to Terminal",
            ));
        }

        if self.active_tab_id.as_deref() == Some(tab_id) {
            self.park_active_sftp_tab();
        }
        // Close the SFTP subsystem channel before opening a shell channel.
        self.sftp_live.remove(&session_key);

        #[cfg(test)]
        {
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            runtime.set_view_mode(SessionViewMode::Terminal);
            runtime.clear_visible_terminal();
            self.tabs[index].kind = TabKind::Terminal {
                session_id: session_key.clone(),
            };
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.sftp_listing = SftpListingState::RefreshNeedsSession;
            self.set_status_kind(
                "tab-switched-terminal",
                "Switched the tab to Terminal.".to_owned(),
                self.tab_display_title(tab_id),
                String::new(),
            );
        }

        #[cfg(not(test))]
        {
            let (transport, pty) = {
                let runtime = self
                    .sessions
                    .get(&session_key)
                    .ok_or_else(|| AppError::new("active runtime session is missing"))?;
                let transport = runtime.transport.clone().ok_or_else(|| {
                    AppError::new("this SFTP session has no reusable SSH transport")
                })?;
                (transport, runtime.ssh_config.pty.clone())
            };
            let shell = match transport.open_shell(&pty) {
                Ok(shell) => shell,
                Err(error) => {
                    // Put the tab back into SFTP mode so the user keeps a usable
                    // page after a failed shell open.
                    if let Some(runtime) = self.sessions.get_mut(&session_key) {
                        runtime.set_view_mode(SessionViewMode::Sftp);
                    }
                    self.tabs[index].kind = TabKind::Sftp {
                        session_id: session_key.clone(),
                    };
                    if self.active_tab_id.as_deref() == Some(tab_id) {
                        let _ = self.restore_sftp_tab(tab_id);
                        self.sftp_listing_pending = true;
                    }
                    return Err(AppError::from_error(error));
                }
            };
            let runtime = self
                .sessions
                .get_mut(&session_key)
                .ok_or_else(|| AppError::new("active runtime session is missing"))?;
            runtime.set_view_mode(SessionViewMode::Terminal);
            runtime.clear_visible_terminal();
            runtime.attach_shell_session(shell);
            let _ = runtime.poll_shell_output().map_err(AppError::from_error)?;
            self.tabs[index].kind = TabKind::Terminal {
                session_id: session_key.clone(),
            };
            self.sftp_session = SftpSessionLifecycle::Disconnected { session_key: None };
            self.sftp_listing = SftpListingState::RefreshNeedsSession;
            self.set_status_kind(
                "tab-switched-terminal",
                "Switched the tab to Terminal.".to_owned(),
                self.tab_display_title(tab_id),
                String::new(),
            );
        }
        Ok(self.projection())
    }

    /// Stops terminal-only features (input sync targets, logging) before a
    /// terminal shell channel is closed.
    fn detach_mode_from_terminal_support(&mut self, tab_id: &str, session_key: &str) {
        if self.input_sync.source.as_deref() == Some(tab_id) {
            let _ = self.stop_input_sync_command();
        } else if self.input_sync.targets.remove(tab_id) {
            self.reconcile_input_sync();
        }
        let _ = self.stop_session_logging(session_key);
    }
}
