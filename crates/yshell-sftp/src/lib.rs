//! SFTP adapter for YShell.
//!
//! Beyond single-file operations the [`SftpBackend`] trait offers recursive
//! [`SftpBackend::upload_tree`] / [`SftpBackend::download_tree`] transfers with
//! overwrite policies, progress callbacks and cancellation, plus the batch
//! helpers [`SftpBackend::delete_many`] / [`SftpBackend::chmod_many`]. The
//! fake and real backends share the same tree engine, so behaviour is aligned
//! by construction.

pub mod client;
pub mod error;
pub mod fs_entry;
pub mod real;
pub mod remote_edit;
pub mod transfer_queue;
pub mod transfer_task;
pub mod tree;

pub use client::{FakeSftpBackend, SftpBackend, SftpClient};
pub use error::{SftpError, SftpErrorKind, SftpResult};
pub use fs_entry::{DirectoryListing, FsEntry, FsEntryKind};
pub use real::RealSftpBackend;
pub use remote_edit::{prepare_remote_edit_session, RemoteEditSession};
pub use transfer_queue::TransferQueue;
pub use transfer_task::{TransferDirection, TransferStatus, TransferTask};
pub use tree::{
    BatchOpResult, OverwritePolicy, TransferItemKind, TransferProgress, TreeTransferOptions,
    TreeTransferReport,
};
