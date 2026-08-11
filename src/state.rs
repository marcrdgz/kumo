//! Session state contract: the `state.json` schema plus atomic save / tolerant
//! load and the pane-id remap needed when restoring in a new process.
//!
//! This module is deliberately pure data — no `App`, no TUI, no PTY handles.
//! That is what makes it daemon-ready: 0.4.0's daemon reuses these exact types
//! as its wire format and this same save/load path without a rewrite.
//!
//! Some helpers are dormant right now: the light-restore client path was
//! superseded by the daemon, and 0.5.0's persistence (grid re-encode) will
//! revive `save`/`remap`/`from_layout_node` on the daemon side.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::layout::{self, SplitDir};

/// Current schema version. On bump, keep the loader able to read older
/// versions (or reject them gracefully) so restores never crash.
pub const STATE_VERSION: u32 = 1;

/// The full persisted state: every session, its layout tree and its panes.
#[derive(Serialize, Deserialize)]
pub struct SavedState {
    pub version: u32,
    /// Index of the session focused when kumo detached.
    pub active: usize,
    pub sessions: Vec<SavedSession>,
}

/// One restored session. Pane ids inside `tree`/`focus` refer to the saved
/// (pre-remap) ids stored in each `SavedPane`.
#[derive(Serialize, Deserialize)]
pub struct SavedSession {
    pub name: String,
    pub workspace: PathBuf,
    pub zoom: bool,
    pub focus: u64,
    pub tree: SavedNode,
    pub panes: Vec<SavedPane>,
}

/// Layout tree mirror of `layout::Node`, independent of `LayoutTree` bookkeeping.
#[derive(Serialize, Deserialize, Clone)]
pub enum SavedNode {
    Pane { id: u64 },
    Split {
        id: u64,
        dir: SplitDir,
        ratio: f32,
        a: Box<SavedNode>,
        b: Box<SavedNode>,
    },
}

/// Everything needed to respawn a pane (or, later, hand it to the daemon).
/// `id` is the *saved* id used to correlate with the tree before remapping.
#[derive(Serialize, Deserialize)]
pub struct SavedPane {
    pub id: u64,
    pub is_ai: bool,
    pub shell: String,
    pub program: Option<(String, Vec<String>)>,
    pub cwd: PathBuf,
    pub custom_name: Option<String>,
}

/// Write `state` to `path` atomically (temp file + rename) so a crash mid-write
/// never leaves a truncated `state.json`.
pub fn save(path: &Path, state: &SavedState) -> Result<()> {
    let json = serde_json::to_string_pretty(state)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Load `path`. Missing file → `Ok(None)`. Corrupt JSON or an unknown schema
/// version → warn and treat as no state (fresh start), never crash.
pub fn load(path: &Path) -> Result<Option<SavedState>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    match serde_json::from_str::<SavedState>(&content) {
        Ok(s) if s.version == STATE_VERSION => Ok(Some(s)),
        Ok(s) => {
            log::warn!("kumo: ignoring state.json with unknown version {}", s.version);
            Ok(None)
        }
        Err(e) => {
            log::warn!("kumo: ignoring unreadable state.json: {e}");
            Ok(None)
        }
    }
}

/// Remap every pane id in `state` through `map` (new process, fresh ids).
/// Returns the state with ids rewritten in place; panes whose id is missing
/// from `map` are dropped.
pub fn remap_pane_ids(state: &mut SavedState, map: &HashMap<u64, u64>) {
    for session in &mut state.sessions {
        remap_node(&mut session.tree, map);
        session.focus = map.get(&session.focus).copied().unwrap_or(0);
        session.panes.retain(|p| map.contains_key(&p.id));
        for pane in &mut session.panes {
            pane.id = map[&pane.id];
        }
    }
}

fn remap_node(node: &mut SavedNode, map: &HashMap<u64, u64>) {
    match node {
        SavedNode::Pane { id } => *id = map.get(id).copied().unwrap_or(0),
        SavedNode::Split { a, b, .. } => {
            remap_node(a, map);
            remap_node(b, map);
        }
    }
}

/// Saved pane ids present in the tree, depth-first.
pub fn tree_pane_ids(node: &SavedNode, out: &mut Vec<u64>) {
    match node {
        SavedNode::Pane { id } => out.push(*id),
        SavedNode::Split { a, b, .. } => {
            tree_pane_ids(a, out);
            tree_pane_ids(b, out);
        }
    }
}

/// Convert a live `layout::Node` into its saved mirror.
pub fn from_layout_node(node: &layout::Node) -> SavedNode {
    match node {
        layout::Node::Pane { id } => SavedNode::Pane { id: *id },
        layout::Node::Split { id, dir, ratio, a, b } => SavedNode::Split {
            id: *id,
            dir: *dir,
            ratio: *ratio,
            a: Box::new(from_layout_node(a)),
            b: Box::new(from_layout_node(b)),
        },
    }
}

/// Convert a saved node back into a live tree node (ids already remapped).
pub fn to_layout_node(node: &SavedNode) -> layout::Node {
    match node {
        SavedNode::Pane { id } => layout::Node::Pane { id: *id },
        SavedNode::Split { id, dir, ratio, a, b } => layout::Node::Split {
            id: *id,
            dir: *dir,
            ratio: *ratio,
            a: Box::new(to_layout_node(a)),
            b: Box::new(to_layout_node(b)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> SavedState {
        SavedState {
            version: STATE_VERSION,
            active: 0,
            sessions: vec![SavedSession {
                name: "session-1".into(),
                workspace: PathBuf::from("/work"),
                zoom: false,
                focus: 11,
                tree: SavedNode::Split {
                    id: 7,
                    dir: SplitDir::V,
                    ratio: 0.5,
                    a: Box::new(SavedNode::Pane { id: 11 }),
                    b: Box::new(SavedNode::Pane { id: 12 }),
                },
                panes: vec![
                    SavedPane {
                        id: 11,
                        is_ai: false,
                        shell: "/bin/zsh".into(),
                        program: None,
                        cwd: PathBuf::from("/work"),
                        custom_name: None,
                    },
                    SavedPane {
                        id: 12,
                        is_ai: true,
                        shell: "/bin/zsh".into(),
                        program: Some(("opencode".into(), Vec::new())),
                        cwd: PathBuf::from("/work"),
                        custom_name: Some("ai".into()),
                    },
                ],
            }],
        }
    }

    #[test]
    fn save_load_roundtrip_preserves_state() {
        let dir = std::env::temp_dir().join(format!("kumo-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        save(&path, &sample_state()).unwrap();
        let loaded = load(&path).unwrap().expect("state present");
        assert_eq!(loaded.version, STATE_VERSION);
        assert_eq!(loaded.sessions.len(), 1);
        let s = &loaded.sessions[0];
        assert_eq!(s.name, "session-1");
        assert_eq!(s.panes.len(), 2);
        assert_eq!(s.panes[1].program, Some(("opencode".into(), Vec::new())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = std::env::temp_dir().join(format!("kumo-state-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nope.json");
        assert!(load(&path).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_corrupt_json_returns_none() {
        let dir = std::env::temp_dir().join(format!("kumo-state-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&path).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_unknown_version_returns_none() {
        let dir = std::env::temp_dir().join(format!("kumo-state-ver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, r#"{"version": 99, "active": 0, "sessions": []}"#).unwrap();
        assert!(load(&path).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remap_rewrites_tree_focus_and_panes() {
        let mut state = sample_state();
        let mut map = HashMap::new();
        map.insert(11, 100);
        map.insert(12, 200);
        remap_pane_ids(&mut state, &map);
        let s = &state.sessions[0];
        assert_eq!(s.focus, 100);
        assert_eq!(s.panes[0].id, 100);
        assert_eq!(s.panes[1].id, 200);
        assert!(matches!(s.tree, SavedNode::Split { ref a, ref b, .. }
            if matches!(**a, SavedNode::Pane { id: 100 }) && matches!(**b, SavedNode::Pane { id: 200 })));
    }

    #[test]
    fn remap_drops_panes_not_in_map() {
        let mut state = sample_state();
        let mut map = HashMap::new();
        map.insert(11, 100);
        remap_pane_ids(&mut state, &map);
        assert_eq!(state.sessions[0].panes.len(), 1);
        assert_eq!(state.sessions[0].panes[0].id, 100);
    }
}
