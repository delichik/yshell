//! MCP control lock, consent, and per-tab terminal I/O for the GUI bridge.

use std::sync::mpsc;

use yshell_core::{CommandDispatcher, SessionCommand, SessionState};
use yshell_mcp::{
    ConnectedSessionInfo, IpcErrorBody, IpcRequest, IpcResponse, IpcResult, OutputMode,
    SavedSessionInfo, MCP_BOUNDARIES_TEXT, MCP_HOST_CONFIG_SNIPPET, METHOD_CONNECT_SAVED,
    METHOD_DISCONNECT_SESSION, METHOD_DUPLICATE_SESSION, METHOD_GET_MCP_INFO, METHOD_GET_SESSION,
    METHOD_LIST_CONNECTED, METHOD_LIST_SAVED, METHOD_LIST_SESSIONS, METHOD_RELEASE_CONTROL,
    METHOD_REQUEST_CONTROL, METHOD_SESSION_INPUT, METHOD_SESSION_OUTPUT, METHOD_SESSION_SEND_KEY,
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
    Controlling { client_id: String },
}

impl Default for McpControlPhase {
    fn default() -> Self {
        Self::Idle
    }
}

/// Reply channel for a pending `request_control`.
pub type McpControlReply = mpsc::Sender<Result<(), String>>;

/// Listener status for Settings / projection (owned by runtime; updated by WindowManager).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum McpListenerStatus {
    #[default]
    Disabled,
    Ready { port: u16 },
    Failed { message: String },
}

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
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn controlling_client_id(&self) -> Option<&str> {
        match &self.phase {
            McpControlPhase::Controlling { client_id }
            | McpControlPhase::AwaitingConsent { client_id } => Some(client_id.as_str()),
            McpControlPhase::Idle => None,
        }
    }
}

impl AppRuntime {
    #[must_use]
    pub fn mcp_enabled(&self) -> bool {
        self.config_document.ui.mcp_enabled
    }

    /// Persist MCP enable flag. WindowManager reconciles the listener on the next tick.
    pub fn set_mcp_enabled(&mut self, enabled: bool) -> AppResult<AppProjection> {
        if self.config_document.ui.mcp_enabled != enabled {
            self.config_document.ui.mcp_enabled = enabled;
            self.config_store
                .save(&self.config_document)
                .map_err(AppError::from_error)?;
        }
        if !enabled {
            // Drop control immediately when turning MCP off.
            if self.mcp.is_controlling() || self.mcp.consent_visible() {
                if let Some(sender) = self.mcp.pending_consent.take() {
                    let _ = sender.send(Err("MCP disabled in Settings".to_owned()));
                }
                self.mcp.phase = McpControlPhase::Idle;
            }
            self.mcp_listener_status = McpListenerStatus::Disabled;
            self.mcp_endpoint_token = None;
            self.set_status_kind(
                "settings-mcp-disabled",
                "MCP is off. AI hosts cannot connect until you enable it.".to_owned(),
                String::new(),
                String::new(),
            );
        } else {
            // Clear Failed so WindowManager will attempt bind again this cycle.
            self.mcp_listener_status = McpListenerStatus::Disabled;
            self.set_status_kind(
                "settings-mcp-enabled",
                "MCP enabled. Starting local endpoint…".to_owned(),
                String::new(),
                String::new(),
            );
        }
        self.ui_dirty = true;
        Ok(self.projection())
    }

    pub fn set_mcp_listener_status(&mut self, status: McpListenerStatus) {
        if self.mcp_listener_status != status {
            self.mcp_listener_status = status;
            self.ui_dirty = true;
        }
    }

    #[must_use]
    pub fn mcp_status_text(&self) -> String {
        if self.mcp.is_controlling() {
            return "Controlling — AI holds control (Exit in the status bar)".to_owned();
        }
        if self.mcp.consent_visible() {
            return "Waiting for approval…".to_owned();
        }
        match &self.mcp_listener_status {
            McpListenerStatus::Disabled => {
                if self.mcp_enabled() {
                    "Starting…".to_owned()
                } else {
                    "Disabled".to_owned()
                }
            }
            McpListenerStatus::Ready { port } => {
                format!("Ready — listening on 127.0.0.1:{port}")
            }
            McpListenerStatus::Failed { message } => format!("Failed — {message}"),
        }
    }

    #[must_use]
    pub fn mcp_control_state_label(&self) -> &'static str {
        match &self.mcp.phase {
            McpControlPhase::Idle => "idle",
            McpControlPhase::AwaitingConsent { .. } => "awaiting_consent",
            McpControlPhase::Controlling { .. } => "controlling",
        }
    }

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
                    self.mcp.phase = McpControlPhase::Controlling { client_id };
                    if let Some(sender) = sender {
                        let _ = sender.send(Ok(()));
                    }
                    self.set_status_kind(
                        "mcp-control-granted",
                        "AI control is active. Exit anytime from the status bar.".to_owned(),
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
        if !self.mcp_enabled() {
            let _ = reply.send(Err("MCP is disabled in Settings → AI / MCP".to_owned()));
            return;
        }
        match &self.mcp.phase {
            McpControlPhase::Controlling {
                client_id: holding, ..
            } if holding == &client_id => {
                let _ = reply.send(Ok(()));
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
            self.mcp.phase = McpControlPhase::Controlling { client_id };
            self.ui_dirty = true;
            let _ = reply.send(Ok(()));
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
    /// `session_run` is handled on the IPC worker (wait loop).
    pub fn handle_mcp_ipc(&mut self, request: IpcRequest) -> IpcResponse {
        if !self.verify_mcp_endpoint_token(&request.token) {
            return IpcResponse::err(request.id, "unauthorized", "invalid MCP endpoint token");
        }
        if !self.mcp_enabled() {
            return IpcResponse::err(
                request.id,
                "mcp_disabled",
                "MCP is disabled in Settings → AI / MCP",
            );
        }
        match request.method.as_str() {
            METHOD_GET_MCP_INFO => IpcResponse::ok(
                request.id,
                IpcResult::McpInfo {
                    enabled: true,
                    status: self.mcp_status_text(),
                    control: self.mcp_control_state_label().to_owned(),
                    boundaries: MCP_BOUNDARIES_TEXT.to_owned(),
                    host_config: MCP_HOST_CONFIG_SNIPPET.to_owned(),
                },
            ),
            METHOD_LIST_SAVED => match self.mcp_list_saved_sessions() {
                Ok(sessions) => {
                    IpcResponse::ok(request.id, IpcResult::SavedSessions { sessions })
                }
                Err(error) => IpcResponse::err(request.id, "error", error.to_string()),
            },
            METHOD_LIST_CONNECTED | METHOD_LIST_SESSIONS => IpcResponse::ok(
                request.id,
                IpcResult::ConnectedSessions {
                    sessions: self.mcp_list_connected_sessions(),
                },
            ),
            METHOD_GET_SESSION => self.mcp_get_session_ipc(request),
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
            METHOD_SESSION_SEND_KEY => self.mcp_session_send_key_ipc(request),
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
            if let Some(info) = self.mcp_connected_info_for_tab(&tab.tab_id) {
                out.push(info);
            } else {
                let _ = session_id;
            }
        }
        out
    }

    fn mcp_connected_info_for_tab(&self, tab_id: &str) -> Option<ConnectedSessionInfo> {
        let tab = self.tabs.iter().find(|t| t.tab_id == tab_id)?;
        let session_id = tab.session_id()?.to_owned();
        if !matches!(tab.kind, tabs::TabKind::Terminal { .. }) {
            return None;
        }
        let session = self.sessions.get(&session_id)?;
        Some(ConnectedSessionInfo {
            tab_id: tab.tab_id.clone(),
            session_id: session_id.clone(),
            title: self.tab_display_title(&tab.tab_id),
            state: session_state_id(session.state).to_owned(),
            connected: session.state == SessionState::Connected,
            kind: tab.kind.mode_text().to_owned(),
            blocker: self.mcp_blocker_for_session(&session_id, session.state),
        })
    }

    fn mcp_blocker_for_session(&self, session_key: &str, state: SessionState) -> String {
        if let Some(prompt) = &self.pending_host_key_prompt {
            if prompt.session_key == session_key {
                return "host_key".to_owned();
            }
        }
        if let Some(prompt) = &self.pending_password_prompt {
            if self.session_matches_password_prompt(session_key, prompt) {
                return "password".to_owned();
            }
        }
        match state {
            SessionState::Connecting => "connecting".to_owned(),
            SessionState::Disconnected => "disconnected".to_owned(),
            SessionState::Failed => "failed".to_owned(),
            SessionState::Idle => "idle".to_owned(),
            SessionState::Connected => "none".to_owned(),
        }
    }

    fn session_matches_password_prompt(
        &self,
        session_key: &str,
        prompt: &connection::PendingPasswordPrompt,
    ) -> bool {
        if let Some(reuse) = prompt.reuse_tab_id.as_deref() {
            if let Some(tab) = self.tabs.iter().find(|t| t.tab_id == reuse) {
                if tab.session_id() == Some(session_key) {
                    return true;
                }
            }
        }
        self.sessions.get(session_key).is_some_and(|session| {
            session.ssh_config.host == prompt.host
                && session.ssh_config.port == prompt.port
                && session.ssh_config.username() == prompt.username
        })
    }

    fn mcp_get_session_ipc(&self, request: IpcRequest) -> IpcResponse {
        let tab_id = request
            .params
            .get("tab_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match self.mcp_connected_info_for_tab(tab_id) {
            Some(session) => IpcResponse::ok(request.id, IpcResult::SessionInfo { session }),
            None => IpcResponse::err(
                request.id,
                "not_found",
                format!("terminal tab `{tab_id}` was not found"),
            ),
        }
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

    fn mcp_session_send_key_ipc(&mut self, request: IpcRequest) -> IpcResponse {
        if let Err(error) = self.mcp_require_control(&request.client_id) {
            return IpcResponse::err(request.id, error.code, error.message);
        }
        let tab_id = request
            .params
            .get("tab_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let key = request
            .params
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match self.mcp_send_terminal_key(tab_id, key) {
            Ok(()) => IpcResponse::ok(request.id, IpcResult::Ok),
            Err(error) => IpcResponse::err(request.id, "key_failed", error.to_string()),
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
                let info = self.mcp_connected_info_for_tab(&tab_id);
                let session_id = info.as_ref().map(|s| s.session_id.clone());
                let state = info
                    .as_ref()
                    .map(|s| s.state.clone())
                    .unwrap_or_else(|| "unknown".to_owned());
                let blocker = info
                    .as_ref()
                    .map(|s| s.blocker.clone())
                    .unwrap_or_else(|| "none".to_owned());
                IpcResponse::ok(
                    request.id,
                    IpcResult::TabOpened {
                        tab_id,
                        session_id,
                        state,
                        blocker,
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
                let info = self.mcp_connected_info_for_tab(&new_tab);
                IpcResponse::ok(
                    request.id,
                    IpcResult::TabOpened {
                        tab_id: new_tab,
                        session_id: info.as_ref().map(|s| s.session_id.clone()),
                        state: info
                            .as_ref()
                            .map(|s| s.state.clone())
                            .unwrap_or_else(|| "unknown".to_owned()),
                        blocker: info
                            .as_ref()
                            .map(|s| s.blocker.clone())
                            .unwrap_or_else(|| "none".to_owned()),
                    },
                )
            }
            Err(error) => IpcResponse::err(request.id, "duplicate_failed", error.to_string()),
        }
    }

    /// Send bytes to a specific terminal tab without focus steal or N9 sync fan-out.
    pub fn mcp_send_terminal_input(
        &mut self,
        tab_id: &str,
        text: &str,
        append_newline: bool,
    ) -> AppResult<()> {
        if tab_id.is_empty() {
            return Err(AppError::new("tab_id is required"));
        }
        let mut payload = text.to_owned();
        if append_newline && !payload.ends_with('\n') {
            payload.push('\n');
        }
        self.mcp_write_terminal_bytes(tab_id, payload.as_bytes())
    }

    pub fn mcp_send_terminal_key(&mut self, tab_id: &str, key: &str) -> AppResult<()> {
        if tab_id.is_empty() {
            return Err(AppError::new("tab_id is required"));
        }
        let bytes = mcp_encode_key_spec(key)?;
        if bytes.is_empty() {
            return Err(AppError::new(format!("key `{key}` produced no input")));
        }
        self.mcp_write_terminal_bytes(tab_id, &bytes)
    }

    fn mcp_write_terminal_bytes(&mut self, tab_id: &str, bytes: &[u8]) -> AppResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let session_key = self
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .and_then(|tab| tab.session_id().map(str::to_owned))
            .ok_or_else(|| AppError::new(format!("tab `{tab_id}` was not found")))?;
        let session_id = self
            .sessions
            .get(&session_key)
            .map(|session| session.session_id().clone())
            .ok_or_else(|| AppError::new("runtime session is missing"))?;
        self.dispatcher
            .dispatch(SessionCommand::SendTerminalInput {
                session_id,
                bytes: bytes.to_vec(),
            })
            .map_err(AppError::from_error)?;
        let runtime = self
            .sessions
            .get_mut(&session_key)
            .ok_or_else(|| AppError::new("runtime session is missing"))?;
        let _ = runtime
            .write_terminal_input(bytes)
            .map_err(AppError::from_error)?;
        // Intentionally no activate_tab and no broadcast_synced_input.
        self.fold_logging_notice_from_session(&session_key);
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

fn mcp_encode_key_spec(spec: &str) -> AppResult<Vec<u8>> {
    let raw = spec.trim();
    if raw.is_empty() {
        return Err(AppError::new("key is required"));
    }
    let lower = raw.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err(AppError::new("key is required"));
    }
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let key = parts[parts.len() - 1];
    for modifier in &parts[..parts.len() - 1] {
        match *modifier {
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            other => {
                return Err(AppError::new(format!("unknown modifier `{other}`")));
            }
        }
    }
    let text = match key {
        "enter" | "return" => "\n",
        "tab" => "\t",
        "escape" | "esc" => "\u{001b}",
        "backspace" | "bs" => "\u{0008}",
        "delete" | "del" => "\u{007f}",
        "up" => "\u{F700}",
        "down" => "\u{F701}",
        "left" => "\u{F702}",
        "right" => "\u{F703}",
        "home" => "\u{F729}",
        "end" => "\u{F72B}",
        "pageup" | "page_up" => "\u{F72C}",
        "pagedown" | "page_down" => "\u{F72D}",
        other if other.chars().count() == 1 => other,
        other => {
            return Err(AppError::new(format!(
                "unsupported key `{other}`; use enter/tab/escape/arrows or ctrl+c"
            )));
        }
    };
    Ok(yshell_terminal::encode_key(text, ctrl, alt, shift, false))
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
    fn disabled_mcp_rejects_control() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        runtime.mcp_endpoint_token = Some("secret".into());
        assert!(!runtime.mcp_enabled());
        let (tx, rx) = mpsc::channel();
        runtime.mcp_begin_request_control("client-a".into(), tx);
        let err = rx.recv().expect("reply").expect_err("denied");
        assert!(err.contains("disabled"));
    }

    #[test]
    fn auto_approve_grants_control_without_dialog() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        runtime.mcp_endpoint_token = Some("secret".into());
        runtime.set_mcp_enabled(true).expect("enable");
        runtime
            .set_mcp_auto_approve_control(true)
            .expect("persist");
        let (tx, rx) = mpsc::channel();
        runtime.mcp_begin_request_control("client-a".into(), tx);
        rx.recv().expect("reply").expect("granted");
        assert!(runtime.mcp.is_controlling());
        assert_eq!(runtime.mcp.controlling_client_id(), Some("client-a"));
    }

    #[test]
    fn mutating_tools_require_control() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        runtime.mcp_endpoint_token = Some("secret".into());
        runtime.set_mcp_enabled(true).expect("enable");
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
        runtime.set_mcp_enabled(true).expect("enable");
        let _ = runtime.set_mcp_auto_approve_control(true);
        let (tx, rx) = mpsc::channel();
        runtime.mcp_begin_request_control("client-a".into(), tx);
        rx.recv().expect("reply").expect("granted");
        assert!(runtime.mcp.is_controlling());
        runtime.exit_mcp_control();
        assert!(!runtime.mcp.is_controlling());
    }

    #[test]
    fn list_saved_does_not_need_control() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        runtime.mcp_endpoint_token = Some("secret".into());
        runtime.set_mcp_enabled(true).expect("enable");
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
    fn get_mcp_info_returns_boundaries() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        runtime.mcp_endpoint_token = Some("secret".into());
        runtime.set_mcp_enabled(true).expect("enable");
        let response = runtime.handle_mcp_ipc(IpcRequest {
            id: 4,
            method: METHOD_GET_MCP_INFO.to_owned(),
            params: serde_json::json!({}),
            token: "secret".into(),
            client_id: "client-a".into(),
        });
        let Some(IpcResult::McpInfo {
            boundaries,
            host_config,
            ..
        }) = response.result
        else {
            panic!("expected mcp info");
        };
        assert!(boundaries.contains("Cannot"));
        assert!(host_config.contains("--mcp"));
    }

    #[test]
    fn connected_session_reports_password_blocker() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        let key = "sess-mcp-block".to_owned();
        let mut session = SessionRuntime::from_quick_connect(
            yshell_config::QuickConnectTarget {
                username: Some("alice".into()),
                host: "example.test".into(),
                port: 22,
            },
            1,
        );
        session.state = SessionState::Connecting;
        runtime.sessions.insert(key.clone(), session);
        runtime.tabs.push(tabs::TabEntry::terminal(
            "tab-mcp-block".into(),
            key.clone(),
        ));
        runtime.pending_password_prompt = Some(connection::PendingPasswordPrompt {
            profile_id: "p1".into(),
            host: "example.test".into(),
            port: 22,
            username: "alice".into(),
            auth_method: connection::PendingPasswordAuthMethod::Password,
            reuse_tab_id: Some("tab-mcp-block".into()),
            view_mode: crate::session_runtime::SessionViewMode::Terminal,
        });
        let info = runtime
            .mcp_connected_info_for_tab("tab-mcp-block")
            .expect("info");
        assert_eq!(info.blocker, "password");
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

    #[test]
    fn mcp_encode_ctrl_c() {
        let bytes = mcp_encode_key_spec("ctrl+c").expect("key");
        assert_eq!(bytes, b"\x03");
    }

    #[test]
    fn session_run_quiet_timeout_on_seeded_grid() {
        let dir = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(dir.path().to_path_buf()).expect("runtime");
        runtime.mcp_endpoint_token = Some("secret".into());
        runtime.set_mcp_enabled(true).expect("enable");
        runtime.set_mcp_auto_approve_control(true).expect("auto");
        let (tx, rx) = mpsc::channel();
        runtime.mcp_begin_request_control("client-a".into(), tx);
        rx.recv().expect("reply").expect("granted");

        let key = "sess-run".to_owned();
        let mut session = SessionRuntime::from_quick_connect(
            yshell_config::QuickConnectTarget {
                username: Some("u".into()),
                host: "example.test".into(),
                port: 22,
            },
            1,
        );
        session.state = SessionState::Connected;
        session.terminal_grid.resize(40, 2);
        for (col, ch) in "ready".chars().enumerate() {
            if let Some(cell) = session.terminal_grid.cells.get_mut(col) {
                cell.grapheme = ch.to_string();
            }
        }
        runtime.sessions.insert(key.clone(), session);
        runtime
            .tabs
            .push(tabs::TabEntry::terminal("tab-run".into(), key));

        let out = runtime
            .mcp_read_terminal_output("tab-run", OutputMode::Visible, None)
            .expect("visible");
        assert!(out.contains("ready"));
        let info = runtime.mcp_connected_info_for_tab("tab-run").expect("info");
        assert_eq!(info.blocker, "none");
        assert_eq!(info.state, "connected");
    }
}
