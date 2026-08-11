use ratatui::layout::{Margin, Rect};
use serde::{Deserialize, Serialize};

/// Split orientation. `V` = side-by-side columns, `H` = stacked rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SplitDir {
    V,
    H,
}

/// Layout tree node. `Split` carries a stable id so mouse drags can target it.
#[derive(Clone, Debug)]
pub enum Node {
    Pane { id: u64 },
    Split {
        id: u64,
        dir: SplitDir,
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

pub struct LayoutTree {
    pub root: Option<Node>,
    pub focus: u64,
    next_split: u64,
}

impl LayoutTree {
    pub fn new(pane_id: u64) -> Self {
        Self {
            root: Some(Node::Pane { id: pane_id }),
            focus: pane_id,
            next_split: 1,
        }
    }

    /// Split the pane holding `pane_id`, inserting `new_pane` to its right (V)
    /// or below (H) and keeping focus on the original pane.
    pub fn split(&mut self, pane_id: u64, new_pane: u64, dir: SplitDir) -> bool {
        if let Some(root) = self.root.as_mut() {
            if split_at(root, pane_id, new_pane, dir, self.next_split) {
                self.next_split += 1;
                return true;
            }
        }
        false
    }

    /// Remove a pane, collapsing splits that are left with a single child.
    /// Returns true only when the tree is now empty (caller should drop the
    /// session); a pane that was already removed is a no-op that returns false.
    pub fn remove_pane(&mut self, pane_id: u64) -> bool {
        let mut empty = false;
        if let Some(root) = self.root.take() {
            self.root = remove_from(root, pane_id);
            empty = self.root.is_none();
        }
        // If focus pointed at the removed pane, move it to a surviving pane.
        if let Some(root) = &self.root {
            let all = pane_ids(root, &mut Vec::new());
            if !all.contains(&self.focus) {
                self.focus = all[0];
            }
        }
        empty
    }

    pub fn pane_count(&self) -> usize {
        match &self.root {
            Some(r) => pane_ids(r, &mut Vec::new()).len(),
            None => 0,
        }
    }

    pub fn set_ratio(&mut self, split_id: u64, ratio: f32) {
        if let Some(root) = self.root.as_mut() {
            set_ratio_at(root, split_id, ratio);
        }
    }

    pub fn contains(&self, pane_id: u64) -> bool {
        self.root.as_ref().is_some_and(|r| pane_ids(r, &mut Vec::new()).contains(&pane_id))
    }

    /// Pane ids in tree (depth-first) order.
    pub fn pane_ids(&self) -> Vec<u64> {
        match &self.root {
            Some(r) => pane_ids(r, &mut Vec::new()),
            None => Vec::new(),
        }
    }

    /// Rebuild a tree from a restored node, with `focus` naming a surviving
    /// pane. `next_split` is derived from the tree so future splits never
    /// collide with restored split ids.
    pub fn from_node(root: Node, focus: u64) -> Self {
        let mut max_split = 0u64;
        max_split_id(&root, &mut max_split);
        Self { root: Some(root), focus, next_split: max_split + 1 }
    }
}

/// Highest split id in the tree (used to seed `LayoutTree::next_split`).
fn max_split_id(n: &Node, out: &mut u64) {
    match n {
        Node::Split { id, a, b, .. } => {
            *out = (*out).max(*id);
            max_split_id(a, out);
            max_split_id(b, out);
        }
        Node::Pane { .. } => {}
    }
}

fn split_at(n: &mut Node, pane_id: u64, new_pane: u64, dir: SplitDir, split_id: u64) -> bool {
    match n {
        Node::Pane { id } if *id == pane_id => {
            *n = Node::Split {
                id: split_id,
                dir,
                ratio: 0.5,
                a: Box::new(Node::Pane { id: pane_id }),
                b: Box::new(Node::Pane { id: new_pane }),
            };
            true
        }
        Node::Split { a, b, .. } => {
            split_at(a, pane_id, new_pane, dir, split_id)
                || split_at(b, pane_id, new_pane, dir, split_id)
        }
        _ => false,
    }
}

fn remove_from(n: Node, pane_id: u64) -> Option<Node> {
    match n {
        Node::Pane { id } if id == pane_id => None,
        Node::Pane { .. } => Some(n),
        Node::Split { id, dir, ratio, a, b } => {
            let a = remove_from(*a, pane_id);
            let b = remove_from(*b, pane_id);
            match (a, b) {
                (Some(x), None) | (None, Some(x)) => Some(x),
                (Some(x), Some(y)) => Some(Node::Split {
                    id,
                    dir,
                    ratio,
                    a: Box::new(x),
                    b: Box::new(y),
                }),
                (None, None) => None,
            }
        }
    }
}

fn set_ratio_at(n: &mut Node, split_id: u64, ratio: f32) -> bool {
    match n {
        Node::Split { id, ratio: r, .. } if *id == split_id => {
            *r = ratio.clamp(0.05, 0.95);
            true
        }
        Node::Split { a, b, .. } => set_ratio_at(a, split_id, ratio) || set_ratio_at(b, split_id, ratio),
        _ => false,
    }
}

fn pane_ids(n: &Node, out: &mut Vec<u64>) -> Vec<u64> {
    match n {
        Node::Pane { id } => out.push(*id),
        Node::Split { a, b, .. } => {
            pane_ids(a, out);
            pane_ids(b, out);
        }
    }
    out.clone()
}

/// Result of laying a tree out over a rect.
#[derive(Default)]
pub struct TreeGeom {
    pub panes: Vec<PaneGeom>,
    pub splitters: Vec<SplitGeom>,
}

#[derive(Clone, Copy)]
pub struct PaneGeom {
    pub pane_id: u64,
    pub rect: Rect,
}

impl PaneGeom {
    /// Inner area where the emulator renders: the slot rect inset by the border.
    pub fn inner(&self) -> Rect {
        self.rect.inner(Margin { horizontal: 1, vertical: 1 })
    }
}

#[derive(Clone, Copy)]
pub struct SplitGeom {
    pub split_id: u64,
    pub dir: SplitDir,
    pub area: Rect,
    pub rect: Rect,
}

pub fn compute_geometry(n: &Node, area: Rect, out: &mut TreeGeom) {
    if area.width < 1 || area.height < 1 {
        return;
    }
    match n {
        Node::Pane { id } => out.panes.push(PaneGeom { pane_id: *id, rect: area }),
        Node::Split { id, dir, ratio, a, b } => {
            let (ra, rb, sep) = match dir {
                SplitDir::V => {
                    let wa = ((area.width as f32) * ratio).round().max(1.0).min((area.width - 1) as f32) as u16;
                    let sep = Rect::new(area.x + wa, area.y, 1, area.height);
                    let ra = Rect::new(area.x, area.y, wa, area.height);
                    let rb = Rect::new(area.x + wa + 1, area.y, area.width - wa - 1, area.height);
                    (ra, rb, sep)
                }
                SplitDir::H => {
                    let ha = ((area.height as f32) * ratio).round().max(1.0).min((area.height - 1) as f32) as u16;
                    let sep = Rect::new(area.x, area.y + ha, area.width, 1);
                    let ra = Rect::new(area.x, area.y, area.width, ha);
                    let rb = Rect::new(area.x, area.y + ha + 1, area.width, area.height - ha - 1);
                    (ra, rb, sep)
                }
            };
            out.splitters.push(SplitGeom {
                split_id: *id,
                dir: *dir,
                area,
                rect: sep,
            });
            compute_geometry(a, ra, out);
            compute_geometry(b, rb, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_missing_pane_keeps_tree() {
        let mut tree = LayoutTree::new(1);
        tree.split(1, 2, SplitDir::V);
        // Closing a pane that was already removed must NOT report the tree as
        // empty (that used to close the whole session).
        assert!(!tree.remove_pane(99));
        assert_eq!(tree.pane_count(), 2);
    }

    #[test]
    fn removing_last_pane_reports_empty() {
        let mut tree = LayoutTree::new(1);
        assert!(tree.remove_pane(1));
        assert_eq!(tree.pane_count(), 0);
    }

    #[test]
    fn removing_one_of_two_keeps_other_and_repairs_focus() {
        let mut tree = LayoutTree::new(1);
        tree.split(1, 2, SplitDir::V);
        // Split keeps focus on the original pane, tmux-style.
        assert_eq!(tree.focus, 1);
        assert!(!tree.remove_pane(2));
        assert_eq!(tree.pane_count(), 1);
        // Focus must move to the surviving pane, not stay stale.
        assert_eq!(tree.focus, 1);
        assert!(tree.contains(1));
    }

    #[test]
    fn removing_stale_focus_keeps_tree() {
        // Simulate poll_exits closing a pane while focus still names it.
        let mut tree = LayoutTree::new(1);
        tree.split(1, 2, SplitDir::V);
        tree.focus = 2;
        assert!(!tree.remove_pane(2));
        assert_eq!(tree.focus, 1);
        assert_eq!(tree.pane_count(), 1);
        // A second close of the same (now stale) pane is a no-op, not empty.
        assert!(!tree.remove_pane(2));
        assert_eq!(tree.pane_count(), 1);
    }

    #[test]
    fn from_node_restores_tree_and_seeds_split_ids() {
        let mut tree = LayoutTree::new(1);
        tree.split(1, 2, SplitDir::V);
        tree.split(2, 3, SplitDir::H);
        tree.focus = 3;
        let root = tree.root.clone().unwrap();

        let mut restored = LayoutTree::from_node(root, 3);
        assert_eq!(restored.pane_ids(), vec![1, 2, 3]);
        assert_eq!(restored.focus, 3);

        // A new split must not collide with the restored split id (1).
        assert!(restored.split(1, 99, SplitDir::V));
        assert!(restored.contains(99));
    }
}
