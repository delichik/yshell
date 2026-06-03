//! Port-forwarding and tunnel configuration.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelConfig {
    pub id: String,
    pub kind: ForwardingKind,
    pub listen_host: String,
    pub listen_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardingKind {
    Local,
    Remote,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelState {
    Stopped,
    Starting,
    Running { bound_address: String },
    Failed { reason: String },
}
