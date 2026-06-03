//! Remote filesystem entry model.

use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEntry {
    pub path: String,
    pub name: String,
    pub kind: FsEntryKind,
    pub size_bytes: u64,
    pub permissions: u32,
    pub modified: Option<SystemTime>,
}

impl FsEntry {
    pub fn directory(path: impl Into<String>) -> Self {
        let path = path.into();
        let name = path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("/")
            .to_owned();
        Self {
            path,
            name,
            kind: FsEntryKind::Directory,
            size_bytes: 0,
            permissions: 0o755,
            modified: None,
        }
    }

    pub fn file(path: impl Into<String>, size_bytes: u64) -> Self {
        let path = path.into();
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        Self {
            path,
            name,
            kind: FsEntryKind::File,
            size_bytes,
            permissions: 0o644,
            modified: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryListing {
    pub path: String,
    pub entries: Vec<FsEntry>,
}
