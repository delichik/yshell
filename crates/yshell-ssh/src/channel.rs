//! SSH channel placeholders.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Shell,
    Sftp,
    DirectTcpIp,
}
