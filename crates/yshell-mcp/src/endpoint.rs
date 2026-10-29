//! Loopback endpoint discovery file under the YShell config directory.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const ENDPOINT_FILE_NAME: &str = "mcp-endpoint.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EndpointInfo {
    pub host: String,
    pub port: u16,
    /// Shared secret required on every IPC request.
    pub token: String,
}

impl EndpointInfo {
    #[must_use]
    pub fn loopback(port: u16, token: impl Into<String>) -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port,
            token: token.into(),
        }
    }
}

#[must_use]
pub fn endpoint_path(config_dir: &Path) -> PathBuf {
    config_dir.join(ENDPOINT_FILE_NAME)
}

pub fn write_endpoint(config_dir: &Path, info: &EndpointInfo) -> io::Result<PathBuf> {
    fs::create_dir_all(config_dir)?;
    let path = endpoint_path(config_dir);
    let body = serde_json::to_vec_pretty(info)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(&path, body)?;
    Ok(path)
}

pub fn load_endpoint(config_dir: &Path) -> io::Result<EndpointInfo> {
    let path = endpoint_path(config_dir);
    let bytes = fs::read(&path)?;
    serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn clear_endpoint(config_dir: &Path) -> io::Result<()> {
    let path = endpoint_path(config_dir);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn endpoint_round_trip() {
        let dir = tempdir().expect("tempdir");
        let info = EndpointInfo::loopback(43210, "secret-token");
        write_endpoint(dir.path(), &info).expect("write");
        let loaded = load_endpoint(dir.path()).expect("load");
        assert_eq!(loaded, info);
        clear_endpoint(dir.path()).expect("clear");
        assert!(load_endpoint(dir.path()).is_err());
    }
}
