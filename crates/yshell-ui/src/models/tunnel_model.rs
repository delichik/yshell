//! Port-forwarding panel view model.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelItem {
    pub id: String,
    pub display_name: String,
    pub kind: TunnelKind,
    pub listen_address: String,
    pub target_address: String,
    pub state: String,
    pub is_running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelKind {
    Local,
    Remote,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelModel {
    pub tunnels: Vec<TunnelItem>,
    pub selected_id: Option<String>,
}

impl TunnelModel {
    pub fn placeholder() -> Self {
        Self {
            tunnels: Vec::new(),
            selected_id: None,
        }
    }
}
