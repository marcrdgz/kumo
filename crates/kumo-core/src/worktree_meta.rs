//! Lightweight checkpoints per worktree: free-text comment + status (`todo`/`in-progress`/...).
//! Persisted atomically to `state_dir()/worktrees.json` so it survives daemon restarts
//! and `kumo update`’s `--resume` cycle. Each worktree path is the key (canonicalized
//! when on disk, else absolute). Warnings never abort worktree creation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const VERSION: u32 = 2;

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
    /// User-facing task/workspace name supplied at creation time.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Git ref or commit used as the creation base (`HEAD` for legacy behavior).
    #[serde(default)]
    pub base_ref: Option<String>,
    /// Agent requested when the worktree was created.
    #[serde(default)]
    pub created_with_agent: Option<String>,
    /// Model requested for the initial agent launch.
    #[serde(default)]
    pub created_with_model: Option<String>,
    /// Reasoning effort requested for the initial agent launch.
    #[serde(default)]
    pub created_with_effort: Option<String>,
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

fn parse_store(bytes: &[u8]) -> Option<Store> {
    let mut store = serde_json::from_slice::<Store>(bytes).ok()?;
    match store.version {
        1 => {
            store.version = VERSION;
            Some(store)
        }
        VERSION => Some(store),
        _ => None,
    }
}

fn load_store() -> Store {
    let p = file();
    if let Ok(bytes) = std::fs::read(&p) {
        if let Some(store) = parse_store(&bytes) {
            return store;
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
    let empty = entry.branch.is_none()
        && entry.comment.is_none()
        && entry.status.is_none()
        && !entry.is_ephemeral
        && entry.display_name.is_none()
        && entry.base_ref.is_none()
        && entry.created_with_agent.is_none()
        && entry.created_with_model.is_none()
        && entry.created_with_effort.is_none();
    if empty {
        store.entries.remove(&k);
        save_store(&store)?;
        return Ok(Checkpoint::default());
    }
    store.entries.insert(k.clone(), entry.clone());
    save_store(&store)?;
    Ok(entry)
}

/// Metadata captured atomically when a worktree is created.
pub struct CreateMetadata {
    pub branch: Option<String>,
    pub comment: Option<String>,
    pub is_ephemeral: bool,
    pub display_name: Option<String>,
    pub base_ref: Option<String>,
    pub created_with_agent: Option<String>,
    pub created_with_model: Option<String>,
    pub created_with_effort: Option<String>,
}

pub fn seed(path: &Path, metadata: CreateMetadata) -> Result<(), String> {
    let mut store = load_store();
    let key = key_for_path(path);
    store.entries.insert(
        key,
        Checkpoint {
            branch: metadata.branch.filter(|value| !value.trim().is_empty()),
            comment: metadata.comment.filter(|value| !value.trim().is_empty()),
            status: None,
            is_ephemeral: metadata.is_ephemeral,
            display_name: metadata.display_name.filter(|value| !value.trim().is_empty()),
            base_ref: metadata.base_ref.filter(|value| !value.trim().is_empty()),
            created_with_agent: metadata.created_with_agent.filter(|value| !value.trim().is_empty()),
            created_with_model: metadata.created_with_model.filter(|value| !value.trim().is_empty()),
            created_with_effort: metadata.created_with_effort.filter(|value| !value.trim().is_empty()),
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0),
        },
    );
    save_store(&store)?;
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
        store.entries.insert(_key.clone(), Checkpoint {
            branch: Some("feat/a".into()), comment: Some("note".into()), status: Some("todo".into()),
            is_ephemeral: true, display_name: None, base_ref: None, created_with_agent: None,
            created_with_model: None, created_with_effort: None,
            updated_at: 1,
        });
        assert_eq!(store.entries.get(&_key).unwrap().branch.as_deref(), Some("feat/a"));
    }

    #[test]
    fn version_one_store_migrates_without_losing_checkpoints() {
        let legacy = br#"{
            "version": 1,
            "entries": {
                "/tmp/legacy": {
                    "branch": "feat/legacy",
                    "comment": "keep me",
                    "status": "in-progress",
                    "is_ephemeral": true,
                    "updated_at": 42
                }
            }
        }"#;

        let store = parse_store(legacy).expect("v1 store must migrate");
        let checkpoint = store.entries.get("/tmp/legacy").unwrap();
        assert_eq!(store.version, VERSION);
        assert_eq!(checkpoint.comment.as_deref(), Some("keep me"));
        assert_eq!(checkpoint.display_name, None);
        assert_eq!(checkpoint.base_ref, None);
        assert_eq!(checkpoint.created_with_agent, None);
        assert_eq!(checkpoint.created_with_model, None);
        assert_eq!(checkpoint.created_with_effort, None);
    }
}
