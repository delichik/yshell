//! SFTP client lifecycle and backend adapters.

use std::collections::BTreeMap;

use crate::error::{SftpError, SftpErrorKind, SftpResult};
use crate::fs_entry::{DirectoryListing, FsEntry, FsEntryKind};

pub trait SftpBackend {
    fn list_dir(&self, path: &str) -> SftpResult<DirectoryListing>;
    fn chmod(&mut self, path: &str, permissions: u32) -> SftpResult<()>;
    fn delete(&mut self, path: &str) -> SftpResult<()>;
    fn rename(&mut self, from: &str, to: &str) -> SftpResult<()>;
    fn mkdir(&mut self, path: &str) -> SftpResult<()>;
}

#[derive(Debug, Default)]
pub struct SftpClient<B = FakeSftpBackend> {
    backend: B,
}

impl SftpClient<FakeSftpBackend> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<B> SftpClient<B>
where
    B: SftpBackend,
{
    pub fn with_backend(backend: B) -> Self {
        Self { backend }
    }

    pub fn list_dir(&self, path: &str) -> SftpResult<DirectoryListing> {
        self.backend.list_dir(path)
    }

    pub fn chmod(&mut self, path: &str, permissions: u32) -> SftpResult<()> {
        self.backend.chmod(path, permissions)
    }

    pub fn delete(&mut self, path: &str) -> SftpResult<()> {
        self.backend.delete(path)
    }

    pub fn rename(&mut self, from: &str, to: &str) -> SftpResult<()> {
        self.backend.rename(from, to)
    }

    pub fn mkdir(&mut self, path: &str) -> SftpResult<()> {
        self.backend.mkdir(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeSftpBackend {
    entries: BTreeMap<String, FsEntry>,
}

impl Default for FakeSftpBackend {
    fn default() -> Self {
        let mut entries = BTreeMap::new();
        entries.insert("/".to_owned(), FsEntry::directory("/"));
        Self { entries }
    }
}

impl FakeSftpBackend {
    pub fn insert(&mut self, entry: FsEntry) {
        self.entries.insert(entry.path.clone(), entry);
    }
}

impl SftpBackend for FakeSftpBackend {
    fn list_dir(&self, path: &str) -> SftpResult<DirectoryListing> {
        let directory = self.entries.get(path).ok_or_else(|| {
            SftpError::new(
                SftpErrorKind::NotFound,
                format!("directory {path} not found"),
            )
        })?;
        if directory.kind != FsEntryKind::Directory {
            return Err(SftpError::new(
                SftpErrorKind::InvalidPath,
                format!("{path} is not a directory"),
            ));
        }
        let prefix = if path == "/" {
            "/".to_owned()
        } else {
            format!("{}/", path.trim_end_matches('/'))
        };
        let entries = self
            .entries
            .values()
            .filter(|entry| entry.path != path && entry.path.starts_with(&prefix))
            .filter(|entry| !entry.path[prefix.len()..].contains('/'))
            .cloned()
            .collect();
        Ok(DirectoryListing {
            path: path.to_owned(),
            entries,
        })
    }

    fn chmod(&mut self, path: &str, permissions: u32) -> SftpResult<()> {
        let entry = self.entry_mut(path)?;
        entry.permissions = permissions;
        Ok(())
    }

    fn delete(&mut self, path: &str) -> SftpResult<()> {
        if path == "/" {
            return Err(SftpError::new(
                SftpErrorKind::InvalidPath,
                "cannot delete root",
            ));
        }
        self.entries
            .remove(path)
            .ok_or_else(|| SftpError::new(SftpErrorKind::NotFound, format!("{path} not found")))?;
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> SftpResult<()> {
        if self.entries.contains_key(to) {
            return Err(SftpError::new(
                SftpErrorKind::AlreadyExists,
                format!("{to} already exists"),
            ));
        }
        let mut entry = self
            .entries
            .remove(from)
            .ok_or_else(|| SftpError::new(SftpErrorKind::NotFound, format!("{from} not found")))?;
        entry.path = to.to_owned();
        entry.name = to.rsplit('/').next().unwrap_or(to).to_owned();
        self.entries.insert(to.to_owned(), entry);
        Ok(())
    }

    fn mkdir(&mut self, path: &str) -> SftpResult<()> {
        if self.entries.contains_key(path) {
            return Err(SftpError::new(
                SftpErrorKind::AlreadyExists,
                format!("{path} already exists"),
            ));
        }
        self.entries
            .insert(path.to_owned(), FsEntry::directory(path));
        Ok(())
    }
}

impl FakeSftpBackend {
    fn entry_mut(&mut self, path: &str) -> SftpResult<&mut FsEntry> {
        self.entries
            .get_mut(path)
            .ok_or_else(|| SftpError::new(SftpErrorKind::NotFound, format!("{path} not found")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_backend_lists_and_mutates_files() {
        let mut backend = FakeSftpBackend::default();
        backend.mkdir("/tmp").unwrap();
        backend.insert(FsEntry::file("/tmp/a.txt", 42));
        let listing = backend.list_dir("/tmp").unwrap();
        assert_eq!(listing.entries.len(), 1);
        backend.chmod("/tmp/a.txt", 0o600).unwrap();
        backend.rename("/tmp/a.txt", "/tmp/b.txt").unwrap();
        backend.delete("/tmp/b.txt").unwrap();
        assert!(backend.list_dir("/tmp").unwrap().entries.is_empty());
    }
}
