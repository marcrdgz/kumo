//! `claude.toml` manifest.
//!
//! Unlike opencode, Claude renders its live state outside the main transcript:
//! an interactive prompt box below a horizontal rule (where approval forms
//! appear) plus a status spinner in the OSC window title (a braille spinner
//! while working, `✳ ` when idle).

use super::{contains_ci, ends_with_ci, Snapshot};

/// Navigation hints in Claude's option lists, paired with "enter to select".
const NAV_HINTS: &[&str] = &[
    "tab/arrow keys to navigate",
    "arrow keys to navigate",
    "arrows to navigate",
    "\u{2191}/\u{2193} to navigate",
    "\u{2191}\u{2193} to navigate",
];

/// Whether Claude is waiting on an approval form or permission dialog.
/// `snap.form` (the text below the last horizontal rule) is the live
/// prompt/forms region; `snap.screen` is the recent buffer tail.
pub(crate) fn blocked(snap: &Snapshot) -> bool {
    // Live form / select prompt: "esc to cancel" plus a confirm or select
    // action (a select list also needs a navigation hint).
    if contains_ci(&snap.form, "esc to cancel")
        && (contains_ci(&snap.form, "enter to confirm")
            || (contains_ci(&snap.form, "enter to select")
                && NAV_HINTS.iter().any(|m| contains_ci(&snap.form, m))))
    {
        return true;
    }
    // Dynamic workflow confirmation.
    if contains_ci(&snap.form, "run a dynamic workflow?")
        && contains_ci(&snap.form, "esc to cancel")
    {
        return true;
    }
    // Bash command approval: "do you want to proceed?" with command chrome and
    // a yes/no option line.
    if contains_ci(&snap.screen, "do you want to proceed?")
        && [
            "bash command",
            "bash(",
            "contains expansion",
            "tab to amend",
            "ctrl+e to explain",
        ]
        .iter()
        .any(|m| contains_ci(&snap.screen, m))
        && snap.screen.lines().any(yes_no_line)
    {
        return true;
    }
    // Generic permission with numbered yes/no options rendered in the form.
    if contains_ci(&snap.form, "do you want to proceed?")
        && contains_ci(&snap.form, "esc to cancel")
        && snap.form.lines().any(yes_no_line)
    {
        return true;
    }
    // Standalone approval markers (legacy / connection / bash-approval hints).
    [
        "waiting for permission",
        "do you want to allow this connection?",
        "review your answers",
        "skip interview and plan immediately",
        "tab to amend",
        "ctrl+e to explain",
    ]
    .iter()
    .any(|m| contains_ci(&snap.screen, m))
}

/// Markers of Claude's idle prompt box. The live prompt/forms region shows
/// `? for shortcuts · ← for agents` while idle; while it runs it pins
/// `· esc to interrupt ·` instead (matched by `working`). The `✳ ` OSC
/// window title is the same idle verdict for older UIs.
const IDLE_PROMPT_MARKERS: &[&str] = &["? for shortcuts", "\u{2190} for agents"];

/// Whether Claude is conclusively idle. This is NOT the fallback: a
/// recognized agent with no marker at all reports `Unknown`, so the idle
/// signal must be a real marker of the idle prompt box or OSC title.
pub(crate) fn idle(snap: &Snapshot) -> bool {
    IDLE_PROMPT_MARKERS
        .iter()
        .any(|m| contains_ci(&snap.form, m) || contains_ci(&snap.footer, m))
        || snap.title.trim_start().starts_with('\u{2733}')
}

/// Dingbat spinner glyphs newer Claude paints inside the prompt box while
/// working (the braille OSC-title spinner moved into the UI). Each is a single
/// codepoint in the U+2700 block, so a scan of the form/footer region catches
/// whatever frame the spinner is on.
const DINGBAT_SPINNER: &[char] = &[
    '\u{2722}', // ✢
    '\u{2736}', // ✶
    '\u{273b}', // ✻
    '\u{273d}', // ✽
];

/// Maximum trimmed line length for a dingbat to count as a working spinner.
/// Real spinners are short (`✻`, `✻ Thinking…`); completion summaries like
/// `✻ Sautéed for 43s` exceed this threshold.
const DINGBAT_LINE_MAX_LEN: usize = 16;

/// Whether Claude is actively working. Signals, oldest to newest:
/// - a braille or half-circle spinner leading the OSC window title;
/// - the `/btw` reasoning overlay;
/// - newer claude's working prompt box: a `· esc to interrupt ·` hint or a
///   short dingbat spinner in the live form/footer region.
pub(crate) fn working(snap: &Snapshot) -> bool {
    if snap.title.chars().next().is_some_and(|c| {
        ('\u{2800}'..='\u{28ff}').contains(&c) || ('\u{25d0}'..='\u{25d3}').contains(&c)
    }) {
        return true;
    }
    // The working prompt box pins `· esc to interrupt ·` only while a task
    // runs; at idle it shows `? for shortcuts · ← for agents` instead.
    if contains_ci(&snap.form, "esc to interrupt") || contains_ci(&snap.footer, "esc to interrupt")
    {
        return true;
    }
    if dingbat_spinner_in(&snap.form) || dingbat_spinner_in(&snap.footer) {
        return true;
    }
    btw_overlay(&snap.screen)
}

/// True when `text` contains a dingbat spinner on a short line (≤16 chars
/// trimmed). Longer lines like `✻ Sautéed for 43s` are completion summaries,
/// not active spinners.
fn dingbat_spinner_in(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.len() <= DINGBAT_LINE_MAX_LEN
            && DINGBAT_SPINNER.iter().any(|c| trimmed.contains(*c))
    })
}

/// True when Claude's `/btw` reasoning overlay is on screen: within the last
/// five non-empty lines a header line starts with `/btw` and a footer line
/// ends with `esc to close`.
fn btw_overlay(screen: &str) -> bool {
    let tail: Vec<&str> = screen
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .rev()
        .take(5)
        .collect();
    let has_btw = tail.iter().any(|l| {
        l.trim_start().starts_with("/btw")
            && l.trim_start()[4..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
    });
    let has_close = tail.iter().any(|l| ends_with_ci(l, "esc to close"));
    has_btw && has_close
}

/// Whether `line` is a Claude yes/no option like `1. yes` / `2. no` (a bare
/// `claude.toml` line regexes.
fn yes_no_line(line: &str) -> bool {
    let mut t = line.trim_start();
    if let Some(rest) = t.strip_prefix('\u{276f}') {
        t = rest.trim_start();
    }
    let (num, rest) = match t.split_once('.') {
        Some((n, r)) => (n.trim(), r),
        None => ("", t),
    };
    let word = rest
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    let yes = word == "yes";
    let no = word == "no";
    (num.is_empty() && yes)
        || (num == "1" && yes)
        || (num == "2" && (yes || no))
        || (num == "3" && no)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::agents::after_last_rule;

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
