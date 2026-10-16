//! Recursive workspace split tree (binary nodes, up to [`WorkspaceSplit::MAX_LEAVES`] leaves).

use super::{SplitOrientation, SplitPane};

/// Normalized rectangle in 0..1 workspace space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NormRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl NormRect {
    pub(crate) const FULL: Self = Self {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
}

/// Flattened leaf for projection / absolute UI layout.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LaidOutLeaf {
    pub pane_id: u64,
    pub rect: NormRect,
    pub pane: SplitPane,
}

/// Flattened splitter handle for ratio drag.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LaidOutSplitter {
    pub split_id: u64,
    pub orientation: SplitOrientation,
    pub rect: NormRect,
    /// Parent split region (for converting pointer → ratio).
    pub parent: NormRect,
    pub ratio: f32,
}

/// Binary split tree node.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SplitNode {
    Leaf {
        pane_id: u64,
        pane: SplitPane,
    },
    Split {
        /// Stable id for ratio updates from UI.
        split_id: u64,
        orientation: SplitOrientation,
        /// Share of the first child along the main axis (0.2..=0.8).
        ratio: f32,
        first: Box<SplitNode>,
        second: Box<SplitNode>,
    },
}

impl SplitNode {
    fn leaf_count(&self) -> usize {
        match self {
            Self::Leaf { .. } => 1,
            Self::Split { first, second, .. } => first.leaf_count() + second.leaf_count(),
        }
    }

    fn contains_pane(&self, pane_id: u64) -> bool {
        match self {
            Self::Leaf { pane_id: id, .. } => *id == pane_id,
            Self::Split { first, second, .. } => {
                first.contains_pane(pane_id) || second.contains_pane(pane_id)
            }
        }
    }

    fn find_pane_mut(&mut self, pane_id: u64) -> Option<&mut SplitPane> {
        match self {
            Self::Leaf {
                pane_id: id, pane, ..
            } if *id == pane_id => Some(pane),
            Self::Leaf { .. } => None,
            Self::Split { first, second, .. } => first
                .find_pane_mut(pane_id)
                .or_else(|| second.find_pane_mut(pane_id)),
        }
    }

    fn find_pane(&self, pane_id: u64) -> Option<&SplitPane> {
        match self {
            Self::Leaf {
                pane_id: id, pane, ..
            } if *id == pane_id => Some(pane),
            Self::Leaf { .. } => None,
            Self::Split { first, second, .. } => first
                .find_pane(pane_id)
                .or_else(|| second.find_pane(pane_id)),
        }
    }

    fn for_each_leaf_mut(&mut self, f: &mut dyn FnMut(u64, &mut SplitPane)) {
        match self {
            Self::Leaf { pane_id, pane } => f(*pane_id, pane),
            Self::Split { first, second, .. } => {
                first.for_each_leaf_mut(f);
                second.for_each_leaf_mut(f);
            }
        }
    }

    fn for_each_leaf(&self, f: &mut dyn FnMut(u64, &SplitPane)) {
        match self {
            Self::Leaf { pane_id, pane } => f(*pane_id, pane),
            Self::Split { first, second, .. } => {
                first.for_each_leaf(f);
                second.for_each_leaf(f);
            }
        }
    }

    /// Split a leaf into two. `dragged_is_first` places the new leaf on first/second.
    /// Returns the new pane id.
    fn split_leaf(
        &mut self,
        target: u64,
        orientation: SplitOrientation,
        ratio: f32,
        new_pane_id: u64,
        new_split_id: u64,
        new_pane: SplitPane,
        dragged_is_first: bool,
    ) -> bool {
        match self {
            Self::Leaf { pane_id, pane } if *pane_id == target => {
                let remnant = std::mem::take(pane);
                let remnant_id = *pane_id;
                let (first_pane, first_id, second_pane, second_id) = if dragged_is_first {
                    (new_pane, new_pane_id, remnant, remnant_id)
                } else {
                    (remnant, remnant_id, new_pane, new_pane_id)
                };
                *self = Self::Split {
                    split_id: new_split_id,
                    orientation,
                    ratio,
                    first: Box::new(Self::Leaf {
                        pane_id: first_id,
                        pane: first_pane,
                    }),
                    second: Box::new(Self::Leaf {
                        pane_id: second_id,
                        pane: second_pane,
                    }),
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => first
                .split_leaf(
                    target,
                    orientation,
                    ratio,
                    new_pane_id,
                    new_split_id,
                    new_pane.clone(),
                    dragged_is_first,
                )
                || second.split_leaf(
                    target,
                    orientation,
                    ratio,
                    new_pane_id,
                    new_split_id,
                    new_pane,
                    dragged_is_first,
                ),
        }
    }

    /// Close a leaf; promote sibling. Returns true if the closed node was this leaf
    /// (caller should replace parent). Returns `Ok(true)` when this entire node should
    /// be replaced by its sibling (signaled via replacing self when child returns).
    fn close_leaf(&mut self, target: u64) -> CloseLeafResult {
        match self {
            Self::Leaf { pane_id, .. } => {
                if *pane_id == target {
                    CloseLeafResult::ClosedHere
                } else {
                    CloseLeafResult::NotFound
                }
            }
            Self::Split { first, second, .. } => {
                match first.close_leaf(target) {
                    CloseLeafResult::ClosedHere => {
                        let replacement = std::mem::replace(
                            second.as_mut(),
                            SplitNode::Leaf {
                                pane_id: 0,
                                pane: SplitPane::default(),
                            },
                        );
                        *self = replacement;
                        CloseLeafResult::Replaced
                    }
                    CloseLeafResult::Replaced => CloseLeafResult::Replaced,
                    CloseLeafResult::NotFound => match second.close_leaf(target) {
                        CloseLeafResult::ClosedHere => {
                            let replacement = std::mem::replace(
                                first.as_mut(),
                                SplitNode::Leaf {
                                    pane_id: 0,
                                    pane: SplitPane::default(),
                                },
                            );
                            *self = replacement;
                            CloseLeafResult::Replaced
                        }
                        other => other,
                    },
                }
            }
        }
    }

    fn set_split_ratio(&mut self, split_id: u64, ratio: f32) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split {
                split_id: id,
                ratio: r,
                first,
                second,
                ..
            } => {
                if *id == split_id {
                    *r = ratio.clamp(0.2, 0.8);
                    true
                } else {
                    first.set_split_ratio(split_id, ratio) || second.set_split_ratio(split_id, ratio)
                }
            }
        }
    }

    fn layout(
        &self,
        rect: NormRect,
        splitter_thickness: f32,
        leaves: &mut Vec<LaidOutLeaf>,
        splitters: &mut Vec<LaidOutSplitter>,
    ) {
        match self {
            Self::Leaf { pane_id, pane } => {
                leaves.push(LaidOutLeaf {
                    pane_id: *pane_id,
                    rect,
                    pane: pane.clone(),
                });
            }
            Self::Split {
                split_id,
                orientation,
                ratio,
                first,
                second,
            } => {
                let ratio = ratio.clamp(0.2, 0.8);
                let gap = splitter_thickness;
                match orientation {
                    SplitOrientation::Horizontal => {
                        let avail = (rect.w - gap).max(0.0);
                        let w0 = avail * ratio;
                        let w1 = avail - w0;
                        let first_rect = NormRect {
                            x: rect.x,
                            y: rect.y,
                            w: w0,
                            h: rect.h,
                        };
                        let split_rect = NormRect {
                            x: rect.x + w0,
                            y: rect.y,
                            w: gap,
                            h: rect.h,
                        };
                        let second_rect = NormRect {
                            x: rect.x + w0 + gap,
                            y: rect.y,
                            w: w1,
                            h: rect.h,
                        };
                        splitters.push(LaidOutSplitter {
                            split_id: *split_id,
                            orientation: *orientation,
                            rect: split_rect,
                            parent: rect,
                            ratio,
                        });
                        first.layout(first_rect, splitter_thickness, leaves, splitters);
                        second.layout(second_rect, splitter_thickness, leaves, splitters);
                    }
                    SplitOrientation::Vertical => {
                        let avail = (rect.h - gap).max(0.0);
                        let h0 = avail * ratio;
                        let h1 = avail - h0;
                        let first_rect = NormRect {
                            x: rect.x,
                            y: rect.y,
                            w: rect.w,
                            h: h0,
                        };
                        let split_rect = NormRect {
                            x: rect.x,
                            y: rect.y + h0,
                            w: rect.w,
                            h: gap,
                        };
                        let second_rect = NormRect {
                            x: rect.x,
                            y: rect.y + h0 + gap,
                            w: rect.w,
                            h: h1,
                        };
                        splitters.push(LaidOutSplitter {
                            split_id: *split_id,
                            orientation: *orientation,
                            rect: split_rect,
                            parent: rect,
                            ratio,
                        });
                        first.layout(first_rect, splitter_thickness, leaves, splitters);
                        second.layout(second_rect, splitter_thickness, leaves, splitters);
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseLeafResult {
    NotFound,
    ClosedHere,
    Replaced,
}

/// Window-level split state (recursive tree).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorkspaceSplit {
    pub(crate) root: SplitNode,
    pub(crate) focused_pane: u64,
    next_pane_id: u64,
    next_split_id: u64,
}

impl WorkspaceSplit {
    pub(crate) const MAX_LEAVES: usize = 10;
    /// Normalized splitter thickness (~4px on a 800px axis → 0.005).
    pub(crate) const SPLITTER_NORM: f32 = 0.006;

    /// Legacy alias used by older call sites.
    pub(crate) const MAX_PANES: usize = Self::MAX_LEAVES;

    pub(crate) fn two_pane(
        orientation: SplitOrientation,
        first: Option<String>,
        second: Option<String>,
    ) -> Self {
        let first_tabs = first.iter().cloned().collect();
        let second_tabs = second.iter().cloned().collect();
        Self {
            root: SplitNode::Split {
                split_id: 1,
                orientation,
                ratio: 0.5,
                first: Box::new(SplitNode::Leaf {
                    pane_id: 1,
                    pane: SplitPane {
                        tabs: first_tabs,
                        active_tab: first,
                    },
                }),
                second: Box::new(SplitNode::Leaf {
                    pane_id: 2,
                    pane: SplitPane {
                        tabs: second_tabs,
                        active_tab: second,
                    },
                }),
            },
            focused_pane: 2,
            next_pane_id: 3,
            next_split_id: 2,
        }
    }

    pub(crate) fn pane_count(&self) -> usize {
        self.root.leaf_count()
    }

    pub(crate) fn focused_tab(&self) -> Option<&str> {
        self.root
            .find_pane(self.focused_pane)
            .and_then(|pane| pane.active_tab.as_deref())
    }

    pub(crate) fn shows_tab(&self, tab_id: &str) -> bool {
        let mut found = false;
        self.root.for_each_leaf(&mut |_id, pane| {
            if pane.active_tab.as_deref() == Some(tab_id)
                || pane.tabs.iter().any(|id| id.as_str() == tab_id)
            {
                found = true;
            }
        });
        found
    }

    pub(crate) fn pane_ids(&self) -> Vec<u64> {
        let mut ids = Vec::new();
        self.root.for_each_leaf(&mut |id, _| ids.push(id));
        ids
    }

    pub(crate) fn find_pane(&self, pane_id: u64) -> Option<&SplitPane> {
        self.root.find_pane(pane_id)
    }

    pub(crate) fn find_pane_mut(&mut self, pane_id: u64) -> Option<&mut SplitPane> {
        self.root.find_pane_mut(pane_id)
    }

    pub(crate) fn find_pane_id_for_tab(&self, tab_id: &str) -> Option<u64> {
        let mut found = None;
        self.root.for_each_leaf(&mut |id, pane| {
            if found.is_none()
                && (pane.active_tab.as_deref() == Some(tab_id)
                    || pane.tabs.iter().any(|t| t.as_str() == tab_id))
            {
                found = Some(id);
            }
        });
        found
    }

    pub(crate) fn for_each_leaf_mut(&mut self, f: &mut dyn FnMut(u64, &mut SplitPane)) {
        self.root.for_each_leaf_mut(f);
    }

    pub(crate) fn layout(&self) -> (Vec<LaidOutLeaf>, Vec<LaidOutSplitter>) {
        let mut leaves = Vec::new();
        let mut splitters = Vec::new();
        self.root.layout(
            NormRect::FULL,
            Self::SPLITTER_NORM,
            &mut leaves,
            &mut splitters,
        );
        (leaves, splitters)
    }

    /// Split `target` leaf; move `dragged_tab` into the new leaf on the drop side.
    pub(crate) fn split_leaf_with_tab(
        &mut self,
        target: u64,
        orientation: SplitOrientation,
        pane0_ratio: f32,
        dragged_is_first: bool,
        dragged_tab: &str,
    ) -> Result<u64, &'static str> {
        if self.pane_count() >= Self::MAX_LEAVES {
            return Err("workspace already has the maximum number of panes");
        }
        if !self.root.contains_pane(target) {
            return Err("target pane was not found");
        }
        // Remove tab from all leaves first.
        self.remove_tab_from_all(dragged_tab);
        let new_pane = SplitPane {
            tabs: vec![dragged_tab.to_owned()],
            active_tab: Some(dragged_tab.to_owned()),
        };
        let new_pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let new_split_id = self.next_split_id;
        self.next_split_id += 1;
        let ratio = pane0_ratio.clamp(0.2, 0.8);
        let ok = self.root.split_leaf(
            target,
            orientation,
            ratio,
            new_pane_id,
            new_split_id,
            new_pane,
            dragged_is_first,
        );
        if !ok {
            return Err("failed to split target pane");
        }
        self.focused_pane = new_pane_id;
        Ok(new_pane_id)
    }

    pub(crate) fn remove_tab_from_all(&mut self, tab_id: &str) {
        self.root.for_each_leaf_mut(&mut |_id, pane| {
            pane.tabs.retain(|id| id.as_str() != tab_id);
            if pane.active_tab.as_deref() == Some(tab_id) {
                pane.active_tab = pane.tabs.first().cloned();
            }
        });
    }

    pub(crate) fn move_tab_to_pane(&mut self, tab_id: &str, pane_id: u64) -> Result<(), &'static str> {
        if !self.root.contains_pane(pane_id) {
            return Err("pane was not found");
        }
        self.remove_tab_from_all(tab_id);
        let pane = self
            .root
            .find_pane_mut(pane_id)
            .ok_or("pane was not found")?;
        if !pane.tabs.iter().any(|id| id == tab_id) {
            pane.tabs.push(tab_id.to_owned());
        }
        pane.active_tab = Some(tab_id.to_owned());
        self.focused_pane = pane_id;
        Ok(())
    }

    /// Remove empty leaves (and collapse tree). Returns whether split should be cleared
    /// (≤1 leaf remaining).
    pub(crate) fn compact_empty_leaves(&mut self) -> CompactOutcome {
        loop {
            let empty_id = {
                let mut empty = None;
                self.root.for_each_leaf(&mut |id, pane| {
                    if empty.is_none() && pane.tabs.is_empty() {
                        empty = Some(id);
                    }
                });
                empty
            };
            let Some(empty_id) = empty_id else {
                break;
            };
            if self.pane_count() <= 1 {
                return CompactOutcome::Unsplit;
            }
            match self.root.close_leaf(empty_id) {
                CloseLeafResult::NotFound => break,
                CloseLeafResult::ClosedHere => {
                    // Root was the only leaf and empty.
                    return CompactOutcome::Unsplit;
                }
                CloseLeafResult::Replaced => {
                    if matches!(self.root, SplitNode::Leaf { .. }) {
                        // One leaf left — caller clears split and promotes tabs to window.
                        return CompactOutcome::Unsplit;
                    }
                }
            }
        }
        if !self.root.contains_pane(self.focused_pane) {
            if let Some(id) = self.pane_ids().first().copied() {
                self.focused_pane = id;
            }
        }
        CompactOutcome::Keep
    }

    pub(crate) fn close_pane(&mut self, pane_id: u64) -> Result<CompactOutcome, &'static str> {
        if self.pane_count() <= 1 {
            return Err("cannot close the last pane");
        }
        if !self.root.contains_pane(pane_id) {
            return Err("pane was not found");
        }
        match self.root.close_leaf(pane_id) {
            CloseLeafResult::NotFound => Err("pane was not found"),
            CloseLeafResult::ClosedHere => Ok(CompactOutcome::Unsplit),
            CloseLeafResult::Replaced => {
                if matches!(self.root, SplitNode::Leaf { .. }) {
                    Ok(CompactOutcome::Unsplit)
                } else {
                    if !self.root.contains_pane(self.focused_pane) {
                        if let Some(id) = self.pane_ids().first().copied() {
                            self.focused_pane = id;
                        }
                    }
                    Ok(CompactOutcome::Keep)
                }
            }
        }
    }

    pub(crate) fn set_split_ratio(&mut self, split_id: u64, ratio: f32) -> bool {
        self.root.set_split_ratio(split_id, ratio)
    }

    pub(crate) fn first_leaf_pane(&self) -> Option<(u64, &SplitPane)> {
        let id = self.pane_ids().first().copied()?;
        self.root.find_pane(id).map(|pane| (id, pane))
    }

    pub(crate) fn leaf_at_index(&self, index: usize) -> Option<(u64, &SplitPane)> {
        let id = *self.pane_ids().get(index)?;
        self.root.find_pane(id).map(|pane| (id, pane))
    }

    pub(crate) fn focused_pane_index(&self) -> usize {
        self.pane_ids()
            .iter()
            .position(|id| *id == self.focused_pane)
            .unwrap_or(0)
    }

    /// Non-focused leaf sessions for secondary render (all except focused).
    pub(crate) fn secondary_pane_ids(&self) -> Vec<u64> {
        self.pane_ids()
            .into_iter()
            .filter(|id| *id != self.focused_pane)
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompactOutcome {
    Keep,
    Unsplit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::SplitOrientation;

    #[test]
    fn nested_split_and_compact_empty() {
        let mut split = WorkspaceSplit::two_pane(
            SplitOrientation::Horizontal,
            Some("a".into()),
            Some("b".into()),
        );
        assert_eq!(split.pane_count(), 2);
        split
            .split_leaf_with_tab(1, SplitOrientation::Vertical, 0.5, true, "c")
            .expect("nest");
        assert_eq!(split.pane_count(), 3);

        // Move b onto the leaf that has a (id after nest: remnant of 1 may still be 1 or new).
        let a_pane = split.find_pane_id_for_tab("a").expect("a");
        split.move_tab_to_pane("b", a_pane).expect("move b");
        assert_eq!(split.compact_empty_leaves(), CompactOutcome::Keep);
        assert_eq!(split.pane_count(), 2);

        let a_pane = split.find_pane_id_for_tab("a").expect("a");
        split.move_tab_to_pane("c", a_pane).expect("move c");
        assert_eq!(split.compact_empty_leaves(), CompactOutcome::Unsplit);
    }

    #[test]
    fn layout_emits_leaves_and_splitters() {
        let split = WorkspaceSplit::two_pane(
            SplitOrientation::Horizontal,
            Some("a".into()),
            Some("b".into()),
        );
        let (leaves, splitters) = split.layout();
        assert_eq!(leaves.len(), 2);
        assert_eq!(splitters.len(), 1);
        assert_eq!(splitters[0].orientation, SplitOrientation::Horizontal);
        assert!((leaves[0].rect.w + leaves[1].rect.w + splitters[0].rect.w - 1.0).abs() < 0.02);
    }
}
