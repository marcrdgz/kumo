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
#[derive(Debug)]
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

/// The detection precedence chain, in human-readable form. A blocked signal
/// wins over working; working wins over explicit idle; `Unknown` is the
/// fallback when no signal matches.
pub(crate) const PRECEDENCE: &str = "blocked > working > idle > unknown";

/// The evidence region a marker was found in. Each agent predicate is bound
/// to a specific region of the pane's terminal output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Region {
    /// Recent screen-buffer tail (`Snapshot::screen`).
    Screen,
    /// Text below the last horizontal rule (`Snapshot::form`).
    Form,
    /// Bottom rows pinned to opencode's prompt footer (`Snapshot::footer`).
    Footer,
    /// The OSC window title (`Snapshot::title`).
    Title,
}

/// One matched detection marker: the signal phrase/glyph and the region it
/// was found in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct MarkerMatch {
    pub marker: &'static str,
    pub region: Region,
}

/// Marker evidence of one agent's verdicts over a snapshot.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct AgentEvidence {
    pub agent: &'static str,
    pub blocked: Vec<MarkerMatch>,
    pub working: Vec<MarkerMatch>,
    pub idle: Vec<MarkerMatch>,
}

/// Full detection explanation: the verdict plus every matched marker, per
/// agent. Diagnostics only (built on demand by `kumo agent explain`); the hot
/// path stays on the zero-allocation `detect` above.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Explanation {
    pub status: AgentStatus,
    pub blocked: Vec<AgentEvidence>,
    pub working: Vec<AgentEvidence>,
    pub idle: Vec<AgentEvidence>,
}

/// Detect *and* explain, on demand for `kumo agent explain`: computes each
/// agent's per-region marker evidence and derives the status with the same
/// precedence `detect` uses (kept in sync by the consistency tests below).
pub(crate) fn explain(snap: &Snapshot) -> Explanation {
    let mut blocked = Vec::new();
    let mut working = Vec::new();
    let mut idle = Vec::new();
    for ev in [opencode::evidence(snap), claude::evidence(snap)] {
        if !ev.blocked.is_empty() {
            blocked.push(ev.clone());
        }
        if !ev.working.is_empty() {
            working.push(ev.clone());
        }
        if !ev.idle.is_empty() {
            idle.push(ev.clone());
        }
    }
    let has_blocked = blocked.iter().any(|e| !e.blocked.is_empty());
    let has_working = working.iter().any(|e| !e.working.is_empty());
    let has_idle = idle.iter().any(|e| !e.idle.is_empty());
    let status = if has_blocked {
        AgentStatus::Blocked
    } else if has_working {
        AgentStatus::Working
    } else if has_idle {
        AgentStatus::Idle
    } else {
        AgentStatus::Unknown
    };
    Explanation { status, blocked, working, idle }
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

    /// `detect` (hot path) and `explain` (diagnostics) must agree on every
    /// snapshot: same status, and each boolean predicate matches the presence
    /// of evidence markers.
    #[test]
    fn detect_and_explain_agree() {
        let snapshots = [
            snap("△ Permission required\nAllow once\nAllow always\nesc interrupt", "esc interrupt", ""),
            snap("\u{21c6} tab   \u{2191}\u{2193} select\nenter submit   esc dismiss\n", "", ""),
            snap("opencode 1.18.15\nAsk anything... \"\"\nesc dismiss", "esc dismiss", ""),
            snap("opencode 1.18.15\n~/.opencode\n", "~/.opencode", ""),
            snap("some transcript text\nesc interrupt", "esc interrupt", ""),
            snap("", "", "\u{280b} Fixing the bug"),
            snap("", "", "\u{2733} ~/proj"),
            snap("─────\nDo you want to proceed?\n  1. yes\n  2. no\n  esc to cancel\n", "", ""),
            snap("─────\n✢\n✶\n✻\nNoodling…\n", "", ""),
            snap("─────\n❯\n? for shortcuts · \u{2190} for agents\n", "", ""),
            snap("/btw reasoning about the bug\n  esc to close\n", "", ""),
            snap("assistant: do you want to proceed? (y/n)\n\u{276f} ", "", ""),
        ];
        for s in &snapshots {
            assert_eq!(detect(s), explain(s).status, "status mismatch for {s:?}");
            assert_eq!(opencode::blocked(s), !opencode::evidence(s).blocked.is_empty());
            assert_eq!(opencode::working(s), !opencode::evidence(s).working.is_empty());
            assert_eq!(opencode::idle(s), !opencode::evidence(s).idle.is_empty());
            assert_eq!(claude::blocked(s), !claude::evidence(s).blocked.is_empty());
            assert_eq!(claude::working(s), !claude::evidence(s).working.is_empty());
            assert_eq!(claude::idle(s), !claude::evidence(s).idle.is_empty());
        }
    }

    #[test]
    fn explain_reports_region_and_marker() {
        // opencode idle marker lives in the screen region; the status derives.
        let s = snap("opencode 1.18.15\nAsk anything... \"\"\nesc dismiss", "esc dismiss", "");
        let exp = explain(&s);
        assert_eq!(exp.status, AgentStatus::Idle);
        assert_eq!(exp.idle.len(), 1);
        let ev = &exp.idle[0];
        assert_eq!(ev.agent, "opencode");
        assert!(ev.idle.iter().any(|m| m.marker == "ask anything" && m.region == Region::Screen));
    }

    #[test]
    fn explain_reports_precedence_winner() {
        // Both a blocked dialog and a working footer are present: explain
        // records both but the status is Blocked (precedence).
        let s = snap("△ Permission required\nAllow once\nesc interrupt", "esc interrupt", "");
        let exp = explain(&s);
        assert_eq!(exp.status, AgentStatus::Blocked);
        assert_eq!(exp.blocked.len(), 1);
        assert_eq!(exp.working.len(), 1);
        assert!(exp.working[0].working.iter().any(|m| m.marker == "esc interrupt" && m.region == Region::Footer));
    }
}
