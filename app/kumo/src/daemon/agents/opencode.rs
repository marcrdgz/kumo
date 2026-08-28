//! opencode lifecycle detection, driven by the bundled
//! `rules/opencode.toml` manifest (see [`super::rules`]):
//!
//! opencode signals its state on screen: permission dialogs and a question
//! prompt when blocked, and a prompt footer ("esc interrupt", spinner,
//! progress bar) while working, plus the `Ask anything...` prompt box when
//! idle.

use super::{AgentEvidence, Snapshot};
use super::rules;

/// Whether opencode is waiting on a command approval.
#[cfg(test)]
pub(crate) fn blocked(snap: &Snapshot) -> bool {
    rules::agent("opencode").blocked(snap)
}

/// Every matched opencode marker, per signal kind and evidence region
/// (diagnostics for `kumo agent explain`; kept in sync with the boolean
/// predicates by the consistency tests in `super`).
#[cfg(test)]
pub(crate) fn evidence(snap: &Snapshot) -> AgentEvidence {
    rules::agent("opencode").evidence(snap)
}

/// Whether opencode is actively producing output. The footer is scanned
/// instead of the whole screen so a frozen "esc interrupt" from an earlier
/// turn in the scrolled transcript is not misread as currently working.
#[cfg(test)]
pub(crate) fn working(snap: &Snapshot) -> bool {
    rules::agent("opencode").working(snap)
}

/// Whether opencode is conclusively idle. This is NOT the fallback: a
/// recognized agent with no marker at all reports `Unknown`, so the idle
/// signal must be a real marker of the prompt box.
#[cfg(test)]
pub(crate) fn idle(snap: &Snapshot) -> bool {
    rules::agent("opencode").idle(snap)
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
