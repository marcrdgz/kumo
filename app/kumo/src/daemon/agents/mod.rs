//! Agent lifecycle-state detection, one module per supported AI CLI.
//!
//! Each agent module implements two predicates over a [`Snapshot`] of the
//! pane's live terminal buffer:
//!
//! - `blocked(&Snapshot) -> bool` — the agent is waiting on a command approval
//! - `working(&Snapshot) -> bool` — the agent is actively producing output
//!
//! `detect` dispatches across every implemented agent: a blocked signal wins
//! over working, and idle is the fallback when no marker matches. The rules
//! mirror herdr's per-agent detection manifests (`~/.config/herdr` /
//! `agent-detection/<agent>.toml`), where the same split between blocked,
//! working, and idle fallback applies.

pub mod claude;
pub mod opencode;

use crate::daemon::vt;

/// How many rows from the bottom of the terminal buffer to scan for
/// agent-state markers. The live prompt/footer and any dialog live in the
/// last screenful, while older transcript rows are excluded.
const DETECTION_TAIL_LINES: usize = 200;
/// How many bottom rows hold opencode's prompt footer ("esc interrupt",
/// spinner, progress bar). This area is pinned to the buffer tail and never
/// scrolls with the transcript, so its signals reflect the live agent state.
const DETECTION_FOOTER_LINES: usize = 8;

/// Lifecycle state of an AI agent, inferred from its output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentStatus {
    /// Actively producing output (working on a task).
    Working,
    /// Quiet but waiting for a command approval.
    Blocked,
    /// Quiet and idle.
    Idle,
}

/// A text snapshot of an AI pane's terminal, captured from the screen buffer
/// (not the viewport) so detection ignores whatever the user is scrolling.
pub struct Snapshot {
    /// Recent screen-buffer tail (`bottom_text(DETECTION_TAIL_LINES)`).
    screen: String,
    /// Lowercased `screen`, for case-insensitive marker scans.
    screen_lower: String,
    /// Text below the last horizontal rule: the live prompt/forms region
    /// where Claude Code renders its approval dialogs.
    form: String,
    /// Lowercased `form`.
    form_lower: String,
    /// Bottom rows pinned to opencode's prompt footer
    /// (`bottom_text(DETECTION_FOOTER_LINES)`).
    footer: String,
    /// Lowercased `footer`.
    footer_lower: String,
    /// The OSC window title (Claude Code's status spinner).
    title: String,
}

impl Snapshot {
    /// Capture the current agent-state snapshot from a terminal.
    pub fn capture(vt: &vt::Terminal) -> Snapshot {
        let screen = vt.bottom_text(DETECTION_TAIL_LINES);
        let form = after_last_rule(&screen);
        let footer = vt.bottom_text(DETECTION_FOOTER_LINES);
        Snapshot {
            screen_lower: screen.to_lowercase(),
            form_lower: form.to_lowercase(),
            footer_lower: footer.to_lowercase(),
            screen,
            form,
            footer,
            title: vt.title(),
        }
    }
}

/// Detect the agent lifecycle state across every implemented agent. A blocked
/// signal wins over working; idle is the fallback when no marker matches.
pub fn detect(snap: &Snapshot) -> AgentStatus {
    if opencode::blocked(snap) || claude::blocked(snap) {
        return AgentStatus::Blocked;
    }
    if opencode::working(snap) || claude::working(snap) {
        return AgentStatus::Working;
    }
    AgentStatus::Idle
}

/// Text of `screen` below its last horizontal rule (a run of box-drawing
/// dashes), where Claude Code renders the live prompt and approval forms.
/// Mirrors herdr's `after_last_horizontal_rule` region.
fn after_last_rule(screen: &str) -> String {
    let mut start = 0usize;
    for (i, line) in screen.lines().enumerate() {
        if is_hrule(line) {
            start = i + 1;
        }
    }
    screen.lines().skip(start).collect::<Vec<_>>().join("\n")
}

/// True when a line is a horizontal rule: at least three box-drawing dashes.
fn is_hrule(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 3 && t.chars().all(|c| matches!(c, '\u{2500}' | '\u{2501}' | '\u{254c}' | '\u{2014}'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(screen: &str, footer: &str, title: &str) -> Snapshot {
        let form = after_last_rule(screen);
        Snapshot {
            screen: screen.to_string(),
            screen_lower: screen.to_lowercase(),
            form: form.clone(),
            form_lower: form.to_lowercase(),
            footer: footer.to_string(),
            footer_lower: footer.to_lowercase(),
            title: title.to_string(),
        }
    }

    #[test]
    fn blocked_wins_over_working_markers() {
        // A working footer hint alongside an approval dialog must read Blocked.
        let s = snap("△ Permission required\nAllow once\nAllow always\nesc interrupt", "esc interrupt", "");
        assert_eq!(detect(&s), AgentStatus::Blocked);
    }

    #[test]
    fn working_when_any_agent_reports_it() {
        // Claude's OSC title spinner is enough, even with an opencode-like
        // empty screen.
        let s = snap("", "", "\u{280b} Fixing the bug");
        assert_eq!(detect(&s), AgentStatus::Working);
    }

    #[test]
    fn idle_is_the_fallback() {
        let s = snap("opencode 1.18.15\n~/.opencode\n", "", "\u{2733} ~/proj");
        assert_eq!(detect(&s), AgentStatus::Idle);
    }

    #[test]
    fn after_last_rule_returns_prompt_region() {
        let screen = "transcript line\n───────\nDo you want to proceed?\n  1. yes\n  2. no\n  esc to cancel\n";
        assert_eq!(after_last_rule(screen), "Do you want to proceed?\n  1. yes\n  2. no\n  esc to cancel");
    }

    #[test]
    fn after_last_rule_returns_whole_screen_without_a_rule() {
        let screen = "just a transcript\nwith two lines\n";
        assert_eq!(after_last_rule(screen), "just a transcript\nwith two lines");
    }
}
