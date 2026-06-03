//! SFTP panel view model.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SftpModel {
    pub current_path: Option<String>,
    pub entries: Vec<SftpEntry>,
    pub transfer_status: String,
}

impl SftpModel {
    pub fn placeholder() -> Self {
        Self {
            current_path: None,
            entries: Vec::new(),
            transfer_status: "No active transfers".to_owned(),
        }
    }
}
