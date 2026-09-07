//! Persistent storage for ADE workspaces and agent runs.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: u32 = 1;
const CLOSED_HISTORY_LIMIT: usize = 256;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    pub id: u64,
    pub path: PathBuf,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: u64,
    pub workspace_id: u64,
    pub pane_id: u64,
    pub child_pid: Option<u32>,
    pub agent: String,
    pub prompt: String,
    pub created_at_ms: u64,
    pub state: RunState,
}

/// A durable actionable event associated with a run. `key` is stable across
/// daemon restarts so repeated lifecycle observations do not duplicate inbox
/// entries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InboxRecord {
    pub id: u64,
    pub run_id: u64,
    pub key: String,
    pub kind: InboxKind,
    pub created_at_ms: u64,
    pub acknowledged: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    Approval,
    Question,
    Blocked,
    Completed,
    Review,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Submitted,
    Working,
    Blocked,
    Idle,
    Unknown,
    Done,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Store {
    version: u32,
    next_id: u64,
    workspaces: Vec<WorkspaceRecord>,
    runs: Vec<RunRecord>,
    #[serde(default)]
    inbox: Vec<InboxRecord>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_id: 1,
            workspaces: Vec::new(),
            runs: Vec::new(),
            inbox: Vec::new(),
        }
    }
}

impl Store {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => {
                return Err(error).with_context(|| format!("read ADE store {}", path.display()))
            }
        };
        let store: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse ADE store {}", path.display()))?;
        if store.version != VERSION {
            return Err(anyhow!("unsupported ADE store version {}", store.version));
        }
        store.validate()?;
        Ok(store)
    }

    pub fn workspace(&mut self, path: &Path, label: &str) -> Result<u64> {
        let normalized = normalize_path(path)?;
        if let Some(workspace) = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.path == normalized)
        {
            workspace.label = label.to_owned();
            return Ok(workspace.id);
        }
        let id = self.allocate_id()?;
        self.workspaces.push(WorkspaceRecord {
            id,
            path: normalized,
            label: label.to_owned(),
        });
        Ok(id)
    }

    pub fn start_run(
        &mut self,
        workspace_id: u64,
        pane_id: u64,
        child_pid: Option<u32>,
        agent: String,
        prompt: String,
    ) -> Result<u64> {
        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
        {
            return Err(anyhow!("unknown workspace id {workspace_id}"));
        }
        for run in &mut self.runs {
            if run.pane_id == pane_id && run.state != RunState::Closed {
                run.state = RunState::Closed;
            }
        }
        let id = self.allocate_id()?;
        self.runs.push(RunRecord {
            id,
            workspace_id,
            pane_id,
            child_pid,
            agent,
            prompt,
            created_at_ms: now_ms(),
            state: RunState::Submitted,
        });
        self.prune_closed_history();
        Ok(id)
    }

    pub fn workspaces(&self) -> &[WorkspaceRecord] {
        &self.workspaces
    }

    pub fn runs(&self) -> &[RunRecord] {
        &self.runs
    }

    pub fn inbox(&self) -> &[InboxRecord] {
        &self.inbox
    }

    /// Append an inbox event once. Returns its stable ID, or the existing ID
    /// when the same run/key pair has already been recorded.
    pub fn push_inbox(&mut self, run_id: u64, key: String, kind: InboxKind) -> Result<u64> {
        if !self.runs.iter().any(|run| run.id == run_id) {
            return Err(anyhow!("unknown run id {run_id}"));
        }
        if let Some(item) = self
            .inbox
            .iter()
            .find(|item| item.run_id == run_id && item.key == key)
        {
            return Ok(item.id);
        }
        let id = self.allocate_id()?;
        self.inbox.push(InboxRecord {
            id,
            run_id,
            key,
            kind,
            created_at_ms: now_ms(),
            acknowledged: false,
        });
        Ok(id)
    }

    pub fn acknowledge_inbox(&mut self, id: u64) -> bool {
        if let Some(item) = self.inbox.iter_mut().find(|item| item.id == id) {
            if item.acknowledged {
                return false;
            }
            item.acknowledged = true;
            return true;
        }
        false
    }

    pub fn set_run_state(&mut self, id: u64, state: RunState) -> bool {
        if let Some(run) = self.runs.iter_mut().find(|run| run.id == id) {
            if run.state == state {
                return false;
            }
            run.state = state;
            self.prune_closed_history();
            true
        } else {
            false
        }
    }

    pub fn rebind_run(&mut self, id: u64, pane_id: u64, child_pid: Option<u32>) -> bool {
        if let Some(run) = self
            .runs
            .iter_mut()
            .find(|run| run.id == id && run.state != RunState::Closed)
        {
            run.pane_id = pane_id;
            run.child_pid = child_pid;
            true
        } else { false }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let name = path
            .file_name()
            .ok_or_else(|| anyhow!("store path has no file name"))?;
        let data = serde_json::to_vec_pretty(self).context("serialize ADE store")?;
        let pid = std::process::id();
        let mut temporary = None;
        let mut file = None;
        for _ in 0..100 {
            let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let candidate =
                parent.join(format!(".{}.tmp-{}-{}", name.to_string_lossy(), pid, count));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&candidate) {
                Ok(created) => {
                    temporary = Some(candidate);
                    file = Some(created);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("create temporary ADE store beside {}", path.display())
                    })
                }
            }
        }
        let temporary =
            temporary.ok_or_else(|| anyhow!("could not create unique temporary ADE store"))?;
        let result = (|| -> Result<()> {
            let mut file: File = file.expect("temporary file accompanies path");
            file.write_all(&data)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)
                .with_context(|| format!("replace ADE store {}", path.display()))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn allocate_id(&mut self) -> Result<u64> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| anyhow!("ADE store ID exhausted"))?;
        Ok(id)
    }

    fn validate(&self) -> Result<()> {
        let mut ids = HashSet::with_capacity(self.workspaces.len() + self.runs.len());
        let mut max_id = 0;
        for workspace in &self.workspaces {
            if !ids.insert(workspace.id) {
                return Err(anyhow!("duplicate ADE record id {}", workspace.id));
            }
            max_id = max_id.max(workspace.id);
        }
        for run in &self.runs {
            if !ids.insert(run.id) {
                return Err(anyhow!("duplicate ADE record id {}", run.id));
            }
            if !self
                .workspaces
                .iter()
                .any(|workspace| workspace.id == run.workspace_id)
            {
                return Err(anyhow!(
                    "run {} references unknown workspace {}",
                    run.id,
                    run.workspace_id
                ));
            }
            max_id = max_id.max(run.id);
        }
        for item in &self.inbox {
            if !ids.insert(item.id) {
                return Err(anyhow!("duplicate ADE record id {}", item.id));
            }
            if !self.runs.iter().any(|run| run.id == item.run_id) {
                return Err(anyhow!(
                    "inbox {} references unknown run {}",
                    item.id,
                    item.run_id
                ));
            }
            max_id = max_id.max(item.id);
        }
        if self.next_id <= max_id {
            return Err(anyhow!(
                "next ADE record id is not greater than existing ids"
            ));
        }
        Ok(())
    }

    fn prune_closed_history(&mut self) {
        // Pending events remain actionable even after their terminal exits.
        let pending: HashSet<_> = self.inbox.iter().filter(|item| !item.acknowledged).map(|item| item.run_id).collect();
        let closed = self
            .runs
            .iter()
            .filter(|run| run.state == RunState::Closed && !pending.contains(&run.id))
            .count();
        if closed <= CLOSED_HISTORY_LIMIT {
            return;
        }
        let mut remove = closed - CLOSED_HISTORY_LIMIT;
        let mut removed = HashSet::new();
        self.runs.retain(|run| {
            if remove > 0 && run.state == RunState::Closed && !pending.contains(&run.id) {
                remove -= 1;
                removed.insert(run.id);
                false
            } else {
                true
            }
        });
        if !removed.is_empty() {
            self.inbox.retain(|item| !removed.contains(&item.run_id));
        }
    }
}

fn normalize_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        Ok(path.canonicalize()?)
    } else if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "kumo-ade-test-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn roundtrip_and_stable_workspace_ids() {
        let dir = temp_dir();
        let file = dir.join("store.json");
        let mut store = Store::default();
        let id = store.workspace(&dir, "first").unwrap();
        assert_eq!(id, store.workspace(&dir.join("."), "renamed").unwrap());
        assert_eq!(store.workspaces()[0].label, "renamed");
        store.save(&file).unwrap();
        assert_eq!(Store::load(&file).unwrap(), store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_unknown_version_without_rewriting_file() {
        let dir = temp_dir();
        let file = dir.join("store.json");
        fs::write(
            &file,
            br#"{"version":99,"next_id":1,"workspaces":[],"runs":[]}"#,
        )
        .unwrap();
        let before = fs::read(&file).unwrap();
        assert!(Store::load(&file).is_err());
        assert_eq!(fs::read(&file).unwrap(), before);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn replaces_pane_run_and_prunes_old_closed_history() {
        let mut store = Store::default();
        store.workspace(Path::new("."), "workspace").unwrap();
        for index in 0..257 {
            store
                .start_run(1, 7, None, "agent".into(), index.to_string())
                .unwrap();
        }
        assert_eq!(store.runs().len(), 257);
        let pane = store
            .start_run(1, 7, None, "agent".into(), "replacement".into())
            .unwrap();
        assert_eq!(store.runs().len(), 257);
        assert_eq!(
            store
                .runs()
                .iter()
                .filter(|run| run.state == RunState::Closed)
                .count(),
            256
        );
        assert_eq!(
            store
                .runs()
                .iter()
                .find(|run| run.id == pane)
                .unwrap()
                .state,
            RunState::Submitted
        );
        assert!(!store.set_run_state(pane, RunState::Submitted));
        assert!(!store.set_run_state(u64::MAX, RunState::Idle));
    }

    #[test]
    fn failed_save_preserves_existing_data() {
        let dir = temp_dir();
        let file = dir.join("store.json");
        let mut store = Store::default();
        store.workspace(&dir, "original").unwrap();
        store.save(&file).unwrap();
        let before = fs::read(&file).unwrap();
        let failed = store.save(&dir.join("missing").join("store.json"));
        assert!(failed.is_err());
        assert_eq!(fs::read(&file).unwrap(), before);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn inbox_events_are_deduplicated_and_acknowledged() {
        let mut store = Store::default();
        store.workspace(Path::new("."), "workspace").unwrap();
        let run = store
            .start_run(1, 7, None, "agent".into(), "prompt".into())
            .unwrap();
        let first = store
            .push_inbox(run, "blocked:7".into(), InboxKind::Blocked)
            .unwrap();
        assert_eq!(
            store
                .push_inbox(run, "blocked:7".into(), InboxKind::Blocked)
                .unwrap(),
            first
        );
        assert_eq!(store.inbox().len(), 1);
        assert!(store.acknowledge_inbox(first));
        assert!(!store.acknowledge_inbox(first));
    }

    #[test]
    fn rebind_updates_live_identity_and_rejects_unknown_runs() {
        let mut store = Store::default();
        let workspace = store.workspace(Path::new("."), "workspace").unwrap();
        let run = store
            .start_run(workspace, 7, Some(10), "agent".into(), "prompt".into())
            .unwrap();
        assert!(store.rebind_run(run, 11, Some(12)));
        assert_eq!(store.runs()[0].pane_id, 11);
        assert_eq!(store.runs()[0].child_pid, Some(12));
        assert!(!store.rebind_run(u64::MAX, 13, None));
        assert!(store.set_run_state(run, RunState::Closed));
        assert!(!store.rebind_run(run, 14, None));
    }
}
