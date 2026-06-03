//! Port-forwarding placeholders.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardingKind {
    Local,
    Remote,
    Dynamic,
}
