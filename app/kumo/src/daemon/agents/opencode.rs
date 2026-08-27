//! opencode lifecycle detection.
//!
//! opencode signals its state on screen: permission dialogs and a question
//! prompt when blocked, and a prompt footer ("esc interrupt", spinner,
//! bundled `opencode.toml` manifest.

use super::{contains_ci, AgentEvidence, MarkerMatch, Region, Snapshot};

/// Output markers that indicate the agent is waiting on a command approval.
///
/// Only markers tied to a real on-screen dialog qualify. Generic prompts
/// ("proceed?", "(y/n)", "would you like to", ...) are deliberately excluded:
/// they also match conversation transcript text, falsely flagging an idle
const BLOCKED_MARKERS: &[&str] = &[
    // opencode permission dialog ("△ Permission required" header + buttons).
    "permission required",
    "allow once",
    "allow always",
    "always allow",
    "reject permission",
    "waiting for permission",
];

/// opencode's question dialog footer strings (QuestionPrompt). All three must
/// be present together — "esc dismiss" alone also matches the idle prompt.
const QUESTION_DIALOG_ENTER: &[&str] = &["enter submit", "enter confirm", "enter toggle"];
const QUESTION_DIALOG_NAV: &[&str] = &["\u{2191}\u{2193} select", "\u{21c6} tab"];

/// Markers, scanned against the current screen text, that indicate the agent
/// is actively working. Idle is the fallback when none match (manifest-based
/// detection instead of an output-recently window).
const WORKING_MARKERS: &[&str] = &[
    // opencode prompt footer ("esc interrupt" / "esc again to interrupt").
    "esc interrupt",
    "esc again to interrupt",
    "ctrl+c to interrupt",
    "press esc to interrupt",
    // Generic in-progress text.
    "waiting for assistant",
    "sending prompt",
    "retrying in",
];

/// True when opencode's question dialog is on screen: its footer pairs
/// opencode manifest rule (state = "blocked").
fn question_dialog_visible(snap: &Snapshot) -> bool {
    if !contains_ci(&snap.screen, "esc dismiss") {
        return false;
    }
    let enter = QUESTION_DIALOG_ENTER
        .iter()
        .any(|m| contains_ci(&snap.screen, m));
    let nav = QUESTION_DIALOG_NAV.iter().any(|m| snap.screen.contains(m));
    enter && nav
}

/// Whether opencode is waiting on a command approval.
pub(crate) fn blocked(snap: &Snapshot) -> bool {
    question_dialog_visible(snap) || BLOCKED_MARKERS.iter().any(|m| contains_ci(&snap.screen, m))
}

/// Every matched opencode marker, per signal kind and evidence region
/// (diagnostics for `kumo agent explain`; kept in sync with the boolean
/// predicates by the consistency tests below).
pub(crate) fn evidence(snap: &Snapshot) -> AgentEvidence {
    AgentEvidence {
        agent: "opencode",
        blocked: blocked_evidence(snap),
        working: working_evidence(snap),
        idle: idle_evidence(snap),
    }
}

fn blocked_evidence(snap: &Snapshot) -> Vec<MarkerMatch> {
    let mut out = Vec::new();
    if question_dialog_visible(snap) {
        out.push(MarkerMatch { marker: "question dialog", region: Region::Screen });
        if contains_ci(&snap.screen, "esc dismiss") {
            out.push(MarkerMatch { marker: "esc dismiss", region: Region::Screen });
        }
        for m in QUESTION_DIALOG_ENTER {
            if contains_ci(&snap.screen, m) {
                out.push(MarkerMatch { marker: m, region: Region::Screen });
            }
        }
        for m in QUESTION_DIALOG_NAV {
            if snap.screen.contains(m) {
                out.push(MarkerMatch { marker: m, region: Region::Screen });
            }
        }
    }
    for m in BLOCKED_MARKERS {
        if contains_ci(&snap.screen, m) {
            out.push(MarkerMatch { marker: m, region: Region::Screen });
        }
    }
    out
}

fn working_evidence(snap: &Snapshot) -> Vec<MarkerMatch> {
    let mut out = Vec::new();
    for m in WORKING_MARKERS {
        if contains_ci(&snap.footer, m) {
            out.push(MarkerMatch { marker: m, region: Region::Footer });
        }
    }
    if ["■■■■", "⬝⬝⬝⬝"].iter().any(|p| snap.footer.contains(p)) {
        out.push(MarkerMatch { marker: "knight-rider bar", region: Region::Footer });
    }
    if snap.footer.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)) {
        out.push(MarkerMatch { marker: "braille spinner", region: Region::Footer });
    }
    out
}

fn idle_evidence(snap: &Snapshot) -> Vec<MarkerMatch> {
    let mut out = Vec::new();
    for m in IDLE_MARKERS {
        if contains_ci(&snap.screen, m) {
            out.push(MarkerMatch { marker: m, region: Region::Screen });
        }
    }
    if contains_ci(&snap.footer, "esc dismiss") && !question_dialog_visible(snap) {
        out.push(MarkerMatch { marker: "esc dismiss (idle prompt)", region: Region::Footer });
    }
    out
}

/// Whether opencode is actively producing output. The footer is scanned
/// instead of the whole screen so a frozen "esc interrupt" from an earlier
/// turn in the scrolled transcript is not misread as currently working.
pub(crate) fn working(snap: &Snapshot) -> bool {
    WORKING_MARKERS.iter().any(|m| contains_ci(&snap.footer, m))
        // Knight-rider status bar: 4+ block cells in a row.
        || ["■■■■", "⬝⬝⬝⬝"].iter().any(|p| snap.footer.contains(p))
        // Braille spinner (tool call / thinking) in the prompt footer.
        || snap.footer.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c))
}

/// Markers of opencode's idle prompt box: the `Ask anything...` placeholder
/// and the keymap-hints row (`tab agents`/`ctrl+p commands`) pinned under the
/// input bar while the agent is not running. The hints row persists once you
/// type into the prompt (when the placeholder is replaced by your text), so a
/// prompt waiting for Enter still classifies as idle instead of unknown. A
/// bare `esc dismiss` only counts when it is the prompt bar, never a question
/// dialog (see `question_dialog_visible`) — and `working` (`esc interrupt`
/// footer) wins by precedence when the agent is mid-turn.
const IDLE_MARKERS: &[&str] = &["ask anything", "tab agents", "ctrl+p commands"];

/// Whether opencode is conclusively idle. This is NOT the fallback: a
/// recognized agent with no marker at all reports `Unknown`, so the idle
/// signal must be a real marker of the prompt box.
pub(crate) fn idle(snap: &Snapshot) -> bool {
    IDLE_MARKERS.iter().any(|m| contains_ci(&snap.screen, m))
        || (contains_ci(&snap.footer, "esc dismiss") && !question_dialog_visible(snap))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::agents::after_last_rule;

    fn snap(screen: &str, footer: &str) -> Snapshot {
        let form = after_last_rule(screen);
        Snapshot {
            screen: screen.to_string(),
            form,
            footer: footer.to_string(),
            title: String::new(),
        }
    }

    #[test]
    fn blocked_on_permission_dialog() {
        let s = snap(
            "△ Permission required\nAllow once\nAllow always\nReject",
            "",
        );
        assert!(blocked(&s));
    }

    #[test]
    fn blocked_on_question_dialog() {
        let s = snap(
            "\u{21c6} tab   \u{2191}\u{2193} select\nenter submit   esc dismiss\n",
            "",
        );
        assert!(blocked(&s));
    }

    #[test]
    fn not_blocked_when_esc_dismiss_without_question_footer() {
        let s = snap("Ask anything... \"\"\nesc dismiss\n", "");
        assert!(!blocked(&s));
    }

    #[test]
    fn not_blocked_by_generic_transcript_prompt_text() {
        let s = snap("the assistant asked: Do you want to proceed? (y/n)\n", "");
        assert!(!blocked(&s));
    }

    #[test]
    fn working_when_footer_shows_interrupt_hint() {
        let s = snap("some transcript text\nesc interrupt", "esc interrupt");
        assert!(working(&s));
    }

    #[test]
    fn not_working_when_interrupt_hint_is_older_transcript_not_footer() {
        let s = snap(
            "previous turn output - esc interrupt\nAsk anything... \"\"",
            "Ask anything... \"\"",
        );
        assert!(!working(&s));
    }

    #[test]
    fn working_when_knight_rider_bar_in_footer() {
        let s = snap(
            "\n\n\n\u{25a0}\u{25a0}\u{25a0}\u{25a0}running...",
            "\u{25a0}\u{25a0}\u{25a0}\u{25a0}running...",
        );
        assert!(working(&s));
    }

    #[test]
    fn not_working_when_screen_has_no_working_marker() {
        let s = snap("opencode 1.18.15\n~/.opencode\n", "~/.opencode");
        assert!(!working(&s));
    }

    #[test]
    fn idle_on_ask_anything_prompt_box() {
        let s = snap("opencode 1.18.15\nAsk anything... \"\"\nesc dismiss", "esc dismiss");
        assert!(idle(&s));
    }

    #[test]
    fn idle_when_typing_without_placeholder() {
        // Typed input replaces "Ask anything..."; the keymap-hints row stays
        // pinned under the input bar, so a prompt waiting for Enter is idle
        // rather than unknown.
        let s = snap(
            "opencode 1.18.15\ntab agentsctrl+p commands\nwhy?",
            "",
        );
        assert!(idle(&s));
    }

    #[test]
    fn idle_on_esc_dismiss_footer_without_dialog() {
        let s = snap("done\n", "esc dismiss");
        assert!(idle(&s));
    }

    #[test]
    fn not_idle_when_esc_dismiss_is_a_question_dialog() {
        let s = snap("\u{21c6} tab   \u{2191}\u{2193} select\nenter submit   esc dismiss\n", "");
        assert!(!idle(&s));
    }

    #[test]
    fn not_idle_without_any_prompt_marker() {
        let s = snap("opencode 1.18.15\n~/.opencode\n", "~/.opencode");
        assert!(!idle(&s));
    }
}
