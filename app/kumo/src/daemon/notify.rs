//! Desktop notifications for agent lifecycle transitions.
//!
//! Fire-and-forget like the audible alerts (`alert.rs`): spawn the platform
//! notifier and never block the daemon loop. macOS uses Notification Center
//! via `osascript`, Linux uses `notify-send`; other platforms stay silent.
//! Failures are ignored on purpose — a missing or broken notifier must never
//! take down panes.

use crate::daemon::alert::AlertKind;

/// Raise a desktop notification for an agent transition. `agent` is the
/// detected CLI name ("claude", "opencode", …); `context` is a human-readable
/// location (the owning session's workspace path).
pub fn send(kind: AlertKind, agent: &str, context: &str) {
    let (title, body) = message(kind, agent, context);

    #[cfg(target_os = "macos")]
    {
        // display notification "<body>" with title "Kumo" subtitle "<title>"
        let script = format!(
            "display notification {} with title \"Kumo\" subtitle {}",
            apple_str(&body),
            apple_str(&title),
        );
        let _ = std::process::Command::new("osascript").arg("-e").arg(script).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .args(["-a", "Kumo"])
            .arg(&title)
            .arg(&body)
            .spawn();
    }
}

/// The notification's `(title, body)` for a transition. The title leads with
/// the agent name; the body carries the workspace when known and falls back
/// to a plain description of the event.
fn message(kind: AlertKind, agent: &str, context: &str) -> (String, String) {
    let agent = match agent.trim() {
        "" => "Agent",
        other => other,
    };
    let title = match kind {
        AlertKind::Blocked => format!("{agent} is blocked"),
        AlertKind::Finished => format!("{agent} finished"),
    };
    let fallback = match kind {
        AlertKind::Blocked => "Waiting for your approval",
        AlertKind::Finished => "Task complete",
    };
    let body = match context.trim() {
        "" => fallback.to_string(),
        other => other.to_string(),
    };
    (title, body)
}

/// Quote a string as an AppleScript double-quoted literal, escaping the two
/// characters AppleScript honors inside strings: backslash and double quote.
#[cfg(target_os = "macos")]
fn apple_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_lead_with_agent_name() {
        let (title, _) = message(AlertKind::Blocked, "claude", "/work");
        assert_eq!(title, "claude is blocked");
        let (title, _) = message(AlertKind::Finished, "opencode", "/work");
        assert_eq!(title, "opencode finished");
    }

    #[test]
    fn blank_agent_becomes_generic() {
        let (title, _) = message(AlertKind::Blocked, "", "/work");
        assert_eq!(title, "Agent is blocked");
    }

    #[test]
    fn body_prefers_workspace_over_fallback() {
        let (_, body) = message(AlertKind::Blocked, "claude", "~/dev/kumo");
        assert_eq!(body, "~/dev/kumo");
    }

    #[test]
    fn empty_context_falls_back_to_event_text() {
        let (_, blocked_body) = message(AlertKind::Blocked, "claude", "");
        assert_eq!(blocked_body, "Waiting for your approval");
        let (_, finished_body) = message(AlertKind::Finished, "claude", "  ");
        assert_eq!(finished_body, "Task complete");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_str_escapes_quotes_and_backslashes() {
        assert_eq!(apple_str("plain"), "\"plain\"");
        assert_eq!(apple_str("a\"b"), "\"a\\\"b\"");
        assert_eq!(apple_str("a\\b"), "\"a\\\\b\"");
    }
}
