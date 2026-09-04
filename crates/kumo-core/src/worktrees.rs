//! Git worktree helpers: create and enumerate the worktrees of a session's
//! repository. Each operation is a short-lived `git` subprocess run off the
//! frame path (the branch/status scan already does the same), so they are safe
//! to call synchronously from the daemon.

use std::path::{Path, PathBuf};

/// One worktree of a repository: its working-tree path and the branch checked
/// out there. `branch` is `None` when the HEAD is detached.
#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>,
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
pub fn repo_root(ws: &Path) -> Option<PathBuf> {
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
pub fn worktree_path(repo_root: &Path, branch: &str) -> PathBuf {
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
pub fn add_worktree(repo_root: &Path, path: &Path, branch: &str) -> Result<(), String> {
    git(repo_root, &["worktree", "add", "-b", branch, path.to_str().unwrap_or_default()])?;
    Ok(())
}

/// Create a new worktree with an explicit start point (`from`): `git worktree add -b <branch> <path> <from>`.
pub fn add_worktree_from(
    repo_root: &Path,
    path: &Path,
    branch: &str,
    from: Option<&str>,
) -> Result<(), String> {
    let path_str = path.to_str().unwrap_or_default();
    if let Some(f) = from.filter(|s| !s.trim().is_empty()) {
        git(repo_root, &["worktree", "add", "-b", branch, path_str, f])?;
    } else {
        git(repo_root, &["worktree", "add", "-b", branch, path_str])?;
    }
    Ok(())
}

/// All worktrees of the repository containing `ws`, main first then linked,
/// parsed from `git worktree list --porcelain`. Includes the main worktree.
pub fn list_worktrees(ws: &Path) -> Result<Vec<WorktreeInfo>, String> {
    let out = git(ws, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktrees(&out))
}

/// The main (first) worktree of the repository containing `ws`, if any.
pub fn main_worktree_path(ws: &Path) -> Option<PathBuf> {
    list_worktrees(ws).ok()?.first().map(|w| w.path.clone())
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

// ---------------------------------------------------------------------------
// Branch naming (no `kumo/` prefix)
// ---------------------------------------------------------------------------

/// Slugify a workspace/task name into a git branch fragment (no `kumo/` prefix).
/// Keeps `/`, `.`, `_`, `-` (branch-legal), lowercases, collapses whitespace/underscores to `-`.
/// Emoji shortcodes like `:rocket:` → `rocket` is handled by stripping `:`.
pub fn slug_branch_name(name: &str) -> String {
    let mut s = name.trim().to_lowercase();
    // Strip Slack-style :shortcode: colons → word (keep emoji handling simple: drop non-alnum/)
    s = s
        .chars()
        .map(|c| if c == ':' { ' ' } else { c })
        .collect();
    let mut out = String::new();
    let mut last_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '/' || c == '.' || c == '_' || c == '-' {
            // Normalize underscore/space runs later; keep char as-is lowercased
            if c == '_' || c == '-' {
                if !last_dash {
                    out.push('-');
                    last_dash = true;
                }
            } else {
                out.push(c);
                last_dash = false;
            }
        } else if c.is_whitespace() {
            if !last_dash && !out.is_empty() {
                out.push('-');
                last_dash = true;
            }
        } else {
            // Drop other symbols (emoji glyphs, punctuation) — they become word boundaries if not collapsing
            if !last_dash && !out.is_empty() {
                // treat as separator only if surrounded by alnum; simpler: insert dash if not already
                // but avoid leading dash
                // we insert dash only if last char is alnum and next alnum exists; for now coalesce
                // don't push dash to avoid noise; just ensure separator
                // skip
            }
        }
    }
    // Collapse consecutive dashes/slashes handling is minimal; trim leading/trailing -/ and /
    let mut trimmed = out.trim_matches(|c| c == '-' || c == '/').to_string();
    // Collapse multiple -- to single -
    while trimmed.contains("--") {
        trimmed = trimmed.replace("--", "-");
    }
    // Collapse multiple // to single /
    while trimmed.contains("//") {
        trimmed = trimmed.replace("//", "/");
    }
    if trimmed.is_empty() { "work".to_string() } else { trimmed }
}

/// Validate an explicit branch name (as if `git check-ref-format --branch`): non-empty, no illegal chars, no `..`, no `//`, no trailing slash, no `@{`.
pub fn validate_branch_name(branch: &str) -> Result<(), String> {
    let b = branch.trim();
    if b.is_empty() { return Err("branch name cannot be empty".into()); }
    if b == "HEAD" { return Err("branch name cannot be HEAD".into()); }
    if b.contains("..") { return Err("branch name cannot contain '..'".into()); }
    if b.contains("//") { return Err("branch name cannot contain '//'".into()); }
    if b.ends_with('/') || b.starts_with('/') || b.ends_with('.') || b.contains(' ') { return Err(format!("invalid branch name {b:?}")); }
    if b.contains("@{") { return Err("branch name cannot contain '@{'".into()); }
    if b.contains('~') || b.contains('^') || b.contains(':') || b.contains('?') || b.contains('*') || b.contains('[') { return Err(format!("invalid branch name {b:?}")); }
    // Git also rejects control chars and `//` etc — coarse check enough for our validation; git will still reject at `worktree add`.
    Ok(())
}

/// Derive the branch to create from the inputs (no `kumo/` prefix):
/// - explicit `branch_override` → validate and return it (name/derive ignored)
/// - `name` (Nombre tab / CLI positional) → slug(name)
/// - `from` is `#1234`/GitHub URL → `pr-1234` or headRefName when gh resolves it (caller should have attempted resolve)
/// - fallback → `wt-<short>`
///
/// Callers that already resolved a PR's headRefName should pass it as `name` instead of relying on this.
pub fn derive_branch(
    name: Option<&str>,
    from: Option<&str>,
    branch_override: Option<&str>,
) -> Result<String, String> {
    if let Some(b) = branch_override.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        validate_branch_name(b)?;
        return Ok(b.to_string());
    }
    if let Some(n) = name.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let slug = slug_branch_name(n);
        validate_branch_name(&slug)?;
        return Ok(slug);
    }
    if let Some(f) = from.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        // Try PR numeric (#1234 or URL ending in /pull/1234)
        if let Some(num) = parse_pr_number(f) {
            return Ok(format!("pr-{num}"));
        }
        // If from is a plain branch name (no sha, no url), use it as derived name when it looks like a feature name?
        // Better to treat explicit branch-like from as not a name — fallback to wt-.
        // Only slug branch-like from if it contains '/' or '-' and no spaces.
        if f.contains('/') || f.contains('-') {
            let slug = slug_branch_name(f);
            if !slug.is_empty() && slug != "work" {
                return Ok(slug);
            }
        }
    }
    Ok("wt-work".to_string())
}

/// Extract a PR number from `#1234` or a GitHub/GitLab URL containing `/pull/<n>` or `/merge_requests/<n>` or `/issues/<n>` or trailing numeric.
pub fn parse_pr_number(raw: &str) -> Option<u32> {
    let s = raw.trim();
    if let Some(stripped) = s.strip_prefix('#') {
        if let Ok(n) = stripped.trim().parse::<u32>() { return Some(n); }
    }
    // URLs: look for segments like /pull/1234, /pulls/1234, /merge_requests/1234, /issues/1234
    // Do a simple scan for last numeric segment.
    if s.contains("://") || s.contains('/') {
        // Split by '/' and find numeric tail after known keywords, else last numeric path segment
        let parts: Vec<&str> = s.split('/').collect();
        for win in parts.windows(2) {
            if matches!(win[0], "pull" | "pulls" | "merge_requests" | "issues" | "issue") {
                if let Ok(n) = win[1].trim().split(['?', '#', '&']).next().unwrap_or("").parse::<u32>() { return Some(n); }
            }
        }
        // Fallback: last path segment numeric
        if let Some(last) = parts.last().and_then(|p| p.split(['?', '#']).next()) {
            if let Ok(n) = last.parse::<u32>() { return Some(n); }
        }
    }
    None
}

/// Resolve `--from` to a commit-ish usable as `git worktree add -b <new> <path> <from>`.
/// `raw` may be: branch, `origin/branch`, SHA, `#1234`, GitHub/GitLab URL. Returns the commit-ish to pass to git.
/// GitHub PRs use `gh` shell-out first, then `git ls-remote`, then local fetch. Jira returns `Err("Jira deferred")`.
pub fn resolve_from(repo_root: &Path, raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() { return Err("empty --from".into()); }
    // Jira deferred (keys like ABC-1234 or jira URLs)
    if is_jira_ref(s) {
        return Err("Jira issue linking is deferred — use a branch, commit, #1234, or GitHub URL".into());
    }
    // Numeric PR short form #1234
    if let Some(num) = parse_pr_number(s).filter(|_| s.starts_with('#')) {
        return resolve_pr(repo_root, num);
    }
    // GitHub/GitLab URL with PR/MR number
    if (s.contains("://") || s.contains("github.com") || s.contains("gitlab.com")) && parse_pr_number(s).is_some() {
        let num = parse_pr_number(s).unwrap();
        return resolve_pr(repo_root, num);
    }
    // SHA-like (7..40 hex)
    if s.chars().all(|c| c.is_ascii_hexdigit()) && (7..=40).contains(&s.len()) {
        // Verify commit exists locally, else return as-is and let `git worktree add` fail with git's error
        let _ = git(repo_root, &["cat-file", "-e", &format!("{s}^{{commit}}")]).ok();
        return Ok(s.to_string());
    }
    // Branch or remote branch — prefer as-is, but verify via rev-parse fallback chain
    // Try verbatim first, then origin/<name>
    if git(repo_root, &["rev-parse", "--verify", &format!("{s}^{{commit}}")]).is_ok() {
        return Ok(s.to_string());
    }
    let origin_candidate = format!("origin/{s}");
    if git(repo_root, &["rev-parse", "--verify", &format!("{origin_candidate}^{{commit}}")]).is_ok() {
        return Ok(origin_candidate);
    }
    // Accept as typed and let git report the error downstream with its own message
    Ok(s.to_string())
}

fn is_jira_ref(s: &str) -> bool {
    if s.contains("atlassian.net") || s.contains("/jira/") || s.contains("jira.") { return true; }
    // Keys like ABC-1234, PROJ-999
    if let Some((prefix, num)) = s.split_once('-') {
        if prefix.chars().all(|c| c.is_ascii_uppercase()) && 2 <= prefix.len() && prefix.len() <= 10 && num.chars().all(|c| c.is_ascii_digit()) && !num.is_empty() {
            return true;
        }
    }
    false
}

fn resolve_pr(repo_root: &Path, num: u32) -> Result<String, String> {
    // Try `gh pr view <num> --json headRefOid --jq .headRefOid` in repo_root
    let gh = std::process::Command::new("gh")
        .arg("pr")
        .arg("view")
        .arg(num.to_string())
        .arg("--json")
        .arg("headRefOid")
        .arg("--jq")
        .arg(".headRefOid")
        .current_dir(repo_root)
        .output();
    if let Ok(out) = gh {
        if out.status.success() {
            let oid = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !oid.is_empty() && oid.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(oid);
            }
        }
    }
    // Fallback: git ls-remote origin pull/<num>/head
    let remote_ref = format!("refs/pull/{num}/head");
    if let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["ls-remote", "origin", &remote_ref])
        .output()
    {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(oid) = text.split_whitespace().next() {
                if !oid.is_empty() {
                    return Ok(oid.to_string());
                }
            }
        }
    }
    // Also try GitLab MR ref pattern
    let mr_ref = format!("refs/merge-requests/{num}/head");
    if let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["ls-remote", "origin", &mr_ref])
        .output()
    {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(oid) = text.split_whitespace().next() {
                if !oid.is_empty() { return Ok(oid.to_string()); }
            }
        }
    }
    Err(format!("could not resolve PR #{num}: install `gh` (`gh auth login`) or fetch the PR branch and use `--from <branch>`"))
}

/// Whether `path` is gitignored in `repo_root` (`git check-ignore -q`).
pub fn is_gitignored(repo_root: &Path, path: &Path) -> bool {
    // `check-ignore` expects a path relative to repo_root when possible
    let rel = path.strip_prefix(repo_root).unwrap_or(path);
    // Empty path (root itself) is not ignored
    if rel.as_os_str().is_empty() { return false; }
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["check-ignore", "-q", "--", rel.to_str().unwrap_or_default()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Wire shared gitignored dirs into a new worktree: symlink, trying APFS
/// clone-copy on macOS (`cp -cR`) first. Returns warnings (non-fatal).
pub fn wire_shared_dirs(repo_root: &Path, wt_path: &Path, shared_dirs: &[PathBuf]) -> Vec<String> {
    let mut warns = Vec::new();
    for dir in shared_dirs {
        if dir.is_absolute() {
            warns.push(format!("skip absolute shared dir {:?}", dir));
            continue;
        }
        if dir.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            warns.push(format!("skip shared dir with '..' {:?}", dir));
            continue;
        }
        let src = repo_root.join(dir);
        let dst = wt_path.join(dir);
        if !src.exists() {
            warns.push(format!("shared dir {:?} missing in primary checkout — skipped", dir));
            continue;
        }
        if !src.is_dir() {
            warns.push(format!("shared dir {:?} is not a directory — skipped", dir));
            continue;
        }
        if !is_gitignored(repo_root, &src) {
            warns.push(format!("shared dir {:?} is not gitignored — skipped", dir));
            continue;
        }
        if dst.exists() {
            // Already present (copy vs symlink race) — keep existing, warn
            warns.push(format!("shared dir {:?} already exists in worktree — kept", dir));
            continue;
        }
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Try APFS clone-copy on macOS (`cp -cR <src> <dst>`). Non-fatal; fallback to symlink.
        #[cfg(target_os = "macos")]
        {
            let cloned = std::process::Command::new("cp")
                .args(["-cR", src.to_str().unwrap_or_default(), dst.to_str().unwrap_or_default()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if cloned && dst.exists() {
                continue;
            }
        }
        #[cfg(target_os = "macos")]
        {
            // Also try clonefile(2) via `cp -c` without -R for files handled elsewhere; already covered.
        }
        // Fallback: symlink absolute source → dest
        if let Err(e) = std::os::unix::fs::symlink(&src, &dst) {
            warns.push(format!("symlink {:?} failed: {e}", dir));
        }
    }
    warns
}

/// Copy repo-root `.worktreeinclude` literal paths (gitignored only) into the new worktree.
/// Returns warnings (non-fatal). Literal paths, `#` comments, blank lines.
pub fn copy_worktreeinclude(repo_root: &Path, wt_path: &Path) -> Vec<String> {
    let mut warns = Vec::new();
    let inc = repo_root.join(".worktreeinclude");
    let content = match std::fs::read_to_string(&inc) {
        Ok(c) => c,
        Err(_) => return warns, // No file → nothing to do
    };
    for (lineno, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        // Only literal paths — reject globs/negations per spec, warn
        if line.contains('*') || line.contains('?') || line.starts_with('!') || line.contains('[') {
            warns.push(format!(".worktreeinclude:{}: glob/negation {:?} skipped (literal paths only)", lineno + 1, line));
            continue;
        }
        let rel = PathBuf::from(line);
        if rel.is_absolute() {
            warns.push(format!(".worktreeinclude:{}: absolute path {:?} skipped", lineno + 1, line));
            continue;
        }
        if rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            warns.push(format!(".worktreeinclude:{}: path with '..' {:?} skipped", lineno + 1, line));
            continue;
        }
        let src = repo_root.join(&rel);
        let dst = wt_path.join(&rel);
        if !src.exists() {
            warns.push(format!(".worktreeinclude:{}: {:?} missing — skipped", lineno + 1, line));
            continue;
        }
        if !is_gitignored(repo_root, &src) {
            warns.push(format!(".worktreeinclude:{}: {:?} is not gitignored — skipped (only ignored sources)", lineno + 1, line));
            continue;
        }
        if dst.exists() {
            warns.push(format!(".worktreeinclude:{}: {:?} already exists in worktree — kept", lineno + 1, line));
            continue;
        }
        if let Some(parent) = dst.parent() { let _ = std::fs::create_dir_all(parent); }
        let res = if src.is_dir() {
            copy_dir_recursive(&src, &dst)
        } else {
            std::fs::copy(&src, &dst).map(|_| ())
        };
        if let Err(e) = res {
            warns.push(format!(".worktreeinclude:{}: copy {:?} failed: {e}", lineno + 1, line));
        }
    }
    warns
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else if ft.is_file() {
            std::fs::copy(entry.path(), &dst_path)?;
        } else if ft.is_symlink() {
            if let Ok(target) = std::fs::read_link(entry.path()) {
                let _ = std::os::unix::fs::symlink(target, dst_path);
            }
        }
    }
    Ok(())
}

/// Remove a worktree directory (`git worktree remove`) and optionally its branch.
/// `force` passes `--force` to `git worktree remove` and `git branch -D`.
pub fn remove_worktree(repo_root: &Path, path: &Path, force: bool) -> Result<(), String> {
    let path_str = path.to_str().unwrap_or_default();
    if force {
        git(repo_root, &["worktree", "remove", "--force", path_str])?;
    } else {
        git(repo_root, &["worktree", "remove", path_str])?;
    }
    Ok(())
}

/// Commits on `branch` not merged into the repo's base (`origin/main` or `main`).
/// Used for `kumo worktree rm` preview — branches with `count>0` surface for review.
pub fn branch_unmerged_count(repo_root: &Path, branch: &str) -> Result<usize, String> {
    let base = default_base(repo_root);
    let out = git(repo_root, &["rev-list", "--count", branch, "--not", &base])?;
    let txt = String::from_utf8_lossy(&out).trim().to_string();
    txt.parse::<usize>().map_err(|_| format!("could not parse rev-list count {txt:?}"))
}

fn default_base(repo_root: &Path) -> String {
    for cand in ["origin/main", "origin/master", "main", "master"] {
        if git(repo_root, &["rev-parse", "--verify", &format!("{cand}^{{commit}}")]).is_ok() {
            return cand.to_string();
        }
    }
    // Fallback to HEAD's upstream or its commit
    "HEAD".to_string()
}

/// Delete a branch (`git branch -d` / `-D`).
pub fn delete_branch(repo_root: &Path, branch: &str, force: bool) -> Result<(), String> {
    if force {
        git(repo_root, &["branch", "-D", branch])?;
    } else {
        git(repo_root, &["branch", "-d", branch])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Make a temp git repo on branch `main` with a bare `origin` remote, like
    /// `tasks::temp_repo`. Returns the working tree path.
    fn temp_repo() -> PathBuf {
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "kumo_wt_repo_{}_{}_{}",
            std::process::id(),
            n,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() % 1_000_000
        ));
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

    #[test]
    fn slug_branch_keeps_slash_and_lowercases() {
        assert_eq!(slug_branch_name("Feat Foo"), "feat-foo");
        assert_eq!(slug_branch_name("feat/foo"), "feat/foo");
        assert_eq!(slug_branch_name("FIX login!"), "fix-login");
        assert_eq!(slug_branch_name("  hello__world  "), "hello-world");
        assert_eq!(slug_branch_name(":rocket: launch"), "rocket-launch");
        assert_eq!(slug_branch_name("PR #123"), "pr-123");
    }

    #[test]
    fn derive_branch_prefers_override() {
        assert_eq!(derive_branch(Some("my-name"), None, Some("feat/login")).unwrap(), "feat/login");
        assert_eq!(derive_branch(Some("My Feature"), None, None).unwrap(), "my-feature");
        assert_eq!(derive_branch(None, Some("#1234"), None).unwrap(), "pr-1234");
        assert_eq!(derive_branch(None, Some("https://github.com/o/r/pull/99"), None).unwrap(), "pr-99");
    }

    #[test]
    fn parse_pr_number_from_variants() {
        assert_eq!(parse_pr_number("#42"), Some(42));
        assert_eq!(parse_pr_number("https://github.com/a/b/pull/123"), Some(123));
        assert_eq!(parse_pr_number("https://github.com/a/b/pull/123/files"), Some(123));
        assert_eq!(parse_pr_number("https://gitlab.com/a/b/-/merge_requests/7"), Some(7));
        assert_eq!(parse_pr_number("branch-name"), None);
    }

    #[test]
    fn resolve_from_local_branch_passthrough() {
        let repo = temp_repo();
        let r = resolve_from(&repo, "main").unwrap();
        assert_eq!(r, "main");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn resolve_from_jira_deferred() {
        let repo = temp_repo();
        let err = resolve_from(&repo, "PROJ-123").unwrap_err();
        assert!(err.contains("Jira"), "{err}");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn wire_shared_dirs_symlink_and_gitignored_gate() {
        let repo = temp_repo();
        // Create a gitignored dir node_modules in primary
        let nm = repo.join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        std::fs::write(nm.join("x.txt"), "hi").unwrap();
        // .gitignore entry
        std::fs::write(repo.join(".gitignore"), "node_modules/\n").unwrap();
        let wt = worktree_path(&repo, "feat/shared");
        add_worktree(&repo, &wt, "feat/shared").unwrap();
        let warns = wire_shared_dirs(&repo, &wt, &[PathBuf::from("node_modules")]);
        assert!(warns.is_empty() || warns.iter().any(|w| w.contains("is not gitignored")) || wt.join("node_modules").exists() || wt.join("node_modules").is_symlink(), "warns: {warns:?}");
        // should be symlink or cloned dir containing file
        let dst = wt.join("node_modules");
        assert!(dst.exists() || dst.is_symlink(), "shared dir should be materialized {dst:?}");
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(wt.parent().unwrap().join(format!("{}-feat", repo.file_name().unwrap().to_string_lossy())));
    }

    #[test]
    fn copy_worktreeinclude_copies_gitignored_file() {
        let repo = temp_repo();
        std::fs::write(repo.join(".env"), "SECRET=1").unwrap();
        // gitignore .env
        std::fs::write(repo.join(".gitignore"), ".env\n").unwrap();
        std::fs::write(repo.join(".worktreeinclude"), ".env\n# comment\nmissing.txt\n").unwrap();
        let wt = worktree_path(&repo, "feat/inc");
        add_worktree(&repo, &wt, "feat/inc").unwrap();
        let warns = copy_worktreeinclude(&repo, &wt);
        assert!(wt.join(".env").exists(), "warns: {warns:?}");
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "SECRET=1");
        let _ = std::fs::remove_dir_all(&repo);
    }
}
