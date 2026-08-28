//! Claude Code lifecycle detection, driven by the bundled
//! `rules/claude.toml` manifest (see [`super::rules`]):
//!
//! Unlike opencode, Claude renders its live state outside the main transcript:
//! an interactive prompt box below a horizontal rule (where approval forms
//! appear) plus a status spinner in the OSC window title (a braille spinner
//! while working, `✳ ` when idle).

use super::{AgentEvidence, Snapshot};
use super::rules;

/// Whether Claude is waiting on an approval form or permission dialog.
#[cfg(test)]
pub(crate) fn blocked(snap: &Snapshot) -> bool {
    rules::agent("claude").blocked(snap)
}

/// Every matched Claude marker, per signal kind and evidence region
/// (diagnostics for `kumo agent explain`; kept in sync with the boolean
/// predicates by the consistency tests in `super`).
#[cfg(test)]
pub(crate) fn evidence(snap: &Snapshot) -> AgentEvidence {
    rules::agent("claude").evidence(snap)
}

/// Whether Claude is conclusively idle. This is NOT the fallback: a
/// recognized agent with no marker at all reports `Unknown`, so the idle
/// signal must be a real marker of the idle prompt box or OSC title.
#[cfg(test)]
pub(crate) fn idle(snap: &Snapshot) -> bool {
    rules::agent("claude").idle(snap)
}

/// Whether Claude is actively working. Signals, oldest to newest:
/// - a braille or half-circle spinner leading the OSC window title;
/// - the `/btw` reasoning overlay;
/// - newer claude's working prompt box: a `· esc to interrupt ·` hint or a
///   short dingbat spinner in the live form/footer region.
#[cfg(test)]
pub(crate) fn working(snap: &Snapshot) -> bool {
    rules::agent("claude").working(snap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::agents::after_last_rule;
    use crate::daemon::agents::rules::yes_no_line;

    fn snap(screen: &str, title: &str) -> Snapshot {
        let form = after_last_rule(screen);
        Snapshot {
            screen: screen.to_string(),
            form,
            footer: String::new(),
            title: title.to_string(),
        }
    }

    #[test]
    fn working_when_osc_title_has_spinner() {
        let s = snap("", "\u{280b} Fixing the bug");
        assert!(working(&s));
    }

    #[test]
    fn not_working_when_osc_title_has_idle_marker() {
        let s = snap("\u{276f} ", "\u{2733} ~/proj");
        assert!(!working(&s));
    }

    #[test]
    fn blocked_on_approval_form() {
        let s = snap("Claude wants to run a command\n─────\nDo you want to proceed?\n  1. yes\n  2. no\n  esc to cancel\n", "");
        assert!(blocked(&s));
    }

    #[test]
    fn blocked_on_live_form() {
        let s = snap(
            "─────\nRun a dynamic workflow?\n  enter to confirm\n  esc to cancel\n",
            "",
        );
        assert!(blocked(&s));
    }

    #[test]
    fn blocked_on_bash_approval() {
        let s = snap(
            "Do you want to proceed?\n  bash(rm -rf build)\n  1. yes\n  2. no\n  esc to cancel\n",
            "",
        );
        assert!(blocked(&s));
    }

    #[test]
    fn not_blocked_without_form_chrome() {
        // "esc to cancel" needs the matching enter action; a select list
        // without a navigation hint stays idle.
        let s = snap("─────\nenter to select\nesc to cancel\n", "");
        assert!(!blocked(&s));
    }

    #[test]
    fn not_blocked_by_generic_transcript_text() {
        // A bare `❯` prompt with question text in the transcript is idle.
        let s = snap("assistant: do you want to proceed? (y/n)\n\u{276f} ", "");
        assert!(!blocked(&s));
    }

    #[test]
    fn working_on_btw_overlay() {
        let s = snap("/btw reasoning about the bug\n  esc to close\n", "");
        assert!(working(&s));
    }

    #[test]
    fn working_on_prompt_box_interrupt_hint() {
        // Newer claude pins `· esc to interrupt ·` in the working prompt box,
        // below the last horizontal rule.
        let s = snap("─────\n❯  · esc to interrupt · ← for agents\n", "");
        assert!(working(&s));
    }

    #[test]
    fn working_on_prompt_box_dingbat_spinner() {
        // Newer claude spins dingbats (✢✶✻✽) in the prompt box while working.
        let s = snap("─────\n✢\n✶\n✻\nNoodling…\n", "");
        assert!(working(&s));
    }

    #[test]
    fn not_working_on_completion_summary() {
        // "✻ Sautéed for 43s" (18 chars) is a completion summary, not an
        // active spinner.
        let s = snap("─────\n✻ Sautéed for 43s\n", "");
        assert!(!working(&s));
    }

    #[test]
    fn working_on_short_dingbat_line() {
        // Short dingbat lines (≤30 chars) are active spinners.
        let s = snap("─────\n✻ Thinking…\n", "");
        assert!(working(&s));
    }

    #[test]
    fn working_on_half_circle_osc_title() {
        // Claude 2.1.228+ uses half-circles (◐◓◑◒) as busy spinners in OSC title.
        for title in ["◐ Working", "◑ Processing", "◒ Thinking", "◓ Busy"] {
            let s = snap("", title);
            assert!(working(&s), "should detect {title:?} as working");
        }
    }

    #[test]
    fn not_working_on_idle_prompt_box() {
        // Idle prompt box shows the shortcuts hint, never `esc to interrupt`
        // or a spinner.
        let s = snap("─────\n❯\n? for shortcuts · ← for agents\n", "");
        assert!(!working(&s));
    }

    #[test]
    fn working_on_interrupt_hint_in_footer() {
        let s = snap("─────\n❯\n", "");
        let form = after_last_rule(&s.screen);
        let snap = Snapshot {
            screen: s.screen.clone(),
            form,
            footer: "  · esc to interrupt ·".to_string(),
            title: String::new(),
        };
        assert!(working(&snap));
    }

    #[test]
    fn idle_on_prompt_box_shortcuts_hint() {
        let s = snap("─────\n❯\n? for shortcuts · ← for agents\n", "");
        assert!(idle(&s));
    }

    #[test]
    fn not_idle_without_prompt_box_marker() {
        let s = snap("assistant: hello\n❯ ", "");
        assert!(!idle(&s));
    }

    #[test]
    fn yes_no_line_accepts_numbered_and_bare_options() {
        for line in ["1. yes", "2. no", "2. yes", "3. no", "❯ yes", "  1. yes"] {
            assert!(yes_no_line(line), "should accept {line:?}");
        }
    }

    #[test]
    fn yes_no_line_rejects_non_options() {
        for line in [
            "", "1. maybe", "5. yes", "no", "proceed?", "2.5. yes", "yes/no",
        ] {
            assert!(!yes_no_line(line), "should reject {line:?}");
        }
    }
}
