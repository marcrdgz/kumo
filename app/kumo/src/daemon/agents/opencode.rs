//! opencode lifecycle detection.
//!
//! opencode signals its state on screen: permission dialogs and a question
//! prompt when blocked, and a prompt footer ("esc interrupt", spinner,
//! progress bar) pinned to the bottom rows while working. Mirrors herdr's
//! bundled `opencode.toml` manifest.

use super::Snapshot;

/// Output markers that indicate the agent is waiting on a command approval.
///
/// Only markers tied to a real on-screen dialog qualify. Generic prompts
/// ("proceed?", "(y/n)", "would you like to", ...) are deliberately excluded:
/// they also match conversation transcript text, falsely flagging an idle
/// agent as blocked. Mirrors herdr's opencode manifest (state = "blocked").
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
/// "esc dismiss" with an enter action and a navigation hint. Mirrors herdr's
/// opencode manifest rule (state = "blocked").
fn question_dialog_visible(screen_lower: &str, screen: &str) -> bool {
    if !screen_lower.contains("esc dismiss") {
        return false;
    }
    let enter = QUESTION_DIALOG_ENTER.iter().any(|m| screen_lower.contains(m));
    let nav = QUESTION_DIALOG_NAV.iter().any(|m| screen.contains(m));
    enter && nav
}

/// Whether opencode is waiting on a command approval.
pub(crate) fn blocked(snap: &Snapshot) -> bool {
    question_dialog_visible(&snap.screen_lower, &snap.screen) || BLOCKED_MARKERS.iter().any(|m| snap.screen_lower.contains(m))
}

/// Whether opencode is actively producing output. The footer is scanned
/// instead of the whole screen so a frozen "esc interrupt" from an earlier
/// turn in the scrolled transcript is not misread as currently working.
pub(crate) fn working(snap: &Snapshot) -> bool {
    WORKING_MARKERS.iter().any(|m| snap.footer_lower.contains(m))
        // Knight-rider status bar: 4+ block cells in a row.
        || ["■■■■", "⬝⬝⬝⬝"].iter().any(|p| snap.footer.contains(p))
        // Braille spinner (tool call / thinking) in the prompt footer.
        || snap.footer.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::agents::after_last_rule;

    fn snap(screen: &str, footer: &str) -> Snapshot {
        let form = after_last_rule(screen);
        Snapshot {
            screen: screen.to_string(),
            screen_lower: screen.to_lowercase(),
            form: form.clone(),
            form_lower: form.to_lowercase(),
            footer: footer.to_string(),
            footer_lower: footer.to_lowercase(),
            title: String::new(),
        }
    }

    #[test]
    fn blocked_on_permission_dialog() {
        let s = snap("△ Permission required\nAllow once\nAllow always\nReject", "");
        assert!(blocked(&s));
    }

    #[test]
    fn blocked_on_question_dialog() {
        let s = snap("\u{21c6} tab   \u{2191}\u{2193} select\nenter submit   esc dismiss\n", "");
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
        let s = snap("previous turn output - esc interrupt\nAsk anything... \"\"", "Ask anything... \"\"");
        assert!(!working(&s));
    }

    #[test]
    fn working_when_knight_rider_bar_in_footer() {
        let s = snap("\n\n\n\u{25a0}\u{25a0}\u{25a0}\u{25a0}running...", "\u{25a0}\u{25a0}\u{25a0}\u{25a0}running...");
        assert!(working(&s));
    }

    #[test]
    fn not_working_when_screen_has_no_working_marker() {
        let s = snap("opencode 1.18.15\n~/.opencode\n", "~/.opencode");
        assert!(!working(&s));
    }
}
