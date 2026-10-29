//! MCP control lock, consent, and per-tab terminal I/O for the GUI bridge.

use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use yshell_core::SessionState;
use yshell_mcp::{
    ConnectedSessionInfo, IpcErrorBody, IpcRequest, IpcResponse, IpcResult, OutputMode,
    SavedSessionInfo, METHOD_CONNECT_SAVED, METHOD_DISCONNECT_SESSION, METHOD_DUPLICATE_SESSION,
    METHOD_LIST_CONNECTED, METHOD_LIST_SAVED, METHOD_RELEASE_CONTROL, METHOD_REQUEST_CONTROL,
    METHOD_SESSION_INPUT, METHOD_SESSION_OUTPUT,
};

use crate::error::{AppError, AppResult};
use crate::session_runtime::SessionRuntime;

use super::*;

/// Decision returned from the MCP consent dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConsentDecision {
    Accept { dont_ask_again: bool },
    Reject,
}

/// Phase of the single global MCP control lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpControlPhase {
    Idle,
    AwaitingConsent { client_id: String },
    Controlling { client_id: String, token: String },
}

impl Default for McpControlPhase {
    fn default() -> Self {
        Self::Idle
    }
}

/// Reply channel for a pending `request_control` (Ok = control token).
pub type McpControlReply = mpsc::Sender<Result<String, String>>;

/// In-memory MCP control state owned by [`AppRuntime`].
#[derive(Debug, Default)]
pub struct McpControlState {
    pub phase: McpControlPhase,
    /// Completes the blocked IPC worker waiting on `request_control`.
    pub(crate) pending_consent: Option<McpControlReply>,
}

impl McpControlState {
    #[must_use]
    pub fn is_controlling(&self) -> bool {
        matches!(self.phase, McpControlPhase::Controlling { .. })
    }

    #[must_use]
    pub fn consent_visible(&self) -> bool {
        matches!(self.phase, McpControlPhase::AwaitingConsent { .. })
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn control_token(&self) -> Option<&str> {
        match &self.phase {
            McpControlPhase::Controlling { token, .. } => Some(token.as_str()),
            _ => None,
        }
    }

    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn controlling_client_id(&self) -> Option<&str> {
        match &self.phase {
            McpControlPhase::Controlling { client_id, .. }
            | McpControlPhase::AwaitingConsent { client_id } => Some(client_id.as_str()),
            McpControlPhase::Idle => None,
        }
    }
}

impl AppRuntime {
    /// Settings / dialog preference: skip the MCP consent dialog.
    #[must_use]
    pub fn mcp_auto_approve_control_enabled(&self) -> bool {
        self.config_document.ui.mcp_auto_approve_control
    }

    pub fn set_mcp_auto_approve_control(&mut self, enabled: bool) -> AppResult<AppProjection> {
        if self.config_document.ui.mcp_auto_approve_control != enabled {
            self.config_document.ui.mcp_auto_approve_control = enabled;
            self.config_store
                .save(&self.config_document)
                .map_err(AppError::from_error)?;
        }
        if enabled {
            self.set_status_kind(
                "settings-mcp-auto-approve-enabled",
                "MCP control requests will be approved automatically.".to_owned(),
                String::new(),
                String::new(),
            );
        } else {
            self.set_status_kind(
                "settings-mcp-auto-approve-disabled",
                "MCP control requests will ask for confirmation.".to_owned(),
                String::new(),
                String::new(),
            );
        }
        Ok(self.projection())
    }

    /// User clicked Exit on the "AI controlling" chip.
    pub fn exit_mcp_control(&mut self) -> AppProjection {
        if let McpControlPhase::AwaitingConsent { .. } = &self.mcp.phase {
            self.resolve_mcp_consent(McpConsentDecision::Reject);
            return self.projection();
        }
        if self.mcp.is_controlling() {
            self.mcp.phase = McpControlPhase::Idle;
            self.mcp.pending_consent = None;
            self.set_status_kind(
                "mcp-control-exited",
                "AI control ended. The agent must request control again.".to_owned(),
                String::new(),
                String::new(),
            );
            self.ui_dirty = true;
        }
        self.projection()
    }

    /// Complete a pending consent dialog (or reject a stale one).
    pub fn resolve_mcp_consent(&mut self, decision: McpConsentDecision) {
        let sender = self.mcp.pending_consent.take();
        match decision {
            McpConsentDecision::Accept { dont_ask_again } => {
                if dont_ask_again {
                    self.config_document.ui.mcp_auto_approve_control = true;
                    let _ = self.config_store.save(&self.config_document);
                }
                if let McpControlPhase::AwaitingConsent { client_id } = &self.mcp.phase {
                    let client_id = client_id.clone();
                    let token = new_control_token();
                    self.mcp.phase = McpControlPhase::Controlling {
                        client_id,
                        token: token.clone(),
                    };
                    if let Some(sender) = sender {
                        let _ = sender.send(Ok(token));
                    }
                    self.set_status_kind(
                        "mcp-control-granted",
                        "AI control is active.".to_owned(),
                        String::new(),
                        String::new(),
                    );
                } else if let Some(sender) = sender {
                    let _ = sender.send(Err("control request was cancelled".to_owned()));
                }
            }
            McpConsentDecision::Reject => {
                self.mcp.phase = McpControlPhase::Idle;
                if let Some(sender) = sender {
                    let _ = sender.send(Err("user denied AI control".to_owned()));
                }
                self.set_status_kind(
                    "mcp-control-denied",
                    "AI control request was denied.".to_owned(),
                    String::new(),
                    String::new(),
                );
            }
        }
        self.ui_dirty = true;
    }

    /// Drop control when the MCP bridge for `client_id` disconnects.
    pub fn release_mcp_control_for_client(&mut self, client_id: &str) {
        match &self.mcp.phase {
            McpControlPhase::Controlling {
                client_id: holding, ..
            } if holding == client_id => {
                self.mcp.phase = McpControlPhase::Idle;
                self.ui_dirty = true;
            }
            McpControlPhase::AwaitingConsent {
                client_id: waiting, ..
            } if waiting == client_id => {
                self.resolve_mcp_consent(McpConsentDecision::Reject);
            }
            _ => {}
        }
    }

    /// Start `request_control` on the UI thread. Never blocks: either replies
    /// immediately on `reply` or stores it for [`Self::resolve_mcp_consent`].
    pub fn mcp_begin_request_control(&mut self, client_id: String, reply: McpControlReply) {
        match &self.mcp.phase {
            McpControlPhase::Controlling {
                client_id: holding,
                token,
            } if holding == &client_id => {
                let _ = reply.send(Ok(token.clone()));
                return;
            }
            McpControlPhase::Controlling { .. } => {
                let _ = reply.send(Err(
                    "another client already holds AI control".to_owned()
                ));
                return;
            }
            McpControlPhase::AwaitingConsent { client_id: waiting }
                if waiting != &client_id =>
            {
                let _ = reply.send(Err(
                    "another control request is waiting for the user".to_owned()
                ));
                return;
            }
            _ => {}
        }

        if self.mcp_auto_approve_control_enabled() {
            let token = new_control_token();
            self.mcp.phase = McpControlPhase::Controlling {
                client_id,
                token: token.clone(),
            };
            self.ui_dirty = true;
            let _ = reply.send(Ok(token));
            return;
        }

        self.mcp.pending_consent = Some(reply);
        self.mcp.phase = McpControlPhase::AwaitingConsent { client_id };
        self.ui_dirty = true;
    }

    /// Dispatch one non-blocking IPC request on the UI thread.
    ///
    /// `request_control` must use [`Self::mcp_begin_request_control`] from the
    /// IPC worker instead — blocking here would freeze the consent dialog.
    pub fn handle_mcp_ipc(&mut self, request: IpcRequest) -> IpcResponse {
        if !self.verify_mcp_endpoint_token(&request.token) {
            return IpcResponse::err(request.id, "unauthorized", "invalid MCP endpoint token");
        }
        match request.method.as_str() {
            METHOD_LIST_SAVED => match self.mcp_list_saved_sessions() {
                Ok(sessions) => {
                    IpcResponse::ok(request.id, IpcResult::SavedSessions { sessions })
                }
                Err(error) => IpcResponse::err(request.id, "error", error.to_string()),
            },
            METHOD_LIST_CONNECTED => IpcResponse::ok(
                request.id,
                IpcResult::ConnectedSessions {
                    sessions: self.mcp_list_connected_sessions(),
                },
            ),
            METHOD_REQUEST_CONTROL => IpcResponse::err(
                request.id,
                "internal",
                "request_control must use the async consent path",
            ),
            METHOD_RELEASE_CONTROL => {
                if let Err(error) = self.mcp_require_control(&request.client_id) {
                    return IpcResponse::err(request.id, error.code, error.message);
                }
                self.mcp.phase = McpControlPhase::Idle;
                self.ui_dirty = true;
                IpcResponse::ok(request.id, IpcResult::Ok)
            }
            METHOD_SESSION_INPUT => self.mcp_session_input_ipc(request),
            METHOD_SESSION_OUTPUT => self.mcp_session_output_ipc(request),
            METHOD_CONNECT_SAVED => self.mcp_connect_saved_ipc(request),
            METHOD_DISCONNECT_SESSION => self.mcp_disconnect_ipc(request),
            METHOD_DUPLICATE_SESSION => self.mcp_duplicate_ipc(request),
            other => IpcResponse::err(
                request.id,
                "method_not_found",
                format!("unknown method `{other}`"),
            ),
        }
    }

    fn verify_mcp_endpoint_token(&self, token: &str) -> bool {
        self.mcp_endpoint_token
            .as_deref()
            .is_some_and(|expected| expected == token)
    }

    fn mcp_require_control(&self, client_id: &str) -> Result<(), IpcErrorBody> {
        match &self.mcp.phase {
            McpControlPhase::Controlling {
                client_id: holding, ..
            } if holding == client_id => Ok(()),
            McpControlPhase::Controlling { .. } => Err(IpcErrorBody {
                code: "control_held".into(),
                message: "another client holds AI control".into(),
            }),
            _ => Err(IpcErrorBody {
                code: "control_required".into(),
                message: "call request_control and wait for user approval first".into(),
            }),
        }
    }

    pub fn mcp_list_saved_sessions(&self) -> AppResult<Vec<SavedSessionInfo>> {
        let mut out = Vec::new();
        for session in self.saved_session_profiles() {
            let folder = find_session_folder_id(&self.config_document.folders, &session.id)
                .and_then(|folder_id| {
                    self.config_document
                        .folders
                        .iter()
                        .find_map(|folder| folder_name_by_id(folder, &folder_id))
                })
                .unwrap_or_default();
            let (host, port, username) = session_endpoint_parts(&session);
            out.push(SavedSessionInfo {
                id: session.id,
                name: session.name,
                host,
                port,
                username,
                folder,
            });
        }
        Ok(out)
    }

    pub fn mcp_list_connected_sessions(&self) -> Vec<ConnectedSessionInfo> {
        let mut out = Vec::new();
        for tab in &self.tabs {
            let Some(session_id) = tab.session_id() else {
                continue;
            };
            if !matches!(tab.kind, tabs::TabKind::Terminal { .. }) {
                continue;
            }
            let Some(session) = self.sessions.get(session_id) else {
                continue;
            };
            out.push(ConnectedSessionInfo {
                tab_id: tab.tab_id.clone(),
                session_id: session_id.to_owned(),
                title: self.tab_display_title(&tab.tab_id),
                state: session_state_id(session.state).to_owned(),
                connected: session.state == SessionState::Connected,
                kind: tab.kind.mode_text().to_owned(),
            });
        }
        out
    }

    fn mcp_session_input_ipc(&mut self, request: IpcRequest) -> IpcResponse {
        if let Err(error) = self.mcp_require_control(&request.client_id) {
            return IpcResponse::err(request.id, error.code, error.message);
        }
        let tab_id = request
            .params
            .get("tab_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let text = request
            .params
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let append_newline = request
            .params
            .get("append_newline")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        match self.mcp_send_terminal_input(tab_id, text, append_newline) {
            Ok(()) => IpcResponse::ok(request.id, IpcResult::Ok),
            Err(error) => IpcResponse::err(request.id, "input_failed", error.to_string()),
        }
    }

    fn mcp_session_output_ipc(&mut self, request: IpcRequest) -> IpcResponse {
        if let Err(error) = self.mcp_require_control(&request.client_id) {
            return IpcResponse::err(request.id, error.code, error.message);
        }
        let tab_id = request
            .params
            .get("tab_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let mode_raw = request
            .params
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let Some(mode) = OutputMode::parse(mode_raw) else {
            return IpcResponse::err(
                request.id,
                "invalid_mode",
                format!("unknown mode `{mode_raw}`"),
            );
        };
        let n = request
            .params
            .get("n")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        match self.mcp_read_terminal_output(tab_id, mode, n) {
            Ok(text) => IpcResponse::ok(
                request.id,
                IpcResult::SessionOutput {
                    text,
                    mode: mode.as_str().to_owned(),
                },
            ),
            Err(error) => IpcResponse::err(request.id, "output_failed", error.to_string()),
        }
    }

    fn mcp_connect_saved_ipc(&mut self, request: IpcRequest) -> IpcResponse {
        if let Err(error) = self.mcp_require_control(&request.client_id) {
            return IpcResponse::err(request.id, error.code, error.message);
        }
        let profile_id = request
            .params
            .get("profile_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let before: Vec<_> = self.tabs.iter().map(|t| t.tab_id.clone()).collect();
        match self.open_saved_session(profile_id) {
            Ok(_) => {
                let tab_id = self
                    .tabs
                    .iter()
                    .map(|t| t.tab_id.clone())
                    .find(|id| !before.contains(id))
                    .or_else(|| self.active_tab_id.clone())
                    .unwrap_or_default();
                let session_id = self
                    .tabs
                    .iter()
                    .find(|t| t.tab_id == tab_id)
                    .and_then(|t| t.session_id().map(str::to_owned));
                IpcResponse::ok(
                    request.id,
                    IpcResult::TabOpened {
                        tab_id,
                        session_id,
                    },
                )
            }
            Err(error) => IpcResponse::err(request.id, "connect_failed", error.to_string()),
        }
    }

    fn mcp_disconnect_ipc(&mut self, request: IpcRequest) -> IpcResponse {
        if let Err(error) = self.mcp_require_control(&request.client_id) {
            return IpcResponse::err(request.id, error.code, error.message);
        }
        let tab_id = request
            .params
            .get("tab_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match self.disconnect_tab_session(tab_id) {
            Ok(_) => IpcResponse::ok(request.id, IpcResult::Ok),
            Err(error) => IpcResponse::err(request.id, "disconnect_failed", error.to_string()),
        }
    }

    fn mcp_duplicate_ipc(&mut self, request: IpcRequest) -> IpcResponse {
        if let Err(error) = self.mcp_require_control(&request.client_id) {
            return IpcResponse::err(request.id, error.code, error.message);
        }
        let tab_id = request
            .params
            .get("tab_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let before: Vec<_> = self.tabs.iter().map(|t| t.tab_id.clone()).collect();
        match self.duplicate_tab(tab_id) {
            Ok(_) => {
                let new_tab = self
                    .tabs
                    .iter()
                    .map(|t| t.tab_id.clone())
                    .find(|id| !before.contains(id))
                    .or_else(|| self.active_tab_id.clone())
                    .unwrap_or_default();
                let session_id = self
                    .tabs
                    .iter()
                    .find(|t| t.tab_id == new_tab)
                    .and_then(|t| t.session_id().map(str::to_owned));
                IpcResponse::ok(
                    request.id,
                    IpcResult::TabOpened {
                        tab_id: new_tab,
                        session_id,
                    },
                )
            }
            Err(error) => IpcResponse::err(request.id, "duplicate_failed", error.to_string()),
        }
    }

    /// Send bytes to a specific terminal tab (activates it first).
    pub fn mcp_send_terminal_input(
        &mut self,
        tab_id: &str,
        text: &str,
        append_newline: bool,
    ) -> AppResult<()> {
        if tab_id.is_empty() {
            return Err(AppError::new("tab_id is required"));
        }
        self.activate_tab(tab_id)?;
        let mut payload = text.to_owned();
        if append_newline && !payload.ends_with('\n') {
            payload.push('\n');
        }
        self.send_active_terminal_input(&payload)?;
        Ok(())
    }

    /// Read terminal text for a tab using the requested output mode.
    pub fn mcp_read_terminal_output(
        &mut self,
        tab_id: &str,
        mode: OutputMode,
        n: Option<usize>,
    ) -> AppResult<String> {
        if tab_id.is_empty() {
            return Err(AppError::new("tab_id is required"));
        }
        let session_key = self
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .and_then(|tab| tab.session_id().map(str::to_owned))
            .ok_or_else(|| AppError::new(format!("tab `{tab_id}` was not found")))?;
        // Drain PTY before reading so since_input / last_n see fresh output.
        if let Some(session) = self.sessions.get_mut(&session_key) {
            let _ = session.poll_shell_output();
        }
        let session = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("runtime session is missing"))?;
        Ok(session.mcp_output_text(mode, n))
    }
}

impl SessionRuntime {
    /// Absolute line index of the cursor row (scrollback + screen).
    #[must_use]
    pub fn absolute_cursor_line(&self) -> usize {
        self.terminal_grid
            .scrollback_len()
            .saturating_add(usize::from(self.terminal_grid.cursor_row))
    }

    /// Mark the watermark used by [`OutputMode::SinceInput`].
    pub fn mark_mcp_input_watermark(&mut self) {
        self.mcp_output_watermark = Some(self.absolute_cursor_line());
    }

    #[must_use]
    pub fn mcp_output_text(&self, mode: OutputMode, n: Option<usize>) -> String {
        match mode {
            OutputMode::Visible => self.visible_text(),
            OutputMode::LastLine => {
                let total = self.terminal_grid.total_lines();
                if total == 0 {
                    String::new()
                } else {
                    self.terminal_grid
                        .line_text_absolute(total - 1)
                        .unwrap_or_default()
                }
            }
            OutputMode::LastN => {
                let count = n.unwrap_or(20).max(1);
                let total = self.terminal_grid.total_lines();
                let start = total.saturating_sub(count);
                (start..total)
                    .filter_map(|idx| self.terminal_grid.line_text_absolute(idx))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            OutputMode::SinceInput => {
                let total = self.terminal_grid.total_lines();
                let start = self.mcp_output_watermark.unwrap_or(0).min(total);
                (start..total)
                    .filter_map(|idx| self.terminal_grid.line_text_absolute(idx))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
    }
}

fn new_control_token() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("ctl-{millis}")
}

fn session_state_id(state: SessionState) -> &'static str {
    match state {
        SessionState::Idle => "idle",
        SessionState::Connecting => "connecting",
        SessionState::Connected => "connected",
        SessionState::Disconnected => "disconnected",
        SessionState::Failed => "failed",
    }
}

fn session_endpoint_parts(session: &yshell_config::SessionProfile) -> (String, u16, String) {
    (
        session.host.clone(),
        session.port,
        session.username.clone().unwrap_or_default(),
    )
}

fn folder_name_by_id(folder: &yshell_config::FolderProfile, id: &str) -> Option<String> {
    if folder.id == id {
        return Some(folder.name.clone());
    }
    folder
        .folders
        .iter()
        .find_map(|child| folder_name_by_id(child, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn auto_approve_grants_control_without_dialog() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        runtime.mcp_endpoint_token = Some("secret".into());
        runtime
            .set_mcp_auto_approve_control(true)
            .expect("persist");
        let (tx, rx) = mpsc::channel();
        runtime.mcp_begin_request_control("client-a".into(), tx);
        let token = rx.recv().expect("reply").expect("granted");
        assert!(!token.is_empty());
        assert!(runtime.mcp.is_controlling());
        assert_eq!(runtime.mcp.controlling_client_id(), Some("client-a"));
    }

    #[test]
    fn mutating_tools_require_control() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        runtime.mcp_endpoint_token = Some("secret".into());
        let response = runtime.handle_mcp_ipc(IpcRequest {
            id: 2,
            method: METHOD_SESSION_INPUT.to_owned(),
            params: serde_json::json!({ "tab_id": "missing", "text": "pwd" }),
            token: "secret".into(),
            client_id: "client-a".into(),
        });
        let error = response.error.expect("error");
        assert_eq!(error.code, "control_required");
    }

    #[test]
    fn exit_control_clears_lock() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        runtime.mcp_endpoint_token = Some("secret".into());
        let _ = runtime.set_mcp_auto_approve_control(true);
        let (tx, rx) = mpsc::channel();
        runtime.mcp_begin_request_control("client-a".into(), tx);
        let _ = rx.recv().expect("reply").expect("granted");
        assert!(runtime.mcp.is_controlling());
        runtime.exit_mcp_control();
        assert!(!runtime.mcp.is_controlling());
    }

    #[test]
    fn list_saved_does_not_need_control() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        runtime.mcp_endpoint_token = Some("secret".into());
        let response = runtime.handle_mcp_ipc(IpcRequest {
            id: 3,
            method: METHOD_LIST_SAVED.to_owned(),
            params: serde_json::json!({}),
            token: "secret".into(),
            client_id: "client-a".into(),
        });
        assert!(response.error.is_none());
        assert!(matches!(
            response.result,
            Some(IpcResult::SavedSessions { .. })
        ));
    }

    #[test]
    fn output_modes_read_grid_lines() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        let key = "sess-mcp-out".to_owned();
        let mut session = SessionRuntime::from_quick_connect(
            yshell_config::QuickConnectTarget {
                username: Some("u".into()),
                host: "example.test".into(),
                port: 22,
            },
            1,
        );
        // Seed a few absolute lines via the public grid.
        session.terminal_grid.resize(40, 4);
        for (row, text) in ["one", "two", "three", "four"].into_iter().enumerate() {
            for (col, ch) in text.chars().enumerate() {
                if let Some(cell) = session.terminal_grid.cells.get_mut(row * 40 + col) {
                    cell.grapheme = ch.to_string();
                }
            }
        }
        session.mcp_output_watermark = Some(1);
        runtime.sessions.insert(key.clone(), session);
        runtime.tabs.push(tabs::TabEntry::terminal(
            "tab-mcp-out".into(),
            key.clone(),
        ));
        let last = runtime
            .mcp_read_terminal_output("tab-mcp-out", OutputMode::LastLine, None)
            .expect("last");
        assert_eq!(last, "four");
        let last_n = runtime
            .mcp_read_terminal_output("tab-mcp-out", OutputMode::LastN, Some(2))
            .expect("last_n");
        assert!(last_n.contains("three"));
        assert!(last_n.contains("four"));
        let since = runtime
            .mcp_read_terminal_output("tab-mcp-out", OutputMode::SinceInput, None)
            .expect("since");
        assert!(since.contains("two"));
        assert!(!since.starts_with("one"));
    }
}
