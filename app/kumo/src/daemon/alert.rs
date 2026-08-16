/// Audible alerts for agent lifecycle transitions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlertKind {
    /// The agent finished its task (Working -> Idle).
    Finished,
    /// The agent is blocked waiting for an approval (Working -> Blocked).
    Blocked,
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
