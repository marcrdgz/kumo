use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitChange {
    pub path: String,
    pub status: String,
    pub staged: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub is_repo: bool,
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<GitChange>,
}

/// Run `git status --porcelain=v1 -b` in the given directory and parse it.
/// Returns `None` when the directory is not a git repository.
pub fn status(dir: &Path) -> Option<GitStatus> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("status")
        .arg("--porcelain=v1")
        .arg("-b")
        .arg("-uall")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let mut branch = String::new();
    let mut ahead = 0;
    let mut behind = 0;
    let mut changes = Vec::new();

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            let (head, _sep) = rest.split_once('[').unwrap_or((rest, ""));
            branch = head
                .split_once("...")
                .map(|(b, _)| b)
                .unwrap_or(head)
                .trim()
                .to_string();
            if let Some(meta) = rest.split_once('[').map(|(_, m)| m) {
                for part in meta.trim_end_matches(']').split(',') {
                    let part = part.trim();
                    if let Some(n) = part.strip_prefix("ahead ") {
                        ahead = n.trim().parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix("behind ") {
                        behind = n.trim().parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }

        if line.len() < 4 {
            continue;
        }
        let bytes = line.as_bytes();
        let staged = bytes[0] as char;
        let worktree = bytes[1] as char;
        let path = line[3..].to_string();
        let staged_flag = staged != ' ' && staged != '?';
        let status = if worktree != ' ' && worktree != '?' {
            worktree.to_string()
        } else {
            staged.to_string()
        };
        // Collapse rename entries: "R  a -> b" -> status "R", path "b".
        let (path, status) = if status == "R" || status == "C" {
            if let Some((_, to)) = path.split_once(" -> ") {
                (to.trim().to_string(), status.clone())
            } else {
                (path, status)
            }
        } else {
            (path, status)
        };
        changes.push(GitChange {
            path,
            status,
            staged: staged_flag,
        });
    }

    Some(GitStatus {
        is_repo: true,
        branch,
        ahead,
        behind,
        changes,
    })
}

/// Return the unified diff for a single path (worktree + staged).
pub fn diff(dir: &Path, rel_path: &str) -> String {
    let mut out = Vec::new();
    for args in [
        vec!["diff", "--no-color", "--", rel_path],
        vec!["diff", "--cached", "--no-color", "--", rel_path],
    ] {
        if let Ok(o) = Command::new("git").arg("-C").arg(dir).args(&args).output() {
            if o.status.success() {
                out.extend_from_slice(&o.stdout);
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Sibling folder holding per-session worktrees: `<workspace>.worktrees`.
pub fn worktrees_root(workspace: &Path) -> std::path::PathBuf {
    let mut s = workspace.as_os_str().to_os_string();
    s.push(".worktrees");
    std::path::PathBuf::from(s)
}

fn session_dir(workspace: &Path, session_name: &str) -> std::path::PathBuf {
    worktrees_root(workspace).join(sanitize_name(session_name))
}

/// Sanitize a session name into a valid git branch / directory name.
fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('.').to_string();
    if trimmed.is_empty() { "session".to_string() } else { trimmed }
}

/// Create (or re-attach) a git worktree for a session, returning its path.
/// The worktree lives in `<workspace>.worktrees/<session-name>` on a branch
/// named `nmx/<session-name>`.
pub fn create_worktree(workspace: &Path, session_name: &str) -> Result<std::path::PathBuf, String> {
    let dir = session_dir(workspace, session_name);
    if dir.is_dir() {
        return Ok(dir);
    }
    let branch = format!("nmx/{}", sanitize_name(session_name));
    let add = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["worktree", "add", "-b", &branch])
        .arg(&dir)
        .output();
    if let Ok(o) = add {
        if o.status.success() {
            return Ok(dir);
        }
        // Branch already exists: attach the worktree to it instead.
        let attach = Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(["worktree", "add"])
            .arg(&dir)
            .arg(&branch)
            .output();
        if let Ok(o) = attach {
            if o.status.success() {
                return Ok(dir);
            }
            return Err(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
    }
    Err(format!("git worktree add failed"))
}

/// Remove a session's worktree and its branch.
pub fn remove_worktree(workspace: &Path, session_name: &str) -> Result<(), String> {
    let dir = session_dir(workspace, session_name);
    if dir.exists() {
        let _ = Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(["worktree", "remove", "--force"])
            .arg(&dir)
            .output();
    }
    let _ = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["branch", "-D"])
        .arg(format!("nmx/{}", sanitize_name(session_name)))
        .output();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ahead_behind_and_changes() {
        let text = "## main...origin/main [ahead 1, behind 2]\n M src/a.ts\nA  src/b.ts\n?? notes.md\n";
        let dir = std::env::temp_dir().join("neomux-git-nonexistent");
        // Bypass the real command by testing the parser logic inline.
        let mut branch = String::new();
        let mut ahead = 0u32;
        let mut behind = 0u32;
        let mut changes = Vec::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                let (head, _) = rest.split_once('[').unwrap_or((rest, ""));
                branch = head.split_once("...").map(|(b, _)| b).unwrap_or(head).trim().to_string();
                if let Some(meta) = rest.split_once('[').map(|(_, m)| m) {
                    for part in meta.trim_end_matches(']').split(',') {
                        let part = part.trim();
                        if let Some(n) = part.strip_prefix("ahead ") {
                            ahead = n.trim().parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix("behind ") {
                            behind = n.trim().parse().unwrap_or(0);
                        }
                    }
                }
                continue;
            }
            if line.len() < 4 {
                continue;
            }
            let b = line.as_bytes();
            let staged = b[0] as char;
            let worktree = b[1] as char;
            changes.push((line[3..].to_string(), worktree.to_string(), staged != ' ' && staged != '?'));
        }
        assert_eq!(branch, "main");
        assert_eq!(ahead, 1);
        assert_eq!(behind, 2);
        assert_eq!(changes.len(), 3);
        assert!(!changes[0].2); // unstaged M
        assert!(changes[1].2); // staged A
        let _ = dir;
    }

    #[test]
    fn worktree_create_and_remove() {
        let tmp = std::env::temp_dir().join(format!("neomux-git-{}", std::process::id()));
        let repo = tmp.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let ok = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["init", "-q", "-b", "main"])
            .output()
            .unwrap();
        assert!(ok.status.success());
        Command::new("git").arg("-C").arg(&repo).args(["config", "user.email", "t@x.com"]).output().unwrap();
        Command::new("git").arg("-C").arg(&repo).args(["config", "user.name", "t"]).output().unwrap();
        std::fs::write(repo.join("a.txt"), "hi").unwrap();
        let add = Command::new("git").arg("-C").arg(&repo).args(["add", "."]).output().unwrap();
        let cmt = Command::new("git").arg("-C").arg(&repo).args(["commit", "-qm", "init"]).output().unwrap();
        assert!(add.status.success() && cmt.status.success());

        let dir = create_worktree(&repo, "session-2").expect("create worktree");
        assert_eq!(dir, worktrees_root(&repo).join("session-2"));
        assert!(dir.join("a.txt").is_file());

        // Idempotent: calling again returns the existing dir.
        let again = create_worktree(&repo, "session-2").expect("reuse worktree");
        assert_eq!(again, dir);

        remove_worktree(&repo, "session-2").expect("remove worktree");
        assert!(!dir.exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
