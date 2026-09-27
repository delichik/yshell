//! SFTP client lifecycle and backend adapters.

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::atomic::AtomicBool,
};

use crate::error::{SftpError, SftpErrorKind, SftpResult};
use crate::fs_entry::{DirectoryListing, FsEntry, FsEntryKind};
use crate::real::RealSftpBackend;
use crate::tree::{
    BatchOpResult, TransferProgress, TreeTransferOptions, TreeTransferReport,
};

pub trait SftpBackend {
    fn list_dir(&self, path: &str) -> SftpResult<DirectoryListing>;
    fn chmod(&mut self, path: &str, permissions: u32) -> SftpResult<()>;
    fn delete(&mut self, path: &str) -> SftpResult<()>;
    fn rename(&mut self, from: &str, to: &str) -> SftpResult<()>;
    fn mkdir(&mut self, path: &str) -> SftpResult<()>;
    fn upload_file(&mut self, local_path: &Path, remote_path: &str) -> SftpResult<()>;
    fn download_file(&self, remote_path: &str, local_path: &Path) -> SftpResult<()>;
    /// Remove an **empty** remote directory. Recursive deletion is orchestrated
    /// by the caller (for example with [`SftpBackend::delete_many`]).
    fn remove_dir(&mut self, path: &str) -> SftpResult<()>;

    /// Upload a local directory tree below `remote_root`.
    ///
    /// Single item failures never stop the transfer; see [`TreeTransferReport`].
    fn upload_tree(
        &mut self,
        local_root: &Path,
        remote_root: &str,
        options: TreeTransferOptions,
        progress: &mut dyn FnMut(TransferProgress),
        cancel: &AtomicBool,
    ) -> SftpResult<TreeTransferReport> {
        crate::tree::upload_tree(self, local_root, remote_root, options, progress, cancel)
    }

    /// Download a remote directory tree below `local_root`.
    ///
    /// Single item failures never stop the transfer; see [`TreeTransferReport`].
    fn download_tree(
        &mut self,
        remote_root: &str,
        local_root: &Path,
        options: TreeTransferOptions,
        progress: &mut dyn FnMut(TransferProgress),
        cancel: &AtomicBool,
    ) -> SftpResult<TreeTransferReport> {
        crate::tree::download_tree(self, remote_root, local_root, options, progress, cancel)
    }

    /// Delete many remote paths, returning one result per path without
    /// stopping on failures.
    fn delete_many(&mut self, paths: &[String]) -> Vec<BatchOpResult> {
        crate::tree::delete_many(self, paths)
    }

    /// Apply `permissions` to many remote paths, returning one result per path
    /// without stopping on failures.
    fn chmod_many(&mut self, paths: &[String], permissions: u32) -> Vec<BatchOpResult> {
        crate::tree::chmod_many(self, paths, permissions)
    }
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

impl SftpClient<RealSftpBackend> {
    pub fn with_real_backend(config: yshell_ssh::SshConnectionConfig) -> Self {
        Self {
            backend: RealSftpBackend::new(config),
        }
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

    pub fn upload_file(&mut self, local_path: &Path, remote_path: &str) -> SftpResult<()> {
        self.backend.upload_file(local_path, remote_path)
    }

    pub fn download_file(&self, remote_path: &str, local_path: &Path) -> SftpResult<()> {
        self.backend.download_file(remote_path, local_path)
    }

    pub fn remove_dir(&mut self, path: &str) -> SftpResult<()> {
        self.backend.remove_dir(path)
    }

    pub fn upload_tree(
        &mut self,
        local_root: &Path,
        remote_root: &str,
        options: TreeTransferOptions,
        progress: &mut dyn FnMut(TransferProgress),
        cancel: &AtomicBool,
    ) -> SftpResult<TreeTransferReport> {
        self.backend
            .upload_tree(local_root, remote_root, options, progress, cancel)
    }

    pub fn download_tree(
        &mut self,
        remote_root: &str,
        local_root: &Path,
        options: TreeTransferOptions,
        progress: &mut dyn FnMut(TransferProgress),
        cancel: &AtomicBool,
    ) -> SftpResult<TreeTransferReport> {
        self.backend
            .download_tree(remote_root, local_root, options, progress, cancel)
    }

    pub fn delete_many(&mut self, paths: &[String]) -> Vec<BatchOpResult> {
        self.backend.delete_many(paths)
    }

    pub fn chmod_many(&mut self, paths: &[String], permissions: u32) -> Vec<BatchOpResult> {
        self.backend.chmod_many(paths, permissions)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeSftpBackend {
    entries: BTreeMap<String, FsEntry>,
    contents: BTreeMap<String, Vec<u8>>,
}

impl Default for FakeSftpBackend {
    fn default() -> Self {
        let mut entries = BTreeMap::new();
        entries.insert("/".to_owned(), FsEntry::directory("/"));
        Self {
            entries,
            contents: BTreeMap::new(),
        }
    }
}

impl FakeSftpBackend {
    pub fn insert(&mut self, entry: FsEntry) {
        self.entries.insert(entry.path.clone(), entry);
    }

    /// Test/diagnostic accessor: the entry stored at `path`.
    pub fn entry(&self, path: &str) -> Option<&FsEntry> {
        self.entries.get(path)
    }

    /// Test/diagnostic accessor: the content stored for a fake file.
    pub fn file_contents(&self, path: &str) -> Option<&[u8]> {
        self.contents.get(path).map(Vec::as_slice)
    }

    fn ensure_not_root(&self, path: &str) -> SftpResult<()> {
        if path == "/" {
            return Err(SftpError::new(
                SftpErrorKind::InvalidPath,
                "cannot delete root",
            ));
        }
        Ok(())
    }

    fn has_children(&self, path: &str) -> bool {
        let prefix = if path == "/" {
            "/".to_owned()
        } else {
            format!("{}/", path.trim_end_matches('/'))
        };
        self.entries
            .keys()
            .any(|candidate| candidate != path && candidate.starts_with(&prefix))
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
        self.ensure_not_root(path)?;
        let entry = self
            .entries
            .get(path)
            .ok_or_else(|| SftpError::new(SftpErrorKind::NotFound, format!("{path} not found")))?;
        if entry.kind == FsEntryKind::Directory && self.has_children(path) {
            return Err(SftpError::new(
                SftpErrorKind::Backend,
                format!("directory {path} is not empty"),
            ));
        }
        self.entries.remove(path);
        self.contents.remove(path);
        Ok(())
    }

    fn remove_dir(&mut self, path: &str) -> SftpResult<()> {
        self.ensure_not_root(path)?;
        let entry = self
            .entries
            .get(path)
            .ok_or_else(|| SftpError::new(SftpErrorKind::NotFound, format!("{path} not found")))?;
        if entry.kind != FsEntryKind::Directory {
            return Err(SftpError::new(
                SftpErrorKind::InvalidPath,
                format!("{path} is not a directory"),
            ));
        }
        if self.has_children(path) {
            return Err(SftpError::new(
                SftpErrorKind::Backend,
                format!("directory {path} is not empty"),
            ));
        }
        self.entries.remove(path);
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
        let contents = self.contents.remove(from);
        entry.path = to.to_owned();
        entry.name = to.rsplit('/').next().unwrap_or(to).to_owned();
        self.entries.insert(to.to_owned(), entry);
        if let Some(contents) = contents {
            self.contents.insert(to.to_owned(), contents);
        }
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

    fn upload_file(&mut self, local_path: &Path, remote_path: &str) -> SftpResult<()> {
        let bytes = fs::read(local_path).map_err(|error| {
            SftpError::new(
                SftpErrorKind::Backend,
                format!(
                    "failed to read local upload source `{}`: {error}",
                    local_path.display()
                ),
            )
        })?;
        self.entries.insert(
            remote_path.to_owned(),
            FsEntry::file(remote_path, bytes.len() as u64),
        );
        self.contents.insert(remote_path.to_owned(), bytes);
        Ok(())
    }

    fn download_file(&self, remote_path: &str, local_path: &Path) -> SftpResult<()> {
        let bytes = self.contents.get(remote_path).ok_or_else(|| {
            SftpError::new(
                SftpErrorKind::NotFound,
                format!("remote file `{remote_path}` has no stored fake content"),
            )
        })?;
        fs::write(local_path, bytes).map_err(|error| {
            SftpError::new(
                SftpErrorKind::Backend,
                format!(
                    "failed to write fake download target `{}`: {error}",
                    local_path.display()
                ),
            )
        })
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

    #[test]
    fn fake_backend_round_trips_upload_and_download() {
        let temp = tempfile::tempdir().unwrap();
        let upload_path = temp.path().join("upload.txt");
        let download_path = temp.path().join("download.txt");
        fs::write(&upload_path, b"hello fake sftp").unwrap();

        let mut backend = FakeSftpBackend::default();
        backend.upload_file(&upload_path, "/upload.txt").unwrap();
        backend.download_file("/upload.txt", &download_path).unwrap();

        assert_eq!(fs::read(&download_path).unwrap(), b"hello fake sftp");
        assert_eq!(backend.file_contents("/upload.txt"), Some(&b"hello fake sftp"[..]));
        assert_eq!(
            backend.entry("/upload.txt").map(|entry| entry.size_bytes),
            Some(15)
        );
    }

    #[test]
    fn fake_backend_delete_and_remove_dir_align_with_the_real_backend() {
        let mut backend = FakeSftpBackend::default();
        backend.mkdir("/srv").unwrap();
        backend.mkdir("/srv/sub").unwrap();
        backend.insert(FsEntry::file("/srv/sub/a.txt", 1));

        assert_eq!(
            backend.delete("/srv/sub").unwrap_err().kind,
            SftpErrorKind::Backend
        );
        assert_eq!(
            backend.remove_dir("/srv/sub").unwrap_err().kind,
            SftpErrorKind::Backend
        );
        assert_eq!(
            backend.remove_dir("/srv/sub/a.txt").unwrap_err().kind,
            SftpErrorKind::InvalidPath
        );
        assert_eq!(
            backend.remove_dir("/missing").unwrap_err().kind,
            SftpErrorKind::NotFound
        );

        backend.delete("/srv/sub/a.txt").unwrap();
        backend.remove_dir("/srv/sub").unwrap();
        assert!(backend.list_dir("/srv").unwrap().entries.is_empty());
    }
}
