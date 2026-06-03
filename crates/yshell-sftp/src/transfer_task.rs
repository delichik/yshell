//! Transfer task model.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferTask {
    pub id: String,
    pub direction: TransferDirection,
    pub source_path: String,
    pub destination_path: String,
    pub bytes_done: u64,
    pub total_bytes: Option<u64>,
    pub status: TransferStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}
