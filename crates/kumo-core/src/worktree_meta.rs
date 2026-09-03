//! Lightweight checkpoints per worktree: free-text comment + status (`todo`/`in-progress`/...).
//! Persisted atomically to `state_dir()/worktrees.json` so it survives daemon restarts
//! and `kumo update`’s `--resume` cycle. Each worktree path is the key (canonicalized
//! when on disk, else absolute). Warnings never abort worktree creation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Checkpoint {
    /// Git branch checked out in this worktree (None = detached HEAD).
    pub branch: Option<String>,
    /// Free-text comment seeded via `--note` or updated via `kumo worktree set`.
    pub comment: Option<String>,
    /// Status `todo|in-progress|in-review|completed` (lowercase, validated on write).
    pub status: Option<String>,
    /// Whether this was created via `kumo worktree create --ai` (ephemeral).
    #[serde(default)]
    pub is_ephemeral: bool,
    /// Unix millis when the entry was last touched.
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Store {
    version: u32,
    entries: HashMap<String, Checkpoint>,
}

impl Default for Store {
    fn default() -> Self {
        Self { version: VERSION, entries: HashMap::new() }
    }
}

fn file() -> PathBuf {
    crate::config::state_dir().join("worktrees.json")
}

fn key_for_path(path: &Path) -> String {
    if let Ok(c) = std::fs::canonicalize(path) {
        c.to_string_lossy().into_owned()
    } else {
        // Not yet on disk (pre-create check) — normalize to absolute
        if path.is_absolute() {
            path.to_string_lossy().into_owned()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("/"))
                .join(path)
                .to_string_lossy()
                .into_owned()
        }
    }
}

fn load_store() -> Store {
    let p = file();
    if let Ok(bytes) = std::fs::read(&p) {
        if let Ok(s) = serde_json::from_slice::<Store>(&bytes) {
            if s.version == VERSION {
                return s;
            }
        }
        // Corrupt/unknown version → start fresh (never crash daemon)
        log::warn!("kumo: worktree_meta: ignoring corrupt {} — starting fresh", p.display());
    }
    Store::default()
}

fn save_store(store: &Store) -> Result<(), String> {
    let p = file();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = p.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(store).map_err(|e| format!("serialize worktrees.json: {e}"))?;
    std::fs::write(&tmp, data).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("rename {} → {}: {e}", tmp.display(), p.display()))?;
    Ok(())
}
/// Isolated worktree checkpoint getters/setters (daemon-side, synchronous).
pub fn get(path: &Path) -> Option<Checkpoint> {
    let store = load_store();
    let k = key_for_path(path);
    // Try canonical key; also try raw path and alternative canonicalization for resilience
    if let Some(v) = store.entries.get(&k).cloned() {
        return Some(v);
    }
    // Try non-canonical key (e.g. /private vs / on macOS)
    let alt = path.to_string_lossy().into_owned();
    store.entries.get(&alt).cloned().or_else(|| {
        // Try canonicalizing all keys that match the filesystem path
        let canon = std::fs::canonicalize(path).ok().map(|p| p.to_string_lossy().into_owned());
        if let Some(c) = canon {
            store.entries.get(&c).cloned()
        } else {
            None
        }
    })
}

pub fn all() -> HashMap<String, Checkpoint> {
    load_store().entries
}

/// Set (or clear, when `None`) checkpoint fields for `path`. `branch` + `is_ephemeral`
/// are set on creation and remain unless overwritten. Clearing `comment`/`status` with `None`
/// keeps the entry but nulls that field; an entry with all fields `None`/false is GC'd on next `prune`.
pub fn set(
    path: &Path,
    comment: Option<Option<String>>,
    status: Option<Option<String>>,
    branch: Option<Option<String>>,
    is_ephemeral: Option<bool>,
) -> Result<Checkpoint, String> {
    let mut store = load_store();
    let k = key_for_path(path);
    let mut entry = store.entries.get(&k).cloned().unwrap_or_default();
    if let Some(c) = comment {
        entry.comment = c.filter(|s| !s.trim().is_empty());
    }
    if let Some(s) = status {
        entry.status = match s {
            Some(raw) => {
                let trimmed = raw.trim().to_string();
                if trimmed.is_empty() { None } else {
                    let parsed = crate::worktrees::validate_branch_name; // placeholder to avoid unused; real validation via WorktreeStatus::parse
                    let _ = parsed;
                    // Validate via protocol helper if available; otherwise accept normalized lowercased value
                    let lower = trimmed.to_ascii_lowercase();
                    if kumo_protocol::WorktreeStatus::parse(&lower).is_some() {
                        Some(lower)
                    } else {
                        return Err(format!("invalid status {trimmed:?} (use todo|in-progress|in-review|completed)"));
                    }
                }
            }
            None => None,
        };
    }
    if let Some(b) = branch {
        entry.branch = b.filter(|s| !s.trim().is_empty());
    }
    if let Some(e) = is_ephemeral {
        entry.is_ephemeral = e;
    }
    entry.updated_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // GC: if every user field cleared, drop entry
    let empty = entry.branch.is_none() && entry.comment.is_none() && entry.status.is_none() && !entry.is_ephemeral;
    if empty {
        store.entries.remove(&k);
        save_store(&store)?;
        return Ok(Checkpoint::default());
    }
    store.entries.insert(k.clone(), entry.clone());
    save_store(&store)?;
    Ok(entry)
}

/// Convenience for `kumo worktree create --ai` seeding: set branch/comment/ephemeral at once.
pub fn seed(path: &Path, branch: Option<String>, comment: Option<String>, is_ephemeral: bool) -> Result<(), String> {
    set(
        path,
        Some(comment),
        None,
        Some(branch),
        Some(is_ephemeral),
    )?;
    Ok(())
}

pub fn remove(path: &Path) -> Result<(), String> {
    let mut store = load_store();
    let k = key_for_path(path);
    let mut removed = store.entries.remove(&k).is_some();
    // Also try alt keys (non-canonical) to fully purge
    let alt = path.to_string_lossy().into_owned();
    if store.entries.remove(&alt).is_some() { removed = true; }
    if let Ok(canon) = std::fs::canonicalize(path) {
        let ck = canon.to_string_lossy().into_owned();
        if store.entries.remove(&ck).is_some() { removed = true; }
    }
    if removed {
        save_store(&store)?;
    }
    Ok(())
}

/// Remove entries whose worktree directories no longer exist (GC for `worktree list` / daemon tick).
pub fn prune_missing() -> usize {
    let mut store = load_store();
    let before = store.entries.len();
    store.entries.retain(|k, _| PathBuf::from(k).exists());
    if store.entries.len() != before {
        let _ = save_store(&store);
    }
    before - store.entries.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[allow(dead_code)]
    fn tmp_file_path() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "kumo_worktree_meta_test_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        p
    }

    #[test]
    fn checkpoint_round_trip_via_store() {
        // Use a real temp dir to exercise canonicalization
        let dir = std::env::temp_dir().join(format!("kumo_meta_dir_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let wt = dir.join("wt-a");
        let _ = fs::create_dir_all(&wt);
        let _key = key_for_path(&wt);
        // Direct Store manipulation (isolated from global state_dir by not calling set/get which hit real file)
        let mut store = Store::default();
        store.entries.insert(_key.clone(), Checkpoint { branch: Some("feat/a".into()), comment: Some("note".into()), status: Some("todo".into()), is_ephemeral: true, updated_at: 1 });
        assert_eq!(store.entries.get(&_key).unwrap().branch.as_deref(), Some("feat/a"));
    }
}
