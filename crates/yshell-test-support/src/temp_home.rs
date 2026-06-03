//! Temporary home/config-directory helpers for tests.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// Owns a temporary directory that can stand in for a user's home directory.
#[derive(Debug)]
pub struct TempHome {
    root: TempDir,
}

impl TempHome {
    /// Creates a temporary home directory.
    pub fn new() -> std::io::Result<Self> {
        tempfile::tempdir().map(|root| Self { root })
    }

    /// Returns the root path.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.root.path()
    }

    /// Returns a nested path under this temporary home.
    #[must_use]
    pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.path().join(path)
    }
}
