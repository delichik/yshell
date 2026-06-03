//! SFTP adapter for YShell.

pub mod client;
pub mod error;
pub mod fs_entry;
pub mod remote_edit;
pub mod transfer_queue;
pub mod transfer_task;

pub use client::{FakeSftpBackend, SftpBackend, SftpClient};
pub use error::{SftpError, SftpErrorKind, SftpResult};
pub use fs_entry::{DirectoryListing, FsEntry, FsEntryKind};
pub use transfer_queue::TransferQueue;
pub use transfer_task::{TransferDirection, TransferStatus, TransferTask};
