//! SFTP panel view model.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size_bytes: u64,
    pub permissions: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferItem {
    pub id: String,
    pub label: String,
    pub progress_percent: Option<u8>,
    pub status: String,
    pub can_retry: bool,
    pub can_cancel: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SftpModel {
    pub current_path: Option<String>,
    pub entries: Vec<SftpEntry>,
    pub transfers: Vec<TransferItem>,
    pub transfer_status: String,
    pub is_loading: bool,
}

impl SftpModel {
    pub fn placeholder() -> Self {
        Self {
            current_path: None,
            entries: Vec::new(),
            transfers: Vec::new(),
            transfer_status: "No active transfers".to_owned(),
            is_loading: false,
        }
    }
}
