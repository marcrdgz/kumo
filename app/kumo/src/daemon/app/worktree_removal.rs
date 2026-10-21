//! Worktree removal, split so the daemon loop never waits on the filesystem.
//!
//! `git worktree remove` deletes the whole checkout, including ignored build
//! output (`target/`, `node_modules/`), which can take a minute. The loop runs
//! the cheap, side-effect-free preflight and the session teardown
//! ([`App::begin_worktree_removal`]); a worker thread then waits for the PTY
//! children and deletes the checkout ([`WorktreeRemoval::run`]); the loop
//! applies the result, recovering the session on failure
//! ([`App::finish_worktree_removal`]).

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use super::App;

/// A worktree whose session is already torn down, ready to be deleted off the
/// daemon loop.
pub(crate) struct WorktreeRemoval {
    path: PathBuf,
    /// Canonical path identifying the in-flight removal.
    key: PathBuf,
    repo_root: PathBuf,
    branch: Option<String>,
    force: bool,
    /// PTY cleanup workers of the closed session.
    pty_workers: Vec<JoinHandle<()>>,
    /// Name and workspace of the closed session, to reopen it on failure.
    recovery: Option<(String, PathBuf)>,
}

/// The result of [`WorktreeRemoval::run`], applied on the daemon loop.
pub(crate) struct WorktreeRemovalOutcome {
    key: PathBuf,
    recovery: Option<(String, PathBuf)>,
    result: Result<String, String>,
}

impl WorktreeRemoval {
    /// Delete the checkout and clean up its branch. Blocking; never touches
    /// `App`, so it can run on any thread.
    pub(crate) fn run(self) -> WorktreeRemovalOutcome {
        let WorktreeRemoval { path, key, repo_root, branch, force, pty_workers, recovery } = self;
        // Wait for the PTY cleanup workers before allowing Git to remove the
        // directory. This avoids leaving children with a deleted cwd.
        for worker in pty_workers {
            let _ = worker.join();
        }
        let result = remove_checkout(&path, &repo_root, branch, force);
        WorktreeRemovalOutcome { key, recovery, result }
    }
}

fn remove_checkout(path: &Path, root: &Path, branch: Option<String>, force: bool) -> Result<String, String> {
    if !force {
        let dirty = kumo_core::worktrees::dirty_worktree_entries(path)?;
        if !dirty.is_empty() {
            return Err(format!(
                "worktree changed while closing; refusing removal ({}): {}",
                dirty.len(),
                dirty.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    kumo_core::worktrees::remove_worktree(root, path, force)?;

    // The checkout is gone at this point, so metadata can safely be
    // removed even when branch cleanup below needs to report an error.
    let mut warnings = Vec::new();
    if let Err(error) = kumo_core::worktree_meta::remove(path) {
        warnings.push(format!("metadata cleanup failed: {error}"));
    }

    if let Some(br) = branch {
        match kumo_core::worktrees::list_worktrees(root) {
            Ok(worktrees) => {
                let still_used = worktrees.iter().any(|w| w.branch.as_deref() == Some(&br));
                if !still_used {
                    let preserve = if !force {
                        match kumo_core::worktrees::branch_unmerged_count(root, &br) {
                            Ok(cnt) if cnt > 0 => match kumo_core::worktrees::branch_head(root, &br) {
                                Ok(head) => {
                                    warnings.push(format!("branch {br:?} preserved: {cnt} unmerged commits, head {head}"));
                                    true
                                }
                                Err(error) => {
                                    warnings.push(format!("could not resolve preserved branch {br:?}: {error}"));
                                    false
                                }
                            },
                            Ok(_) => false,
                            Err(error) => {
                                warnings.push(format!("could not inspect branch {br:?}: {error}"));
                                false
                            }
                        }
                    } else {
                        false
                    };
                    if !preserve {
                        if let Err(error) = kumo_core::worktrees::delete_branch(root, &br, force) {
                            warnings.push(format!("branch cleanup failed for {br:?}: {error}"));
                        }
                    }
                }
            }
            Err(error) => warnings.push(format!("could not inspect remaining worktrees for branch cleanup: {error}")),
        }
    }
    let mut message = format!("removed worktree {}", path.display());
    for warning in warnings {
        message.push_str("; warning: ");
        message.push_str(&warning);
    }
    Ok(message)
}

impl App {
    /// Start removing the worktree at `path` (and optionally its branch).
    /// Finds the repo root from `path` or the given `repo_hint`, performs a
    /// clean-tree preflight and stops the owning session; the returned job
    /// does the slow part. Errors here leave the session untouched.
    pub(super) fn begin_worktree_removal(
        &mut self,
        path: &Path,
        force: bool,
        repo_hint: Option<&Path>,
    ) -> Result<WorktreeRemoval, String> {
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if self.worktree_removals.contains_key(&key) {
            return Err(format!("worktree {} is already being removed", path.display()));
        }
        // Determine branch for this worktree (for delete / preview)
        let branch = kumo_core::worktrees::list_worktrees(path)
            .ok()
            .and_then(|list| list.into_iter().find(|w| {
                let canon = std::fs::canonicalize(&w.path).ok();
                canon.as_ref().map(|c| c == &key).unwrap_or(w.path == path)
            }).and_then(|w| w.branch))
            .or_else(|| kumo_core::worktree_meta::get(path).and_then(|m| m.branch))
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()));
        // Find the main worktree's path (common repo root) — `repo_root` on a linked worktree returns its own top, not main
        let repo_root = repo_hint.and_then(kumo_core::worktrees::main_worktree_path)
            .or_else(|| kumo_core::worktrees::main_worktree_path(path))
            .or_else(|| self.sessions.iter().find_map(|s| kumo_core::worktrees::main_worktree_path(&s.workspace)))
            .or_else(|| repo_hint.and_then(kumo_core::worktrees::repo_root))
            .or_else(|| kumo_core::worktrees::repo_root(path));
        let Some(repo_root) = repo_root else { return Err("not a git repository".to_string()); };
        if kumo_core::worktrees::is_main_worktree(&repo_root, path)? {
            return Err(format!("refusing to remove the main worktree {}", path.display()));
        }

        // The safe action must be non-destructive: reject dirty or untracked
        // work before touching the session or any PTY process.
        if !force {
            let dirty = kumo_core::worktrees::dirty_worktree_entries(path)?;
            if !dirty.is_empty() {
                return Err(format!(
                    "worktree has local changes; refusing removal ({}): {}",
                    dirty.len(),
                    dirty.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }

        // Keep the session identity because canonicalization stops working
        // once Git removes the directory. A post-teardown failure may recover
        // the session.
        let session_idx = self.session_for_workspace(path);
        let recovery = session_idx
            .and_then(|idx| self.sessions.get(idx))
            .map(|session| (session.name.clone(), session.workspace.clone()));
        let pty_workers = session_idx.map(|idx| self.close_session_with_workers(idx)).unwrap_or_default();
        Ok(WorktreeRemoval { path: path.to_path_buf(), key, repo_root, branch, force, pty_workers, recovery })
    }

    /// Run `removal` on a worker thread; its outcome arrives on
    /// `worktree_removal_rx` tagged with `client_id`.
    pub(super) fn spawn_worktree_removal(&mut self, client_id: usize, removal: WorktreeRemoval) {
        let key = removal.key.clone();
        let recovery = removal.recovery.clone();
        let tx = self.worktree_removal_tx.clone();
        let spawned = std::thread::Builder::new()
            .name("kumo-worktree-rm".into())
            .spawn(move || {
                let _ = tx.send((client_id, removal.run()));
            });
        match spawned {
            Ok(worker) => {
                self.worktree_removals.insert(key, worker);
            }
            Err(error) => {
                // The session is already closed: report the failure through the
                // normal path so it gets recovered.
                let result = Err(format!("could not start worktree removal: {error}"));
                let _ = self.worktree_removal_tx.send((client_id, WorktreeRemovalOutcome { key, recovery, result }));
            }
        }
    }

    /// Apply a finished removal: forget it and, on failure, reopen the session
    /// it closed.
    pub(super) fn finish_worktree_removal(&mut self, outcome: WorktreeRemovalOutcome) -> Result<String, String> {
        self.worktree_removals.remove(&outcome.key);
        outcome.result.map_err(|error| self.recover_after_teardown(outcome.recovery, error))
    }

    /// Block until every in-flight removal has finished, so stopping or
    /// re-executing the daemon never cuts a deletion short. The outcomes stay
    /// queued on `worktree_removal_rx`.
    pub(super) fn join_worktree_removals(&mut self) {
        for (_, worker) in self.worktree_removals.drain() {
            let _ = worker.join();
        }
    }
}
