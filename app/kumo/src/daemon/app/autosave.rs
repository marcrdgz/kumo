//! Reboot recovery: keeps `state.json` current so a daemon started cold
//! (after a reboot, logout or crash) restores every session.
//!
//! Cost stays off the hot path: a layout change is saved after a short
//! debounce, screen contents at most every `autosave-interval`, and only
//! panes with new output are re-encoded. JSON encoding and the fsynced write
//! run on a writer thread that always takes the newest pending state.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{App, SnapshotCache};
use crate::daemon::state::{self, SavedState};

/// Delay between a layout change and its save, so a burst of splits,
/// resizes or focus moves costs one write.
const LAYOUT_DEBOUNCE: Duration = Duration::from_secs(2);

pub(crate) struct Autosave {
    path: PathBuf,
    /// `None` saves screen contents only on shutdown.
    interval: Option<Duration>,
    writer: mpsc::Sender<SavedState>,
    writer_thread: JoinHandle<()>,
    snapshots: SnapshotCache,
    saved_layout: Option<u64>,
    layout_changed_at: Option<Instant>,
    last_content_check: Instant,
}

impl Autosave {
    pub(crate) fn new(path: PathBuf, interval_secs: u64) -> std::io::Result<Self> {
        let (writer, rx) = mpsc::channel::<SavedState>();
        let target = path.clone();
        let writer_thread = std::thread::Builder::new()
            .name("kumo-autosave".into())
            .spawn(move || {
                while let Ok(mut latest) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        latest = newer;
                    }
                    if let Err(e) = state::save(&target, &latest) {
                        log::warn!("kumo: autosave to {} failed: {e:#}", target.display());
                    }
                }
            })?;
        Ok(Self {
            path,
            interval: (interval_secs > 0).then(|| Duration::from_secs(interval_secs)),
            writer,
            writer_thread,
            snapshots: SnapshotCache::new(),
            saved_layout: None,
            layout_changed_at: None,
            last_content_check: Instant::now(),
        })
    }

    /// Called once per daemon loop iteration; cheap unless a save is due.
    pub(crate) fn tick(&mut self, app: &App) {
        let now = Instant::now();
        if self.saved_layout != Some(app.layout_version) {
            let changed_at = *self.layout_changed_at.get_or_insert(now);
            if now.duration_since(changed_at) >= LAYOUT_DEBOUNCE {
                self.save(app);
            }
            return;
        }
        let Some(interval) = self.interval else { return };
        if now.duration_since(self.last_content_check) >= interval {
            self.last_content_check = now;
            if self.content_changed(app) {
                self.save(app);
            }
        }
    }

    /// Whether any pane has output (or appeared) since its cached snapshot.
    fn content_changed(&self, app: &App) -> bool {
        app.panes.len() != self.snapshots.len()
            || app.panes.iter().any(|(pid, pane)| self.snapshots.get(pid).is_none_or(|(seq, _)| *seq != pane.output_seq()))
    }

    fn save(&mut self, app: &App) {
        self.saved_layout = Some(app.layout_version);
        self.layout_changed_at = None;
        self.last_content_check = Instant::now();
        if let Some(state) = app.to_saved_state(&mut self.snapshots) {
            let _ = self.writer.send(state);
        }
    }

    /// Daemon shutdown. With `Some(app)` (the OS is stopping the daemon:
    /// reboot, logout) the final state is written for the next cold start;
    /// with `None` (`kumo kill`, last session closed) nothing is left to
    /// restore and the file is removed. Waits for the writer either way so
    /// no pending write lands after this returns.
    pub(crate) fn finish(mut self, app: Option<&App>) {
        if let Some(app) = app {
            self.save(app);
        }
        drop(self.writer);
        let _ = self.writer_thread.join();
        if app.is_none() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
