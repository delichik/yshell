//! Transfer queue state machine.

use crate::error::{SftpError, SftpErrorKind, SftpResult};
use crate::transfer_task::{TransferStatus, TransferTask};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferQueue {
    pub tasks: Vec<TransferTask>,
}

impl TransferQueue {
    pub fn enqueue(&mut self, task: TransferTask) {
        self.tasks.push(task);
    }

    pub fn start_next(&mut self) -> Option<&TransferTask> {
        let task = self
            .tasks
            .iter_mut()
            .find(|task| task.status == TransferStatus::Queued)?;
        task.status = TransferStatus::Running;
        Some(task)
    }

    pub fn update_progress(&mut self, id: &str, bytes_done: u64) -> SftpResult<()> {
        let task = self.task_mut(id)?;
        task.bytes_done = match task.total_bytes {
            Some(total) => bytes_done.min(total),
            None => bytes_done,
        };
        if task
            .total_bytes
            .is_some_and(|total| task.bytes_done >= total)
        {
            task.status = TransferStatus::Completed;
        }
        Ok(())
    }

    pub fn fail(&mut self, id: &str, reason: impl Into<String>) -> SftpResult<()> {
        let task = self.task_mut(id)?;
        task.status = TransferStatus::Failed;
        task.last_error = Some(reason.into());
        Ok(())
    }

    pub fn complete(&mut self, id: &str, bytes_done: u64) -> SftpResult<()> {
        let task = self.task_mut(id)?;
        task.bytes_done = match task.total_bytes {
            Some(total) => bytes_done.min(total),
            None => bytes_done,
        };
        task.status = TransferStatus::Completed;
        Ok(())
    }

    /// Re-queue a failed task for a fresh attempt.
    ///
    /// Retries are capped at [`TransferTask::max_retries`]: once the cap is
    /// reached this returns `false` and the task stays failed. Every attempt
    /// starts from zero progress and may fail again, consuming another retry.
    pub fn retry(&mut self, id: &str) -> SftpResult<bool> {
        let task = self.task_mut(id)?;
        if task.status != TransferStatus::Failed || task.retry_count >= task.max_retries {
            return Ok(false);
        }
        task.retry_count += 1;
        task.status = TransferStatus::Queued;
        task.bytes_done = 0;
        task.last_error = None;
        Ok(true)
    }

    pub fn cancel(&mut self, id: &str) -> SftpResult<()> {
        let task = self.task_mut(id)?;
        task.status = TransferStatus::Cancelled;
        Ok(())
    }

    /// Pause a queued or running task. Returns `true` when the state changed.
    pub fn pause(&mut self, id: &str) -> SftpResult<bool> {
        let task = self.task_mut(id)?;
        if !matches!(
            task.status,
            TransferStatus::Queued | TransferStatus::Running
        ) {
            return Ok(false);
        }
        task.status = TransferStatus::Paused;
        Ok(true)
    }

    /// Put a paused task back into the queue (the runner picks it up again via
    /// [`TransferQueue::start_next`]). Returns `true` when the state changed.
    pub fn resume(&mut self, id: &str) -> SftpResult<bool> {
        let task = self.task_mut(id)?;
        if task.status != TransferStatus::Paused {
            return Ok(false);
        }
        task.status = TransferStatus::Queued;
        Ok(true)
    }

    /// Remove a task from the queue regardless of its status.
    pub fn remove(&mut self, id: &str) -> SftpResult<()> {
        let before = self.tasks.len();
        self.tasks.retain(|task| task.id != id);
        if self.tasks.len() == before {
            return Err(SftpError::new(
                SftpErrorKind::NotFound,
                format!("unknown transfer {id}"),
            ));
        }
        Ok(())
    }

    /// Drop every completed task, returning how many were removed.
    pub fn clear_completed(&mut self) -> usize {
        let before = self.tasks.len();
        self.tasks
            .retain(|task| task.status != TransferStatus::Completed);
        before - self.tasks.len()
    }

    /// True when a failed task has retries left.
    pub fn can_retry(&self, id: &str) -> SftpResult<bool> {
        let task = self
            .tasks
            .iter()
            .find(|task| task.id == id)
            .ok_or_else(|| {
                SftpError::new(SftpErrorKind::NotFound, format!("unknown transfer {id}"))
            })?;
        Ok(task.status == TransferStatus::Failed && task.retry_count < task.max_retries)
    }

    fn task_mut(&mut self, id: &str) -> SftpResult<&mut TransferTask> {
        self.tasks
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| {
                SftpError::new(SftpErrorKind::NotFound, format!("unknown transfer {id}"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer_task::TransferDirection;

    #[test]
    fn retries_cancel_and_progresses_tasks() {
        let mut queue = TransferQueue::default();
        queue.enqueue(TransferTask::new(
            "t1",
            TransferDirection::Download,
            "/remote/a",
            "a",
            Some(10),
        ));
        queue.start_next();
        queue.update_progress("t1", 5).unwrap();
        assert_eq!(queue.tasks[0].progress_percent(), Some(50));
        queue.complete("t1", 10).unwrap();
        assert_eq!(queue.tasks[0].status, TransferStatus::Completed);
        queue.fail("t1", "network").unwrap();
        assert!(queue.retry("t1").unwrap());
        queue.cancel("t1").unwrap();
        assert_eq!(queue.tasks[0].status, TransferStatus::Cancelled);
    }

    #[test]
    fn pause_resume_remove_and_completion_cleanup() {
        let mut queue = TransferQueue::default();
        queue.enqueue(TransferTask::new(
            "running",
            TransferDirection::Upload,
            "a",
            "/remote/a",
            Some(4),
        ));
        queue.enqueue(TransferTask::new(
            "queued",
            TransferDirection::Download,
            "/remote/b",
            "b",
            None,
        ));
        assert_eq!(queue.start_next().map(|task| task.id.as_str()), Some("running"));

        assert!(queue.pause("running").unwrap());
        assert_eq!(queue.tasks[0].status, TransferStatus::Paused);
        // A paused task is not handed out again by the runner.
        assert_eq!(queue.start_next().map(|task| task.id.as_str()), Some("queued"));
        assert!(!queue.pause("running").unwrap());

        assert!(queue.resume("running").unwrap());
        assert_eq!(queue.tasks[0].status, TransferStatus::Queued);
        assert!(!queue.resume("running").unwrap());
        assert!(queue.pause("queued").unwrap());
        assert!(queue.resume("missing").is_err());

        queue.complete("queued", 0).unwrap();
        assert_eq!(queue.clear_completed(), 1);
        assert_eq!(queue.tasks.len(), 1);
        assert_eq!(queue.clear_completed(), 0);

        queue.remove("running").unwrap();
        assert!(queue.tasks.is_empty());
        assert!(queue.remove("running").is_err());
    }

    #[test]
    fn retry_is_capped_and_a_retried_task_can_fail_again() {
        let mut queue = TransferQueue::default();
        let mut task = TransferTask::new(
            "t1",
            TransferDirection::Upload,
            "a",
            "/remote/a",
            Some(100),
        );
        task.max_retries = 2;
        queue.enqueue(task);
        queue.start_next();
        queue.update_progress("t1", 40).unwrap();

        queue.fail("t1", "first").unwrap();
        assert!(queue.can_retry("t1").unwrap());
        assert!(queue.retry("t1").unwrap());
        let task = &queue.tasks[0];
        assert_eq!(task.status, TransferStatus::Queued);
        assert_eq!(task.retry_count, 1);
        assert_eq!(task.bytes_done, 0);
        assert!(task.last_error.is_none());

        // Same task fails again after the retry: still allowed one more retry.
        queue.start_next();
        queue.fail("t1", "second").unwrap();
        assert!(queue.can_retry("t1").unwrap());
        assert!(queue.retry("t1").unwrap());
        assert_eq!(queue.tasks[0].retry_count, 2);

        // Third failure hits the cap: the task stays failed.
        queue.start_next();
        queue.fail("t1", "third").unwrap();
        assert!(!queue.can_retry("t1").unwrap());
        assert!(!queue.retry("t1").unwrap());
        assert_eq!(queue.tasks[0].status, TransferStatus::Failed);
        assert_eq!(queue.tasks[0].last_error.as_deref(), Some("third"));

        // Completed tasks cannot be retried either.
        queue.complete("t1", 100).unwrap();
        assert!(!queue.retry("t1").unwrap());
    }
}
