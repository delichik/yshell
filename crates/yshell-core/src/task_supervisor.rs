//! Placeholder task-supervision registry.

use std::collections::BTreeSet;

/// Identifier for a supervised background task.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskId(String);

impl TaskId {
    /// Creates a task identifier from a caller-provided value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string representation of this task identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Tracks task identifiers that will later map to cancellable async tasks.
#[derive(Debug, Default)]
pub struct TaskSupervisor {
    active_tasks: BTreeSet<TaskId>,
}

impl TaskSupervisor {
    /// Creates an empty supervisor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a task and returns true if it was newly inserted.
    pub fn register(&mut self, task_id: TaskId) -> bool {
        self.active_tasks.insert(task_id)
    }

    /// Marks a task as complete/removed and returns true if it existed.
    pub fn finish(&mut self, task_id: &TaskId) -> bool {
        self.active_tasks.remove(task_id)
    }

    /// Returns true if the task is currently registered.
    #[must_use]
    pub fn contains(&self, task_id: &TaskId) -> bool {
        self.active_tasks.contains(task_id)
    }
}
