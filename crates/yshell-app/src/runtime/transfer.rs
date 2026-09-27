//! SFTP transfer queue projection and operations.
//!
//! The queue lives in `yshell_sftp::TransferQueue`; this module folds it into
//! the shapes N1 needs: a row model for the transfer drawer, pause/resume/
//! retry/cancel/remove operations, and the glue between F0 tree transfers
//! (progress, cancel flag, report) and queue task state.

use crate::sftp_view::format_sftp_size;
use yshell_sftp::{
    OverwritePolicy, TransferDirection, TransferQueue, TransferStatus, TransferTask,
    TreeTransferReport,
};

use super::*;

impl AppRuntime {
    pub(crate) fn enqueue_sftp_transfer(
        &mut self,
        direction: TransferDirection,
        source_path: String,
        destination_path: String,
        total_bytes: Option<u64>,
    ) -> String {
        let transfer_id = format!("sftp-transfer-{}", self.next_transfer_ordinal);
        self.next_transfer_ordinal += 1;
        self.transfer_queue.enqueue(TransferTask::new(
            transfer_id.clone(),
            direction,
            source_path,
            destination_path,
            total_bytes,
        ));
        let _ = self.transfer_queue.start_next();
        transfer_id
    }

    pub(crate) fn complete_sftp_transfer(&mut self, transfer_id: &str, bytes_done: u64) {
        let _ = self.transfer_queue.complete(transfer_id, bytes_done);
    }

    pub(crate) fn fail_sftp_transfer(&mut self, transfer_id: &str, reason: String) {
        let _ = self.transfer_queue.fail(transfer_id, reason);
    }

    pub(crate) fn transfer_queue_parts(&self) -> (String, bool) {
        if self.transfer_queue.tasks.is_empty() {
            return (String::new(), true);
        }
        let rows = self
            .transfer_queue
            .tasks
            .iter()
            .rev()
            .take(5)
            .map(|task| {
                let progress = task
                    .progress_percent()
                    .map(|percent| format!("{percent}%"))
                    .unwrap_or_else(|| format!("{} B", task.bytes_done));
                let error = task
                    .last_error
                    .as_ref()
                    .map(|error| format!(" error={error}"))
                    .unwrap_or_default();
                format!(
                    "{} {} {} {} {} -> {}{}",
                    task.id,
                    transfer_direction_label(task.direction),
                    transfer_status_label(task.status),
                    progress,
                    task.source_path,
                    task.destination_path,
                    error
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        (rows, false)
    }
}

/// A transfer that stopped on Ask-policy conflicts: the conflict dialog shows
/// the paths and re-runs the stored spec with an explicit policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpConflictPrompt {
    pub transfer_id: String,
    pub source_text: String,
    pub destination_text: String,
    pub conflicts: Vec<String>,
    /// Policy the transfer ran with (always `ask` today; shown in the dialog).
    pub policy_id: String,
    pub spec: Box<crate::sftp_jobs::SftpJobSpec>,
}

/// N1 Phase 2 additions: the transfer drawer row model and the queue menu
/// operations used by `sftp_panel.slint` / `bootstrap.rs`.
impl AppRuntime {
    /// Row model for the transfer drawer (N1 §1): newest first, with the flags
    /// the row context menu needs to enable/disable each queue action.
    pub fn sftp_transfer_rows(&self) -> Vec<TransferRowData> {
        self.transfer_queue
            .tasks
            .iter()
            .rev()
            .map(transfer_row_data)
            .collect()
    }

    /// Counts for the drawer summary line.
    pub fn sftp_transfer_counts(&self) -> TransferQueueCounts {
        let mut counts = TransferQueueCounts {
            total: self.transfer_queue.tasks.len(),
            ..TransferQueueCounts::default()
        };
        for task in &self.transfer_queue.tasks {
            match task.status {
                TransferStatus::Queued => counts.queued += 1,
                TransferStatus::Running => counts.running += 1,
                TransferStatus::Paused => counts.paused += 1,
                TransferStatus::Completed => counts.completed += 1,
                TransferStatus::Failed => counts.failed += 1,
                TransferStatus::Cancelled => counts.cancelled += 1,
            }
            if task.status == TransferStatus::Failed && task.retry_count < task.max_retries {
                counts.retryable += 1;
            }
        }
        counts.active = counts.queued + counts.running + counts.paused;
        counts
    }

    /// Pauses a queued/running transfer.
    pub fn pause_sftp_transfer(&mut self, transfer_id: &str) -> AppResult<AppProjection> {
        let changed = self
            .transfer_queue
            .pause(transfer_id)
            .map_err(AppError::from_error)?;
        self.status_text = if changed {
            format!("Paused transfer `{transfer_id}`.")
        } else {
            format!("Transfer `{transfer_id}` is not queued or running, so it was not paused.")
        };
        Ok(self.projection())
    }

    /// Puts a paused transfer back into the queue.
    pub fn resume_sftp_transfer(&mut self, transfer_id: &str) -> AppResult<AppProjection> {
        let changed = self
            .transfer_queue
            .resume(transfer_id)
            .map_err(AppError::from_error)?;
        if changed {
            let _ = self.transfer_queue.start_next();
            self.status_text = format!("Resumed transfer `{transfer_id}`.");
        } else {
            self.status_text =
                format!("Transfer `{transfer_id}` is not paused, so it was not resumed.");
        }
        Ok(self.projection())
    }

    /// Re-queues a failed transfer while retries are left.
    pub fn retry_sftp_transfer(&mut self, transfer_id: &str) -> AppResult<AppProjection> {
        let task = self.sftp_transfer_task(transfer_id)?;
        let (status, retry_count, max_retries) = (task.status, task.retry_count, task.max_retries);
        let retried = self
            .transfer_queue
            .retry(transfer_id)
            .map_err(AppError::from_error)?;
        if retried {
            let _ = self.transfer_queue.start_next();
            self.status_text = format!("Re-queued transfer `{transfer_id}` for another attempt.");
        } else if status == TransferStatus::Failed {
            self.status_text = format!(
                "Transfer `{transfer_id}` already used its {retry_count}/{max_retries} retries."
            );
        } else {
            self.status_text = format!(
                "Only failed transfers can be retried; `{transfer_id}` is {}.",
                transfer_status_label(status)
            );
        }
        Ok(self.projection())
    }

    /// Cancels a transfer that is still in flight.
    pub fn cancel_sftp_transfer(&mut self, transfer_id: &str) -> AppResult<AppProjection> {
        let status = self.sftp_transfer_task(transfer_id)?.status;
        if matches!(
            status,
            TransferStatus::Completed | TransferStatus::Cancelled
        ) {
            self.status_text = format!(
                "Transfer `{transfer_id}` is already {}, so it was not cancelled.",
                transfer_status_label(status)
            );
            return Ok(self.projection());
        }
        self.transfer_queue
            .cancel(transfer_id)
            .map_err(AppError::from_error)?;
        // Also flip the worker's cancel flag so an in-flight F0 tree transfer
        // stops between items (single-file transfers finish their current file).
        if let Some(handle) = self.sftp_jobs.as_mut() {
            handle.cancel(transfer_id);
        }
        self.status_text = format!("Cancelled transfer `{transfer_id}`.");
        Ok(self.projection())
    }

    /// Removes a transfer from the queue regardless of status.
    pub fn remove_sftp_transfer(&mut self, transfer_id: &str) -> AppResult<AppProjection> {
        self.transfer_queue
            .remove(transfer_id)
            .map_err(AppError::from_error)?;
        self.status_text = format!("Removed transfer `{transfer_id}` from the queue.");
        Ok(self.projection())
    }

    /// Drops every completed transfer; returns how many were cleared.
    pub fn clear_completed_sftp_transfers(&mut self) -> AppResult<AppProjection> {
        let removed = self.transfer_queue.clear_completed();
        self.status_text = if removed == 0 {
            "No completed transfers to clear.".to_owned()
        } else {
            format!("Cleared {removed} completed transfer(s).")
        };
        Ok(self.projection())
    }

    fn sftp_transfer_task(&self, transfer_id: &str) -> AppResult<&TransferTask> {
        self.transfer_queue
            .tasks
            .iter()
            .find(|task| task.id == transfer_id)
            .ok_or_else(|| AppError::new(format!("unknown transfer `{transfer_id}`")))
    }
}

/// One transfer-drawer row, ready for the Slint model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferRowData {
    pub id: String,
    pub direction_id: String,
    pub direction_text: String,
    /// The file name shown as the row title (source side of the transfer).
    pub file_name_text: String,
    pub source_text: String,
    pub destination_text: String,
    pub status_id: String,
    pub status_text: String,
    pub progress_text: String,
    /// `-1` when the total size is unknown.
    pub progress_percent: i32,
    pub error_text: String,
    pub retry_count: i32,
    pub max_retries: i32,
    pub can_pause: bool,
    pub can_resume: bool,
    pub can_retry: bool,
    pub can_cancel: bool,
    pub can_remove: bool,
    pub is_active: bool,
    pub is_failed: bool,
    pub is_completed: bool,
}

/// Aggregate queue counts for the drawer summary line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransferQueueCounts {
    pub total: usize,
    pub queued: usize,
    pub running: usize,
    pub paused: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
    /// queued + running + paused
    pub active: usize,
    /// failed tasks that still have retries left
    pub retryable: usize,
}

/// Pushes one F0 tree-progress event into its queue task.
///
/// The task is enqueued without a known total; the first progress event
/// upgrades it to the F0 pre-scan estimate (so the drawer can show a percent)
/// and promotes a queued task to running. Completion is decided by the final
/// [`TreeTransferReport`], not by progress, because Ask-policy conflicts and
/// item failures also end a transfer.
pub(crate) fn update_sftp_transfer_progress(
    queue: &mut TransferQueue,
    transfer_id: &str,
    bytes_done: u64,
    bytes_total: Option<u64>,
) -> bool {
    let Some(task) = queue.tasks.iter_mut().find(|task| task.id == transfer_id) else {
        return false;
    };
    if let Some(total) = bytes_total.filter(|total| *total > 0) {
        task.total_bytes = Some(total);
    }
    task.bytes_done = match task.total_bytes {
        Some(total) => bytes_done.min(total),
        None => bytes_done,
    };
    if task.status == TransferStatus::Queued {
        task.status = TransferStatus::Running;
    }
    true
}

/// Final queue state for a finished F0 tree transfer.
///
/// * cancelled transfers stay cancelled;
/// * item failures fail the task with the first reason;
/// * Ask-policy conflicts also fail the task — the N1 flow prompts the user
///   and re-runs with an explicit policy (overwrite/rename/skip).
pub(crate) fn queue_task_state_for_report(
    report: &TreeTransferReport,
) -> (TransferStatus, Option<String>) {
    if report.cancelled {
        return (TransferStatus::Cancelled, None);
    }
    if let Some((path, reason)) = report.failed.first() {
        return (
            TransferStatus::Failed,
            Some(format!(
                "{} item(s) failed; first: `{path}` ({reason})",
                report.failed.len()
            )),
        );
    }
    if !report.conflicts.is_empty() {
        return (
            TransferStatus::Failed,
            Some(format!(
                "{} name conflict(s); choose overwrite, rename or skip and retry",
                report.conflicts.len()
            )),
        );
    }
    (TransferStatus::Completed, None)
}

/// Applies a finished F0 report to its queue task.
pub(crate) fn apply_sftp_tree_report(
    queue: &mut TransferQueue,
    transfer_id: &str,
    report: &TreeTransferReport,
) {
    let (status, error) = queue_task_state_for_report(report);
    match status {
        TransferStatus::Completed => {
            let _ = queue.complete(transfer_id, report.bytes_transferred);
        }
        TransferStatus::Cancelled => {
            let _ = queue.cancel(transfer_id);
        }
        TransferStatus::Failed => {
            let _ = queue.fail(transfer_id, error.unwrap_or_default());
        }
        TransferStatus::Queued | TransferStatus::Running | TransferStatus::Paused => {}
    }
}

/// Human-readable result of a finished tree transfer (legacy status channel).
pub(crate) fn tree_transfer_status_text(
    report: &TreeTransferReport,
    direction: TransferDirection,
    destination: &str,
) -> String {
    let verb = match direction {
        TransferDirection::Upload => "Uploaded",
        TransferDirection::Download => "Downloaded",
    };
    format!(
        "{verb} tree `{destination}` through SFTP: {} completed · {} skipped · {} failed · {} conflict(s) · {}.",
        report.completed,
        report.skipped,
        report.failed.len(),
        report.conflicts.len(),
        format_sftp_size(report.bytes_transferred)
    )
}

/// Builds one drawer row from a queue task.
fn transfer_row_data(task: &TransferTask) -> TransferRowData {
    let progress_percent = task.progress_percent();
    let status_id = transfer_status_label(task.status);
    TransferRowData {
        id: task.id.clone(),
        direction_id: transfer_direction_label(task.direction).to_owned(),
        direction_text: transfer_direction_label(task.direction).to_owned(),
        file_name_text: transfer_file_name(&task.source_path),
        source_text: task.source_path.clone(),
        destination_text: task.destination_path.clone(),
        status_id: status_id.to_owned(),
        status_text: status_id.to_owned(),
        progress_text: match progress_percent {
            Some(percent) => format!("{percent}%"),
            None => format_sftp_size(task.bytes_done),
        },
        progress_percent: progress_percent.map(i32::from).unwrap_or(-1),
        error_text: task.last_error.clone().unwrap_or_default(),
        retry_count: i32::from(task.retry_count),
        max_retries: i32::from(task.max_retries),
        can_pause: matches!(
            task.status,
            TransferStatus::Queued | TransferStatus::Running
        ),
        can_resume: task.status == TransferStatus::Paused,
        can_retry: task.status == TransferStatus::Failed && task.retry_count < task.max_retries,
        can_cancel: !matches!(
            task.status,
            TransferStatus::Completed | TransferStatus::Cancelled
        ),
        can_remove: true,
        is_active: matches!(
            task.status,
            TransferStatus::Queued | TransferStatus::Running | TransferStatus::Paused
        ),
        is_failed: task.status == TransferStatus::Failed,
        is_completed: task.status == TransferStatus::Completed,
    }
}

fn transfer_file_name(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .to_owned()
}

/// Slint callback id for an overwrite policy (`ask`/`overwrite`/`rename`/`skip`).
pub fn overwrite_policy_id(policy: OverwritePolicy) -> &'static str {
    match policy {
        OverwritePolicy::Ask => "ask",
        OverwritePolicy::Overwrite => "overwrite",
        OverwritePolicy::Rename => "rename",
        OverwritePolicy::Skip => "skip",
    }
}

/// Parses the overwrite policy id sent by the conflict dialog callbacks.
/// `overwrite-all` is accepted as an alias so a "apply to all" choice can reuse
/// the same channel.
pub fn overwrite_policy_from_id(id: &str) -> Option<OverwritePolicy> {
    match id.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "ask" => Some(OverwritePolicy::Ask),
        "overwrite" | "overwrite-all" => Some(OverwritePolicy::Overwrite),
        "rename" => Some(OverwritePolicy::Rename),
        "skip" | "skip-all" => Some(OverwritePolicy::Skip),
        _ => None,
    }
}

/// Display label for an overwrite policy (conflict dialog radio labels).
pub fn overwrite_policy_label(policy: OverwritePolicy) -> &'static str {
    match policy {
        OverwritePolicy::Ask => "Ask",
        OverwritePolicy::Overwrite => "Overwrite",
        OverwritePolicy::Rename => "Rename",
        OverwritePolicy::Skip => "Skip",
    }
}

pub(crate) fn transfer_direction_label(direction: TransferDirection) -> &'static str {
    match direction {
        TransferDirection::Upload => "upload",
        TransferDirection::Download => "download",
    }
}

pub(crate) fn transfer_status_label(status: TransferStatus) -> &'static str {
    match status {
        TransferStatus::Queued => "queued",
        TransferStatus::Running => "running",
        TransferStatus::Paused => "paused",
        TransferStatus::Completed => "completed",
        TransferStatus::Failed => "failed",
        TransferStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use yshell_sftp::{
        OverwritePolicy, TransferDirection, TransferQueue, TransferStatus, TreeTransferReport,
    };

    use super::{
        apply_sftp_tree_report, overwrite_policy_from_id, overwrite_policy_id,
        queue_task_state_for_report, tree_transfer_status_text, update_sftp_transfer_progress,
        AppRuntime,
    };

    fn runtime() -> (tempfile::TempDir, AppRuntime) {
        let temp = tempdir().expect("tempdir");
        let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        (temp, runtime)
    }

    #[test]
    fn overwrite_policy_ids_round_trip_for_the_conflict_dialog() {
        for policy in [
            OverwritePolicy::Ask,
            OverwritePolicy::Overwrite,
            OverwritePolicy::Rename,
            OverwritePolicy::Skip,
        ] {
            assert_eq!(
                overwrite_policy_from_id(overwrite_policy_id(policy)),
                Some(policy)
            );
        }
        assert_eq!(
            overwrite_policy_from_id("overwrite-all"),
            Some(OverwritePolicy::Overwrite)
        );
        assert_eq!(
            overwrite_policy_from_id(" Overwrite "),
            Some(OverwritePolicy::Overwrite)
        );
        assert_eq!(overwrite_policy_from_id("nope"), None);
    }

    #[test]
    fn queue_rows_and_counts_track_pause_resume_retry_and_remove() {
        let (_temp, mut runtime) = runtime();
        let first = runtime.enqueue_sftp_transfer(
            TransferDirection::Upload,
            "/local/report.txt".to_owned(),
            "/srv/report.txt".to_owned(),
            Some(100),
        );
        let second = runtime.enqueue_sftp_transfer(
            TransferDirection::Download,
            "/srv/logs/app.log".to_owned(),
            "/local/app.log".to_owned(),
            None,
        );

        // Newest first. `enqueue` starts the next queued task, so both tasks
        // are running while the (synchronous) UI thread is not blocked in a
        // transfer.
        let rows = runtime.sftp_transfer_rows();
        assert_eq!(rows[0].id, second);
        assert_eq!(rows[0].status_id, "running");
        assert_eq!(rows[0].file_name_text, "app.log");
        assert_eq!(rows[0].progress_text, "0 B");
        assert_eq!(rows[0].progress_percent, -1);
        assert_eq!(rows[1].id, first);
        assert_eq!(rows[1].status_id, "running");
        assert!(rows[1].is_active);
        assert!(rows[1].can_pause);
        assert!(!rows[1].can_retry);

        runtime.pause_sftp_transfer(&first).expect("pause");
        let paused = runtime.sftp_transfer_rows();
        assert_eq!(paused[1].status_id, "paused");
        assert!(paused[1].can_resume);
        assert!(!paused[1].can_pause);
        assert_eq!(runtime.sftp_transfer_counts().paused, 1);
        assert_eq!(runtime.sftp_transfer_counts().active, 2);

        runtime.resume_sftp_transfer(&first).expect("resume");
        assert_eq!(runtime.sftp_transfer_rows()[1].status_id, "running");

        runtime.fail_sftp_transfer(&first, "network".to_owned());
        let failed = runtime.sftp_transfer_rows();
        assert!(failed[1].is_failed);
        assert!(failed[1].can_retry);
        assert!(failed[1].can_cancel);
        assert_eq!(failed[1].error_text, "network");
        assert_eq!(runtime.sftp_transfer_counts().failed, 1);
        assert_eq!(runtime.sftp_transfer_counts().retryable, 1);

        runtime.retry_sftp_transfer(&first).expect("retry");
        assert_eq!(runtime.sftp_transfer_rows()[1].status_id, "running");

        runtime
            .remove_sftp_transfer(&second)
            .expect("remove queued");
        assert_eq!(runtime.sftp_transfer_counts().total, 1);

        runtime.cancel_sftp_transfer(&first).expect("cancel");
        assert_eq!(runtime.sftp_transfer_rows()[0].status_id, "cancelled");
        // Cancelled rows stay removable but are not retryable.
        assert!(runtime.sftp_transfer_rows()[0].can_remove);
        assert!(!runtime.sftp_transfer_rows()[0].can_retry);
        assert!(runtime.remove_sftp_transfer("sftp-transfer-404").is_err());
    }

    #[test]
    fn clear_completed_reports_how_many_rows_disappeared() {
        let (_temp, mut runtime) = runtime();
        // The legacy summary row stays empty until a task exists.
        assert!(
            runtime
                .clear_completed_sftp_transfers()
                .expect("clear")
                .transfer_queue_empty
        );

        let id = runtime.enqueue_sftp_transfer(
            TransferDirection::Upload,
            "a".to_owned(),
            "/srv/a".to_owned(),
            Some(1),
        );
        runtime.complete_sftp_transfer(&id, 1);
        let projection = runtime.clear_completed_sftp_transfers().expect("clear");
        assert!(projection.transfer_queue_empty);
        assert_eq!(runtime.sftp_transfer_counts().total, 0);
        assert!(runtime.status_text.contains("Cleared 1 completed transfer"));
    }

    #[test]
    fn tree_progress_upgrades_the_total_and_promotes_a_queued_task() {
        let (_temp, mut runtime) = runtime();
        let id = runtime.enqueue_sftp_transfer(
            TransferDirection::Upload,
            "/local/tree".to_owned(),
            "/srv/tree".to_owned(),
            None,
        );
        assert!(update_sftp_transfer_progress(
            &mut runtime.transfer_queue,
            &id,
            40,
            Some(100)
        ));
        let row = runtime.sftp_transfer_rows()[0].clone();
        assert_eq!(row.status_id, "running");
        assert_eq!(row.progress_text, "40%");
        assert_eq!(row.progress_percent, 40);
        // A later event cannot exceed the pre-scan estimate.
        update_sftp_transfer_progress(&mut runtime.transfer_queue, &id, 130, Some(100));
        assert_eq!(runtime.sftp_transfer_rows()[0].progress_percent, 100);
        assert!(!update_sftp_transfer_progress(
            &mut runtime.transfer_queue,
            "missing",
            1,
            None
        ));
    }

    #[test]
    fn tree_reports_map_to_queue_state() {
        let mut report = TreeTransferReport {
            completed: 3,
            bytes_transferred: 1024,
            ..TreeTransferReport::default()
        };
        assert_eq!(
            queue_task_state_for_report(&report),
            (TransferStatus::Completed, None)
        );

        report
            .failed
            .push(("/srv/x".to_owned(), "permission denied".to_owned()));
        let (status, error) = queue_task_state_for_report(&report);
        assert_eq!(status, TransferStatus::Failed);
        assert!(error.expect("reason").contains("permission denied"));

        let conflicts = TreeTransferReport {
            conflicts: vec!["/srv/a".to_owned(), "/srv/b".to_owned()],
            ..TreeTransferReport::default()
        };
        let (status, error) = queue_task_state_for_report(&conflicts);
        assert_eq!(status, TransferStatus::Failed);
        assert!(error.expect("reason").contains("2 name conflict"));

        let cancelled = TreeTransferReport {
            cancelled: true,
            ..TreeTransferReport::default()
        };
        assert_eq!(
            queue_task_state_for_report(&cancelled),
            (TransferStatus::Cancelled, None)
        );
    }

    #[test]
    fn applying_a_tree_report_updates_the_queue_task() {
        let (_temp, mut runtime) = runtime();
        let id = runtime.enqueue_sftp_transfer(
            TransferDirection::Upload,
            "/local/tree".to_owned(),
            "/srv/tree".to_owned(),
            None,
        );

        let report = TreeTransferReport {
            completed: 2,
            skipped: 1,
            bytes_transferred: 4096,
            ..TreeTransferReport::default()
        };
        apply_sftp_tree_report(&mut runtime.transfer_queue, &id, &report);
        let row = runtime.sftp_transfer_rows()[0].clone();
        assert!(row.is_completed);
        assert_eq!(row.progress_text, "4.0 KB");

        let failing = TreeTransferReport {
            failed: vec![("/srv/tree/a".to_owned(), "io".to_owned())],
            bytes_transferred: 10,
            ..TreeTransferReport::default()
        };
        apply_sftp_tree_report(&mut runtime.transfer_queue, &id, &failing);
        let row = runtime.sftp_transfer_rows()[0].clone();
        assert!(row.is_failed);
        assert!(row.can_retry);
        assert!(row.error_text.contains("/srv/tree/a"));

        let cancelled = TreeTransferReport {
            cancelled: true,
            ..TreeTransferReport::default()
        };
        apply_sftp_tree_report(&mut runtime.transfer_queue, &id, &cancelled);
        assert_eq!(runtime.sftp_transfer_rows()[0].status_id, "cancelled");
    }

    #[test]
    fn tree_status_text_counts_every_outcome() {
        let report = TreeTransferReport {
            completed: 4,
            skipped: 2,
            failed: vec![("/srv/a".to_owned(), "x".to_owned())],
            conflicts: vec!["/srv/b".to_owned()],
            bytes_transferred: 2048,
            cancelled: false,
            skipped_items: Vec::new(),
        };
        let text = tree_transfer_status_text(&report, TransferDirection::Download, "/local/dir");
        assert!(text.contains("Downloaded tree `/local/dir`"));
        assert!(text.contains("4 completed"));
        assert!(text.contains("2 skipped"));
        assert!(text.contains("1 failed"));
        assert!(text.contains("1 conflict"));
        assert!(text.contains("2.0 KB"));
    }

    #[test]
    fn progress_events_for_unknown_tasks_are_ignored() {
        let mut queue = TransferQueue::default();
        assert!(!update_sftp_transfer_progress(
            &mut queue, "missing", 5, None
        ));
        assert!(queue.tasks.is_empty());
    }
}
