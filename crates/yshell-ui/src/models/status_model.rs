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
            // D14：默认空串；"就绪"文案由启动投影写入（含 i18n）。
            message: String::new(),
            connection_state: "Disconnected".to_owned(),
            active_session_id: None,
            busy: false,
        }
    }
}
