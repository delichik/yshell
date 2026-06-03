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

    pub fn retry(&mut self, id: &str) -> SftpResult<bool> {
        let task = self.task_mut(id)?;
        if task.status != TransferStatus::Failed || task.retry_count >= task.max_retries {
            return Ok(false);
        }
        task.retry_count += 1;
        task.status = TransferStatus::Queued;
        task.last_error = None;
        Ok(true)
    }

    pub fn cancel(&mut self, id: &str) -> SftpResult<()> {
        let task = self.task_mut(id)?;
        task.status = TransferStatus::Cancelled;
        Ok(())
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
        queue.fail("t1", "network").unwrap();
        assert!(queue.retry("t1").unwrap());
        queue.cancel("t1").unwrap();
        assert_eq!(queue.tasks[0].status, TransferStatus::Cancelled);
    }
}
