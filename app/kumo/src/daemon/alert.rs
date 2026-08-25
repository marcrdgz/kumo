/// Audible alerts for agent lifecycle transitions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlertKind {
    /// The agent finished its task unseen (Working -> Done).
    Finished,
    /// The agent is blocked waiting for an approval (Working -> Blocked).
    Blocked,
}

/// An agent lifecycle notification raised by the daemon: broadcast to every
/// attached viewer as a transient corner toast.
#[derive(Clone, Debug)]
pub(crate) struct AgentToast {
    /// The kumo pane id of the agent pane, for click-to-focus on clients.
    pub(crate) pane_id: u64,
    pub(crate) kind: AlertKind,
    /// Short headline, e.g. `claude is blocked`.
    pub(crate) title: String,
    /// Location line (the owning session's workspace path); may be empty.
    pub(crate) body: String,
}

impl From<AlertKind> for kumo_protocol::ToastKind {
    fn from(kind: AlertKind) -> Self {
        match kind {
            AlertKind::Blocked => kumo_protocol::ToastKind::Blocked,
            AlertKind::Finished => kumo_protocol::ToastKind::Finished,
        }
    }
}

/// Play an audible alert without blocking the frame loop.
///
/// Uses the platform's sound player when available (afplay on macOS, paplay
/// on Linux) and falls back to ringing the host terminal bell.
pub fn play(kind: AlertKind) {
    #[cfg(target_os = "macos")]
    {
        let sound = match kind {
            AlertKind::Finished => "/System/Library/Sounds/Glass.aiff",
            AlertKind::Blocked => "/System/Library/Sounds/Funk.aiff",
        };
        if std::process::Command::new("afplay").arg(sound).spawn().is_ok() {
            return;
        }
    }
    #[cfg(target_os = "linux")]
    {
        let sound = match kind {
            AlertKind::Finished => "/usr/share/sounds/freedesktop/stereo/complete.oga",
            AlertKind::Blocked => "/usr/share/sounds/freedesktop/stereo/message.oga",
        };
        for cmd in [
            ("paplay", sound),
            ("aplay", "/usr/share/sounds/alsa/Front_Center.wav"),
        ] {
            if std::process::Command::new(cmd.0).arg(cmd.1).spawn().is_ok() {
                return;
            }
        }
    }
    // Fallback: ring the host terminal bell (any platform).
    let mut out = std::io::stdout();
    let _ = std::io::Write::write_all(&mut out, b"\x07");
}
