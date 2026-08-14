//! Git worktree helpers: create and enumerate the worktrees of a session's
//! repository. Each operation is a short-lived `git` subprocess run off the
//! frame path (the branch/status scan already does the same), so they are safe
//! to call synchronously from the daemon.

use std::path::{Path, PathBuf};

/// One worktree of a repository: its working-tree path and the branch checked
/// out there. `branch` is `None` when the HEAD is detached.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct WorktreeInfo {
    pub(super) path: PathBuf,
    pub(super) branch: Option<String>,
}

/// Run `git -C <ws> <args>`, returning stdout on success or the trimmed
/// stderr (or a synthesized message) on failure.
fn git(ws: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(ws)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("git {} failed", args.first().copied().unwrap_or(""))
        } else {
            err
        });
    }
    Ok(out.stdout)
}

/// The top-level working tree of the repository containing `ws`, if `ws` is
/// inside one. Works from the main checkout or a linked worktree alike.
pub(super) fn repo_root(ws: &Path) -> Option<PathBuf> {
    let out = git(ws, &["rev-parse", "--show-toplevel"]).ok()?;
    let root = String::from_utf8_lossy(&out).trim().to_string();
    if root.is_empty() {
        return None;
    }
    let root = PathBuf::from(root);
    root.is_dir().then_some(root)
}

/// Git's default sibling location for a new worktree: `<parent>/<basename>-<branch>`
/// (e.g. `~/dev/kumo` + `feat/foo` → `~/dev/kumo-feat/foo`).
pub(super) fn worktree_path(repo_root: &Path, branch: &str) -> PathBuf {
    let base = repo_root
        .file_name()
        .map(|b| b.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let parent = repo_root.parent().unwrap_or_else(|| Path::new("/"));
    parent.join(format!("{base}-{branch}"))
}

/// Create a new worktree checking out a fresh branch from the current HEAD:
/// `git worktree add -b <branch> <path>`. On failure the git error (stderr) is
/// returned for display.
pub(super) fn add_worktree(repo_root: &Path, path: &Path, branch: &str) -> Result<(), String> {
    git(repo_root, &["worktree", "add", "-b", branch, path.to_str().unwrap_or_default()])?;
    Ok(())
}

/// All worktrees of the repository containing `ws`, main first then linked,
/// parsed from `git worktree list --porcelain`. Includes the main worktree.
pub(super) fn list_worktrees(ws: &Path) -> Result<Vec<WorktreeInfo>, String> {
    let out = git(ws, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktrees(&out))
}

/// Parse `git worktree list --porcelain`: one blank-line-separated block per
/// worktree, `worktree <path>` plus an optional `branch refs/heads/<name>`
/// (absent for a detached HEAD). `bare`/`locked`/`prunable` markers are
/// ignored.
fn parse_worktrees(out: &[u8]) -> Vec<WorktreeInfo> {
    let text = String::from_utf8_lossy(out);
    let mut list = Vec::new();
    let mut cur: Option<(PathBuf, Option<String>)> = None;
    for line in text.lines() {
        if line.is_empty() {
            if let Some((path, branch)) = cur.take() {
                list.push(WorktreeInfo { path, branch });
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            cur = Some((PathBuf::from(rest.trim()), None));
        } else if let Some(rest) = line.strip_prefix("branch ") {
            if let Some((_, branch)) = cur.as_mut() {
                let name = rest.trim().strip_prefix("refs/heads/").unwrap_or(rest.trim());
                *branch = Some(name.to_string());
            }
        }
    }
    if let Some((path, branch)) = cur.take() {
        list.push(WorktreeInfo { path, branch });
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Make a temp git repo on branch `main` with a bare `origin` remote, like
    /// `tasks::temp_repo`. Returns the working tree path.
    fn temp_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kumo_wt_repo_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let remote = dir.join("remote");
        std::fs::create_dir_all(&remote).unwrap();
        let run = |args: &[&str]| {
            assert!(Command::new("git").args(args).status().unwrap().success(), "{args:?}");
        };
        run(&["init", "-q", "--bare", remote.to_str().unwrap()]);
        run(&["init", "-q", "-b", "main", dir.to_str().unwrap()]);
        run(&["-C", dir.to_str().unwrap(), "config", "user.email", "t@t"]);
        run(&["-C", dir.to_str().unwrap(), "config", "user.name", "t"]);
        run(&["-C", dir.to_str().unwrap(), "commit", "-q", "--allow-empty", "-m", "x"]);
        run(&["-C", dir.to_str().unwrap(), "remote", "add", "origin", remote.to_str().unwrap()]);
        run(&["-C", dir.to_str().unwrap(), "push", "-q", "-u", "origin", "main"]);
        dir
    }

    #[test]
    fn worktree_path_uses_git_sibling_naming() {
        let root = PathBuf::from("/home/user/dev/kumo");
        assert_eq!(
            worktree_path(&root, "feat/foo"),
            PathBuf::from("/home/user/dev/kumo-feat/foo"),
            "branch slash becomes a nested dir under the sibling prefix"
        );
        assert_eq!(
            worktree_path(&root, "fix-typo"),
            PathBuf::from("/home/user/dev/kumo-fix-typo")
        );
    }

    #[test]
    fn parse_worktrees_reads_porcelain_blocks() {
        let fixture = concat!(
            "worktree /work/kumo\n",
            "HEAD 4d5e6f\n",
            "branch refs/heads/main\n",
            "bare\n",
            "\n",
            "worktree /work/kumo-feat/foo\n",
            "HEAD 1a2b3c\n",
            "branch refs/heads/feat/foo\n",
            "\n",
            "worktree /work/kumo-detached\n",
            "HEAD deadbeef\n",
            "detached\n",
        );
        let list = parse_worktrees(fixture.as_bytes());
        assert_eq!(list.len(), 3);
        assert_eq!(
            list[0],
            WorktreeInfo { path: PathBuf::from("/work/kumo"), branch: Some("main".into()) }
        );
        assert_eq!(
            list[1],
            WorktreeInfo {
                path: PathBuf::from("/work/kumo-feat/foo"),
                branch: Some("feat/foo".into()),
            }
        );
        assert_eq!(
            list[2],
            WorktreeInfo { path: PathBuf::from("/work/kumo-detached"), branch: None }
        );
    }

    #[test]
    fn repo_root_outside_git_is_none() {
        let dir = std::env::temp_dir().join(format!("kumo_wt_nonrepo_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert!(repo_root(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_then_list_worktrees_round_trips() {
        let repo = temp_repo();
        // `rev-parse --show-toplevel` resolves symlinks (e.g. `/private` on
        // macOS), so compare against the canonicalized path.
        assert_eq!(
            repo_root(&repo),
            std::fs::canonicalize(&repo).ok(),
            "the repo root resolves"
        );

        let wt = worktree_path(&repo, "feat/wt");
        add_worktree(&repo, &wt, "feat/wt").expect("git worktree add succeeds");
        assert!(wt.is_dir(), "the worktree directory was created");

        // Listing from the linked worktree must see both trees.
        let list = list_worktrees(&wt).expect("list works from a linked worktree");
        let branches: Vec<Option<String>> =
            list.iter().map(|w| w.branch.clone()).collect();
        assert!(
            branches.contains(&Some("main".into())) && branches.contains(&Some("feat/wt".into())),
            "both the main and the new worktree are listed: {branches:?}"
        );

        // A duplicate branch name is rejected with the git error surfaced.
        let err = add_worktree(&repo, &worktree_path(&repo, "feat/wt2"), "feat/wt");
        assert!(err.is_err(), "creating an existing branch must fail");
        let err = err.unwrap_err();
        assert!(!err.trim().is_empty(), "the error carries git's stderr");
        let _ = std::fs::remove_dir_all(&repo);
    }
}
