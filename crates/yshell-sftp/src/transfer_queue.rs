//! Transfer queue placeholder.

use crate::transfer_task::TransferTask;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferQueue {
    pub tasks: Vec<TransferTask>,
}
