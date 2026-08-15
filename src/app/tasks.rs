use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::alert::{self, AlertKind};
use crate::agents::AgentStatus;

use super::App;

/// How often the sidebar re-reads the git branch of each session's workspace.
const BRANCH_REFRESH: Duration = Duration::from_secs(3);
/// How often to re-scan the focused pane's cwd for follow-workspace.
const FOLLOW_INTERVAL: Duration = Duration::from_secs(1);
/// How often to re-scan pane process trees for an AI CLI (opencode/claude).
const AI_SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// How often to recompute agent status from the terminal buffer even when the
/// pane has produced no new output (so a finished agent returns to Idle).
const STATUS_REFRESH: Duration = Duration::from_millis(500);
/// Minimum gap between audible alerts for the same pane, so a status that
/// flickers between Working and Blocked does not repeat the sound.
const ALERT_COOLDOWN: Duration = Duration::from_secs(3);

impl App {
    /// Refresh cached git branches for all session workspaces (every
    /// `BRANCH_REFRESH`). Runs `git` off the hot frame path (once per frame).
    pub(super) fn refresh_branches(&mut self) {
        let now = Instant::now();
        let live: Vec<PathBuf> = self.sessions.iter().map(|s| s.workspace.clone()).collect();
        for ws in &live {
            let stale = match self.branch_cache.get(ws) {
                Some((_, t)) => now.duration_since(*t) >= BRANCH_REFRESH,
                None => true,
            };
            if stale {
                let branch = git_branch(ws);
                self.branch_cache.insert(ws.clone(), (branch, now));
            }
        }
        self.branch_cache.retain(|ws, _| live.contains(ws));
    }

    /// Cached git branch for a session's workspace.
    pub(super) fn session_branch(&self, idx: usize) -> Option<BranchInfo> {
        let ws = &self.sessions[idx].workspace;
        self.branch_cache.get(ws).and_then(|(b, _)| b.clone())
    }

    /// Follow the workspace to the focused pane's actual cwd (`[terminal]
    /// new-cwd = "follow"`, the default), at most every `FOLLOW_INTERVAL`.
    /// The session workspace drives new splits, the sidebar, and the git
    /// branch, so updating it here makes all of them follow. Only real local
    /// directories are adopted (a remote `ssh` pane's OSC 7 path is not), so
    /// new panes never spawn into a nonexistent cwd.
    pub(super) fn refresh_workspace_follow(&mut self) {
        if !matches!(crate::config::new_cwd(), crate::config::NewCwd::Follow) {
            return;
        }
        if self.last_follow_scan.elapsed() < FOLLOW_INTERVAL {
            return;
        }
        self.last_follow_scan = Instant::now();
        let focus = self.sessions[self.active].tree.focus;
        let cwd = {
            let Some(pane) = self.panes.get_mut(&focus) else { return };
            pane.detected_cwd()
        };
        let Some(cwd) = cwd else { return };
        if !cwd.is_dir() {
            return;
        }
        let session = &mut self.sessions[self.active];
        if session.workspace != cwd {
            session.workspace = cwd.clone();
        }
        self.workspace = cwd;
    }

    /// Mark plain shell panes as AI CLI panes when opencode/claude is running
    /// inside them, and clear the flag once the process exits. Runs at most
    /// every `AI_SCAN_INTERVAL`.
    pub(super) fn refresh_ai_cli(&mut self) {
        if self.last_ai_scan.elapsed() < AI_SCAN_INTERVAL {
            return;
        }
        self.last_ai_scan = Instant::now();
        for pane in self.panes.values_mut() {
            let name = pane.ai_cli_name();
            pane.detected_ai_name = name.clone();
            if !pane.is_ai {
                pane.detected_ai = name.is_some();
            }
        }
    }

    /// Recomputed agent status from the terminal buffer at most every
    /// `STATUS_REFRESH`, independent of pane dirty state. `render_dirty` only
    /// refreshes the status when the pane produces output or scrolls, so a
    /// quiet agent that just finished would otherwise stay stuck on the last
    /// Working status forever.
    ///
    /// Also raises an audible alert on the transitions the user cares about
    /// (Working -> Blocked, Working -> Idle), brings blocked agents into view
    /// by scrolling the AGENTS section to its top, and keeps the sidebar's
    /// blocked-first ordering consistent.
    pub(super) fn refresh_agent_statuses(&mut self) {
        if self.last_status_refresh.elapsed() < STATUS_REFRESH {
            return;
        }
        self.last_status_refresh = Instant::now();
        let now = Instant::now();
        let sound_enabled = crate::config::agent_sound_enabled();
        for (&pid, pane) in self.panes.iter_mut() {
            if !pane.is_ai_cli() {
                continue;
            }
            let status = pane.agent_status();
            if self.agent_status_cache.get(&pid) != Some(&status) {
                self.agent_status_cache.insert(pid, status);
            }
            let old = self.last_agent_status.get(&pid).copied();
            if let Some(kind) = old.and_then(|old| should_alert(old, status)) {
                let cooled = self
                    .last_agent_sound
                    .get(&pid)
                    .map(|t| now.duration_since(*t) >= ALERT_COOLDOWN)
                    .unwrap_or(true);
                if sound_enabled && cooled {
                    alert::play(kind);
                    self.last_agent_sound.insert(pid, now);
                }
            }
            self.last_agent_status.insert(pid, status);
        }
    }

    /// Append the per-pane agent status, output age, and detected CLI to
    /// `/tmp/kumo_agent.log` (throttled to 1/s, capped at 512 KiB). Gated
    /// behind `DEBUG_AGENT=1` so it is inert in production but stays in the
    /// codebase for diagnostics.
    #[allow(dead_code)]
    pub(super) fn log_agent_statuses(&mut self) {
        if std::env::var("DEBUG_AGENT").is_err() {
            return;
        }
        if self.last_agent_debug.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_agent_debug = Instant::now();
        use std::io::Write;
        const PATH: &str = "/tmp/kumo_agent.log";
        if std::fs::metadata(PATH).map(|m| m.len()).unwrap_or(0) > 512 * 1024 {
            let _ = std::fs::write(PATH, b"");
        }
        if let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(PATH) {
            for (pid, pane) in self.panes.iter() {
                if !pane.is_ai_cli() {
                    continue;
                }
                let tail = pane.recent_text_tail(200).replace('\n', "\\n");
                let _ = writeln!(
                    log,
                    "pid={} cli={} status={:?} age_ms={} recent={}",
                    pid,
                    pane.detected_ai_name.as_deref().unwrap_or("?"),
                    pane.agent_status(),
                    pane.last_output_age().as_millis(),
                    tail,
                );
            }
        }
    }
}

/// Current git branch of a session's workspace, plus how far it is from its
/// upstream: `ahead` commits not yet pushed, `behind` commits not yet pulled.
#[derive(Debug, Clone, Default)]
pub(super) struct BranchInfo {
    pub(super) name: String,
    pub(super) ahead: u32,
    pub(super) behind: u32,
}

/// Current git branch and ahead/behind counts of `ws`, if it is a git
/// repository on a named branch.
fn git_branch(ws: &std::path::Path) -> Option<BranchInfo> {
    let out = std::process::Command::new("git")
        .args(["-C", ws.to_str().unwrap_or_default(), "status", "--porcelain=v2", "--branch"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut name = None;
    let mut ahead = 0u32;
    let mut behind = 0u32;
    for line in stdout.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            let head = head.trim();
            if head.is_empty() || head == "(detached)" {
                return None;
            }
            name = Some(head.to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut parts = ab.split_whitespace();
            if let Some(rest) = parts.next() {
                if let Some(n) = rest.strip_prefix('+') {
                    ahead = n.parse().unwrap_or(0);
                }
            }
            if let Some(rest) = parts.next() {
                if let Some(n) = rest.strip_prefix('-') {
                    behind = n.parse().unwrap_or(0);
                }
            }
        }
    }
    name.map(|name| BranchInfo { name, ahead, behind })
}

/// The alert a status transition deserves, if any. A Working agent that goes
/// Blocked is waiting for an approval; one that falls back to Idle finished
/// its task. All other transitions (including the very first observation,
/// when `old` is absent) stay silent.
fn should_alert(old: AgentStatus, new: AgentStatus) -> Option<AlertKind> {
    match (old, new) {
        (AgentStatus::Working, AgentStatus::Blocked) => Some(AlertKind::Blocked),
        (AgentStatus::Working, AgentStatus::Idle) => Some(AlertKind::Finished),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Make a temp git repo on branch `name` with a bare `origin` remote.
    /// Returns the working tree path. The upstream points at `origin/<name>`.
    fn temp_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kumo_git_test_{}_{}",
            name.replace('/', "_"),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let remote = dir.join("remote");
        std::fs::create_dir_all(&remote).unwrap();
        let run = |args: &[&str]| {
            assert!(Command::new("git").args(args).status().unwrap().success(), "{args:?}");
        };
        run(&["init", "-q", "--bare", remote.to_str().unwrap()]);
        run(&["init", "-q", "-b", name, dir.to_str().unwrap()]);
        run(&["-C", dir.to_str().unwrap(), "config", "user.email", "t@t"]);
        run(&["-C", dir.to_str().unwrap(), "config", "user.name", "t"]);
        let commit = || {
            run(&["-C", dir.to_str().unwrap(), "commit", "-q", "--allow-empty", "-m", "x"]);
        };
        commit();
        run(&["-C", dir.to_str().unwrap(), "remote", "add", "origin", remote.to_str().unwrap()]);
        run(&["-C", dir.to_str().unwrap(), "push", "-q", "-u", "origin", name]);
        commit();
        commit();
        dir
    }

    #[test]
    fn git_branch_reports_name() {
        let dir = temp_repo("fix/domain");
        let info = git_branch(&dir).unwrap();
        assert_eq!(info.name, "fix/domain");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_branch_reports_ahead_behind() {
        let dir = temp_repo("main");
        let info = git_branch(&dir).unwrap();
        assert_eq!(info.ahead, 2, "two unpushed commits");
        assert_eq!(info.behind, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_branch_none_outside_repo() {
        assert!(git_branch(std::path::Path::new("/nonexistent")).is_none());
    }

    #[test]
    fn alerts_blocked_when_working_agent_blocks() {
        assert_eq!(
            should_alert(AgentStatus::Working, AgentStatus::Blocked),
            Some(AlertKind::Blocked)
        );
    }

    #[test]
    fn alerts_finished_when_working_agent_goes_idle() {
        assert_eq!(
            should_alert(AgentStatus::Working, AgentStatus::Idle),
            Some(AlertKind::Finished)
        );
    }

    #[test]
    fn silent_on_other_transitions() {
        assert_eq!(should_alert(AgentStatus::Idle, AgentStatus::Working), None);
        assert_eq!(should_alert(AgentStatus::Blocked, AgentStatus::Working), None);
        assert_eq!(should_alert(AgentStatus::Blocked, AgentStatus::Idle), None);
        assert_eq!(should_alert(AgentStatus::Idle, AgentStatus::Idle), None);
        assert_eq!(should_alert(AgentStatus::Working, AgentStatus::Working), None);
    }
}
