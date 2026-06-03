//! Status-bar view model.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusModel {
    pub message: String,
    pub connection_state: String,
    pub active_session_id: Option<String>,
    pub busy: bool,
}

impl Default for StatusModel {
    fn default() -> Self {
        Self {
            message: "Ready".to_owned(),
            connection_state: "Disconnected".to_owned(),
            active_session_id: None,
            busy: false,
        }
    }
}
