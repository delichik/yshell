//! Workspace split-pane model used by terminal tabs.

use std::{error::Error, fmt};

use crate::SessionId;

const MAX_LEAVES: usize = 4;

/// A stable leaf/pane identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(u64);

impl PaneId {
    /// Creates a pane id from a caller-provided value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw id value.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Split orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

/// Workspace split tree.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceNode {
    Leaf {
        pane_id: PaneId,
        session_id: Option<SessionId>,
    },
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<WorkspaceNode>,
        second: Box<WorkspaceNode>,
    },
}

impl WorkspaceNode {
    fn count_leaves(&self) -> usize {
        match self {
            Self::Leaf { .. } => 1,
            Self::Split { first, second, .. } => first.count_leaves() + second.count_leaves(),
        }
    }

    fn contains(&self, pane_id: PaneId) -> bool {
        match self {
            Self::Leaf { pane_id: id, .. } => *id == pane_id,
            Self::Split { first, second, .. } => {
                first.contains(pane_id) || second.contains(pane_id)
            }
        }
    }

    fn split_leaf(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        direction: SplitDirection,
        ratio: f32,
        session_id: Option<SessionId>,
    ) -> bool {
        match self {
            Self::Leaf { pane_id, .. } if *pane_id == target => {
                let old = std::mem::replace(
                    self,
                    Self::Leaf {
                        pane_id: target,
                        session_id: None,
                    },
                );
                *self = Self::Split {
                    direction,
                    ratio,
                    first: Box::new(old),
                    second: Box::new(Self::Leaf {
                        pane_id: new_pane,
                        session_id,
                    }),
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => {
                first.split_leaf(target, new_pane, direction, ratio, session_id.clone())
                    || second.split_leaf(target, new_pane, direction, ratio, session_id)
            }
        }
    }

    fn close_leaf(&mut self, target: PaneId) -> bool {
        match self {
            Self::Leaf { pane_id, .. } => *pane_id == target,
            Self::Split { first, second, .. } => {
                if first.contains(target) && first.close_leaf(target) {
                    let replacement = (**second).clone();
                    *self = replacement;
                    true
                } else if second.contains(target) && second.close_leaf(target) {
                    let replacement = (**first).clone();
                    *self = replacement;
                    true
                } else {
                    false
                }
            }
        }
    }

    fn set_ratio(&mut self, target: PaneId, ratio: f32) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split {
                ratio: split_ratio,
                first,
                second,
                ..
            } => {
                if first.contains(target) || second.contains(target) {
                    *split_ratio = ratio;
                    true
                } else {
                    first.set_ratio(target, ratio) || second.set_ratio(target, ratio)
                }
            }
        }
    }

    fn pane_ids(&self, ids: &mut Vec<PaneId>) {
        match self {
            Self::Leaf { pane_id, .. } => ids.push(*pane_id),
            Self::Split { first, second, .. } => {
                first.pane_ids(ids);
                second.pane_ids(ids);
            }
        }
    }
}

/// Split-pane model for one workspace/tab.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceLayout {
    root: WorkspaceNode,
    next_pane_id: u64,
}

impl WorkspaceLayout {
    /// Creates a layout with a single empty leaf.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            root: WorkspaceNode::Leaf {
                pane_id: PaneId(1),
                session_id: None,
            },
            next_pane_id: 2,
        }
    }

    /// Returns the root split tree.
    #[must_use]
    pub const fn root(&self) -> &WorkspaceNode {
        &self.root
    }

    /// Returns current leaf count.
    #[must_use]
    pub fn leaf_count(&self) -> usize {
        self.root.count_leaves()
    }

    /// Returns pane ids in visual tree order.
    #[must_use]
    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        self.root.pane_ids(&mut ids);
        ids
    }

    /// Splits an existing leaf, returning the new pane id.
    pub fn split(
        &mut self,
        target: PaneId,
        direction: SplitDirection,
        ratio: f32,
        session_id: Option<SessionId>,
    ) -> Result<PaneId, WorkspaceLayoutError> {
        validate_ratio(ratio)?;
        if self.leaf_count() >= MAX_LEAVES {
            return Err(WorkspaceLayoutError::TooManyLeaves);
        }
        if !self.root.contains(target) {
            return Err(WorkspaceLayoutError::PaneNotFound(target));
        }
        let new_pane = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        let split = self
            .root
            .split_leaf(target, new_pane, direction, ratio, session_id);
        debug_assert!(split);
        Ok(new_pane)
    }

    /// Closes a leaf and merges its sibling upward.
    pub fn close(&mut self, target: PaneId) -> Result<(), WorkspaceLayoutError> {
        if self.leaf_count() == 1 {
            return Err(WorkspaceLayoutError::CannotCloseLastLeaf);
        }
        if !self.root.contains(target) {
            return Err(WorkspaceLayoutError::PaneNotFound(target));
        }
        let closed = self.root.close_leaf(target);
        debug_assert!(closed);
        Ok(())
    }

    /// Sets the ratio on the split containing `target`.
    pub fn set_ratio(&mut self, target: PaneId, ratio: f32) -> Result<(), WorkspaceLayoutError> {
        validate_ratio(ratio)?;
        if self.root.set_ratio(target, ratio) {
            Ok(())
        } else {
            Err(WorkspaceLayoutError::PaneNotFound(target))
        }
    }
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self::new()
    }
}

/// Layout mutation error.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceLayoutError {
    PaneNotFound(PaneId),
    TooManyLeaves,
    CannotCloseLastLeaf,
    InvalidRatio(f32),
}

impl fmt::Display for WorkspaceLayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PaneNotFound(pane_id) => write!(f, "pane {} was not found", pane_id.value()),
            Self::TooManyLeaves => {
                write!(f, "workspace layouts support at most {MAX_LEAVES} panes")
            }
            Self::CannotCloseLastLeaf => f.write_str("cannot close the last workspace pane"),
            Self::InvalidRatio(ratio) => write!(f, "invalid split ratio {ratio}"),
        }
    }
}

impl Error for WorkspaceLayoutError {}

fn validate_ratio(ratio: f32) -> Result<(), WorkspaceLayoutError> {
    if (0.1..=0.9).contains(&ratio) {
        Ok(())
    } else {
        Err(WorkspaceLayoutError::InvalidRatio(ratio))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_to_four_leaves_and_rejects_fifth() {
        let mut layout = WorkspaceLayout::new();
        let root = PaneId::new(1);
        let second = layout
            .split(root, SplitDirection::Horizontal, 0.5, None)
            .expect("split");
        let third = layout
            .split(second, SplitDirection::Vertical, 0.4, None)
            .expect("split");
        layout
            .split(third, SplitDirection::Horizontal, 0.6, None)
            .expect("split");

        assert_eq!(layout.leaf_count(), 4);
        assert_eq!(
            layout.split(root, SplitDirection::Horizontal, 0.5, None),
            Err(WorkspaceLayoutError::TooManyLeaves)
        );
    }

    #[test]
    fn close_merges_sibling() {
        let mut layout = WorkspaceLayout::new();
        let root = PaneId::new(1);
        let second = layout
            .split(root, SplitDirection::Horizontal, 0.5, None)
            .expect("split");

        layout.close(second).expect("close");

        assert_eq!(layout.leaf_count(), 1);
        assert_eq!(layout.pane_ids(), vec![root]);
    }
}
