use std::path::Path;

use kumo_protocol::{ChipKind, WireChip};

const DIFF_CAP: usize = 8 * 1024;
const STATUS_CAP: usize = 4 * 1024;
const TRACEBACK_CAP: usize = 16 * 1024;

fn cap_text(s: String, cap: usize) -> (String, bool) {
    if s.len() <= cap {
        (s, false)
    } else {
        let mut truncated = s[..cap].to_string();
        truncated.push_str("\n…[truncated]");
        (truncated, true)
    }
}

fn git_output(workspace: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).into_owned();
    if s.trim().is_empty() {
        None
    } else {
        Some(s)
    }
}

pub fn collect_cwd_chip(cwd: &Path) -> WireChip {
    let text = cwd.display().to_string();
    WireChip {
        kind: ChipKind::Cwd,
        label: format!("cwd: {}", text),
        text: text.clone(),
        truncated: false,
    }
}

pub fn collect_traceback_chip(pane: &mut crate::daemon::pane::Pane) -> Option<WireChip> {
    let block = pane.vt.last_prompt_block()?;
    let prompt = block.prompt_text.trim();
    let output = block.output_text.trim();
    if output.is_empty() && prompt.is_empty() {
        return None;
    }
    let raw = if prompt.is_empty() {
        output.to_string()
    } else {
        format!("$ {prompt}\n{output}")
    };
    let (text, truncated) = cap_text(raw, TRACEBACK_CAP);
    Some(WireChip {
        kind: ChipKind::Traceback,
        label: if prompt.is_empty() {
            "traceback".to_string()
        } else {
            format!("traceback: {}", prompt.lines().next().unwrap_or("").chars().take(40).collect::<String>())
        },
        text,
        truncated,
    })
}

pub fn collect_git_chips(workspace: &Path) -> Vec<WireChip> {
    let mut out = Vec::new();
    // quick check if git repo
    if kumo_core::worktrees::repo_root(workspace).is_none() {
        return out;
    }
    // unstaged diff
    if let Some(diff) = git_output(workspace, &["diff", "--unified=1"]) {
        let (text, truncated) = cap_text(diff, DIFF_CAP);
        let label = if truncated {
            "git diff (unstaged, truncated)".to_string()
        } else {
            "git diff (unstaged)".to_string()
        };
        out.push(WireChip {
            kind: ChipKind::GitDiff,
            label,
            text,
            truncated,
        });
    }
    // staged diff
    if let Some(diff) = git_output(workspace, &["diff", "--cached", "--unified=1"]) {
        let (text, truncated) = cap_text(diff, DIFF_CAP);
        let label = if truncated {
            "git diff --cached (truncated)".to_string()
        } else {
            "git diff --cached".to_string()
        };
        out.push(WireChip {
            kind: ChipKind::GitDiff,
            label,
            text,
            truncated,
        });
    }
    // status (dirty files + untracked)
    if let Some(status) = git_output(workspace, &["status", "--porcelain=v2", "--branch"]) {
        // filter to lines not starting with "# branch"
        let lines: Vec<&str> = status
            .lines()
            .filter(|l| !l.starts_with("# branch"))
            .collect();
        if !lines.is_empty() {
            let raw = lines.join("\n");
            let (text, truncated) = cap_text(raw, STATUS_CAP);
            out.push(WireChip {
                kind: ChipKind::GitStatus,
                label: format!("git status: {} files", lines.len()),
                text,
                truncated,
            });
        }
    }
    out
}
