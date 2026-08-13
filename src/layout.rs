use ratatui::layout::{Margin, Rect};
use serde::{Deserialize, Serialize};

/// Split orientation. `V` = side-by-side columns, `H` = stacked rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SplitDir {
    V,
    H,
}

/// Direction of a keyboard pane resize (mirrors the `Dir` focus directions).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResizeDir {
    Left,
    Down,
    Up,
    Right,
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

    /// Nudge the ratio of the split that separates `pane_id` from its neighbor
    /// in `dir`. Only splits whose axis matches the direction move (Left/Right
    /// for vertical splits, Up/Down for horizontal); the nearest matching
    /// ancestor split wins. Returns false when no pane or no matching split.
    pub fn resize_pane(&mut self, pane_id: u64, dir: ResizeDir, delta: f32) -> bool {
        let Some(root) = self.root.as_mut() else { return false };
        resize_pane_at(root, pane_id, dir, delta)
    }

    /// Swap the focused pane with its sibling: exchange the two children of the
    /// pane's nearest ancestor split. Returns false when there is no sibling.
    pub fn swap_with_sibling(&mut self, pane_id: u64) -> bool {
        let Some(root) = self.root.as_mut() else { return false };
        swap_sibling_at(root, pane_id)
    }

    /// Rotate the layout: mirror every split left/right and top/bottom.
    pub fn mirror(&mut self) {
        if let Some(root) = self.root.as_mut() {
            mirror_at(root);
        }
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

/// True when the subtree `n` contains `pane_id`.
fn tree_contains(n: &Node, pane_id: u64) -> bool {
    match n {
        Node::Pane { id } => *id == pane_id,
        Node::Split { a, b, .. } => tree_contains(a, pane_id) || tree_contains(b, pane_id),
    }
}

/// Sign of the ratio change for `dir` given which side of its split the pane
/// sits on (`a` = left/top, `b` = right/bottom). Negative shrinks the pane.
fn resize_sign(dir: ResizeDir, in_a: bool) -> f32 {
    match (dir, in_a) {
        (ResizeDir::Left, true)
        | (ResizeDir::Right, false)
        | (ResizeDir::Up, true)
        | (ResizeDir::Down, false) => -1.0,
        _ => 1.0,
    }
}

fn resize_pane_at(n: &mut Node, pane_id: u64, dir: ResizeDir, delta: f32) -> bool {
    match n {
        Node::Split { dir: split_dir, ratio, a, b, .. } => {
            let axis_matches = match split_dir {
                SplitDir::V => matches!(dir, ResizeDir::Left | ResizeDir::Right),
                SplitDir::H => matches!(dir, ResizeDir::Up | ResizeDir::Down),
            };
            if !axis_matches {
                return false;
            }
            let in_a = tree_contains(a, pane_id);
            let in_b = tree_contains(b, pane_id);
            if !in_a && !in_b {
                return false;
            }
            // The nearest ancestor split with a matching axis wins: try the
            // deeper split first, fall back to adjusting here.
            if (in_a && resize_pane_at(a, pane_id, dir, delta))
                || (in_b && resize_pane_at(b, pane_id, dir, delta))
            {
                return true;
            }
            *ratio = (*ratio + resize_sign(dir, in_a) * delta).clamp(0.05, 0.95);
            true
        }
        Node::Pane { .. } => false,
    }
}

fn swap_sibling_at(n: &mut Node, pane_id: u64) -> bool {
    match n {
        Node::Split { a, b, .. } => {
            let a_is_pane = matches!(&**a, Node::Pane { id } if *id == pane_id);
            let b_is_pane = matches!(&**b, Node::Pane { id } if *id == pane_id);
            if a_is_pane || b_is_pane {
                std::mem::swap(a, b);
                return true;
            }
            (tree_contains(a, pane_id) && swap_sibling_at(a, pane_id))
                || (tree_contains(b, pane_id) && swap_sibling_at(b, pane_id))
        }
        Node::Pane { .. } => false,
    }
}

fn mirror_at(n: &mut Node) {
    match n {
        Node::Split { a, b, .. } => {
            std::mem::swap(a, b);
            mirror_at(a);
            mirror_at(b);
        }
        Node::Pane { .. } => {}
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

    fn split_ratio(tree: &LayoutTree, split_id: u64) -> f32 {
        let mut r = -1.0;
        let mut find = |n: &Node| {
            if let Node::Split { id, ratio, .. } = n {
                if *id == split_id {
                    r = *ratio;
                }
            }
        };
        // Walk the tree manually (tests-only helper).
        fn walk(n: &Node, f: &mut impl FnMut(&Node)) {
            f(n);
            if let Node::Split { a, b, .. } = n {
                walk(a, f);
                walk(b, f);
            }
        }
        walk(tree.root.as_ref().unwrap(), &mut find);
        r
    }

    #[test]
    fn resize_left_shrinks_pane_on_left_side() {
        let mut tree = LayoutTree::new(1);
        tree.split(1, 2, SplitDir::V); // split id 1, ratio 0.5, pane 1 in a
        let before = split_ratio(&tree, 1);
        assert!(tree.resize_pane(1, ResizeDir::Left, 0.05));
        assert!(split_ratio(&tree, 1) < before, "left child resizing left must shrink");
        let mid = split_ratio(&tree, 1);
        assert!(tree.resize_pane(2, ResizeDir::Left, 0.05));
        assert!(split_ratio(&tree, 1) > mid, "right child resizing left must grow");
    }

    #[test]
    fn resize_right_is_the_mirror_of_left() {
        let mut tree = LayoutTree::new(1);
        tree.split(1, 2, SplitDir::V);
        let before = split_ratio(&tree, 1);
        assert!(tree.resize_pane(1, ResizeDir::Right, 0.05));
        assert!(split_ratio(&tree, 1) > before);
        let mid = split_ratio(&tree, 1);
        assert!(tree.resize_pane(2, ResizeDir::Right, 0.05));
        assert!(split_ratio(&tree, 1) < mid);
    }

    #[test]
    fn resize_vertical_dir_does_nothing_on_h_split() {
        let mut tree = LayoutTree::new(1);
        tree.split(1, 2, SplitDir::H);
        let before = split_ratio(&tree, 1);
        assert!(!tree.resize_pane(1, ResizeDir::Left, 0.05), "axis mismatch must no-op");
        assert_eq!(split_ratio(&tree, 1), before);
        assert!(tree.resize_pane(1, ResizeDir::Up, 0.05), "axis match must work");
    }

    #[test]
    fn resize_on_single_pane_noops() {
        let mut tree = LayoutTree::new(1);
        assert!(!tree.resize_pane(1, ResizeDir::Left, 0.05));
    }

    #[test]
    fn resize_nudges_nearest_ancestor_split() {
        let mut tree = LayoutTree::new(1);
        tree.split(1, 2, SplitDir::V); // id 1: 1 | 2
        tree.split(2, 3, SplitDir::V); // id 2: (1|2) then split 2 into 2|3? split pane 2 -> tree 1 | (2|3)
        // Resize pane 3 left: nearest V-split ancestor is split 2 (2|3).
        let before = split_ratio(&tree, 2);
        assert!(tree.resize_pane(3, ResizeDir::Left, 0.05));
        assert!(split_ratio(&tree, 2) > before, "pane 3 is the right child of split 2; left must grow it");
    }

    #[test]
    fn swap_with_sibling_exchanges_pane_sides() {
        let mut tree = LayoutTree::new(1);
        tree.split(1, 2, SplitDir::V);
        assert!(tree.swap_with_sibling(1));
        // Both panes survive; their order swapped (pane 1 now right of 2).
        assert_eq!(tree.pane_ids(), vec![2, 1]);
        assert!(tree.swap_with_sibling(1), "swapping twice swaps back to the original order");
        assert_eq!(tree.pane_ids(), vec![1, 2]);
    }

    #[test]
    fn mirror_flips_whole_layout() {
        let mut tree = LayoutTree::new(1);
        tree.split(1, 2, SplitDir::V);
        tree.split(1, 3, SplitDir::V); // 1 -> (1|3) then split 1? order of ids
        let before: Vec<u64> = tree.pane_ids();
        tree.mirror();
        let after: Vec<u64> = tree.pane_ids();
        assert_eq!(after.len(), before.len());
        assert_ne!(after, before, "mirror must reorder panes");
        assert_eq!(after.iter().copied().sum::<u64>(), before.iter().copied().sum::<u64>());
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
