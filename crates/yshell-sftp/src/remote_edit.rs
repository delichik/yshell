//! Remote file temporary-edit session helpers.

use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    error::{SftpError, SftpErrorKind, SftpResult},
    transfer_task::TransferDirection,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEditSession {
    pub remote_path: String,
    pub local_temp_path: String,
}

impl RemoteEditSession {
    pub fn new(remote_path: impl Into<String>, local_temp_path: impl Into<String>) -> Self {
        Self {
            remote_path: remote_path.into(),
            local_temp_path: local_temp_path.into(),
        }
    }

    pub fn download_direction(&self) -> TransferDirection {
        TransferDirection::Download
    }

    pub fn upload_direction(&self) -> TransferDirection {
        TransferDirection::Upload
    }
}

pub fn prepare_remote_edit_session(
    temp_root: &Path,
    remote_path: &str,
) -> SftpResult<RemoteEditSession> {
    let remote_path = remote_path.trim();
    if remote_path.is_empty() {
        return Err(SftpError::new(
            SftpErrorKind::InvalidPath,
            "remote edit path is empty",
        ));
    }
    fs::create_dir_all(temp_root).map_err(|error| {
        SftpError::new(
            SftpErrorKind::Backend,
            format!(
                "failed to create remote-edit temp directory `{}`: {error}",
                temp_root.display()
            ),
        )
    })?;

    let filename = remote_path
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("remote-file");
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let local_name = format!("{}-{unique_suffix}", sanitize_temp_filename(filename));
    let local_temp_path = temp_root.join(local_name);
    Ok(RemoteEditSession::new(
        remote_path.to_owned(),
        local_temp_path.display().to_string(),
    ))
}

fn sanitize_temp_filename(filename: &str) -> String {
    let sanitized = filename
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.trim_matches('_').is_empty() {
        "remote-file".to_owned()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::prepare_remote_edit_session;

    #[test]
    fn remote_edit_session_uses_safe_local_temp_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session = prepare_remote_edit_session(temp.path(), "/var/log/nginx error.log")
            .expect("remote edit session");

        assert_eq!(session.remote_path, "/var/log/nginx error.log");
        assert!(session.local_temp_path.contains("nginx_error.log"));
        assert!(temp.path().exists());
    }
}
