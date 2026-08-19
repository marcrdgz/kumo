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

use kumo_core::layout::{self, SplitDir};

mod base64_serde {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => BASE64.encode(bytes).serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt = Option::<String>::deserialize(deserializer)?;
        match opt {
            Some(s) if s.is_empty() => Ok(None),
            Some(s) => BASE64
                .decode(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

/// Current schema version. On bump, keep the loader able to read older
/// versions (or reject them gracefully) so restores never crash.
pub const STATE_VERSION: u32 = 2;

/// The full persisted state: every session, its tabs and panes.
#[derive(Serialize, Deserialize)]
pub struct SavedState {
    pub version: u32,
    /// Index of the session focused when kumo detached.
    pub active: usize,
    pub sessions: Vec<SavedSession>,
}

/// One restored session — now a collection of tabs sharing one workspace.
#[derive(Serialize, Deserialize, Clone)]
pub struct SavedSession {
    pub name: String,
    pub workspace: PathBuf,
    #[serde(default)]
    pub active_tab: usize,
    pub tabs: Vec<SavedTab>,
    pub panes: Vec<SavedPane>,
}

/// One restored tab (window) — its own layout tree.
#[derive(Serialize, Deserialize, Clone)]
pub struct SavedTab {
    pub id: u64,
    pub name: String,
    pub zoom: bool,
    pub focus: u64,
    pub tree: SavedNode,
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
///
/// The `master_fd`/`child_pid`/`cols`/`rows` fields are resume-only: they are
/// set by the daemon when it snapshots its live panes for `kumo update`, so the
/// restarted daemon can adopt the inherited PTY master descriptors. They are
/// `#[serde(default)]` so ordinary persisted state (and 0.4.0's snapshots)
/// round-trip unchanged.
#[derive(Serialize, Deserialize, Clone)]
pub struct SavedPane {
    pub id: u64,
    pub is_ai: bool,
    pub shell: String,
    pub program: Option<(String, Vec<String>)>,
    pub cwd: PathBuf,
    pub custom_name: Option<String>,
    #[serde(default)]
    pub master_fd: Option<i64>,
    #[serde(default)]
    pub child_pid: Option<i64>,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
    /// DEC mouse-reporting state before the restart, so the resumed emulator
    /// can re-learn it (the live app kept its mouse mode enabled app-side).
    #[serde(default)]
    pub mouse_tracking: bool,
    /// Inline ghostty snapshot bytes (screen + scrollback + continuation),
    /// base64-encoded in JSON. `None` for old state or when encode failed.
    #[serde(default, with = "base64_serde")]
    pub snapshot: Option<Vec<u8>>,
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
/// v1 state is migrated: each old session's single tree becomes one tab "1".
pub fn load(path: &Path) -> Result<Option<SavedState>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    // Peek version without fully decoding.
    #[derive(Deserialize)]
    struct VersionPeek { version: u32 }
    let version: Option<u32> = serde_json::from_str::<VersionPeek>(&content).ok().map(|v| v.version);
    match version {
        Some(2) => match serde_json::from_str::<SavedState>(&content) {
            Ok(s) => Ok(Some(s)),
            Err(e) => {
                log::warn!("kumo: ignoring unreadable state.json: {e}");
                Ok(None)
            }
        },
        Some(1) => match serde_json::from_str::<SavedStateV1>(&content) {
            Ok(v1) => Ok(Some(v1.into_v2())),
            Err(e) => {
                log::warn!("kumo: ignoring unreadable state.json: {e}");
                Ok(None)
            }
        },
        Some(v) => {
            log::warn!("kumo: ignoring state.json with unknown version {v}");
            Ok(None)
        }
        None => {
            log::warn!("kumo: ignoring unreadable state.json");
            Ok(None)
        }
    }
}

/// v1 types for migration.
#[derive(Deserialize)]
struct SavedStateV1 { version: u32, active: usize, sessions: Vec<SavedSessionV1> }
#[derive(Deserialize, Clone)]
struct SavedSessionV1 { name: String, workspace: PathBuf, zoom: bool, focus: u64, tree: SavedNode, panes: Vec<SavedPane> }
impl SavedStateV1 {
    fn into_v2(self) -> SavedState {
        let sessions = self.sessions.into_iter().map(|s| SavedSession {
            name: s.name,
            workspace: s.workspace,
            active_tab: 0,
            tabs: vec![SavedTab { id: 1, name: "1".to_string(), zoom: s.zoom, focus: s.focus, tree: s.tree }],
            panes: s.panes,
        }).collect();
        SavedState { version: STATE_VERSION, active: self.active, sessions }
    }
}

/// Remap every pane id in `state` through `map` (new process, fresh ids).
/// Returns the state with ids rewritten in place; panes whose id is missing
/// from `map` are dropped.
pub fn remap_pane_ids(state: &mut SavedState, map: &HashMap<u64, u64>) {
    for session in &mut state.sessions {
        for tab in &mut session.tabs {
            remap_node(&mut tab.tree, map);
            tab.focus = map.get(&tab.focus).copied().unwrap_or(0);
        }
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
                active_tab: 0,
                tabs: vec![SavedTab {
                    id: 1,
                    name: "1".into(),
                    zoom: false,
                    focus: 11,
                    tree: SavedNode::Split {
                        id: 7,
                        dir: SplitDir::V,
                        ratio: 0.5,
                        a: Box::new(SavedNode::Pane { id: 11 }),
                        b: Box::new(SavedNode::Pane { id: 12 }),
                    },
                }],
                panes: vec![
                    SavedPane {
                        id: 11,
                        is_ai: false,
                        shell: "/bin/zsh".into(),
                        program: None,
                        cwd: PathBuf::from("/work"),
                        custom_name: None,
                        master_fd: None,
                        child_pid: None,
                        cols: 80,
                        rows: 24,
                        mouse_tracking: false,
                        snapshot: None,
                    },
                    SavedPane {
                        id: 12,
                        is_ai: true,
                        shell: "/bin/zsh".into(),
                        program: Some(("opencode".into(), Vec::new())),
                        cwd: PathBuf::from("/work"),
                        custom_name: Some("ai".into()),
                        master_fd: None,
                        child_pid: None,
                        cols: 80,
                        rows: 24,
                        mouse_tracking: true,
                        snapshot: None,
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
    fn resume_fields_round_trip() {
        // The resume-only fields (master fd / child pid / size) survive a
        // save+load, so the restarted daemon can adopt the inherited PTYs.
        let mut state = sample_state();
        state.sessions[0].panes[0].master_fd = Some(7);
        state.sessions[0].panes[0].child_pid = Some(4242);
        state.sessions[0].panes[0].cols = 120;
        state.sessions[0].panes[0].rows = 30;
        let dir = std::env::temp_dir().join(format!("kumo-state-resume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("resume.json");
        save(&path, &state).unwrap();
        let loaded = load(&path).unwrap().expect("resume present");
        let p = &loaded.sessions[0].panes[0];
        assert_eq!(p.master_fd, Some(7));
        assert_eq!(p.child_pid, Some(4242));
        assert_eq!((p.cols, p.rows), (120, 30));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_state_without_resume_fields_loads() {
        // A pre-resume `state.json` (no fd/pid/size fields) must still load via
        // the serde defaults, so nothing written before breaks a later update.
        let json_v1 = r#"{"version":1,"active":0,"sessions":[{"name":"session-1","workspace":"/tmp","zoom":false,"focus":11,"tree":{"Pane":{"id":11}},"panes":[{"id":11,"is_ai":false,"shell":"/bin/sh","program":null,"cwd":"/tmp","custom_name":null}]}]}"#;
        let dir = std::env::temp_dir().join(format!("kumo-state-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, json_v1).unwrap();
        let loaded = load(&path).unwrap().expect("migrated");
        assert_eq!(loaded.version, STATE_VERSION);
        let p = &loaded.sessions[0].panes[0];
        assert_eq!(p.master_fd, None);
        assert_eq!(p.child_pid, None);
        assert_eq!((p.cols, p.rows), (0, 0));
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
        assert_eq!(s.tabs[0].focus, 100);
        assert_eq!(s.panes[0].id, 100);
        assert_eq!(s.panes[1].id, 200);
        assert!(matches!(s.tabs[0].tree, SavedNode::Split { ref a, ref b, .. }
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

    #[test]
    fn v1_migrates_to_single_tab() {
        let json_v1 = r#"{"version":1,"active":0,"sessions":[{"name":"s1","workspace":"/tmp","zoom":true,"focus":42,"tree":{"Pane":{"id":42}},"panes":[{"id":42,"is_ai":false,"shell":"/bin/sh","program":null,"cwd":"/tmp","custom_name":null}]}]}"#;
        let dir = std::env::temp_dir().join(format!("kumo-state-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, json_v1).unwrap();
        let loaded = load(&path).unwrap().expect("migrated");
        assert_eq!(loaded.version, 2);
        assert_eq!(loaded.sessions[0].tabs.len(), 1);
        assert_eq!(loaded.sessions[0].tabs[0].name, "1");
        assert_eq!(loaded.sessions[0].tabs[0].focus, 42);
        assert!(loaded.sessions[0].tabs[0].zoom);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
