//! SFTP adapter skeleton for YShell.

pub mod client;
pub mod error;
pub mod fs_entry;
pub mod remote_edit;
pub mod transfer_queue;
pub mod transfer_task;

pub use client::SftpClient;
pub use error::{SftpError, SftpResult};
pub use fs_entry::{FsEntry, FsEntryKind};
pub use transfer_task::{TransferDirection, TransferStatus, TransferTask};
