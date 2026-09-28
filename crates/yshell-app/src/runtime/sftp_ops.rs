//! SFTP mutations: upload, download, rename, delete, chmod, mkdir, recursive
//! tree transfers and remote edit operations.

use crate::{
    error::AppError,
    error::AppResult,
    local_fs,
    sftp_jobs::{SftpJobMessage, SftpJobOutcome, SftpJobSpec},
};
use std::{fs, path::Path, path::PathBuf};
use yshell_sftp::{
    prepare_remote_edit_session, OverwritePolicy, SftpClient, TransferDirection,
    TreeTransferOptions,
};

use super::*;
use super::{
    apply_sftp_tree_report, overwrite_policy_from_id, overwrite_policy_id, overwrite_policy_label,
    tree_transfer_status_text, update_sftp_transfer_progress,
};

impl AppRuntime {
    pub fn create_sftp_folder_named(&mut self, name: &str) -> AppResult<AppProjection> {
        let name = name.trim();
        if name.is_empty() {
            self.status_text = "New SFTP folder name must not be empty.".to_owned();
            return Ok(self.projection());
        }
        let target = self.sftp_relative_or_absolute_path(name);
        self.sftp_secondary_target = name.to_owned();
        self.create_sftp_directory()?;
        self.reselect_sftp_path(&target);
        Ok(self.projection())
    }

    pub fn rename_sftp_selected(&mut self, new_name: &str) -> AppResult<AppProjection> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            self.status_text = "New SFTP name must not be empty.".to_owned();
            return Ok(self.projection());
        }
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP entry before renaming it.".to_owned();
            return Ok(self.projection());
        };
        let target = self.sftp_relative_or_absolute_path(new_name);
        self.sftp_remote_target = entry.path;
        self.sftp_secondary_target = new_name.to_owned();
        self.rename_sftp_path()?;
        self.reselect_sftp_path(&target);
        Ok(self.projection())
    }

    pub fn delete_sftp_selected(&mut self) -> AppResult<AppProjection> {
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP entry before deleting it.".to_owned();
            return Ok(self.projection());
        };
        let Some(session_key) = self.prepare_active_sftp_operation("deleting through SFTP")? else {
            return Ok(self.projection());
        };
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        client.delete(&entry.path).map_err(AppError::from_error)?;
        self.sftp_remote_target = entry.path.clone();
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!("Deleted `{}` through SFTP.", entry.path);
        Ok(self.projection())
    }

    pub fn chmod_sftp_selected(&mut self, permissions: &str) -> AppResult<AppProjection> {
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP entry before changing permissions.".to_owned();
            return Ok(self.projection());
        };
        self.sftp_remote_target = entry.path;
        self.sftp_permissions = permissions.trim().to_owned();
        self.chmod_sftp_path()
    }

    pub fn upload_sftp_into_current(&mut self, local_path: &str) -> AppResult<AppProjection> {
        let local_path = local_path.trim();
        if local_path.is_empty() {
            self.status_text = "Local upload path must not be empty.".to_owned();
            return Ok(self.projection());
        }
        self.sftp_local_path = local_path.to_owned();
        self.sftp_remote_target = String::new();
        self.upload_sftp_file()?;
        let uploaded = self.sftp_remote_target.clone();
        self.reselect_sftp_path(&uploaded);
        Ok(self.projection())
    }

    pub fn download_sftp_selected(&mut self, local_path: &str) -> AppResult<AppProjection> {
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP entry before downloading it.".to_owned();
            return Ok(self.projection());
        };
        let local_path = local_path.trim();
        if !local_path.is_empty() {
            self.sftp_local_path = local_path.to_owned();
        }
        if self.sftp_local_path.trim().is_empty() {
            self.status_text = "Local download path must not be empty.".to_owned();
            return Ok(self.projection());
        }
        self.sftp_remote_target = entry.path;
        self.download_sftp_file()
    }

    pub fn edit_sftp_selected(&mut self) -> AppResult<AppProjection> {
        let Some(entry) = self.selected_sftp_entry() else {
            self.status_text = "Select an SFTP file before starting remote edit.".to_owned();
            return Ok(self.projection());
        };
        self.sftp_remote_target = entry.path;
        self.start_sftp_remote_edit()
    }

    pub fn set_sftp_operation_inputs(
        &mut self,
        local_path: &str,
        remote_target: &str,
        secondary_target: &str,
        permissions: &str,
    ) -> AppProjection {
        self.sftp_local_path = local_path.to_owned();
        self.sftp_remote_target = remote_target.to_owned();
        self.sftp_secondary_target = secondary_target.to_owned();
        self.sftp_permissions = permissions.to_owned();
        self.projection()
    }

    pub fn upload_sftp_file(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) = self.prepare_active_sftp_operation("uploading through SFTP")?
        else {
            return Ok(self.projection());
        };
        let local_path = PathBuf::from(self.sftp_local_path.trim());
        if self.sftp_local_path.trim().is_empty() {
            return Err(AppError::new("local upload path is empty"));
        }
        let remote_path = self.sftp_upload_target()?;
        let transfer_bytes = fs::metadata(&local_path)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        let transfer_id = self.enqueue_sftp_transfer(
            TransferDirection::Upload,
            local_path.display().to_string(),
            remote_path.clone(),
            Some(transfer_bytes),
        );
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        if let Err(error) = client.upload_file(&local_path, &remote_path) {
            let app_error = AppError::from_error(error);
            self.fail_sftp_transfer(&transfer_id, app_error.to_string());
            self.status_text = format!("SFTP upload error: {app_error}");
            return Ok(self.projection());
        }
        self.complete_sftp_transfer(&transfer_id, transfer_bytes);
        if let Some(runtime) = self.sessions.get_mut(&session_key) {
            runtime.record_transfer(
                "upload",
                &local_path,
                Path::new(&remote_path),
                transfer_bytes,
            );
        }
        self.sftp_remote_target = remote_path.clone();
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!(
            "Uploaded `{}` to `{}` through the live SFTP backend.",
            local_path.display(),
            remote_path
        );
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub fn download_sftp_file(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) = self.prepare_active_sftp_operation("downloading through SFTP")?
        else {
            return Ok(self.projection());
        };
        if self.sftp_local_path.trim().is_empty() {
            return Err(AppError::new("local download path is empty"));
        }
        let remote_path = self.sftp_download_target()?;
        let local_path = PathBuf::from(self.sftp_local_path.trim());
        let transfer_id = self.enqueue_sftp_transfer(
            TransferDirection::Download,
            remote_path.clone(),
            local_path.display().to_string(),
            None,
        );
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let client = SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        if let Err(error) = client.download_file(&remote_path, &local_path) {
            let app_error = AppError::from_error(error);
            self.fail_sftp_transfer(&transfer_id, app_error.to_string());
            self.status_text = format!("SFTP download error: {app_error}");
            return Ok(self.projection());
        }
        let transfer_bytes = fs::metadata(&local_path)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        self.complete_sftp_transfer(&transfer_id, transfer_bytes);
        if let Some(runtime) = self.sessions.get_mut(&session_key) {
            runtime.record_transfer(
                "download",
                &local_path,
                Path::new(&remote_path),
                transfer_bytes,
            );
        }
        self.status_text = format!(
            "Downloaded `{}` to `{}` through the live SFTP backend.",
            remote_path,
            local_path.display()
        );
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub fn start_sftp_remote_edit(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) = self.prepare_active_sftp_operation("starting remote file edit")?
        else {
            return Ok(self.projection());
        };
        let remote_path = self.sftp_download_target()?;
        let temp_root = self.config_dir.join("remote-edit");
        let edit_session =
            prepare_remote_edit_session(&temp_root, &remote_path).map_err(AppError::from_error)?;
        let local_path = PathBuf::from(&edit_session.local_temp_path);
        let transfer_id = self.enqueue_sftp_transfer(
            edit_session.download_direction(),
            remote_path.clone(),
            local_path.display().to_string(),
            None,
        );
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let client = SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        if let Err(error) = client.download_file(&remote_path, &local_path) {
            let app_error = AppError::from_error(error);
            self.fail_sftp_transfer(&transfer_id, app_error.to_string());
            self.status_text = format!("SFTP remote edit download error: {app_error}");
            return Ok(self.projection());
        }
        let transfer_bytes = fs::metadata(&local_path)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        self.complete_sftp_transfer(&transfer_id, transfer_bytes);
        self.sftp_local_path = local_path.display().to_string();
        self.sftp_remote_target = remote_path.clone();
        self.remote_edit_session = Some(edit_session);
        if let Some(runtime) = self.sessions.get_mut(&session_key) {
            runtime.record_transfer(
                "remote-edit-download",
                &local_path,
                Path::new(&remote_path),
                transfer_bytes,
            );
        }
        self.status_text = format!(
            "Remote edit ready for `{remote_path}`. Edit local temp file `{}` and then save remote edit.",
            local_path.display()
        );
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub fn save_sftp_remote_edit(&mut self) -> AppResult<AppProjection> {
        let Some(edit_session) = self.remote_edit_session.clone() else {
            self.status_text =
                "No active remote edit session to save. Start remote edit first.".to_owned();
            return Ok(self.projection());
        };
        let Some(session_key) = self.prepare_active_sftp_operation("saving remote file edit")?
        else {
            return Ok(self.projection());
        };
        let local_path = PathBuf::from(&edit_session.local_temp_path);
        let transfer_bytes = fs::metadata(&local_path)
            .map(|metadata| metadata.len())
            .map_err(|error| {
                AppError::new(format!(
                    "remote edit temp file `{}` is not readable: {error}",
                    local_path.display()
                ))
            })?;
        let transfer_id = self.enqueue_sftp_transfer(
            edit_session.upload_direction(),
            local_path.display().to_string(),
            edit_session.remote_path.clone(),
            Some(transfer_bytes),
        );
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        if let Err(error) = client.upload_file(&local_path, &edit_session.remote_path) {
            let app_error = AppError::from_error(error);
            self.fail_sftp_transfer(&transfer_id, app_error.to_string());
            self.status_text = format!("SFTP remote edit upload error: {app_error}");
            return Ok(self.projection());
        }
        self.complete_sftp_transfer(&transfer_id, transfer_bytes);
        if let Some(runtime) = self.sessions.get_mut(&session_key) {
            runtime.record_transfer(
                "remote-edit-upload",
                &local_path,
                Path::new(&edit_session.remote_path),
                transfer_bytes,
            );
        }
        self.sftp_remote_target = edit_session.remote_path.clone();
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!(
            "Saved remote edit `{}` from local temp file `{}`.",
            edit_session.remote_path,
            local_path.display()
        );
        self.fold_logging_notice_from_session(&session_key);
        Ok(self.projection())
    }

    pub fn cancel_sftp_remote_edit(&mut self) -> AppResult<AppProjection> {
        let Some(edit_session) = self.remote_edit_session.take() else {
            self.status_text =
                "No active remote edit session to cancel. Start remote edit first.".to_owned();
            return Ok(self.projection());
        };
        match fs::remove_file(&edit_session.local_temp_path) {
            Ok(()) => {
                self.status_text = format!(
                    "Canceled remote edit for `{}` and removed temp file `{}`.",
                    edit_session.remote_path, edit_session.local_temp_path
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.status_text = format!(
                    "Canceled remote edit for `{}`. Temp file was already gone.",
                    edit_session.remote_path
                );
            }
            Err(error) => {
                self.status_text = format!(
                    "Canceled remote edit for `{}`, but temp file `{}` could not be removed: {error}",
                    edit_session.remote_path, edit_session.local_temp_path
                );
            }
        }
        Ok(self.projection())
    }

    pub fn create_sftp_directory(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) =
            self.prepare_active_sftp_operation("creating directories through SFTP")?
        else {
            return Ok(self.projection());
        };
        let remote_path = self.sftp_secondary_target_path("new SFTP directory path is empty")?;
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        client.mkdir(&remote_path).map_err(AppError::from_error)?;
        self.sftp_remote_target = remote_path.clone();
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!("Created remote directory `{remote_path}` through SFTP.");
        Ok(self.projection())
    }

    pub fn rename_sftp_path(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) =
            self.prepare_active_sftp_operation("renaming paths through SFTP")?
        else {
            return Ok(self.projection());
        };
        let from_path = self.sftp_download_target()?;
        let to_path = self.sftp_secondary_target_path("new SFTP path is empty")?;
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        client
            .rename(&from_path, &to_path)
            .map_err(AppError::from_error)?;
        self.sftp_remote_target = to_path.clone();
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!("Renamed `{from_path}` to `{to_path}` through SFTP.");
        Ok(self.projection())
    }

    pub fn chmod_sftp_path(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) =
            self.prepare_active_sftp_operation("changing permissions through SFTP")?
        else {
            return Ok(self.projection());
        };
        let remote_path = self.sftp_download_target()?;
        let permissions = parse_sftp_permissions(&self.sftp_permissions)?;
        let runtime = self
            .sessions
            .get(&session_key)
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        let mut client =
            SftpClient::with_real_backend(self.effective_ssh_config(&runtime.ssh_config));
        client
            .chmod(&remote_path, permissions)
            .map_err(AppError::from_error)?;
        let _ = self.refresh_active_sftp_listing()?;
        self.status_text = format!(
            "Applied chmod {:03o} to `{remote_path}` through SFTP.",
            permissions
        );
        Ok(self.projection())
    }

    pub(crate) fn sftp_upload_target(&self) -> AppResult<String> {
        if !self.sftp_remote_target.trim().is_empty() {
            return normalize_remote_path(&self.sftp_remote_target);
        }
        let filename = Path::new(self.sftp_local_path.trim())
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                AppError::new("could not infer remote upload target from local file name")
            })?;
        Ok(join_remote_path(&self.sftp_path, filename))
    }

    pub(crate) fn sftp_download_target(&self) -> AppResult<String> {
        if self.sftp_remote_target.trim().is_empty() {
            return Err(AppError::new(
                "remote SFTP target is empty; refresh a directory or type a remote file path first",
            ));
        }
        normalize_remote_path(&self.sftp_remote_target)
    }

    pub(crate) fn sftp_secondary_target_path(&self, empty_message: &str) -> AppResult<String> {
        let target = self.sftp_secondary_target.trim();
        if target.is_empty() {
            return Err(AppError::new(empty_message));
        }
        if target.starts_with('/') {
            normalize_remote_path(target)
        } else {
            Ok(join_remote_path(&self.sftp_path, target))
        }
    }
}

impl AppRuntime {
    #[cfg(test)]
    pub fn set_sftp_remote_target(&mut self, path: &str) -> AppProjection {
        self.sftp_remote_target = path.to_owned();
        self.projection()
    }
}

// ---------------------------------------------------------------------------
// N1 Phase 2: worker-backed transfers (gestures, batch actions, conflicts)
// ---------------------------------------------------------------------------
//
// The single-item methods above stay for the entry dialog's typed-path fallback
// and the remote-edit flow. Everything gesture/batch-shaped (drag & drop,
// clipboard, context-menu batch actions, local-pane uploads) goes through the
// `crate::sftp_jobs` worker so the UI thread never blocks; progress and results
// arrive as `SftpJobMessage`s drained by the UI timer.

impl AppRuntime {
    /// Effective SSH config for a job, or a "not connected" error.
    pub(crate) fn active_sftp_job_config(&self) -> AppResult<yshell_ssh::SshConnectionConfig> {
        let Some(session_key) = self.sftp_ready_session_key() else {
            return Err(AppError::new(
                "SFTP session is not ready; connect a runtime session first",
            ));
        };
        let config = self
            .sessions
            .get(session_key)
            .map(|runtime| runtime.ssh_config.clone())
            .ok_or_else(|| AppError::new("active runtime session is missing"))?;
        Ok(self.effective_ssh_config(&config))
    }

    /// Enqueues the queue task and hands the spec to the worker.
    pub(crate) fn submit_sftp_spec(&mut self, spec: SftpJobSpec) -> AppResult<String> {
        let transfer_id = self.enqueue_sftp_transfer(
            spec.direction(),
            spec.source_text(),
            spec.destination_text(),
            None,
        );
        let Some(handle) = self.sftp_jobs.as_mut() else {
            self.fail_sftp_transfer(&transfer_id, "transfer worker is unavailable".to_owned());
            return Err(AppError::new("transfer worker is unavailable"));
        };
        if !handle.submit(&transfer_id, spec) {
            self.fail_sftp_transfer(&transfer_id, "transfer worker is unavailable".to_owned());
            return Err(AppError::new("transfer worker is unavailable"));
        }
        self.transfer_drawer_expanded = true;
        Ok(transfer_id)
    }

    /// Uploads local files/directories into a remote directory (defaults to the
    /// current remote directory). Directories use the F0 tree engine.
    /// Returns the created transfer ids (for move cleanup / tests).
    pub(crate) fn submit_upload_paths(
        &mut self,
        paths: &[PathBuf],
        destination_dir: Option<&str>,
        overwrite: OverwritePolicy,
    ) -> AppResult<Vec<String>> {
        if paths.is_empty() {
            return Err(AppError::new("no local paths to upload"));
        }
        let config = self.active_sftp_job_config()?;
        let destination_dir = match destination_dir {
            Some(dir) => normalize_remote_path(dir)?,
            None => self.sftp_path.clone(),
        };
        let options = TreeTransferOptions {
            overwrite,
            follow_symlinks: false,
        };
        let mut transfer_ids = Vec::new();
        for path in paths {
            let metadata = fs::metadata(path).map_err(|error| {
                AppError::new(format!(
                    "local path `{}` is not readable: {error}",
                    path.display()
                ))
            })?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    AppError::new(format!("local path `{}` has no file name", path.display()))
                })?;
            let remote = join_remote_path(&destination_dir, name);
            let spec = if metadata.is_dir() {
                SftpJobSpec::UploadTree {
                    config: config.clone(),
                    local_root: path.clone(),
                    remote_root: remote,
                    options,
                }
            } else {
                SftpJobSpec::UploadFile {
                    config: config.clone(),
                    local: path.clone(),
                    remote,
                    options,
                }
            };
            transfer_ids.push(self.submit_sftp_spec(spec)?);
        }
        self.status_text = format!(
            "Queued {} upload job{} into `{destination_dir}`.",
            transfer_ids.len(),
            if transfer_ids.len() == 1 { "" } else { "s" }
        );
        Ok(transfer_ids)
    }

    /// Downloads remote entries into a local directory.
    /// Returns the created transfer ids (for move cleanup / tests).
    pub(crate) fn submit_download_entries(
        &mut self,
        entries: &[(String, bool)],
        local_dir: &Path,
        overwrite: OverwritePolicy,
    ) -> AppResult<Vec<String>> {
        if entries.is_empty() {
            return Err(AppError::new("no remote entries to download"));
        }
        let config = self.active_sftp_job_config()?;
        let options = TreeTransferOptions {
            overwrite,
            follow_symlinks: false,
        };
        let mut transfer_ids = Vec::new();
        for (remote_path, is_dir) in entries {
            let name = remote_path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    AppError::new(format!("remote path `{remote_path}` has no entry name"))
                })?;
            let local = local_dir.join(name);
            let spec = if *is_dir {
                SftpJobSpec::DownloadTree {
                    config: config.clone(),
                    remote_root: remote_path.clone(),
                    local_root: local,
                    options,
                }
            } else {
                SftpJobSpec::DownloadFile {
                    config: config.clone(),
                    remote: remote_path.clone(),
                    local,
                    options,
                }
            };
            transfer_ids.push(self.submit_sftp_spec(spec)?);
        }
        self.status_text = format!(
            "Queued {} download job{} into `{}`.",
            transfer_ids.len(),
            if transfer_ids.len() == 1 { "" } else { "s" },
            local_dir.display()
        );
        Ok(transfer_ids)
    }

    /// Remote→remote copy (D10: download into a temp dir, then upload).
    pub(crate) fn submit_remote_copy_entries(
        &mut self,
        entries: &[(String, bool)],
        destination_dir: &str,
        overwrite: OverwritePolicy,
    ) -> AppResult<usize> {
        let destination_dir = normalize_remote_path(destination_dir)?;
        let config = self.active_sftp_job_config()?;
        let options = TreeTransferOptions {
            overwrite,
            follow_symlinks: false,
        };
        let mut submitted = 0usize;
        for (from, is_dir) in entries {
            if from.trim_end_matches('/') == destination_dir.trim_end_matches('/') {
                continue;
            }
            let name = from
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| AppError::new(format!("remote path `{from}` has no entry name")))?;
            let to = join_remote_path(&destination_dir, name);
            let spec = if *is_dir {
                SftpJobSpec::RemoteCopyTree {
                    config: config.clone(),
                    from: from.clone(),
                    to,
                    options,
                }
            } else {
                SftpJobSpec::RemoteCopyFile {
                    config: config.clone(),
                    from: from.clone(),
                    to,
                    options,
                }
            };
            self.submit_sftp_spec(spec)?;
            submitted += 1;
        }
        if submitted == 0 {
            return Err(AppError::new(
                "the destination is already the source directory",
            ));
        }
        self.status_text = format!(
            "Queued {submitted} remote copy job{} into `{destination_dir}`.",
            if submitted == 1 { "" } else { "s" }
        );
        Ok(submitted)
    }

    /// Remote→remote move (drag inside the remote pane / cut+paste).
    pub(crate) fn submit_remote_move_entries(
        &mut self,
        entries: &[(String, bool)],
        destination_dir: &str,
        overwrite: OverwritePolicy,
    ) -> AppResult<usize> {
        if entries.is_empty() {
            return Err(AppError::new("no remote entries to move"));
        }
        let destination_dir = normalize_remote_path(destination_dir)?;
        let config = self.active_sftp_job_config()?;
        let options = TreeTransferOptions {
            overwrite,
            follow_symlinks: false,
        };
        let mut moves = Vec::new();
        for (from, _is_dir) in entries {
            if from.trim_end_matches('/') == destination_dir.trim_end_matches('/') {
                continue;
            }
            let name = from
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| AppError::new(format!("remote path `{from}` has no entry name")))?;
            moves.push((from.clone(), join_remote_path(&destination_dir, name)));
        }
        if moves.is_empty() {
            return Err(AppError::new(
                "the destination is already the source directory",
            ));
        }
        let count = moves.len();
        self.submit_sftp_spec(SftpJobSpec::RemoteMove {
            config,
            moves,
            options,
        })?;
        self.status_text = format!(
            "Queued a move of {count} entr{} into `{destination_dir}`.",
            if count == 1 { "y" } else { "ies" }
        );
        Ok(count)
    }

    /// Recursive remote delete.
    pub(crate) fn submit_remote_delete_entries(
        &mut self,
        entries: &[(String, bool)],
    ) -> AppResult<usize> {
        if entries.is_empty() {
            return Err(AppError::new("no remote entries to delete"));
        }
        let config = self.active_sftp_job_config()?;
        let count = entries.len();
        self.submit_sftp_spec(SftpJobSpec::RemoteDelete {
            config,
            entries: entries.to_vec(),
        })?;
        self.status_text = format!(
            "Queued a delete of {count} remote entr{}.",
            if count == 1 { "y" } else { "ies" }
        );
        Ok(count)
    }

    /// Batch chmod.
    pub(crate) fn submit_remote_chmod_entries(
        &mut self,
        entries: &[(String, bool)],
        permissions: u32,
    ) -> AppResult<usize> {
        if entries.is_empty() {
            return Err(AppError::new("no remote entries to chmod"));
        }
        let config = self.active_sftp_job_config()?;
        let paths = entries
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        let count = paths.len();
        self.submit_sftp_spec(SftpJobSpec::RemoteChmod {
            config,
            paths,
            permissions,
        })?;
        self.status_text = format!(
            "Queued chmod {permissions:03o} for {count} remote entr{}.",
            if count == 1 { "y" } else { "ies" }
        );
        Ok(count)
    }

    fn policy_from_id(policy_id: &str) -> AppResult<OverwritePolicy> {
        overwrite_policy_from_id(policy_id)
            .ok_or_else(|| AppError::new(format!("unknown overwrite policy `{policy_id}`")))
    }

    // ------------------------------------------------------------- UI actions

    /// Uploads the local pane's selection (or explicit paths) to the current
    /// remote directory.
    pub fn upload_local_paths_to_remote(
        &mut self,
        paths: &[PathBuf],
        policy_id: &str,
    ) -> AppResult<AppProjection> {
        let policy = Self::policy_from_id(policy_id)?;
        self.submit_upload_paths(paths, None, policy)?;
        Ok(self.projection())
    }

    /// Drag local → remote (and the local context menu's Upload action).
    pub fn upload_local_selection_to_remote(
        &mut self,
        policy_id: &str,
    ) -> AppResult<AppProjection> {
        let paths = self.local_selected_paths();
        if paths.is_empty() {
            self.status_text = "Select local entries before uploading.".to_owned();
            return Ok(self.projection());
        }
        self.upload_local_paths_to_remote(&paths, policy_id)
    }

    /// Local context menu's "Move to Remote": uploads the selection into the
    /// current remote directory and deletes the local sources on success.
    pub fn move_local_selection_to_remote(&mut self) -> AppResult<AppProjection> {
        let paths = self.local_selected_paths();
        if paths.is_empty() {
            self.status_text = "Select local entries before moving.".to_owned();
            return Ok(self.projection());
        }
        let target = self.sftp_path.clone();
        self.drop_local_paths_on_remote(&paths, &target, true)
    }

    /// Uploads one local path (dialog/typed input) into the current remote
    /// directory, honoring the "Upload Here..." target captured by the menu.
    pub fn upload_local_path_to_remote(
        &mut self,
        path: &str,
        policy_id: &str,
    ) -> AppResult<AppProjection> {
        let path = path.trim();
        if path.is_empty() {
            self.status_text = "Local upload path must not be empty.".to_owned();
            return Ok(self.projection());
        }
        let policy = Self::policy_from_id(policy_id)?;
        let destination = self.sftp_upload_dir_override.take();
        self.submit_upload_paths(&[PathBuf::from(path)], destination.as_deref(), policy)?;
        Ok(self.projection())
    }

    /// "Download To..." with an explicit directory (typed fallback for rfd).
    pub fn download_sftp_selection_to(
        &mut self,
        local_dir: &str,
        policy_id: &str,
    ) -> AppResult<AppProjection> {
        let entries = self.selection_entry_pairs();
        if entries.is_empty() {
            self.status_text = "Select remote entries before downloading.".to_owned();
            return Ok(self.projection());
        }
        self.download_sftp_entries(&entries, local_dir, policy_id)
    }

    /// The selected entry as a directory path (the "Upload Here..." target).
    pub(crate) fn sftp_selected_directory(&self) -> Option<String> {
        self.selected_sftp_entry()
            .filter(|entry| matches!(entry.kind, yshell_sftp::FsEntryKind::Directory))
            .map(|entry| entry.path)
    }

    /// Downloads the remote selection (or explicit entries) into a local dir.
    pub fn download_sftp_entries(
        &mut self,
        entries: &[(String, bool)],
        local_dir: &str,
        policy_id: &str,
    ) -> AppResult<AppProjection> {
        let policy = Self::policy_from_id(policy_id)?;
        let local_dir = local_fs::normalize_local_dir(local_dir, &self.local_pane.dir)
            .map_err(AppError::from_error)?;
        self.submit_download_entries(entries, &local_dir, policy)?;
        Ok(self.projection())
    }

    /// Batch delete of the current remote selection (recursive).
    pub fn delete_sftp_selection(&mut self) -> AppResult<AppProjection> {
        let entries = self.selection_entry_pairs();
        if entries.is_empty() {
            self.status_text = "Select remote entries before deleting.".to_owned();
            return Ok(self.projection());
        }
        self.submit_remote_delete_entries(&entries)?;
        Ok(self.projection())
    }

    /// Batch chmod of the current remote selection.
    pub fn chmod_sftp_selection(&mut self, permissions: &str) -> AppResult<AppProjection> {
        let permissions = parse_sftp_permissions(permissions)?;
        let entries = self.selection_entry_pairs();
        if entries.is_empty() {
            self.status_text = "Select remote entries before changing permissions.".to_owned();
            return Ok(self.projection());
        }
        self.submit_remote_chmod_entries(&entries, permissions)?;
        Ok(self.projection())
    }

    /// Remote→remote copy of the selection into `destination_dir`.
    pub fn copy_sftp_selection_to(
        &mut self,
        destination_dir: &str,
        policy_id: &str,
    ) -> AppResult<AppProjection> {
        let policy = Self::policy_from_id(policy_id)?;
        let entries = self.selection_entry_pairs();
        if entries.is_empty() {
            self.status_text = "Select remote entries before copying.".to_owned();
            return Ok(self.projection());
        }
        self.submit_remote_copy_entries(&entries, destination_dir, policy)?;
        Ok(self.projection())
    }

    /// Re-runs the job that hit Ask-policy conflicts with an explicit policy.
    pub fn resolve_sftp_conflict(&mut self, policy_id: &str) -> AppResult<AppProjection> {
        let Some(prompt) = self.pending_sftp_conflict.take() else {
            self.status_text = "There is no pending conflict to resolve.".to_owned();
            return Ok(self.projection());
        };
        let Some(policy) = overwrite_policy_from_id(policy_id) else {
            // Put the prompt back so a bad id cannot silently drop it.
            self.pending_sftp_conflict = Some(prompt);
            return Err(AppError::new(format!(
                "unknown overwrite policy `{policy_id}`"
            )));
        };
        let spec = (*prompt.spec).with_overwrite(policy);
        let count = prompt.conflicts.len();
        self.submit_sftp_spec(spec)?;
        self.status_text = format!(
            "Re-running the transfer with `{}` for {count} conflicting path(s).",
            overwrite_policy_label(policy)
        );
        Ok(self.projection())
    }

    /// Dismisses the conflict dialog without re-running (the conflicting
    /// entries stay skipped).
    pub fn cancel_sftp_conflict(&mut self) -> AppProjection {
        if let Some(prompt) = self.pending_sftp_conflict.take() {
            self.status_text = format!(
                "Kept {} conflicting path(s) skipped.",
                prompt.conflicts.len()
            );
        }
        self.projection()
    }

    /// Consumes one worker message and folds it into the queue/status.
    pub fn apply_sftp_job_message(&mut self, message: SftpJobMessage) -> AppProjection {
        match message {
            SftpJobMessage::Progress {
                id,
                bytes_done,
                bytes_total,
            } => {
                update_sftp_transfer_progress(
                    &mut self.transfer_queue,
                    &id,
                    bytes_done,
                    Some(bytes_total),
                );
            }
            SftpJobMessage::Finished { id, outcome } => {
                let spec = self.sftp_jobs.as_ref().and_then(|handle| handle.spec(&id));
                if let Some(handle) = self.sftp_jobs.as_mut() {
                    handle.forget(&id);
                }
                let direction = spec
                    .as_ref()
                    .map(SftpJobSpec::direction)
                    .unwrap_or(TransferDirection::Download);
                let destination = spec
                    .as_ref()
                    .map(SftpJobSpec::destination_text)
                    .unwrap_or_default();
                // Move gestures delete their source only after a clean copy.
                let mut moved_cleanly = false;
                match outcome {
                    SftpJobOutcome::Completed(report) => {
                        if !report.conflicts.is_empty() {
                            if let Some(spec) = spec.clone() {
                                let policy_id = spec
                                    .overwrite_policy()
                                    .map(overwrite_policy_id)
                                    .unwrap_or("ask")
                                    .to_owned();
                                self.pending_sftp_conflict = Some(SftpConflictPrompt {
                                    transfer_id: id.clone(),
                                    source_text: spec.source_text(),
                                    destination_text: spec.destination_text(),
                                    conflicts: report.conflicts.clone(),
                                    policy_id,
                                    spec: Box::new(spec),
                                });
                                self.transfer_drawer_expanded = true;
                            }
                        }
                        apply_sftp_tree_report(&mut self.transfer_queue, &id, &report);
                        moved_cleanly = !report.cancelled
                            && report.failed.is_empty()
                            && report.conflicts.is_empty();
                        self.status_text =
                            tree_transfer_status_text(&report, direction, &destination);
                    }
                    SftpJobOutcome::Cancelled(report) => {
                        apply_sftp_tree_report(&mut self.transfer_queue, &id, &report);
                        self.status_text = format!("Cancelled transfer `{id}`.");
                    }
                    SftpJobOutcome::Failed { reason, report } => {
                        let _ = self.transfer_queue.fail(&id, reason.clone());
                        if let Some(report) = report {
                            if !report.conflicts.is_empty() {
                                if let Some(spec) = spec.clone() {
                                    self.pending_sftp_conflict = Some(SftpConflictPrompt {
                                        transfer_id: id.clone(),
                                        source_text: spec.source_text(),
                                        destination_text: spec.destination_text(),
                                        conflicts: report.conflicts.clone(),
                                        policy_id: "ask".to_owned(),
                                        spec: Box::new(spec),
                                    });
                                }
                            }
                        }
                        self.status_text = format!("SFTP transfer failed: {reason}");
                    }
                }
                if moved_cleanly {
                    self.run_move_cleanup(&id);
                } else {
                    self.pending_move_cleanup.remove(&id);
                }
                // Refresh the affected panes so every finished job has a
                // visible effect (new/removed/moved rows, downloaded files).
                if spec.as_ref().is_some_and(|spec| {
                    matches!(
                        spec,
                        SftpJobSpec::UploadFile { .. }
                            | SftpJobSpec::UploadTree { .. }
                            | SftpJobSpec::RemoteCopyFile { .. }
                            | SftpJobSpec::RemoteCopyTree { .. }
                            | SftpJobSpec::RemoteMove { .. }
                            | SftpJobSpec::RemoteDelete { .. }
                            | SftpJobSpec::RemoteChmod { .. }
                    )
                }) {
                    let _ = self.refresh_active_sftp_listing();
                }
                if spec.as_ref().is_some_and(|spec| {
                    matches!(
                        spec,
                        SftpJobSpec::DownloadFile { .. } | SftpJobSpec::DownloadTree { .. }
                    )
                }) {
                    let _ = self.refresh_local_pane();
                }
            }
        }
        self.projection()
    }

    // ------------------------------------------------------- 拖动落点（DnD）

    /// Local paths dropped on a remote directory: copy, or move when
    /// `move_source` (Ctrl-drag). The local source is deleted only after the
    /// upload job succeeds.
    pub fn drop_local_paths_on_remote(
        &mut self,
        paths: &[PathBuf],
        target_dir: &str,
        move_source: bool,
    ) -> AppResult<AppProjection> {
        if paths.is_empty() {
            self.status_text = "The drag payload has no local paths.".to_owned();
            return Ok(self.projection());
        }
        let target_dir = normalize_remote_path(target_dir)?;
        let transfer_ids =
            self.submit_upload_paths(paths, Some(&target_dir), OverwritePolicy::Ask)?;
        if move_source {
            for transfer_id in transfer_ids {
                self.pending_move_cleanup
                    .insert(transfer_id, MoveCleanup::LocalPaths(paths.to_vec()));
            }
        }
        Ok(self.projection())
    }

    /// Remote entries dropped inside the remote pane: move (default gesture
    /// uses copy; Ctrl-drag maps to move) or copy into `target_dir`.
    pub fn drop_remote_entries_on_remote(
        &mut self,
        entries: &[(String, bool)],
        target_dir: &str,
        move_source: bool,
    ) -> AppResult<AppProjection> {
        if move_source {
            self.submit_remote_move_entries(entries, target_dir, OverwritePolicy::Ask)?;
        } else {
            self.submit_remote_copy_entries(entries, target_dir, OverwritePolicy::Ask)?;
        }
        Ok(self.projection())
    }

    /// Remote entries dropped on the local pane: download, or move when
    /// `move_source` (the remote source is deleted after success).
    pub fn drop_remote_entries_on_local(
        &mut self,
        entries: &[(String, bool)],
        move_source: bool,
    ) -> AppResult<AppProjection> {
        let local_dir = self.local_pane.dir.clone();
        let transfer_ids =
            self.submit_download_entries(entries, &local_dir, OverwritePolicy::Ask)?;
        if move_source {
            for transfer_id in transfer_ids {
                self.pending_move_cleanup
                    .insert(transfer_id, MoveCleanup::RemoteEntries(entries.to_vec()));
            }
        }
        Ok(self.projection())
    }

    /// Local paths dropped on the local pane: same-side copy/move inside the
    /// local filesystem.
    pub fn drop_local_paths_on_local(
        &mut self,
        paths: &[PathBuf],
        move_source: bool,
    ) -> AppProjection {
        if paths.is_empty() {
            self.status_text = "The drag payload has no local paths.".to_owned();
            return self.projection();
        }
        let destination_dir = self.local_pane.dir.clone();
        let mut changed = 0usize;
        let mut failures = Vec::new();
        for path in paths {
            let Some(name) = path.file_name() else {
                continue;
            };
            let desired = destination_dir.join(name);
            if desired == *path {
                continue;
            }
            let result = if move_source {
                fs::rename(path, &desired)
            } else {
                crate::runtime::clipboard::copy_local_entry(path, &desired)
            };
            match result {
                Ok(()) => changed += 1,
                Err(error) => failures.push(format!("{}: {error}", path.display())),
            }
        }
        self.refresh_local_pane();
        self.status_text = if failures.is_empty() {
            format!(
                "{} {changed} local entr{}.",
                if move_source { "Moved" } else { "Copied" },
                if changed == 1 { "y" } else { "ies" }
            )
        } else {
            format!(
                "{} {changed} local entr{}; {} failed: {}",
                if move_source { "Moved" } else { "Copied" },
                if changed == 1 { "y" } else { "ies" },
                failures.len(),
                failures.join("; ")
            )
        };
        self.projection()
    }

    /// OS file drops (external paths) onto a remote directory: upload.
    pub fn drop_external_paths_on_remote(
        &mut self,
        paths: &[PathBuf],
        target_dir: &str,
    ) -> AppResult<AppProjection> {
        if paths.is_empty() {
            self.status_text = "The dropped transfer has no local file paths.".to_owned();
            return Ok(self.projection());
        }
        let target_dir = normalize_remote_path(target_dir)?;
        self.submit_upload_paths(paths, Some(&target_dir), OverwritePolicy::Ask)?;
        Ok(self.projection())
    }

    /// Executes the cleanup of a successful move gesture.
    fn run_move_cleanup(&mut self, transfer_id: &str) {
        let Some(cleanup) = self.pending_move_cleanup.remove(transfer_id) else {
            return;
        };
        match cleanup {
            MoveCleanup::LocalPaths(paths) => {
                // Refresh first: the pane refresh overwrites the status line.
                self.refresh_local_pane();
                let mut removed = 0usize;
                let mut failures = Vec::new();
                for path in &paths {
                    let result = if path.is_dir() {
                        fs::remove_dir_all(path)
                    } else {
                        fs::remove_file(path)
                    };
                    match result {
                        Ok(()) => removed += 1,
                        Err(error) => failures.push(format!("{}: {error}", path.display())),
                    }
                }
                if failures.is_empty() {
                    self.status_text = format!(
                        "Moved {removed} local entr{} to the remote pane.",
                        if removed == 1 { "y" } else { "ies" }
                    );
                } else {
                    self.status_text = format!(
                        "Uploaded, but removing {}/{} local source(s) failed: {}",
                        failures.len(),
                        paths.len(),
                        failures.join("; ")
                    );
                }
            }
            MoveCleanup::RemoteEntries(entries) => {
                if let Err(error) = self.submit_remote_delete_entries(&entries) {
                    self.status_text =
                        format!("Uploaded, but the remote source cleanup failed: {error}");
                }
            }
        }
    }

    /// `(path, is_dir)` pairs of the current remote selection.
    fn selection_entry_pairs(&self) -> Vec<(String, bool)> {
        self.selected_sftp_entries()
            .into_iter()
            .map(|entry| {
                (
                    entry.path,
                    matches!(entry.kind, yshell_sftp::FsEntryKind::Directory),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    /// Runtime whose local pane points at `<temp>/pane` and whose config lives
    /// next to it (so it never shows up in local listings).
    fn runtime_with_pane(temp: &tempfile::TempDir) -> AppRuntime {
        let config = temp.path().join("yshell-config");
        fs::create_dir_all(&config).expect("config dir");
        let pane = temp.path().join("pane");
        fs::create_dir_all(&pane).expect("pane dir");
        let mut runtime = AppRuntime::new(config).expect("runtime");
        runtime.local_pane.dir = pane;
        runtime.refresh_local_pane();
        runtime
    }

    #[test]
    fn local_move_cleanup_deletes_the_source_paths() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = runtime_with_pane(&temp);
        let pane = runtime.local_pane.dir.clone();
        fs::write(pane.join("a.txt"), b"a").expect("write");
        fs::create_dir(pane.join("dir")).expect("mkdir");
        fs::write(pane.join("dir/b.txt"), b"b").expect("write");
        runtime.refresh_local_pane();

        runtime.pending_move_cleanup.insert(
            "t1".to_owned(),
            MoveCleanup::LocalPaths(vec![pane.join("a.txt"), pane.join("dir")]),
        );
        runtime.run_move_cleanup("t1");
        assert!(!pane.join("a.txt").exists());
        assert!(!pane.join("dir").exists());
        assert!(runtime.pending_move_cleanup.is_empty());
        assert!(runtime.status_text.contains("Moved 2 local"));
    }

    #[test]
    fn dropping_local_paths_on_the_local_pane_moves_them() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = runtime_with_pane(&temp);
        let pane = runtime.local_pane.dir.clone();
        let source_dir = temp.path().join("source");
        fs::create_dir_all(&source_dir).expect("mkdir");
        fs::write(source_dir.join("a.txt"), b"a").expect("write");
        runtime.refresh_local_pane();

        let projection = runtime.drop_local_paths_on_local(&[source_dir.join("a.txt")], true);
        assert!(pane.join("a.txt").exists(), "moved into the local pane dir");
        assert!(!source_dir.join("a.txt").exists(), "source removed");
        assert!(projection.local_rows.iter().any(|row| row.name == "a.txt"));
    }

    #[test]
    fn dropping_local_paths_on_the_local_pane_copies_by_default() {
        let temp = tempdir().expect("tempdir");
        let mut runtime = runtime_with_pane(&temp);
        let pane = runtime.local_pane.dir.clone();
        let source_dir = temp.path().join("source");
        fs::create_dir_all(&source_dir).expect("mkdir");
        fs::write(source_dir.join("c.txt"), b"c").expect("write");
        runtime.refresh_local_pane();

        runtime.drop_local_paths_on_local(&[source_dir.join("c.txt")], false);
        assert!(pane.join("c.txt").exists());
        assert!(source_dir.join("c.txt").exists(), "copy keeps the source");
    }
}
