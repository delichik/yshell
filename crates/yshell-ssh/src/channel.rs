//! SSH channel and interactive shell session abstractions.

use std::fmt::Debug;

use crate::{pty::PtySize, SshResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Shell,
    Sftp,
    DirectTcpIp,
}

/// Object-safe boundary for an interactive shell session used by the app runtime.
pub trait ShellSession: Debug {
    /// Returns the kind of channel behind this session.
    fn kind(&self) -> ChannelKind {
        ChannelKind::Shell
    }

    /// Returns true when the session is still connected.
    fn is_connected(&self) -> bool;

    /// Polls currently available output bytes from the session.
    fn poll_output(&mut self) -> SshResult<Vec<u8>>;

    /// Writes terminal input bytes into the session.
    fn write_input(&mut self, bytes: &[u8]) -> SshResult<()>;

    /// Resizes the remote PTY.
    fn resize_pty(&mut self, size: PtySize) -> SshResult<()>;

    /// Disconnects the channel.
    fn disconnect(&mut self) -> SshResult<()>;
}
