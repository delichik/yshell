//! Port-forwarding panel view model.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelItem {
    pub id: String,
    pub display_name: String,
    pub listen_address: String,
    pub target_address: String,
    pub is_running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelModel {
    pub tunnels: Vec<TunnelItem>,
}

impl TunnelModel {
    pub fn placeholder() -> Self {
        Self {
            tunnels: Vec::new(),
        }
    }
}
