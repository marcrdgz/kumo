//! Persist ADE mutations off the daemon loop, coalescing pending snapshots.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use kumo_core::ade::{InboxKind, RunState, Store};

#[derive(Default)]
struct Pending {
    latest: Option<(u64, Store)>,
    completed: u64,
    error: Option<String>,
    stopping: bool,
}

struct Writer {
    shared: Arc<(Mutex<Pending>, Condvar)>,
    join: Option<JoinHandle<()>>,
}

impl Writer {
    fn new(path: PathBuf) -> Result<Self> {
        let shared = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker = shared.clone();
        let join = thread::Builder::new().name("kumo-ade-store".into()).spawn(move || {
            let (lock, wake) = &*worker;
            loop {
                let next = {
                    let mut pending = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while pending.latest.is_none() && !pending.stopping {
                        pending = wake.wait(pending).unwrap_or_else(|e| e.into_inner());
                    }
                    match pending.latest.take() {
                        Some(next) => next,
                        None => break,
                    }
                };
                // Serialization and filesystem operations never hold the lock.
                let result = std::panic::catch_unwind(|| next.1.save(&path));
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(format!("{error:#}")),
                    Err(_) => Some("ADE persistence worker panicked while saving".into()),
                };
                if let Some(error) = &error { log::error!("ADE persistence failed: {error}"); }
                let mut pending = lock.lock().unwrap_or_else(|e| e.into_inner());
                pending.completed = next.0;
                pending.error = error;
                wake.notify_all();
            }
        }).context("spawn ADE persistence worker")?;
        Ok(Self { shared, join: Some(join) })
    }

    fn enqueue(&self, revision: u64, store: Store) {
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).latest = Some((revision, store));
        wake.notify_one();
    }

    fn flush(&self, revision: u64) -> Result<()> {
        let (lock, wake) = &*self.shared;
        let pending = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (pending, _) = wake.wait_timeout_while(pending, Duration::from_secs(10), |state| state.completed < revision)
            .map_err(|_| anyhow!("ADE persistence state poisoned"))?;
        if pending.completed < revision { return Err(anyhow!("timed out flushing ADE store")); }
        if let Some(error) = &pending.error { return Err(anyhow!("ADE persistence failed: {error}")); }
        Ok(())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).stopping = true;
        wake.notify_all();
        if let Some(join) = self.join.take() {
            if join.join().is_err() { log::error!("ADE persistence worker exited unexpectedly"); }
        }
    }
}

pub struct AdeRuntime {
    store: Store,
    writer: Writer,
    revision: u64,
}

impl AdeRuntime {
    pub fn load(path: impl AsRef<Path>, _cold: bool) -> Result<Self> {
        let path = path.as_ref();
        // Reject corrupt or newer stores before a writer can overwrite them.
        let store = Store::load(path)?;
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).context("create ADE state directory")?; }
        Ok(Self { store, writer: Writer::new(path.to_owned())?, revision: 0 })
    }
    pub fn snapshot(&self) -> &Store { &self.store }
    pub fn revision(&self) -> u64 { self.revision }
    fn changed(&mut self) {
        self.revision += 1;
        self.writer.enqueue(self.revision, self.store.clone());
    }
    pub fn register_workspace(&mut self, path: &Path, label: &str) -> Result<u64> {
        let id = self.store.workspace(path, label)?;
        self.changed();
        Ok(id)
    }
    pub fn start_run(&mut self, workspace: u64, pane: u64, pid: Option<u32>, agent: String, prompt: String) -> Result<u64> {
        let id = self.store.start_run(workspace, pane, pid, agent, prompt)?;
        self.changed();
        Ok(id)
    }
    pub fn bind_run(&mut self, run: u64, pane: u64, pid: Option<u32>) -> Result<()> {
        if !self.store.rebind_run(run, pane, pid) { return Err(anyhow!("run {run} cannot be resumed")); }
        self.changed();
        Ok(())
    }
    pub fn observe(&mut self, run: u64, state: RunState, key: Option<String>, kind: Option<InboxKind>) -> Result<()> {
        if self.store.runs().iter().any(|record| record.id == run && record.state != state) {
            // Insert the event before closing can prune acknowledged history.
            if let (Some(key), Some(kind)) = (key, kind) { self.store.push_inbox(run, key, kind)?; }
            self.store.set_run_state(run, state);
            self.changed();
        }
        Ok(())
    }
    pub fn acknowledge(&mut self, id: u64) -> bool {
        let changed = self.store.acknowledge_inbox(id);
        if changed { self.changed(); }
        changed
    }
    pub fn flush(&self) -> Result<()> { self.writer.flush(self.revision) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kumo-ade-{name}-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()))
    }

    #[test]
    fn writer_coalesces_latest_snapshot() {
        let file = path("latest");
        let mut runtime = AdeRuntime::load(&file, true).unwrap();
        runtime.register_workspace(Path::new("."), "first").unwrap();
        runtime.register_workspace(Path::new("."), "latest").unwrap();
        runtime.flush().unwrap();
        assert_eq!(Store::load(&file).unwrap().workspaces()[0].label, "latest");
        let _ = fs::remove_file(file);
    }

    #[test]
    fn unchanged_observation_does_not_advance_revision() {
        let file = path("unchanged");
        let mut runtime = AdeRuntime::load(&file, true).unwrap();
        let workspace = runtime.register_workspace(Path::new("."), "test").unwrap();
        let run = runtime.start_run(workspace, 1, None, "agent".into(), "prompt".into()).unwrap();
        let revision = runtime.revision();
        runtime.observe(run, RunState::Submitted, None, None).unwrap();
        assert_eq!(runtime.revision(), revision);
        let _ = fs::remove_file(file);
    }

    #[test]
    fn writer_flush_propagates_save_failure() {
        let target = path("failure");
        let mut runtime = AdeRuntime::load(&target, true).unwrap();
        fs::remove_file(&target).ok();
        fs::create_dir(&target).unwrap();
        runtime.register_workspace(Path::new("."), "test").unwrap();
        assert!(runtime.flush().is_err());
        let _ = fs::remove_dir_all(target);
    }
}
