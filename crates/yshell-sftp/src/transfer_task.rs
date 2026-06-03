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
    pub retry_count: u8,
    pub max_retries: u8,
    pub last_error: Option<String>,
}

impl TransferTask {
    pub fn new(
        id: impl Into<String>,
        direction: TransferDirection,
        source_path: impl Into<String>,
        destination_path: impl Into<String>,
        total_bytes: Option<u64>,
    ) -> Self {
        Self {
            id: id.into(),
            direction,
            source_path: source_path.into(),
            destination_path: destination_path.into(),
            bytes_done: 0,
            total_bytes,
            status: TransferStatus::Queued,
            retry_count: 0,
            max_retries: 3,
            last_error: None,
        }
    }

    pub fn progress_percent(&self) -> Option<u8> {
        let total = self.total_bytes?;
        if total == 0 {
            return Some(100);
        }
        Some(((self.bytes_done.saturating_mul(100) / total).min(100)) as u8)
    }
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
