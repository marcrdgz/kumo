//! Agent lifecycle-state detection, one module per supported AI CLI.
//!
//! Each agent module implements three predicates over a [`Snapshot`] of the
//! pane's live terminal buffer:
//!
//! - `blocked(&Snapshot) -> bool` — the agent is waiting on a command approval
//! - `working(&Snapshot) -> bool` — the agent is actively producing output
//! - `idle(&Snapshot) -> bool` — the agent is conclusively idle (e.g. its
//!   dedicated idle prompt box)
//!
//! `detect` dispatches across every implemented agent: a blocked signal wins
//! over working, working wins over idle, and `Unknown` is the fallback when
//! no signal matches — a recognized agent whose classification failed (see
//! also `agent-detection/<agent>.toml`), where the same split applies.

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
    /// Finished-but-unseen: the agent went idle while its pane was not
    /// focused. Held by the daemon until the pane is focused, which marks it
    /// seen (back to Idle).
    Done,
    /// A recognized agent whose classification failed: no blocked, working,
    /// or idle signal matched its screen.
    Unknown,
}

/// A text snapshot of an AI pane's terminal, captured from the screen buffer
/// (not the viewport) so detection ignores whatever the user is scrolling.
pub struct Snapshot {
    /// Recent screen-buffer tail (`bottom_text(DETECTION_TAIL_LINES)`).
    pub(crate) screen: String,
    /// Text below the last horizontal rule: the live prompt/forms region
    /// where Claude Code renders its approval dialogs.
    pub(crate) form: String,
    /// Bottom rows pinned to opencode's prompt footer
    /// (`bottom_text(DETECTION_FOOTER_LINES)`).
    pub(crate) footer: String,
    /// The OSC window title (Claude Code's status spinner).
    pub(crate) title: String,
}

impl Snapshot {
    /// Capture the current agent-state snapshot from a terminal.
    pub fn capture(vt: &vt::Terminal) -> Snapshot {
        let screen = vt.bottom_text(DETECTION_TAIL_LINES);
        let form = after_last_rule(&screen);
        let footer = vt.bottom_text(DETECTION_FOOTER_LINES);
        Snapshot {
            screen,
            form,
            footer,
            title: vt.title(),
        }
    }
}

/// ASCII case-insensitive `contains` (markers are ASCII lowercase).
pub(crate) fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let n = needle.as_bytes();
    let h = haystack.as_bytes();
    if n.len() > h.len() {
        return false;
    }
    // Sliding window with ascii lowercasing on the fly.
    for i in 0..=h.len() - n.len() {
        let mut ok = true;
        for j in 0..n.len() {
            let a = h[i + j].to_ascii_lowercase();
            let b = n[j];
            if a != b {
                ok = false;
                break;
            }
            // For ASCII markers, ensure we don't split a multi-byte char in
            // haystack mid-codepoint: if haystack byte is part of UTF-8
            // continuation, its ascii lowercased value is the same byte, but
            // the alignment may be off. This is fine for ASCII needle scans
            // as they will simply not match inside a multi-byte sequence in a
            // way that affects correctness (false negatives are rare and markers
            // are outside CJK/emoji runs).
        }
        if ok {
            // Verify the match is on char boundaries for haystack (optional,
            // but prevents matching inside a multi-byte char's bytes that happen
            // to equal ascii). For ASCII needle, the bytes must be ascii, so
            // the haystack bytes must be ascii as well.
            return true;
        }
    }
    false
}

pub(crate) fn ends_with_ci(haystack: &str, needle: &str) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    let start = h.len() - n.len();
    for i in 0..n.len() {
        if h[start + i].to_ascii_lowercase() != n[i] {
            return false;
        }
    }
    true
}

/// Detect the agent lifecycle state across every implemented agent. A blocked
/// signal wins over working; working wins over explicit idle; `Unknown` is
/// the fallback when no signal matches.
pub fn detect(snap: &Snapshot) -> AgentStatus {
    if opencode::blocked(snap) || claude::blocked(snap) {
        return AgentStatus::Blocked;
    }
    if opencode::working(snap) || claude::working(snap) {
        return AgentStatus::Working;
    }
    if opencode::idle(snap) || claude::idle(snap) {
        return AgentStatus::Idle;
    }
    AgentStatus::Unknown
}

/// Text of `screen` below its last horizontal rule (a run of box-drawing
/// dashes), where Claude Code renders the live prompt and approval forms.
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
    t.chars().count() >= 3
        && t.chars()
            .all(|c| matches!(c, '\u{2500}' | '\u{2501}' | '\u{254c}' | '\u{2014}'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(screen: &str, footer: &str, title: &str) -> Snapshot {
        let form = after_last_rule(screen);
        Snapshot {
            screen: screen.to_string(),
            form,
            footer: footer.to_string(),
            title: title.to_string(),
        }
    }

    #[test]
    fn blocked_wins_over_working_markers() {
        // A working footer hint alongside an approval dialog must read Blocked.
        let s = snap(
            "△ Permission required\nAllow once\nAllow always\nesc interrupt",
            "esc interrupt",
            "",
        );
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
    fn idle_when_any_agent_reports_an_idle_marker() {
        // opencode's prompt box (ask anything) is an explicit idle marker.
        let s = snap("opencode 1.18.15\nAsk anything... \"\"\nesc dismiss", "", "");
        assert_eq!(detect(&s), AgentStatus::Idle);
    }

    #[test]
    fn unknown_is_the_fallback() {
        // No blocked/working/idle marker matches: classification failed.
        let s = snap("opencode 1.18.15\n~/.opencode\n", "", "");
        assert_eq!(detect(&s), AgentStatus::Unknown);
    }

    #[test]
    fn after_last_rule_returns_prompt_region() {
        let screen = "transcript line\n───────\nDo you want to proceed?\n  1. yes\n  2. no\n  esc to cancel\n";
        assert_eq!(
            after_last_rule(screen),
            "Do you want to proceed?\n  1. yes\n  2. no\n  esc to cancel"
        );
    }

    #[test]
    fn after_last_rule_returns_whole_screen_without_a_rule() {
        let screen = "just a transcript\nwith two lines\n";
        assert_eq!(after_last_rule(screen), "just a transcript\nwith two lines");
    }
}
