//! XTGETTCAP (`DCS + q ... ST`) responder.
//!
//! Intercepts terminal capability probes from the child process and answers
//! as a plain xterm, vouching only for non-mouse capabilities. Clients like
//! vim, less, tmux, and opencode probe for mouse support (`XM`, `kmous`)
//! before enabling mouse reporting; omitting those keeps the emulator out of
//! mouse-tracking mode so kumo's own text selection always works. This is the
//! same identity trick herdr uses on top of libghostty-vt.

/// Parse `DCS + q` queries (statefully, across chunk boundaries) and queue
/// one `DCS + r` reply per supported capability.
#[derive(Debug, Default)]
pub struct XtgettcapTracker {
    state: State,
    body: Vec<u8>,
    pending: Vec<Vec<u8>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum State {
    #[default]
    Ground,
    Escape,
    DcsIntro,
    DcsIntroPlus,
    DcsBody,
    DcsEscape,
    IgnoreOsc,
    IgnoreOscEscape,
    IgnoreString,
    IgnoreStringEscape,
    OversizedDcs,
    OversizedDcsEscape,
}

impl XtgettcapTracker {
    pub fn new() -> Self {
        Self { state: State::Ground, body: Vec::with_capacity(64), pending: Vec::new() }
    }

    /// Scan a chunk of pty output. Must be called for every byte fed to the
    /// emulator so the state machine stays in sync.
    pub fn observe(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match self.state {
                State::Ground => {
                    if byte == 0x1b {
                        self.state = State::Escape;
                    } else if byte == 0x90 {
                        self.body.clear();
                        self.state = State::DcsIntro;
                    } else if byte == 0x9d {
                        self.state = State::IgnoreOsc;
                    } else if matches!(byte, 0x98 | 0x9e | 0x9f) {
                        self.state = State::IgnoreString;
                    }
                }
                State::Escape => match byte {
                    b'P' => {
                        self.body.clear();
                        self.state = State::DcsIntro;
                    }
                    b']' => {
                        self.body.clear();
                        self.state = State::IgnoreOsc;
                    }
                    b'_' | b'^' | b'X' => {
                        self.body.clear();
                        self.state = State::IgnoreString;
                    }
                    0x1b => self.state = State::Escape,
                    _ => self.state = State::Ground,
                },
                State::DcsIntro => match byte {
                    b'+' => self.state = State::DcsIntroPlus,
                    0x1b => self.state = State::IgnoreStringEscape,
                    0x9c => self.state = State::Ground,
                    _ => self.state = State::IgnoreString,
                },
                State::DcsIntroPlus => match byte {
                    b'q' => self.state = State::DcsBody,
                    0x1b => self.state = State::IgnoreStringEscape,
                    0x9c => self.state = State::Ground,
                    _ => self.state = State::IgnoreString,
                },
                State::DcsBody => match byte {
                    0x1b => self.state = State::DcsEscape,
                    0x9c => {
                        self.finalize();
                        self.state = State::Ground;
                    }
                    _ => self.body.push(byte),
                },
                State::DcsEscape => {
                    if byte == b'\\' {
                        self.finalize();
                        self.state = State::Ground;
                    } else if byte != 0x1b {
                        self.body.clear();
                        self.state = State::IgnoreString;
                    }
                }
                State::IgnoreOsc => {
                    if byte == 0x1b {
                        self.state = State::IgnoreOscEscape;
                    } else if matches!(byte, 0x07 | 0x9c) {
                        self.state = State::Ground;
                    }
                }
                State::IgnoreOscEscape => {
                    if byte == b'\\' {
                        self.state = State::Ground;
                    } else if byte != 0x1b {
                        self.state = State::IgnoreOsc;
                    }
                }
                State::IgnoreString => {
                    if byte == 0x1b {
                        self.state = State::IgnoreStringEscape;
                    } else if byte == 0x9c {
                        self.state = State::Ground;
                    }
                }
                State::IgnoreStringEscape => {
                    if byte == b'\\' {
                        self.state = State::Ground;
                    } else if byte != 0x1b {
                        self.state = State::IgnoreString;
                    }
                }
                State::OversizedDcs => {
                    if byte == 0x1b {
                        self.state = State::OversizedDcsEscape;
                    } else if byte == 0x9c {
                        self.state = State::Ground;
                    }
                }
                State::OversizedDcsEscape => {
                    if byte == b'\\' {
                        self.state = State::Ground;
                    } else if byte != 0x1b {
                        self.state = State::OversizedDcs;
                    }
                }
            }

            if self.body.len() > 1024 {
                self.body.clear();
                self.state = State::OversizedDcs;
            }
        }
    }

    /// Completed replies to write back to the pty, in query order.
    pub fn drain_pending(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.pending)
    }

    fn finalize(&mut self) {
        for cap_hex in self.body.split(|byte| *byte == b';') {
            if let Some(bytes) = xtgettcap_response(cap_hex) {
                self.pending.push(bytes);
            }
        }
        self.body.clear();
    }
}

/// Build the `DCS + r` reply for one capability name, or `None` when the cap
/// is not on the whitelist (e.g. the mouse caps `XM`/`kmous`).
fn xtgettcap_response(cap_hex: &[u8]) -> Option<Vec<u8>> {
    if cap_hex.is_empty() || !cap_hex.len().is_multiple_of(2) {
        return None;
    }
    let mut normalized = Vec::with_capacity(cap_hex.len());
    for &byte in cap_hex {
        if !byte.is_ascii_hexdigit() {
            return None;
        }
        normalized.push(byte.to_ascii_uppercase());
    }
    let value = xtgettcap_value(&normalized)?;
    Some(build_response(&normalized, value))
}

/// Mirror only the xterm terminfo capabilities this pane can stand behind.
/// Deliberately omits `XM` (584d) and `kmous` (6b6d6f7573).
fn xtgettcap_value(cap_hex: &[u8]) -> Option<Option<&'static [u8]>> {
    match cap_hex {
        b"5463" => Some(None),                                   // Tc
        b"524742" => Some(Some(b"8")),                           // RGB
        b"73657472676266" => Some(Some(b"\\E[38:2:%p1%d:%p2%d:%p3%dm")), // setrgbf
        b"73657472676262" => Some(Some(b"\\E[48:2:%p1%d:%p2%d:%p3%dm")), // setrgbb
        b"4D73" => Some(Some(b"\\E]52;%p1%s;%p2%s\\007")),       // Ms
        b"5375" => Some(None),                                   // Su
        b"536D756C78" => Some(Some(b"\\E[4:%p1%dm")),            // Smulx
        b"536574756C63" => Some(Some(
            b"\\E[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m",
        )), // Setulc
        _ => None,
    }
}

fn build_response(cap_hex: &[u8], value: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + cap_hex.len() + value.map_or(0, |bytes| bytes.len() * 2));
    out.extend_from_slice(b"\x1bP1+r");
    out.extend_from_slice(cap_hex);
    if let Some(value) = value {
        out.push(b'=');
        append_upper_hex(value, &mut out);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

fn append_upper_hex(bytes: &[u8], output: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in bytes {
        output.push(HEX[usize::from(byte >> 4)]);
        output.push(HEX[usize::from(byte & 0x0f)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn responses(t: &mut XtgettcapTracker) -> Vec<Vec<u8>> {
        t.drain_pending()
    }

    #[test]
    fn answers_multiple_capabilities_in_order() {
        let mut t = XtgettcapTracker::new();
        t.observe(b"\x1bP+q5463;524742\x1b\\");
        assert_eq!(
            responses(&mut t),
            vec![
                b"\x1bP1+r5463\x1b\\".to_vec(),
                b"\x1bP1+r524742=38\x1b\\".to_vec(),
            ]
        );
    }

    #[test]
    fn normalizes_mixed_case_query_keys() {
        let mut t = XtgettcapTracker::new();
        t.observe(b"\x1bP+q4d73\x1b\\");
        assert_eq!(
            responses(&mut t),
            vec![b"\x1bP1+r4D73=5C455D35323B25703125733B25703225735C303037\x1b\\".to_vec()]
        );
    }

    #[test]
    fn omits_unsupported_capabilities() {
        let mut t = XtgettcapTracker::new();
        t.observe(b"\x1bP+q6E6F7065\x1b\\");
        assert!(responses(&mut t).is_empty());
    }

    #[test]
    fn omits_mouse_capabilities() {
        let mut t = XtgettcapTracker::new();
        // XM and kmous must not be answered so clients don't enable the mouse.
        t.observe(b"\x1bP+q584d;6b6d6f7573\x1b\\");
        assert!(responses(&mut t).is_empty());
    }

    #[test]
    fn keeps_split_query_until_string_terminator() {
        let mut t = XtgettcapTracker::new();
        t.observe(b"\x1bP+q537");
        assert!(responses(&mut t).is_empty());
        t.observe(b"5\x1b");
        assert!(responses(&mut t).is_empty());
        t.observe(b"\\");
        assert_eq!(responses(&mut t), vec![b"\x1bP1+r5375\x1b\\".to_vec()]);
    }

    #[test]
    fn resumes_after_ignored_osc_bel_terminator() {
        let mut t = XtgettcapTracker::new();
        t.observe(b"\x1b]0;title\x07\x1bP+q5463\x1b\\");
        assert_eq!(responses(&mut t), vec![b"\x1bP1+r5463\x1b\\".to_vec()]);
    }
}
